//! Reads (DESIGN §2.5, §3): resolve a program in one `getAccount`, walk the on-chain lists with
//! lazy pagination, and search. Sequencer only: no indexer, no server of ours.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Mutex,
};

use lezreg_types::{self as t, AccountId, AuthorNode, Entry, Head, Link, Release, pda::addr};

use crate::{
    error::{Error, Result},
    rpc::Rpc,
};

/// The LEZ program loader (`lee_core::program::PROGRAM_LOADER_ACCOUNT_ID`).
pub const LOADER: AccountId = AccountId([0xFE; 32]);

/// A loader `ProgramHeader` (borsh, 65 bytes): `image_id` (8 little-endian u32 words),
/// `program_first_segment`, `immutable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub image_id: [u8; 32],
    pub first_segment: AccountId,
    pub immutable: bool,
}

impl Header {
    /// Decodes a loader actor state; `None` if it is not a header.
    #[must_use]
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != 65 || b[64] > 1 {
            return None;
        }
        Some(Self { image_id: b[..32].try_into().ok()?, first_segment: AccountId(b[32..64].try_into().ok()?), immutable: b[64] == 1 })
    }
}

/// How an entry's claimed `image_id` compares with the live program header (DESIGN §3 step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// `entry.image_id == header.image_id`.
    Fresh,
    /// The header now runs a different image than the entry claims.
    Stale { live: [u8; 32] },
    /// No program header at the entry's program account (yet).
    NoProgram,
}

impl Freshness {
    #[must_use]
    pub fn of(entry: &Entry, header: Option<&Header>) -> Self {
        match header {
            None => Self::NoProgram,
            Some(h) if h.image_id == entry.image_id => Self::Fresh,
            Some(h) => Self::Stale { live: h.image_id },
        }
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale { .. } => "stale",
            Self::NoProgram => "no-program",
        }
    }
}

/// What kind of entry an address holds, checked from the address itself (DESIGN §3): a
/// deployer entry lives at its program's own account, a third-party entry at
/// `T(program, registrant)`. Anything else is not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Deployer,
    ThirdParty { publisher: AccountId },
}

impl Kind {
    /// The kind `entry` at `address` proves, or `None` if the address does not match.
    #[must_use]
    pub fn check(reg: &AccountId, address: &AccountId, entry: &Entry) -> Option<Self> {
        match entry.kind {
            t::KIND_DEPLOYER if entry.program == *address => Some(Self::Deployer),
            t::KIND_THIRD_PARTY if addr::third_party_entry(reg, &entry.program, &entry.registrant) == *address => {
                Some(Self::ThirdParty { publisher: entry.registrant })
            }
            _ => None,
        }
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Deployer => "deployer",
            Self::ThirdParty { .. } => "third-party",
        }
    }
}

/// An entry with its address, proven kind, and the live header of its program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub address: AccountId,
    pub entry: Entry,
    pub kind: Kind,
    pub header: Option<Header>,
    pub freshness: Freshness,
}

/// `resolve(program)`: the deployer entry and the loader header, from one `getAccount`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub program: AccountId,
    /// The deployer entry at `(program, REG)`, if any.
    pub deployer: Option<Entry>,
    pub header: Option<Header>,
}

impl Resolved {
    /// Fresh/stale/no-program, if there is a deployer entry.
    #[must_use]
    pub fn freshness(&self) -> Option<Freshness> {
        self.deployer.as_ref().map(|e| Freshness::of(e, self.header.as_ref()))
    }
}

/// One item of a list walk. `entry` is `None` for a link whose entry shows no registry state
/// (a private or unverifiable registration, DESIGN §2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub address: AccountId,
    pub link: Link,
    pub entry: Option<Entry>,
    /// The kind proven from the address, if the entry is present and matches it.
    pub kind: Option<Kind>,
}

/// A page of the global list, newest first. Pass `next` back as `after` for the next page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub items: Vec<Listed>,
    /// The last address of this page, when older entries remain.
    pub next: Option<AccountId>,
    /// Entries in the registry.
    pub total: u64,
}

