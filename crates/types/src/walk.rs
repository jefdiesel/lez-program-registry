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

/// Entries whose list membership includes `author` (registered by them, or transferred to
/// them), newest first, **including** entries since transferred away. Use [`by_author`] for
/// the entries `author` holds now.
pub fn author_list(s: &impl RegistryState, reg: &AccountId, author: &AccountId, limit: usize) -> Result<Vec<AccountId>, WalkError> {
    let h = head(s, &addr::author_head(reg, author))?;
    let mut out: Vec<AccountId> = Vec::new();
    let mut next = h.newest;
    while let Some(e) = next {
        if out.len() >= limit {
            break;
        }
        if out.contains(&e) {
            return Err(WalkError::Cycle(e));
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
        out.push(e);
    }
    Ok(out)
}

/// Entries `author` holds now, newest first.
pub fn by_author(s: &impl RegistryState, reg: &AccountId, author: &AccountId, limit: usize) -> Result<Vec<(AccountId, Entry)>, WalkError> {
    let mut out = Vec::new();
    for e in author_list(s, reg, author, usize::MAX)? {
        if out.len() >= limit {
            break;
        }
        if let Some(entry) = entry(s, &e)?
            && entry.author == *author
        {
            out.push((e, entry));
        }
    }
    Ok(out)
}
