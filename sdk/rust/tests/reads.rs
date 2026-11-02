//! SDK reads against a mocked sequencer. The mock chain is indexed exactly as the registry's
//! index chain does it (DESIGN §2.4: global → name → author → program third-party list → link),
//! with `Link`/`Head`/`AuthorNode` records from `lezreg-types`.

use lezreg_sdk::{Error, Freshness, Header, Kind, LOADER, LinkCache, MockRpc, Registry};
use lezreg_types::{self as t, AccountId, AuthorNode, BuildConfig, ContentRef, Entry, Head, Link, SourceRef, pda::addr};

const REG: AccountId = AccountId([0xEE; 32]);

fn acct(n: u8) -> AccountId {
    let mut b = [n; 32];
    b[0] = 0xA0;
    AccountId(b)
}

fn entry(kind: u8, program: AccountId, author: AccountId, name: &str, image: u8) -> Entry {
    Entry {
        schema: 1,
        kind,
        flags: 0,
        program,
        author,
        registrant: author,
        name: name.into(),
        version: "1.0.0".into(),
        author_label: format!("label of {name}"),
        description: String::new(),
        tags: if name.contains("tok") { vec!["token".into()] } else { vec![] },
        idl: ContentRef { cid: vec![1, 0x55, 0], sha256: [9; 32], size: 3, peer_hint: None },
        source: SourceRef { repo: "https://example.org/r".into(), commit: vec![1; 20], docker_root: ".".into(), archive: None },
        build: BuildConfig { schema: 1, builder: 1, binary: "b".into(), manifest_path: "m".into(), ..BuildConfig::default() },
        image_id: [image; 32],
        registered_at: 1,
        updated_at: 1,
        revision: 1,
    }
}

fn header_bytes(image: u8, immutable: bool) -> Vec<u8> {
    let mut v = vec![image; 32];
    v.extend_from_slice(&[7; 32]);
    v.push(u8::from(immutable));
    v
}

/// A mock chain that indexes entries like the registry program.
struct Chain {
    rpc: MockRpc,
    seq: u64,
}

impl Chain {
    fn new() -> Self {
        Self { rpc: MockRpc::new(), seq: 0 }
    }

    fn head(&self, at: AccountId) -> Head {
        let s = lezreg_sdk::Rpc::get_actor_state(&self.rpc, &at, &REG).unwrap();
        Head::decode_or_empty(&s)
    }

    fn bump(&self, at: AccountId, e: AccountId) -> Option<AccountId> {
        let mut h = self.head(at);
        let prev = h.newest;
        h.count += 1;
        h.newest = Some(e);
        self.rpc.put(at, REG, &h);
        prev
    }

    /// Registers `e` at `address` and returns the address.
    fn register(&mut self, address: AccountId, e: &Entry) -> AccountId {
        self.rpc.put(address, REG, e);
        let prev_global = self.bump(addr::global_head(&REG), address);
        let prev_name = self.bump(addr::name_head(&REG, &e.name), address);
        let prev_author = self.bump(addr::author_head(&REG, &e.registrant), address);
        let prev_program = (e.kind == t::KIND_THIRD_PARTY).then(|| self.bump(addr::program_head(&REG, &e.program), address)).flatten();
        let link = Link {
            schema: 1,
            seq: self.seq,
            kind: e.kind,
            program: e.program,
            name: e.name.clone(),
            author: e.registrant,
            prev_global,
            prev_name,
            prev_author,
            prev_program,
        };
        self.rpc.put(addr::link(&REG, &address), REG, &link);
        self.seq += 1;
        address
    }

    fn deployer(&mut self, program: AccountId, author: AccountId, name: &str, image: u8) -> AccountId {
        self.rpc.set(program, LOADER, header_bytes(image, false));
        self.register(program, &entry(t::KIND_DEPLOYER, program, author, name, image))
    }

    fn third_party(&mut self, program: AccountId, publisher: AccountId, name: &str, image: u8) -> AccountId {
        let at = addr::third_party_entry(&REG, &program, &publisher);
        self.register(at, &entry(t::KIND_THIRD_PARTY, program, publisher, name, image))
    }