/// `Link`s are written once, so they are cached forever, on disk if a path is given. Keyed by
/// entry address; one cache per registry and sequencer.
#[derive(Default)]
pub struct LinkCache {
    links: Mutex<HashMap<AccountId, Link>>,
    path: Option<PathBuf>,
    dirty: Mutex<bool>,
}

impl LinkCache {
    /// A cache backed by `path` (loaded now if it exists; a corrupt file is ignored).
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        let links = std::fs::read(&path)
            .ok()
            .and_then(|b| borsh::from_slice::<Vec<(AccountId, Link)>>(&b).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Self { links: Mutex::new(links), path: Some(path), dirty: Mutex::new(false) }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.links.lock().expect("lock").len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn get(&self, entry: &AccountId) -> Option<Link> {
        self.links.lock().expect("lock").get(entry).cloned()
    }

    fn insert(&self, entry: AccountId, link: Link) {
        self.links.lock().expect("lock").insert(entry, link);
        *self.dirty.lock().expect("lock") = true;
    }

    /// Writes the cache to its file, if it has one and changed.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else { return Ok(()) };
        let mut dirty = self.dirty.lock().expect("lock");
        if !*dirty {
            return Ok(());
        }
        let mut v: Vec<(AccountId, Link)> = self.links.lock().expect("lock").iter().map(|(k, l)| (*k, l.clone())).collect();
        v.sort_by_key(|(_, l)| l.seq);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, borsh::to_vec(&v)?)?;
        std::fs::rename(&tmp, path)?;
        *dirty = false;
        Ok(())
    }
}

/// Read-only registry client.
pub struct Registry<R: Rpc> {
    rpc: R,
    reg: AccountId,
    cache: LinkCache,
}

fn decode<T: borsh::BorshDeserialize>(at: &AccountId, bytes: &[u8], what: &'static str) -> Result<T> {
    borsh::from_slice(bytes).map_err(|_| Error::BadRecord { at: *at, what })
}

impl<R: Rpc> Registry<R> {
    /// A client for the registry program `reg`, reading through `rpc`, with an in-memory cache.
    pub fn new(rpc: R, reg: AccountId) -> Self {
        Self { rpc, reg, cache: LinkCache::default() }
    }

    /// The same, with a `LinkCache` (for example one on disk).
    pub fn with_cache(rpc: R, reg: AccountId, cache: LinkCache) -> Self {
        Self { rpc, reg, cache }
    }

    #[must_use]
    pub const fn registry(&self) -> &AccountId {
        &self.reg
    }

    #[must_use]
    pub const fn rpc(&self) -> &R {
        &self.rpc
    }

    #[must_use]
    pub const fn cache(&self) -> &LinkCache {
        &self.cache
    }

    /// The registry's actor state at `account` (empty if none).
    pub fn state(&self, account: &AccountId) -> Result<Vec<u8>> {
        self.rpc.get_actor_state(account, &self.reg)
    }

    /// The program header at `program`, if one is deployed.
    pub fn header(&self, program: &AccountId) -> Result<Option<Header>> {
        let s = self.rpc.get_actor_state(program, &LOADER)?;
        if s.is_empty() {
            return Ok(None);
        }
        Header::decode(&s).map(Some).ok_or(Error::BadRecord { at: *program, what: "program header" })
    }

    /// **One `getAccount(program)`**: the deployer entry at `(program, REG)` and the loader
    /// header together (DESIGN §3, client procedure steps 1–3).
    pub fn resolve(&self, program: &AccountId) -> Result<Resolved> {
        let Some(account) = self.rpc.get_account(program)? else {
            return Ok(Resolved { program: *program, deployer: None, header: None });
        };
        let deployer = match account.state(&self.reg) {
            [] => None,
            bytes => {
                let e: Entry = decode(program, bytes, "entry")?;
                // An actor state at (X, REG) that is not X's deployer entry is not one.
                (Kind::check(&self.reg, program, &e) == Some(Kind::Deployer)).then_some(e)
            }
        };
        let header = match account.state(&LOADER) {
            [] => None,
            bytes => Some(Header::decode(bytes).ok_or(Error::BadRecord { at: *program, what: "program header" })?),
        };
        Ok(Resolved { program: *program, deployer, header })
    }

