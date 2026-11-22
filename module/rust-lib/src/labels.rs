//! `author_label_proven`: an entry's `author_label` `lns:<name>` is proven when that LNS name is
//! registered and its profile's public account is the entry's author (what nameswallet.app/apps
//! shows as the author's "LNS name"). Everything else stays false: no LNS program configured, a
//! label that isn't `lns:`, a name that isn't registered or sets another account (or none), a
//! read that fails (a busy network included), and names past [`MAX_NEW_NAMES`] new derivations
//! in one call. Never true by default. Forwards are not followed.
//!
//! The same rule as lezreg-sdk's `apps::AuthorLabels` (the commit after f290894). The module pins
//! the SDK at f290894, whose `lns::Profile` drops the profile's public account, so the module
//! opens the sealed profile here; once the pin moves past that commit, this file becomes a call
//! to `apps::AuthorLabels`. Name validation, derivation, the record address and the record's
//! own checks (version, expiry, the profile naming this name) are still the pinned SDK's.

use std::collections::HashMap;

use aes_gcm::{
    Aes256Gcm, KeyInit as _, Nonce,
    aead::{Aead as _, Payload},
};
use lezreg_sdk::{
    Listed, Resolved, Rpc,
    json as js,
    lns::{self, Derived},
    types::{AccountId, Entry},
};
use serde_json::Value;

/// New names one call derives, at most: each is about a second of Argon2, and the module
/// answers one call at a time within a host's ~20 s. The rest stay unproven in that answer;
/// derivations are kept ([`Keys`]), so the next call proves them.
pub const MAX_NEW_NAMES: usize = 4;
/// Derived names kept across calls, at most (then all are dropped and derived again).
const MAX_KEYS: usize = 256;
const RECORD_SEAL: &[u8] = b"logos-names/v5/record";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// The LNS program never stores a larger sealed profile (lezreg-sdk `lns.rs`).
const MAX_SEALED_RECORD_LEN: usize = 12 * 1024;
/// `version u8 ‖ owner_tag [32] ‖ expires u64` before the sealed profile's length.
const SEALED_AT: usize = 1 + 32 + 8;

/// Derived names, kept across calls: a name's derivation never changes.
#[derive(Default)]
pub struct Keys(HashMap<String, Derived>);

/// The labels of one answer (one `list_entries`, one `resolve_program`), checked against `lns`.
pub struct Check<'a, R> {
    keys: &'a mut Keys,
    rpc: R,
    lns: Option<AccountId>,
    now_ms: u64,
    new_left: usize,
    /// Name → its public account, for this answer only (`None` also for a failed read).
    seen: HashMap<String, Option<AccountId>>,
}

impl<'a, R: Rpc> Check<'a, R> {
    pub fn new(keys: &'a mut Keys, rpc: R, lns: Option<AccountId>, now_ms: u64) -> Self {
        Self { keys, rpc, lns, now_ms, new_left: MAX_NEW_NAMES, seen: HashMap::new() }
    }

    /// Whether `entry`'s author label is a proven LNS name of its author.
    pub fn proven(&mut self, entry: &Entry) -> bool {
        let Some(lns) = self.lns else { return false };
        let Some(name) = label_name(&entry.author_label) else { return false };
        if let Some(account) = self.seen.get(&name) {
            return *account == Some(entry.author);
        }
        if !self.keys.0.contains_key(&name) {
            if self.new_left == 0 {
                return false;
            }
            self.new_left -= 1;
            if self.keys.0.len() >= MAX_KEYS {
                self.keys.0.clear();
            }
            self.keys.0.insert(name.clone(), lns::derive(&name));
        }
        let derived = self.keys.0[&name].clone();
        let account = public_account(&self.rpc, &lns, &name, &derived, self.now_ms).unwrap_or(None);
        self.seen.insert(name, account);
        account == Some(entry.author)
    }

    /// `json::listed`, `author_label_proven` checked.
    pub fn listed(&mut self, item: &Listed) -> Value {
        let mut v = js::listed(item);
        if let Some(e) = &item.entry {
            v["entry"]["author_label_proven"] = Value::Bool(self.proven(e));
        }
        v
    }

    /// `json::resolved`, the deployer entry's `author_label_proven` checked.
    pub fn resolved(&mut self, r: &Resolved) -> Value {
        let mut v = js::resolved(r);
        if let Some(e) = &r.deployer {
            v["deployer_entry"]["author_label_proven"] = Value::Bool(self.proven(e));
        }
        v
    }
}

