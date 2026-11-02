//! Discovery by walking the registry's on-chain lists (DESIGN §2.5). Pure functions over a
//! state reader, so the engine tests, the smoke client and the SDK share one implementation.
//!
//! The reader returns the registry program's actor state at an account (empty if none), i.e.
//! `getAccountView(Actor(account, REG))` or `getAccount(account).actor_states[REG]`.

use crate::{AccountId, AuthorNode, Entry, Head, Link, pda::addr};

/// Reads the registry's actor state at an account.
pub trait RegistryState {
    fn registry_state(&self, account: &AccountId) -> Vec<u8>;
}

impl<F: Fn(&AccountId) -> Vec<u8>> RegistryState for F {
    fn registry_state(&self, account: &AccountId) -> Vec<u8> {
        self(account)
    }
}

/// A list walk that stopped on bad data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkError {
    MissingLink(AccountId),
    BadRecord(AccountId),
    Cycle(AccountId),
}

fn decode<T: borsh::BorshDeserialize>(at: &AccountId, state: &[u8]) -> Result<T, WalkError> {
    borsh::from_slice(state).map_err(|_| WalkError::BadRecord(*at))
}

pub fn head(s: &impl RegistryState, at: &AccountId) -> Result<Head, WalkError> {
    let st = s.registry_state(at);
    if st.is_empty() { Ok(Head { schema: crate::SCHEMA, count: 0, newest: None }) } else { decode(at, &st) }
}

pub fn entry(s: &impl RegistryState, at: &AccountId) -> Result<Option<Entry>, WalkError> {
    let st = s.registry_state(at);
    if st.is_empty() { Ok(None) } else { decode(at, &st).map(Some) }
}

pub fn link(s: &impl RegistryState, reg: &AccountId, entry: &AccountId) -> Result<Link, WalkError> {
    let at = addr::link(reg, entry);
    let st = s.registry_state(&at);
    if st.is_empty() {
        return Err(WalkError::MissingLink(*entry));
    }
    decode(&at, &st)
}

fn walk(
    s: &impl RegistryState,
    reg: &AccountId,
    head_at: &AccountId,
    limit: usize,
    mut prev: impl FnMut(&AccountId, &Link) -> Result<Option<AccountId>, WalkError>,
) -> Result<Vec<(AccountId, Link)>, WalkError> {
    let h = head(s, head_at)?;
    let mut out: Vec<(AccountId, Link)> = Vec::new();
    let mut next = h.newest;
    while let Some(e) = next {
        if out.len() >= limit {
            break;
        }
        if out.iter().any(|(x, _)| *x == e) {
            return Err(WalkError::Cycle(e));
        }
        let l = link(s, reg, &e)?;
        next = prev(&e, &l)?;
        out.push((e, l));
    }
    Ok(out)
}

/// Every entry, newest first.
pub fn global(s: &impl RegistryState, reg: &AccountId, limit: usize) -> Result<Vec<(AccountId, Link)>, WalkError> {
    walk(s, reg, &addr::global_head(reg), limit, |_, l| Ok(l.prev_global))
}

/// Entries with exactly this name, newest first.
pub fn by_name(s: &impl RegistryState, reg: &AccountId, name: &str, limit: usize) -> Result<Vec<(AccountId, Link)>, WalkError> {
    let Some(_) = crate::pda::try_seed_str(name) else { return Ok(Vec::new()) };
    walk(s, reg, &addr::name_head(reg, name), limit, |_, l| Ok(l.prev_name))
}

/// Third-party entries for `program`, newest first.
pub fn third_party(s: &impl RegistryState, reg: &AccountId, program: &AccountId, limit: usize) -> Result<Vec<(AccountId, Link)>, WalkError> {
    walk(s, reg, &addr::program_head(reg, program), limit, |_, l| Ok(l.prev_program))
}

/// Walks `author`'s list newest first, calling `visit` on each entry until it returns `false`.
/// Reads one record per step, so a caller that stops early reads only what it used.
fn walk_author_list(
    s: &impl RegistryState,
    reg: &AccountId,
    author: &AccountId,
    mut visit: impl FnMut(AccountId) -> Result<bool, WalkError>,
) -> Result<(), WalkError> {
    let h = head(s, &addr::author_head(reg, author))?;
    let mut seen = std::collections::HashSet::new();
    let mut next = h.newest;
    while let Some(e) = next {
        if !seen.insert(e) {
            return Err(WalkError::Cycle(e));
        }
        if !visit(e)? {
            break;
        }
        let l = link(s, reg, &e)?;
        next = if l.author == *author {
            l.prev_author
        } else {
            let at = addr::author_node(reg, author, &e);
            let st = s.registry_state(&at);
            if st.is_empty() {
                return Err(WalkError::MissingLink(e));
            }
            decode::<AuthorNode>(&at, &st)?.prev
        };
    }
    Ok(())
}

