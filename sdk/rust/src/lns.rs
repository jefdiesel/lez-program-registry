//! Reading an LNS name (logos-names `SPEC.md` §3, §5, §7), as far as app verification needs it
//! (docs/APPS.md): derive a name's identifier and key, find its record, open its sealed
//! profile and read its text entries. Read only: nothing here writes a name.
//!
//! The chain stores only an identifier derived from each name and a profile sealed under a key
//! derived from it, so a record can't be read without the name. That is not secrecy: record
//! addresses are visible on chain and a guessable name can be found by trying it.

use aes_gcm::{
    Aes256Gcm, KeyInit as _, Nonce,
    aead::{Aead as _, Payload},
};
use argon2::{Algorithm, Argon2, Params, Version};
use lezreg_types::{
    AccountId,
    pda::{for_public_pda, sha256},
};

use crate::{
    error::{Error, Result},
    rpc::Rpc,
};

const DERIVATION_SALT: &[u8] = b"logos-names/v5";
const DERIVATION_MEMORY_KIB: u32 = 65_536;
const DERIVATION_PASSES: u32 = 3;
const DERIVATION_LANES: u32 = 1;
const ID_DOMAIN: &[u8] = b"logos-names/v5/id";
const KEY_DOMAIN: &[u8] = b"logos-names/v5/key";
const NAME_SEED_DOMAIN: &[u8] = b"logos-names/v5/name/";
const RECORD_SEAL: &[u8] = b"logos-names/v5/record";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
/// The program never stores a larger sealed profile; one that is larger didn't come from it.
const MAX_SEALED_RECORD_LEN: usize = 12 * 1024;
const MAX_NAME_LEN: usize = 32;
/// A fee-mode name stays registered this long after it expires (SPEC §8).
const GRACE_MS: u64 = 30 * 24 * 60 * 60 * 1000;
/// `expires` of a name held by a deposit: it never lapses.
const NEVER: u64 = u64::MAX;

/// What a name turns into: the identifier the chain knows it by, and the key that opens what
/// it resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub id: [u8; 32],
    pub key: [u8; 32],
}

/// `master = Argon2id(name, "logos-names/v5", 64 MiB, 3 passes, 1 lane)`;
/// `id = SHA-256("logos-names/v5/id" ‖ master)`, `key = SHA-256("logos-names/v5/key" ‖ master)`.
/// Slow and memory-hard on purpose (about a second), so callers should keep the result.
#[must_use]
pub fn derive(name: &str) -> Derived {
    let params = Params::new(DERIVATION_MEMORY_KIB, DERIVATION_PASSES, DERIVATION_LANES, Some(32))
        .expect("the Argon2 parameters are valid");
    let mut master = [0_u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(name.as_bytes(), DERIVATION_SALT, &mut master)
        .expect("Argon2 accepts these inputs");
    Derived {
        id: sha256(&[ID_DOMAIN, &master].concat()),
        key: sha256(&[KEY_DOMAIN, &master].concat()),
    }
}

/// A bare LNS name: 1–32 of `a-z`, `0-9`, `-`, not starting or ending with `-`.
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME_LEN
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
}

/// Why some text is not a name (see [`parse_name`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// Not 1–32 of `a-z 0-9 -` after normalising.
    Invalid,
    /// A valid name that also decodes as a 32-byte LEZ account ID: refused (SPEC §2).
    AccountId,
    /// Surrounded by whitespace that LNS clients trim differently (U+0085 and U+00A0 are
    /// trimmed by the Rust clients, U+FEFF by the JavaScript ones), so they would not agree on
    /// which name it is: refused rather than guessed.
    Ambiguous,
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "not an LNS name (1-32 of a-z, 0-9 or '-', not starting or ending with '-')",
            Self::AccountId => "also an account ID, so it can't be a name",
            Self::Ambiguous => "surrounded by whitespace LNS clients don't agree on",
        })
    }
}

/// The whitespace every LNS client trims alike (ASCII).
const fn is_shared_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r')
}

/// SPEC §2 normalisation: trim surrounding whitespace, lower ASCII case, remove one leading
/// `lns:`, then one leading `@`, then one trailing `.lez`. Only ASCII whitespace is trimmed
/// here; [`parse_name`] refuses input whose other surrounding whitespace clients disagree on.
#[must_use]
pub fn normalize_name(input: &str) -> String {
    let lowered = input.trim_matches(is_shared_space).to_ascii_lowercase();
    let bare = lowered.strip_prefix("lns:").unwrap_or(&lowered);
    let bare = bare.strip_prefix('@').unwrap_or(bare);
    bare.strip_suffix(".lez").unwrap_or(bare).to_owned()
}