/// The LNS name a label claims: `lns:<name>` (the prefix in any case, the name as
/// `lns::parse_name` reads it), else `None`.
pub fn label_name(label: &str) -> Option<String> {
    let start = label.trim_start_matches([' ', '\t', '\n', '\u{0b}', '\u{0c}', '\r']);
    let prefixed = start.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("lns:"));
    prefixed.then(|| lns::parse_name(label).ok()).flatten()
}

/// The public account `name`'s holder set, if the name is registered (the SDK's own checks)
/// and its profile, opened with the name's key, sets one.
fn public_account<R: Rpc>(rpc: &R, lns: &AccountId, name: &str, derived: &Derived, now_ms: u64) -> lezreg_sdk::Result<Option<AccountId>> {
    let Some(record) = lns::resolve(rpc, lns, name, derived, now_ms)? else { return Ok(None) };
    if record.profile.is_none() {
        return Ok(None);
    }
    // The same read again: answered from the module's read cache (net.rs), not the network.
    let bytes = rpc.get_actor_state(&lns::record_address(lns, &derived.id), lns)?;
    Ok(sealed_of(&bytes).and_then(|sealed| account_in(&derived.key, sealed, name)))
}

fn sealed_of(record: &[u8]) -> Option<&[u8]> {
    let len = u32::from_le_bytes(record.get(SEALED_AT..SEALED_AT + 4)?.try_into().ok()?) as usize;
    record.get(SEALED_AT + 4..SEALED_AT.checked_add(4)?.checked_add(len)?)
}