    /// The entry at `address`, if any.
    pub fn entry(&self, address: &AccountId) -> Result<Option<Entry>> {
        match self.state(address)?.as_slice() {
            [] => Ok(None),
            b => decode(address, b, "entry").map(Some),
        }
    }

    /// The entry at `address` with its proven kind and its program's live header.
    pub fn locate(&self, address: &AccountId) -> Result<Option<Located>> {
        let Some(entry) = self.entry(address)? else { return Ok(None) };
        let kind = Kind::check(&self.reg, address, &entry).ok_or(Error::BadRecord { at: *address, what: "entry at its own address" })?;
        let header = self.header(&entry.program)?;
        let freshness = Freshness::of(&entry, header.as_ref());
        Ok(Some(Located { address: *address, entry, kind, header, freshness }))
    }

    /// `T(program, publisher)` and the third-party entry there, if any.
    pub fn third_party_entry(&self, program: &AccountId, publisher: &AccountId) -> Result<(AccountId, Option<Entry>)> {
        let at = addr::third_party_entry(&self.reg, program, publisher);
        Ok((at, self.entry(&at)?))
    }

    /// A list head (an empty actor state is an empty list).
    pub fn head(&self, at: &AccountId) -> Result<Head> {
        match self.state(at)?.as_slice() {
            [] => Ok(Head { schema: t::SCHEMA, count: 0, newest: None }),
            b => decode(at, b, "list head"),
        }
    }

    /// Entries in the registry.
    pub fn count(&self) -> Result<u64> {
        Ok(self.head(&addr::global_head(&self.reg))?.count)
    }

    /// `entry`'s `Link` (cached forever once read).
    pub fn link(&self, entry: &AccountId) -> Result<Link> {
        if let Some(l) = self.cache.get(entry) {
            return Ok(l);
        }
        let at = addr::link(&self.reg, entry);
        let l: Link = match self.state(&at)?.as_slice() {
            [] => return Err(Error::MissingLink(*entry)),
            b => decode(&at, b, "link")?,
        };
        self.cache.insert(*entry, l.clone());
        Ok(l)
    }

    /// The write-once `Release` of `entry` at `revision`.
    pub fn release(&self, entry: &AccountId, revision: u32) -> Result<Option<Release>> {
        let at = addr::release(&self.reg, entry, revision);
        match self.state(&at)?.as_slice() {
            [] => Ok(None),
            b => decode(&at, b, "release").map(Some),
        }
    }

    fn listed(&self, address: AccountId, link: Link, with_entry: bool) -> Result<Listed> {
        let entry = if with_entry { self.entry(&address)? } else { None };
        let kind = entry.as_ref().and_then(|e| Kind::check(&self.reg, &address, e));
        Ok(Listed { address, link, entry, kind })
    }

    /// Walks a list from `start` (inclusive), following `prev`, until `limit` items.
    fn walk(
        &self,
        start: Option<AccountId>,
        limit: usize,
        with_entries: bool,
        prev: impl Fn(&Link) -> Option<AccountId>,
    ) -> Result<(Vec<Listed>, Option<AccountId>)> {
        let mut out: Vec<Listed> = Vec::new();
        let mut seen = HashSet::new();
        let mut next = start;
        while let Some(e) = next {
            if out.len() >= limit {
                break;
            }
            if !seen.insert(e) {
                return Err(Error::Cycle(e));
            }
            let l = self.link(&e)?;
            next = prev(&l);
            out.push(self.listed(e, l, with_entries)?);
        }
        Ok((out, next))
    }

    /// One page of the global list, newest first, starting after `after` (or at the newest
    /// entry). Reads one `Link` per item (cached after the first time) plus each entry.
    pub fn list(&self, after: Option<&AccountId>, limit: usize) -> Result<Page> {
        let head = self.head(&addr::global_head(&self.reg))?;
        let start = match after {
            None => head.newest,
            Some(a) => self.link(a)?.prev_global,
        };
        let (items, rest) = self.walk(start, limit, true, |l| l.prev_global)?;
        let next = rest.and_then(|_| items.last().map(|i| i.address));
        Ok(Page { items, next, total: head.count })
    }