/// Text as people or packages write a name (`Jef`, `@jef`, `lns:jef`, `jef.lez`) to the bare
/// name, exactly as logos-names SPEC.md §2 and the Names CLI (`parse_name`) do it: normalise,
/// then refuse anything that isn't a valid name or that also decodes as an account ID. One
/// difference, on purpose: input surrounded by non-ASCII whitespace is refused
/// ([`NameError::Ambiguous`]) where the Names clients disagree about trimming it.
pub fn parse_name(input: &str) -> std::result::Result<String, NameError> {
    let trimmed = input.trim_matches(is_shared_space);
    let edge = |c: Option<char>| c.is_some_and(|c| c.is_whitespace() || c == '\u{feff}');
    if edge(trimmed.chars().next()) || edge(trimmed.chars().last()) {
        return Err(NameError::Ambiguous);
    }
    let name = normalize_name(trimmed);
    if !is_valid_name(&name) {
        return Err(NameError::Invalid);
    }
    if AccountId::from_base58(&name).is_some() {
        return Err(NameError::AccountId);
    }
    Ok(name)
}

/// The account holding the record of the name `id` under the LNS program `lns`.
#[must_use]
pub fn record_address(lns: &AccountId, id: &[u8; 32]) -> AccountId {
    for_public_pda(lns, &sha256(&[NAME_SEED_DOMAIN, id].concat()))
}

/// What a name resolves to, as its holder sealed it: only the parts app checking reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub name: String,
    pub texts: Vec<(String, String)>,
}

/// What a profile holds under one text key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry<'a> {
    Absent,
    One(&'a str),
    /// The key is there more than once. Clients disagree on which one counts (the Rust ones
    /// take the first, the JavaScript ones the last), so whatever depends on it fails closed.
    Duplicate,
}

impl Profile {
    /// The value of the text entry `key`, if the name has exactly one. For a key that decides
    /// something (a forward, an app record) use [`Profile::entry`], which reports duplicates.
    #[must_use]
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.entry(key) {
            Entry::One(v) => Some(v),
            Entry::Absent | Entry::Duplicate => None,
        }
    }

    /// The text entry `key`: absent, one value, or there more than once.
    #[must_use]
    pub fn entry(&self, key: &str) -> Entry<'_> {
        let mut found = self.texts.iter().filter(|(k, _)| k == key);
        match (found.next(), found.next()) {
            (None, _) => Entry::Absent,
            (Some((_, v)), None) => Entry::One(v),
            (Some(_), Some(_)) => Entry::Duplicate,
        }
    }
}

/// A registered name's record, opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRecord {
    /// The holder's owner tag; the chain stores no account.
    pub owner_tag: [u8; 32],
    /// Milliseconds since the Unix epoch, or `u64::MAX` for a name held by a deposit.
    pub expires: u64,
    /// `None` when the record holds no profile this key opens for this name.
    pub profile: Option<Profile>,
}

impl NameRecord {
    /// Past its grace period, a fee-mode name counts as unregistered.
    #[must_use]
    pub fn gone(&self, now_ms: u64) -> bool {
        self.expires != NEVER && now_ms >= self.expires.saturating_add(GRACE_MS)
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(Error::Invalid("an LNS record ends early".into()));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<usize> {
        let b: [u8; 4] = self.take(4)?.try_into().expect("4 bytes");
        Ok(u32::from_le_bytes(b) as usize)
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8 bytes")))
    }
    fn bytes32(&mut self) -> Result<[u8; 32]> {
        Ok(self.take(32)?.try_into().expect("32 bytes"))
    }
    fn byte_vec(&mut self) -> Result<&'a [u8]> {
        let n = self.u32()?;
        self.take(n)
    }
    fn string(&mut self) -> Result<String> {
        String::from_utf8(self.byte_vec()?.to_vec())
            .map_err(|_| Error::Invalid("an LNS text is not UTF-8".into()))
    }
    fn option32(&mut self) -> Result<()> {
        match self.u8()? {
            0 => Ok(()),
            1 => self.take(32).map(|_| ()),
            _ => Err(Error::Invalid("an LNS option tag is not 0 or 1".into())),
        }
    }
}