/// Opens `nonce ‖ AES-256-GCM(key, nonce, borsh(profile) padded, "logos-names/v5/record")` and
/// reads `profile.name` (must be `name`) and `profile.account` (logos-names SPEC.md §5).
fn account_in(key: &[u8; 32], sealed: &[u8], name: &str) -> Option<AccountId> {
    if sealed.len() < NONCE_LEN + TAG_LEN || sealed.len() > MAX_SEALED_RECORD_LEN {
        return None;
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let plain = Aes256Gcm::new_from_slice(key).ok()?.decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad: RECORD_SEAL }).ok()?;
    let len = u32::from_le_bytes(plain.get(..4)?.try_into().ok()?) as usize;
    if plain.get(4..4usize.checked_add(len)?)? != name.as_bytes() {
        return None;
    }
    match plain.get(4 + len)? {
        0 => None,
        1 => Some(AccountId(plain.get(5 + len..37 + len)?.try_into().ok()?)),
        _ => None,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use lezreg_sdk::MockRpc;
    use lezreg_sdk::types::{BuildConfig, ContentRef, Link, SourceRef};

    pub const LNS: AccountId = AccountId([0x4C; 32]);

    /// A name's record as the LNS program stores it (SPEC.md §5, v6), its profile sealed under
    /// the name's key, `account` its public account.
    pub fn name_record(name: &str, account: Option<AccountId>) -> Vec<u8> {
        let derived = lns::derive(name);
        let mut plain = Vec::new();
        plain.extend((name.len() as u32).to_le_bytes());
        plain.extend(name.as_bytes());
        match account {
            Some(a) => {
                plain.push(1);
                plain.extend(a.0);
            }
            None => plain.push(0),
        }
        plain.push(0); // no npk
        plain.extend(0_u32.to_le_bytes()); // no vpk
        plain.extend(0_u32.to_le_bytes()); // no texts
        plain.resize(plain.len().div_ceil(256) * 256, 0);
        let nonce = [3_u8; 12];
        let sealed = Aes256Gcm::new_from_slice(&derived.key).unwrap().encrypt(Nonce::from_slice(&nonce), Payload { msg: &plain, aad: RECORD_SEAL }).unwrap();
        let sealed = [nonce.as_slice(), &sealed].concat();
        let mut record = vec![6];
        record.extend([1_u8; 32]);
        record.extend(u64::MAX.to_le_bytes());
        record.extend((sealed.len() as u32).to_le_bytes());
        record.extend(&sealed);
        record.push(0);
        record
    }

    pub fn put_name(rpc: &MockRpc, name: &str, account: Option<AccountId>) {
        rpc.set(lns::record_address(&LNS, &lns::derive(name).id), LNS, name_record(name, account));
    }

    fn acct(n: u8) -> AccountId {
        AccountId([n; 32])
    }

    pub fn entry(author: AccountId, label: &str) -> Entry {
        Entry {
            schema: 1, kind: lezreg_sdk::types::KIND_DEPLOYER, flags: 0, program: acct(9), author, registrant: author,
            name: "app".into(), version: "1.0.0".into(), author_label: label.into(), description: String::new(), tags: vec![],
            idl: ContentRef::default(), source: SourceRef::default(), build: BuildConfig::default(), image_id: [0; 32],
            registered_at: 1, updated_at: 1, revision: 1,
        }
    }

    #[test]
    fn a_label_is_proven_only_by_the_names_public_account() {
        let rpc = MockRpc::new();
        let (me, other) = (acct(2), acct(3));
        put_name(&rpc, "names", Some(me));
        put_name(&rpc, "bare", None);
        let mut keys = Keys::default();
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        for (label, author, want) in [
            ("lns:names", me, true),
            ("LNS:Names", me, true),
            ("lns:names", other, false), // a different account
            ("lns:ghost", me, false),    // no such name
            ("lns:bare", me, false),     // registered, no public account set
            ("names", me, false),        // not an lns: label
            ("lns:not a name", me, false),
            ("", me, false),
        ] {
            assert_eq!(c.proven(&entry(author, label)), want, "{label} by {}", author.to_base58());
        }
        // No LNS program configured: never proven, nothing read.
        rpc.reset_reads();
        let mut none = Check::new(&mut keys, &rpc, None, 0);
        assert!(!none.proven(&entry(me, "lns:names")));
        assert_eq!(rpc.reads(), 0);
    }

    #[test]
    fn a_name_is_read_once_per_answer_and_a_failed_read_is_unproven() {
        let rpc = MockRpc::new();
        let me = acct(2);
        put_name(&rpc, "names", Some(me));
        let mut keys = Keys::default();
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        rpc.reset_reads();
        for _ in 0..8 {
            assert!(c.proven(&entry(me, "lns:names")));
        }
        assert_eq!(rpc.reads(), 2, "eight entries: one record read and its (cached in net.rs) re-read");
        *rpc.fail.lock().unwrap() = Some("connection refused".into());
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        assert!(!c.proven(&entry(me, "lns:names")), "a failed read is unproven, not an error");
    }

    #[test]
    fn new_names_per_answer_are_capped_and_kept_for_the_next() {
        let rpc = MockRpc::new();
        let me = acct(2);
        for i in 0..=MAX_NEW_NAMES {
            put_name(&rpc, &format!("pub{i}"), Some(me));
        }
        let mut keys = Keys::default();
        let labels: Vec<String> = (0..=MAX_NEW_NAMES).map(|i| format!("lns:pub{i}")).collect();
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        let first: Vec<bool> = labels.iter().map(|l| c.proven(&entry(me, l))).collect();
        assert_eq!(first.iter().filter(|p| **p).count(), MAX_NEW_NAMES, "{first:?}");
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        assert!(labels.iter().all(|l| c.proven(&entry(me, l))));
    }

    /// The views the module answers with: `list_entries`' items and `resolve_program`.
    #[test]
    fn the_listed_view_carries_the_check() {
        let rpc = MockRpc::new();
        let me = acct(2);
        put_name(&rpc, "names", Some(me));
        let link = Link { schema: 1, seq: 0, kind: 1, program: acct(9), name: "app".into(), author: me, prev_global: None, prev_name: None, prev_author: None, prev_program: None };
        let item = |label: &str| Listed { address: acct(9), link: link.clone(), entry: Some(entry(me, label)), kind: None };
        let mut keys = Keys::default();
        let mut c = Check::new(&mut keys, &rpc, Some(LNS), 0);
        assert_eq!(c.listed(&item("lns:names"))["entry"]["author_label_proven"], true);
        assert_eq!(c.listed(&item("lns:ghost"))["entry"]["author_label_proven"], false);
        assert_eq!(js::listed(&item("lns:names"))["entry"]["author_label_proven"], false, "the SDK view alone never says true");
        let none = Listed { address: acct(9), link, entry: None, kind: None };
        assert_eq!(c.listed(&none)["entry"], Value::Null);
    }
}