    /// `TransferAuthor` of the entry at `at` to `to`.
    fn transfer(&self, at: AccountId, to: AccountId) {
        let mut e: Entry = borsh::from_slice(&lezreg_sdk::Rpc::get_actor_state(&self.rpc, &at, &REG).unwrap()).unwrap();
        e.author = to;
        self.rpc.put(at, REG, &e);
        let prev = self.bump(addr::author_head(&REG, &to), at);
        self.rpc.put(addr::author_node(&REG, &to, &at), REG, &AuthorNode { schema: 1, entry: at, prev });
    }

    fn registry(&self) -> Registry<&MockRpc> {
        Registry::new(&self.rpc, REG)
    }
}

#[test]
fn resolve_is_one_get_account_and_reports_freshness() {
    let mut c = Chain::new();
    let (x, a) = (acct(1), acct(2));
    c.deployer(x, a, "token", 5);
    let r = c.registry();
    c.rpc.reset_reads();
    let res = r.resolve(&x).unwrap();
    assert_eq!(c.rpc.reads(), 1, "deployer entry and header come from one getAccount");
    let e = res.deployer.as_ref().unwrap();
    assert_eq!(e.name, "token");
    assert_eq!(res.header.unwrap().image_id, [5; 32]);
    assert_eq!(res.freshness(), Some(Freshness::Fresh));

    // The header is repointed: stale.
    c.rpc.set(x, LOADER, header_bytes(6, true));
    let res = r.resolve(&x).unwrap();
    assert_eq!(res.freshness(), Some(Freshness::Stale { live: [6; 32] }));
    assert!(res.header.unwrap().immutable);

    // Registered before deploying: no program.
    let y = acct(3);
    c.register(y, &entry(t::KIND_DEPLOYER, y, a, "early", 1));
    let r = c.registry();
    assert_eq!(r.resolve(&y).unwrap().freshness(), Some(Freshness::NoProgram));

    // Nothing at all.
    let z = acct(4);
    let res = r.resolve(&z).unwrap();
    assert!(res.deployer.is_none() && res.header.is_none() && res.freshness().is_none());
}

#[test]
fn a_record_at_x_that_is_not_xs_deployer_entry_is_not_one() {
    let c = Chain::new();
    let (x, other) = (acct(1), acct(9));
    // kind deployer but for another program; and kind third-party at X.
    c.rpc.put(x, REG, &entry(t::KIND_DEPLOYER, other, other, "fake", 1));
    assert!(c.registry().resolve(&x).unwrap().deployer.is_none());
    c.rpc.put(x, REG, &entry(t::KIND_THIRD_PARTY, x, other, "fake", 1));
    assert!(c.registry().resolve(&x).unwrap().deployer.is_none());
}

#[test]
fn third_party_entries_are_told_apart_by_address() {
    let mut c = Chain::new();
    let (x, a, q, m) = (acct(1), acct(2), acct(3), acct(4));
    c.deployer(x, a, "token", 5);
    let t_at = c.third_party(x, q, "token", 5);
    let r = c.registry();
    let tps = r.third_party(&x, 10).unwrap();
    assert_eq!(tps.len(), 1);
    assert_eq!(tps[0].address, t_at);
    assert_eq!(tps[0].kind, Some(Kind::ThirdParty { publisher: q }));
    let (at, e) = r.third_party_entry(&x, &q).unwrap();
    assert_eq!(at, t_at);
    assert!(e.is_some());
    assert!(r.third_party_entry(&x, &m).unwrap().1.is_none());

    // A third-party record at an address that is not T(program, registrant) is unverified.
    let fake = acct(77);
    c.register(fake, &entry(t::KIND_THIRD_PARTY, x, m, "token", 5));
    let tps = c.registry().third_party(&x, 10).unwrap();
    assert_eq!(tps[0].address, fake);
    assert_eq!(tps[0].kind, None);
    assert!(matches!(c.registry().locate(&fake), Err(Error::BadRecord { .. })));
    let loc = c.registry().locate(&t_at).unwrap().unwrap();
    assert_eq!(loc.freshness, Freshness::Fresh);
}