/// Opens a sealed profile (`nonce ‖ AES-256-GCM(key, nonce, borsh(profile) padded, "logos-names/v5/record")`)
/// as the profile of `name`. Anything this key does not open, or a profile of another name,
/// is `None`: the name has nothing set.
#[must_use]
pub fn open_profile(key: &[u8; 32], sealed: &[u8], name: &str) -> Option<Profile> {
    if sealed.len() < NONCE_LEN + TAG_LEN || sealed.len() > MAX_SEALED_RECORD_LEN {
        return None;
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let plain = Aes256Gcm::new_from_slice(key)
        .expect("a 32-byte key")
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: RECORD_SEAL,
            },
        )
        .ok()?;
    let mut r = Reader(&plain);
    let profile_name = r.string().ok()?;
    r.option32().ok()?; // account
    r.option32().ok()?; // npk
    r.byte_vec().ok()?; // vpk
    let count = r.u32().ok()?;
    let mut texts = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        texts.push((r.string().ok()?, r.string().ok()?));
    }
    (profile_name == name).then_some(Profile {
        name: profile_name,
        texts,
    })
}

/// Decodes a record `{version, owner_tag, expires, sealed, claimed}` and opens its profile.
pub fn decode_record(bytes: &[u8], key: &[u8; 32], name: &str) -> Result<NameRecord> {
    let mut r = Reader(bytes);
    let version = r.u8()?;
    let owner_tag = r.bytes32()?;
    let expires = r.u64()?;
    let sealed = r.byte_vec()?;
    // SPEC §5: version 6 ends with `claimed`; earlier versions end at `sealed`. A version this
    // reader doesn't know, or bytes after the record, are refused rather than guessed at (the
    // Names CLI also refuses trailing bytes).
    match version {
        6 => {
            if r.u8()? > 1 {
                return Err(Error::Invalid("an LNS record's claimed flag is not 0 or 1".into()));
            }
        }
        1..=5 => {}
        v => return Err(Error::Invalid(format!("an LNS record of unknown version {v}"))),
    }
    if !r.0.is_empty() {
        return Err(Error::Invalid("an LNS record has bytes after its end".into()));
    }
    Ok(NameRecord {
        owner_tag,
        expires,
        profile: open_profile(key, sealed, name),
    })
}