    /// Links of the global list, newest first, up to `scan` (no entry reads).
    pub fn scan_links(&self, scan: usize) -> Result<Vec<(AccountId, Link)>> {
        let head = self.head(&addr::global_head(&self.reg))?;
        let (items, _) = self.walk(head.newest, scan, false, |l| l.prev_global)?;
        Ok(items.into_iter().map(|i| (i.address, i.link)).collect())
    }

    /// Entries named exactly `name`, newest first (the name's own list).
    pub fn by_name(&self, name: &str, limit: usize) -> Result<Vec<Listed>> {
        if lezreg_types::pda::try_seed_str(name).is_none() {
            return Ok(Vec::new());
        }
        let head = self.head(&addr::name_head(&self.reg, name))?;
        Ok(self.walk(head.newest, limit, true, |l| l.prev_name)?.0)
    }

    /// Third-party entries for `program`, newest first.
    pub fn third_party(&self, program: &AccountId, limit: usize) -> Result<Vec<Listed>> {
        let head = self.head(&addr::program_head(&self.reg, program))?;
        Ok(self.walk(head.newest, limit, true, |l| l.prev_program)?.0)
    }

    /// Entries `author` holds now (registered by them or transferred to them, and not
    /// transferred away), newest first. Proven: `author` co-signed or accepted each one.
    pub fn by_author(&self, author: &AccountId, limit: usize) -> Result<Vec<Listed>> {
        let head = self.head(&addr::author_head(&self.reg, author))?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut next = head.newest;
        while let Some(e) = next {
            if out.len() >= limit {
                break;
            }
            if !seen.insert(e) {
                return Err(Error::Cycle(e));
            }
            let l = self.link(&e)?;
            next = if l.author == *author {
                l.prev_author
            } else {
                // Reached `author` by a transfer: its place in this list is an AuthorNode.
                let at = addr::author_node(&self.reg, author, &e);
                match self.state(&at)?.as_slice() {
                    [] => return Err(Error::MissingLink(e)),
                    b => decode::<AuthorNode>(&at, b, "author node")?.prev,
                }
            };
            let item = self.listed(e, l, true)?;
            if item.entry.as_ref().is_some_and(|en| en.author == *author) {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Name search: exact matches from the name's list first, then substring matches over the
    /// newest `scan` links of the global list (names are in the `Link`, so this reads no
    /// entries until a match).
    pub fn search_name(&self, q: &str, limit: usize, scan: usize) -> Result<Vec<Listed>> {
        let q = q.to_ascii_lowercase();
        let mut out = self.by_name(&q, limit)?;
        let mut have: HashSet<AccountId> = out.iter().map(|i| i.address).collect();
        if out.len() < limit {
            for (address, link) in self.scan_links(scan)? {
                if out.len() >= limit {
                    break;
                }
                if link.name != q && link.name.contains(&q) && have.insert(address) {
                    out.push(self.listed(address, link, true)?);
                }
            }
        }
        Ok(out)
    }

    /// Entries whose free-text `author_label` contains `q` (case-insensitive). **Unproven**:
    /// anyone can write any label.
    pub fn search_label(&self, q: &str, limit: usize, scan: usize) -> Result<Vec<Listed>> {
        let q = q.to_lowercase();
        self.filter_scan(limit, scan, |e| e.author_label.to_lowercase().contains(&q))
    }

    /// Entries carrying tag `tag`.
    pub fn search_tag(&self, tag: &str, limit: usize, scan: usize) -> Result<Vec<Listed>> {
        self.filter_scan(limit, scan, |e| e.tags.iter().any(|t| t == tag))
    }

    fn filter_scan(&self, limit: usize, scan: usize, keep: impl Fn(&Entry) -> bool) -> Result<Vec<Listed>> {
        let mut out = Vec::new();
        for (address, link) in self.scan_links(scan)? {
            if out.len() >= limit {
                break;
            }
            let item = self.listed(address, link, true)?;
            if item.entry.as_ref().is_some_and(&keep) {
                out.push(item);
            }
        }
        Ok(out)
    }
}