/// Entries whose list membership includes `author` (registered by them, or transferred to
/// them), newest first, **including** entries since transferred away. Use [`by_author`] for
/// the entries `author` holds now.
pub fn author_list(s: &impl RegistryState, reg: &AccountId, author: &AccountId, limit: usize) -> Result<Vec<AccountId>, WalkError> {
    let mut out: Vec<AccountId> = Vec::new();
    if limit == 0 {
        return Ok(out);
    }
    walk_author_list(s, reg, author, |e| {
        out.push(e);
        Ok(out.len() < limit)
    })?;
    Ok(out)
}

/// Entries `author` holds now, newest first. Stops reading once `limit` are found.
pub fn by_author(s: &impl RegistryState, reg: &AccountId, author: &AccountId, limit: usize) -> Result<Vec<(AccountId, Entry)>, WalkError> {
    let mut out = Vec::new();
    if limit == 0 {
        return Ok(out);
    }
    walk_author_list(s, reg, author, |e| {
        if let Some(entry) = entry(s, &e)?
            && entry.author == *author
        {
            out.push((e, entry));
        }
        Ok(out.len() < limit)
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::HashMap};

    use super::*;
    use crate::{ContentRef, SourceRef, BuildConfig};

    fn entry_rec(program: AccountId, author: AccountId) -> Entry {
        let c = ContentRef { cid: vec![1], sha256: [0; 32], size: 1, peer_hint: None };
        Entry {
            schema: 1, kind: 0, flags: 0, program, author, registrant: author, name: "n".into(), version: "1".into(),
            author_label: String::new(), description: String::new(), tags: vec![], idl: c,
            source: SourceRef { repo: String::new(), commit: vec![0; 20], docker_root: ".".into(), archive: None },
            build: BuildConfig {
                schema: 1, builder: 1, image_digest: [0; 32], image_tag: String::new(), risc0_build: String::new(),
                host_risc0_rust: String::new(), manifest_path: String::new(), binary: String::new(), features: vec![],
                lock_sha256: [0; 32], docker_env: vec![],
            },
            image_id: [0; 32], registered_at: 0, updated_at: 0, revision: 1,
        }
    }

    /// Security review F6: `by_author` stops reading once it has `limit` matches.
    #[test]
    fn by_author_reads_lazily() {
        let reg = AccountId([0xEE; 32]);
        let a = AccountId([0xA1; 32]);
        let mut st: HashMap<AccountId, Vec<u8>> = HashMap::new();
        let n = 500u16;
        let id = |i: u16| {
            let mut b = [0u8; 32];
            b[..2].copy_from_slice(&i.to_le_bytes());
            b[31] = 1;
            AccountId(b)
        };
        for i in 0..n {
            let e = id(i);
            st.insert(e, borsh::to_vec(&entry_rec(e, a)).unwrap());
            let link = Link {
                schema: 1, seq: u64::from(i), kind: 0, program: e, name: "n".into(), author: a, prev_global: None, prev_name: None,
                prev_author: i.checked_sub(1).map(id), prev_program: None,
            };
            st.insert(addr::link(&reg, &e), borsh::to_vec(&link).unwrap());
        }
        st.insert(addr::author_head(&reg, &a), borsh::to_vec(&Head { schema: 1, count: u64::from(n), newest: Some(id(n - 1)) }).unwrap());
        let reads = Cell::new(0usize);
        let r = |at: &AccountId| {
            reads.set(reads.get() + 1);
            st.get(at).cloned().unwrap_or_default()
        };
        let got = by_author(&r, &reg, &a, 2).unwrap();
        assert_eq!(got.iter().map(|(e, _)| *e).collect::<Vec<_>>(), vec![id(n - 1), id(n - 2)]);
        assert!(reads.get() <= 5, "{} reads for 2 results", reads.get());
        reads.set(0);
        assert_eq!(author_list(&r, &reg, &a, 3).unwrap(), vec![id(n - 1), id(n - 2), id(n - 3)]);
        assert!(reads.get() <= 4, "{} reads for 3 ids", reads.get());
        assert_eq!(author_list(&r, &reg, &a, usize::MAX).unwrap().len(), usize::from(n));
        assert!(by_author(&r, &reg, &a, 0).unwrap().is_empty());
    }
}