/// Reads the name `name` (already derived) from the LNS program `lns`: `None` if it isn't
/// registered (no record, or a fee-mode name past its grace period at `now_ms`).
pub fn resolve<R: Rpc>(
    rpc: &R,
    lns: &AccountId,
    name: &str,
    derived: &Derived,
    now_ms: u64,
) -> Result<Option<NameRecord>> {
    let bytes = rpc.get_actor_state(&record_address(lns, &derived.id), lns)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    let record = decode_record(&bytes, &derived.key, name)?;
    Ok((!record.gone(now_ms)).then_some(record))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s).unwrap()
    }

    /// logos-names SPEC.md §10: derivation, the record address and a sealed profile.
    #[test]
    fn matches_the_lns_specification_vectors() {
        let jef = derive("jef");
        assert_eq!(
            jef.id.to_vec(),
            h("7041d9e447f06bdfe327d870402b1524cfe38bd03445b8fc3d0e87bc978eb763")
        );
        assert_eq!(
            jef.key.to_vec(),
            h("aad91a1a3dd0fb56a213212fbea58fe25d9139db42fc59ce2116c3cfa3440088")
        );
        assert_eq!(
            record_address(&AccountId([7; 32]), &jef.id).to_base58(),
            "5b5xVGob2GYvrq21MSuxm1MfieucDEy9yxhDwdXndbpD"
        );
        let sealed = h(concat!(
            "000102030405060708090a0b7a39f7ff67b20894263a28d040c0f2711c04ba247fcf5cb107e27b59",
            "05b8a0664b28cd25512addcb74af3efa5a8d6eb3bbb8e89fb8dba0aa2739252efed2be01ab019998",
            "a07d416c788d8c9b699891e85bedd1118da7089377220bdb0ab8ae29661bf20e006c5fbffb222237",
            "2ea0a2589ed49023186a77e9afee1af338f4a2377afa40da018789ca715318d981b556f71f9640eb",
            "d056d8dcbe2b71845b45a578721ccc2c889df37b926c7da5a5202dbf3d50dc04883673134ce0772f",
            "bcd7fbc8f31f455bb39b3d867d5e6772c86e59f42a9d04ffaacaa6119b1156d238665c0cf50028a9",
            "cbf8b3189fc45a15a724c5775a835a8ccdff6a573d8e046d78e847b7da5668bc8529aedb3bdc70a9",
            "9db9b9b4"
        ));
        let profile = open_profile(&jef.key, &sealed, "jef").expect("opens");
        assert_eq!(profile.text("chat"), Some("hi"));
        assert_eq!(
            open_profile(&jef.key, &sealed, "ada"),
            None,
            "another name's profile"
        );
        let mut tampered = sealed.clone();
        tampered[40] ^= 1;
        assert_eq!(open_profile(&jef.key, &tampered, "jef"), None);
    }

    #[test]
    fn names_follow_the_lns_rules() {
        for ok in ["jef", "a", "a-b-c", "abcdefghijklmnopqrstuvwxyz012345"] {
            assert!(is_valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "Jef", "a.b", &"a".repeat(33)] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    /// SPEC §2 normalisation, the cases the Names CLI and the JS client agree on, and the
    /// ones they don't (refused here).
    #[test]
    fn names_are_normalised_as_the_spec_says() {
        for (input, want) in [
            ("Jef", "jef"),
            ("@jef", "jef"),
            ("lns:jef", "jef"),
            ("Jef.LEZ", "jef"),
            (" jef \n", "jef"),
            ("LNS:@JEF.lez", "jef"),
            ("bobby.lez", "bobby"),
            ("\t@Bobby ", "bobby"),
            ("xn--jef", "xn--jef"),
        ] {
            assert_eq!(parse_name(input).as_deref(), Ok(want), "{input:?}");
        }
        let long = "a".repeat(33);
        for (input, why) in [
            ("jef.eth", NameError::Invalid),
            ("@lns:jef", NameError::Invalid),
            ("jef.lez.lez", NameError::Invalid),
            ("lns:", NameError::Invalid),
            (".lez", NameError::Invalid),
            ("j\u{200b}ef", NameError::Invalid),
            ("j\u{435}f", NameError::Invalid),
            (long.as_str(), NameError::Invalid),
            ("11111111111111111111111111111112", NameError::AccountId),
            ("1111111111111111111111111111111z", NameError::AccountId),
            ("\u{85}jef", NameError::Ambiguous),
            ("\u{feff}jef", NameError::Ambiguous),
            ("jef\u{a0}", NameError::Ambiguous),
            (" \u{2003}jef", NameError::Ambiguous),
        ] {
            assert_eq!(parse_name(input), Err(why), "{input:?}");
        }
        assert!(parse_name(&"a".repeat(32)).is_ok(), "32 a's is not base58 of 32 bytes");
    }

    #[test]
    fn records_of_unknown_shape_are_refused() {
        let key = [7_u8; 32];
        let record = |version: u8, tail: &[u8]| {
            let mut b = vec![version];
            b.extend([1_u8; 32]);
            b.extend(u64::MAX.to_le_bytes());
            b.extend(0_u32.to_le_bytes());
            b.extend(tail);
            b
        };
        assert!(decode_record(&record(6, &[0]), &key, "jef").is_ok());
        assert!(decode_record(&record(5, &[]), &key, "jef").is_ok());
        assert!(decode_record(&record(6, &[0, 0xab]), &key, "jef").is_err(), "trailing bytes");
        assert!(decode_record(&record(6, &[2]), &key, "jef").is_err(), "claimed is a bool");
        assert!(decode_record(&record(99, &[0]), &key, "jef").is_err(), "unknown version");
        assert!(decode_record(&record(6, &[]), &key, "jef").is_err(), "v6 without claimed");
    }

    #[test]
    fn duplicate_keys_are_reported() {
        let p = Profile {
            name: "x".into(),
            texts: vec![("forward".into(), "a".into()), ("forward".into(), "b".into()), ("chat".into(), "c".into())],
        };
        assert_eq!(p.entry("forward"), Entry::Duplicate);
        assert_eq!(p.text("forward"), None);
        assert_eq!(p.entry("chat"), Entry::One("c"));
        assert_eq!(p.entry("bio"), Entry::Absent);
    }
}