#[test]
fn list_pages_newest_first() {
    let mut c = Chain::new();
    let ids: Vec<AccountId> = (1..=5).map(|i| c.deployer(acct(i), acct(100 + i), &format!("p{i}"), i)).collect();
    let r = c.registry();
    let p1 = r.list(None, 2).unwrap();
    assert_eq!(p1.total, 5);
    assert_eq!(p1.items.iter().map(|i| i.address).collect::<Vec<_>>(), vec![ids[4], ids[3]]);
    assert_eq!(p1.next, Some(ids[3]));
    let p2 = r.list(p1.next.as_ref(), 2).unwrap();
    assert_eq!(p2.items.iter().map(|i| i.address).collect::<Vec<_>>(), vec![ids[2], ids[1]]);
    let p3 = r.list(p2.next.as_ref(), 2).unwrap();
    assert_eq!(p3.items.iter().map(|i| i.address).collect::<Vec<_>>(), vec![ids[0]]);
    assert_eq!(p3.next, None);
    assert_eq!(p3.items[0].link.seq, 0);
    assert!(p3.items.iter().all(|i| i.kind == Some(Kind::Deployer)));
    // An exact page boundary has no next.
    assert_eq!(r.list(None, 5).unwrap().next, None);
    assert!(r.list(None, 0).unwrap().items.is_empty());
}

