//! `lezreg stats`: the adoption evidence table, straight from chain (DESIGN §7.2, §9).
//!
//! Counted: entries whose program has a **live header** (the prize counts only genuinely
//! deployed programs; an entry registered for an empty account is listed as not counted).
//! Entries whose claimed image differs from the live one are counted and flagged stale.
//! Authors are the proven co-signers (`entry.author`), never the free-text label.
//! `--exclude-keys` drops entries whose author or registrant is in the file (the team's own).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use anyhow::{Context as _, Result, anyhow};
use lezreg_sdk::{Header, Kind, Registry, Rpc};
use lezreg_types::{AccountId, Entry};
use serde_json::{Value, json};

use crate::out::Out;

/// One entry with its proven kind and its program's live header.
#[derive(Debug, Clone)]
pub struct Row {
    pub entry: Entry,
    pub kind: Kind,
    pub header: Option<Header>,
}

/// Every entry in the registry, newest first, with headers (batched reads). Links whose entry
/// is missing or does not match its address are counted in `unreadable`.
pub fn collect<R: Rpc>(r: &Registry<R>) -> Result<(Vec<Row>, usize)> {
    let mut items = Vec::new();
    let mut after = None;
    loop {
        let page = r.list(after.as_ref(), 100)?;
        items.extend(page.items);
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    let mut unreadable = 0;
    let mut rows: Vec<Row> = Vec::new();
    for i in items {
        match (i.entry, i.kind) {
            (Some(entry), Some(kind)) => rows.push(Row {
                entry,
                kind,
                header: None,
            }),
            _ => unreadable += 1,
        }
    }
    let programs: Vec<AccountId> = rows.iter().map(|r| r.entry.program).collect();
    let mut headers = Vec::with_capacity(programs.len());
    for chunk in programs.chunks(lezreg_sdk::rpc::BATCH) {
        headers.extend(r.headers(chunk)?);
    }
    for (row, h) in rows.iter_mut().zip(headers) {
        row.header = h;
    }
    Ok((rows, unreadable))
}

/// `YYYY-MM` (UTC) of a millisecond timestamp.
pub fn month(ms: u64) -> String {
    // Howard Hinnant's civil_from_days.
    let z = i64::try_from(ms / 86_400_000).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MonthRow {
    pub entries: usize,
    pub programs: BTreeSet<AccountId>,
    pub authors: BTreeSet<AccountId>,
    /// Authors whose first counted entry is in this month.
    pub new_authors: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AuthorRow {
    pub entries: usize,
    pub names: BTreeSet<String>,
    pub first: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Entries read from the global list (every one, counted or not).
    pub listed: usize,
    pub unreadable: usize,
    pub excluded: usize,
    /// Entries for an account with no program deployed: not counted.
    pub no_program: usize,
    pub counted: usize,
    pub deployer: usize,
    pub third_party: usize,
    pub stale: usize,
    pub programs: BTreeSet<AccountId>,
    pub authors: BTreeSet<AccountId>,
    pub images: BTreeSet<[u8; 32]>,
    pub by_month: BTreeMap<String, MonthRow>,
    pub by_author: BTreeMap<AccountId, AuthorRow>,
}

pub fn aggregate(rows: &[Row], unreadable: usize, exclude: &HashSet<AccountId>) -> Stats {
    let mut s = Stats {
        listed: rows.len() + unreadable,
        unreadable,
        ..Stats::default()
    };
    // Oldest first, so "new author" means the month of their first counted entry.
    let mut order: Vec<&Row> = rows.iter().collect();
    order.sort_by_key(|r| r.entry.registered_at);
    for r in order {
        let e = &r.entry;
        if exclude.contains(&e.author) || exclude.contains(&e.registrant) {
            s.excluded += 1;
            continue;
        }
        let Some(h) = &r.header else {
            s.no_program += 1;
            continue;
        };
        s.counted += 1;
        match r.kind {
            Kind::Deployer => s.deployer += 1,
            Kind::ThirdParty { .. } => s.third_party += 1,
        }
        if h.image_id != e.image_id {
            s.stale += 1;
        }
        s.programs.insert(e.program);
        s.images.insert(h.image_id);
        let new_author = s.authors.insert(e.author);
        let m = s.by_month.entry(month(e.registered_at)).or_default();
        m.entries += 1;
        m.programs.insert(e.program);
        m.authors.insert(e.author);
        if new_author {
            m.new_authors += 1;
        }
        let a = s.by_author.entry(e.author).or_insert_with(|| AuthorRow {
            first: e.registered_at,
            ..AuthorRow::default()
        });
        a.entries += 1;
        a.names.insert(e.name.clone());
    }
    s
}

/// Accounts in a file: one per line (`Public/` prefix allowed), `#` comments, blank lines.
pub fn read_keys(path: &std::path::Path) -> Result<HashSet<AccountId>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    text.lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .map(|l| {
            let bare = l.strip_prefix("Public/").unwrap_or(l);
            AccountId::from_base58(bare)
                .ok_or_else(|| anyhow!("{}: '{l}' is not an account id", path.display()))
        })
        .collect()
}

pub fn report(out: &mut Out, s: &Stats, by_month: bool, by_author: bool) -> Value {
    out.text(&format!(
        "entries listed      {}\n\
         counted             {}  (live program header; {} deployer, {} third-party, {} stale)\n\
         not counted         {} with no program deployed, {} excluded, {} unreadable\n\
         distinct programs   {}\n\
         distinct authors    {}  (proven co-signers)\n\
         distinct images     {}",
        s.listed,
        s.counted,
        s.deployer,
        s.third_party,
        s.stale,
        s.no_program,
        s.excluded,
        s.unreadable,
        s.programs.len(),
        s.authors.len(),
        s.images.len()
    ));
    let mut v = json!({
        "listed": s.listed,
        "counted": s.counted,
        "deployer": s.deployer,
        "third_party": s.third_party,
        "stale": s.stale,
        "no_program": s.no_program,
        "excluded": s.excluded,
        "unreadable": s.unreadable,
        "programs": s.programs.len(),
        "authors": s.authors.len(),
        "images": s.images.len(),
    });
    if by_month {
        out.text("\nmonth     entries  programs  authors  new authors");
        let mut rows = Vec::new();
        for (m, r) in &s.by_month {
            out.text(&format!(
                "{m}  {:>7}  {:>8}  {:>7}  {:>11}",
                r.entries,
                r.programs.len(),
                r.authors.len(),
                r.new_authors
            ));
            rows.push(json!({"month": m, "entries": r.entries, "programs": r.programs.len(), "authors": r.authors.len(), "new_authors": r.new_authors}));
        }
        v["by_month"] = json!(rows);
    }
    if by_author {
        out.text("\nauthor                                          entries  first    names");
        let mut rows: Vec<(&AccountId, &AuthorRow)> = s.by_author.iter().collect();
        rows.sort_by(|a, b| {
            b.1.entries
                .cmp(&a.1.entries)
                .then(a.1.first.cmp(&b.1.first))
        });
        let mut j = Vec::new();
        for (a, r) in rows {
            let names: Vec<&str> = r.names.iter().map(String::as_str).collect();
            out.text(&format!(
                "{:<46}  {:>7}  {}  {}",
                a.to_base58(),
                r.entries,
                month(r.first),
                names.join(", ")
            ));
            j.push(json!({"author": a.to_base58(), "entries": r.entries, "first_month": month(r.first), "names": names}));
        }
        v["by_author"] = json!(j);
    }
    out.json(v.clone());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use lezreg_sdk::MockRpc;
    use lezreg_types::{self as t, BuildConfig, ContentRef, Head, Link, SourceRef, pda::addr};

    const REG: AccountId = AccountId([0xEE; 32]);
    const LOADER: AccountId = AccountId([0xFE; 32]);

    fn acct(n: u8) -> AccountId {
        let mut b = [n; 32];
        b[0] = 0xA0;
        AccountId(b)
    }

    fn entry(kind: u8, program: AccountId, author: AccountId, name: &str, at: u64) -> Entry {
        Entry {
            schema: 1,
            kind,
            flags: 0,
            program,
            author,
            registrant: author,
            name: name.into(),
            version: "1.0.0".into(),
            author_label: String::new(),
            description: String::new(),
            tags: vec![],
            idl: ContentRef {
                cid: vec![1, 0x55, 0],
                sha256: [9; 32],
                size: 3,
                peer_hint: None,
            },
            source: SourceRef {
                repo: "https://example.org/r".into(),
                commit: vec![1; 20],
                docker_root: ".".into(),
                archive: None,
            },
            build: BuildConfig::default(),
            image_id: [7; 32],
            registered_at: at,
            updated_at: at,
            revision: 1,
        }
    }

    /// A mock chain indexed like the registry program (as in sdk/rust/tests/reads.rs).
    struct Chain {
        rpc: MockRpc,
        seq: u64,
    }

    impl Chain {
        fn bump(&self, at: AccountId, e: AccountId) -> Option<AccountId> {
            let s = lezreg_sdk::Rpc::get_actor_state(&self.rpc, &at, &REG).unwrap();
            let mut h = Head::decode_or_empty(&s);
            let prev = h.newest;
            h.count += 1;
            h.newest = Some(e);
            self.rpc.put(at, REG, &h);
            prev
        }

        fn register(&mut self, address: AccountId, e: &Entry) {
            self.rpc.put(address, REG, e);
            let prev_global = self.bump(addr::global_head(&REG), address);
            let prev_name = self.bump(addr::name_head(&REG, &e.name), address);
            let prev_author = self.bump(addr::author_head(&REG, &e.registrant), address);
            let prev_program = (e.kind == t::KIND_THIRD_PARTY)
                .then(|| self.bump(addr::program_head(&REG, &e.program), address))
                .flatten();
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
        }

        fn deploy(&self, program: AccountId, image: u8) {
            let mut v = vec![image; 32];
            v.extend_from_slice(&[3; 32]);
            v.push(0);
            self.rpc.set(program, LOADER, v);
        }
    }

    const OCT: u64 = 1_790_000_000_000; // 2026-09-21
    const NOV: u64 = 1_793_000_000_000; // 2026-10-26
    const DAY: u64 = 86_400_000;

    #[test]
    fn months() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(OCT), "2026-09");
        assert_eq!(month(NOV), "2026-10");
        assert_eq!(month(1_798_761_600_000), "2027-01"); // 2027-01-01T00:00Z
        assert_eq!(month(1_798_761_600_000 - 1), "2026-12");
        assert_eq!(month(951_782_400_000), "2000-02"); // 2000-02-29
    }

    #[test]
    fn counts_live_programs_and_proven_authors() {
        let mut c = Chain {
            rpc: MockRpc::new(),
            seq: 0,
        };
        let (alice, bob, team) = (acct(1), acct(2), acct(3));
        let (p1, p2, p3, p4, p5) = (acct(11), acct(12), acct(13), acct(14), acct(15));
        // alice: two deployed programs (one stale), in two months.
        c.register(p1, &entry(t::KIND_DEPLOYER, p1, alice, "one", OCT));
        c.deploy(p1, 7);
        c.register(p2, &entry(t::KIND_DEPLOYER, p2, alice, "two", NOV));
        c.deploy(p2, 8); // claims 7: stale, still counted
        // bob: a third-party entry for p1, and an entry for an empty account (not counted).
        let tp = addr::third_party_entry(&REG, &p1, &bob);
        c.register(
            tp,
            &entry(t::KIND_THIRD_PARTY, p1, bob, "one-community", NOV + DAY),
        );
        c.register(p3, &entry(t::KIND_DEPLOYER, p3, bob, "ghost", NOV));
        // the team's own program, excluded with --exclude-keys.
        c.register(p4, &entry(t::KIND_DEPLOYER, p4, team, "ours", OCT));
        c.deploy(p4, 7);
        // a link whose entry is gone (unreadable).
        let e5 = entry(t::KIND_DEPLOYER, p5, bob, "gone", NOV);
        c.register(p5, &e5);
        c.rpc.set(p5, REG, vec![]);

        let r = Registry::new(&c.rpc, REG);
        let (rows, unreadable) = collect(&r).unwrap();
        assert_eq!((rows.len(), unreadable), (5, 1));

        let s = aggregate(&rows, unreadable, &HashSet::new());
        assert_eq!(s.listed, 6);
        assert_eq!(s.counted, 4, "p1, p2, the community entry for p1, p4");
        assert_eq!((s.deployer, s.third_party, s.stale), (3, 1, 1));
        assert_eq!(s.no_program, 1);
        assert_eq!(s.programs.len(), 3, "p1 counted once");
        assert_eq!(s.authors.len(), 3);
        assert_eq!(s.images.len(), 2);
        let sep = &s.by_month["2026-09"];
        assert_eq!((sep.entries, sep.authors.len(), sep.new_authors), (2, 2, 2));
        let oct = &s.by_month["2026-10"];
        assert_eq!((oct.entries, oct.authors.len(), oct.new_authors), (2, 2, 1));
        assert_eq!(s.by_author[&alice].entries, 2);

        let s = aggregate(&rows, unreadable, &[team].into());
        assert_eq!((s.counted, s.excluded, s.authors.len()), (3, 1, 2));
        assert!(!s.by_month["2026-09"].authors.contains(&team));
    }

    #[test]
    fn exclude_file() {
        let d = std::env::temp_dir().join(format!("lezreg-keys-{}", std::process::id()));
        std::fs::write(
            &d,
            format!(
                "# our keys\nPublic/{}\n\n{}  # deployer\n",
                acct(1).to_base58(),
                acct(2).to_base58()
            ),
        )
        .unwrap();
        let k = read_keys(&d).unwrap();
        assert_eq!(k, [acct(1), acct(2)].into());
        std::fs::write(&d, "nope\n").unwrap();
        assert!(read_keys(&d).is_err());
        std::fs::remove_file(&d).ok();
    }
}