#[test]
fn links_are_cached_and_persist() {
    let mut c = Chain::new();
    for i in 1..=5 {
        c.deployer(acct(i), acct(100 + i), &format!("p{i}"), i);
    }
    let dir = std::env::temp_dir().join(format!("lezreg-sdk-test-{}", std::process::id()));
    let path = dir.join("links.bin");
    let r = Registry::with_cache(&c.rpc, REG, LinkCache::at(path.clone()));
    r.list(None, 10).unwrap();
    c.rpc.reset_reads();
    r.list(None, 10).unwrap();
    assert_eq!(c.rpc.reads(), 6, "head + 5 entries; every link from the cache");
    r.cache().save().unwrap();
    let reloaded = LinkCache::at(path);
    assert_eq!(reloaded.len(), 5);
    let r2 = Registry::with_cache(&c.rpc, REG, reloaded);
    c.rpc.reset_reads();
    assert_eq!(r2.scan_links(10).unwrap().len(), 5);
    assert_eq!(c.rpc.reads(), 1, "a scan from a warm disk cache reads only the head");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn search_by_name_label_and_tag() {
    let mut c = Chain::new();
    c.deployer(acct(1), acct(50), "token", 1);
    c.deployer(acct(2), acct(51), "token-v2", 2);
    c.deployer(acct(3), acct(52), "amm", 3);
    c.third_party(acct(3), acct(53), "token", 3);
    let r = c.registry();
    let exact = r.by_name("token", 10).unwrap();
    assert_eq!(exact.len(), 2, "both entries named exactly 'token'");
    let s = r.search_name("TOKEN", 10, 100).unwrap();
    assert_eq!(s.iter().map(|i| i.link.name.as_str()).collect::<Vec<_>>(), vec!["token", "token", "token-v2"], "exact first, then substring");
    assert_eq!(r.search_name("tok", 1, 100).unwrap().len(), 1, "limit");
    assert!(r.by_name(&"x".repeat(40), 10).unwrap().is_empty(), "a name too long to be a seed has no list");
    assert_eq!(r.search_label("LABEL OF AMM", 10, 100).unwrap().len(), 1);
    assert_eq!(r.search_tag("token", 10, 100).unwrap().len(), 3);
    assert!(r.search_tag("nope", 10, 100).unwrap().is_empty());
}

#[test]
fn by_author_follows_transfers() {
    let mut c = Chain::new();
    let (a, b) = (acct(60), acct(61));
    let e1 = c.deployer(acct(1), a, "one", 1);
    let e2 = c.deployer(acct(2), a, "two", 2);
    let e3 = c.deployer(acct(3), b, "three", 3);
    c.transfer(e1, b);
    let r = c.registry();
    let held_by_a: Vec<_> = r.by_author(&a, 10).unwrap().into_iter().map(|i| i.address).collect();
    assert_eq!(held_by_a, vec![e2], "e1 left A's holdings");
    let held_by_b: Vec<_> = r.by_author(&b, 10).unwrap().into_iter().map(|i| i.address).collect();
    assert_eq!(held_by_b, vec![e1, e3], "e1 reached B by transfer (author node), then B's own e3");
    assert_eq!(r.by_author(&b, 1).unwrap().len(), 1);
}

#[test]
fn bad_data_and_transport_failures_are_typed() {
    let mut c = Chain::new();
    let x = c.deployer(acct(1), acct(2), "p", 1);
    // A garbage entry.
    c.rpc.set(acct(9), REG, vec![1, 2, 3]);
    assert!(matches!(c.registry().entry(&acct(9)), Err(Error::BadRecord { what: "entry", .. })));
    // A garbage header.
    c.rpc.set(acct(9), LOADER, vec![0; 10]);
    assert!(matches!(c.registry().resolve(&acct(9)), Err(Error::BadRecord { .. })));
    // A missing link.
    c.rpc.set(addr::link(&REG, &x), REG, vec![]);
    assert_eq!(c.registry().list(None, 5).unwrap_err(), Error::MissingLink(x));
    // A cycle.
    let mut c = Chain::new();
    let x = c.deployer(acct(1), acct(2), "p", 1);
    let mut l: Link = borsh::from_slice(&lezreg_sdk::Rpc::get_actor_state(&c.rpc, &addr::link(&REG, &x), &REG).unwrap()).unwrap();
    l.prev_global = Some(x);
    c.rpc.put(addr::link(&REG, &x), REG, &l);
    assert_eq!(c.registry().list(None, 5).unwrap_err(), Error::Cycle(x));
    // The sequencer is down.
    *c.rpc.fail.lock().unwrap() = Some("connection refused".into());
    assert!(matches!(c.registry().resolve(&x), Err(Error::Transport { .. })));
    assert!(matches!(c.registry().count(), Err(Error::Transport { .. })));
}

#[test]
fn header_decoding() {
    let h = Header::decode(&header_bytes(3, true)).unwrap();
    assert_eq!(h.image_id, [3; 32]);
    assert!(h.immutable);
    assert!(Header::decode(&header_bytes(3, true)[..64]).is_none());
    let mut bad = header_bytes(3, true);
    bad[64] = 2;
    assert!(Header::decode(&bad).is_none());
}

#[test]
fn releases_and_count() {
    let mut c = Chain::new();
    let x = c.deployer(acct(1), acct(2), "p", 1);
    let r = c.registry();
    assert_eq!(r.count().unwrap(), 1);
    assert!(r.release(&x, 1).unwrap().is_none());
    let rel = t::Release {
        schema: 1,
        entry: x,
        revision: 1,
        version: "1.0.0".into(),
        idl: ContentRef::default(),
        source: SourceRef::default(),
        build: BuildConfig::default(),
        image_id: [1; 32],
        build_hash: [0; 32],
        at: 5,
    };
    c.rpc.put(addr::release(&REG, &x, 1), REG, &rel);
    assert_eq!(r.release(&x, 1).unwrap().unwrap().at, 5);
}

#[test]
fn json_views_are_stable() {
    let mut c = Chain::new();
    let x = c.deployer(acct(1), acct(2), "p", 1);
    let r = c.registry();
    let v = lezreg_sdk::json::resolved(&r.resolve(&x).unwrap());
    assert_eq!(v["deployer_entry"]["kind"], "deployer");
    assert_eq!(v["freshness"]["status"], "fresh");
    assert_eq!(v["deployer_entry"]["author_label_proven"], false);
    assert!(v["deployer_entry"]["idl"]["cid"].as_str().unwrap().starts_with('z'));
}
