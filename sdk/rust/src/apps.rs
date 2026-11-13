//! Apps found by LNS name and verified by publisher (docs/APPS.md).
//!
//! An app (a Basecamp module `M`) published by the LNS name `N` is **verified** when all three
//! agree: `N`'s profile has exactly one text entry `app.M` naming a package root, the package's
//! manifest says `"author": "lns:N"`, and the root **recomputed from the package's files**
//! ([`crate::lgx::Contents`]) is that root. Only `N`'s holder can change `N`'s profile, so a copy
//! can claim `lns:N` but can't make `N` vouch for its files. `N` may be held privately: the
//! publisher stays anonymous and is still provably the same publisher across updates.
//!
//! The root covers a package's files, not its `manifest.json` (see [`crate::lgx`]). For a `.lgx`
//! file the record's `sha256` covers the rest: when the record has one, the file must match it.

use lezreg_types::AccountId;
use serde_json::{Map, Value, json};

use crate::{
    error::{Error, Result},
    lgx::Contents,
    lns::{self, Derived, Entry, NameError, Profile},
    rpc::Rpc,
};

/// The text-entry key prefix: `app.<module>`.
pub const APP_KEY_PREFIX: &str = "app.";
/// `author` in a package manifest that names its publisher: `lns:<name>`.
pub const AUTHOR_PREFIX: &str = "lns:";
/// LNS text entries hold at most this many bytes (logos-names SPEC.md §5).
pub const MAX_VALUE: usize = 512;
/// LNS text keys are at most 32 bytes, so module names at most 28.
pub const MAX_MODULE: usize = 32 - APP_KEY_PREFIX.len();
/// How many roots one record may vouch for (newest first), so an older install stays verified
/// while an upgrade rolls out.
pub const MAX_ROOTS: usize = 4;
/// How many program ids one record may list.
pub const MAX_PROGRAMS: usize = 8;

/// What a name says about one of its apps: the value of `app.<module>`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppRecord {
    /// Package root hashes the name vouches for, newest first; at least one.
    pub roots: Vec<[u8; 32]>,
    pub version: Option<String>,
    /// The catalog's `logos-repo.json` URL (https): one place to download it.
    pub catalog: Option<String>,
    /// SHA-256 of the `.lgx` file itself. It covers what the root doesn't (the manifest), so a
    /// download is accepted only with it ([`check_download`]), and a `.lgx` checked against a
    /// record that has it must match it.
    pub sha256: Option<[u8; 32]>,
    /// The `.lgx` on Logos Storage: a download that depends on no web host.
    pub cid: Option<String>,
    /// LEZ programs the app calls; each one's source status comes from this registry.
    pub programs: Vec<AccountId>,
}

fn hex32(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text.trim())
        .map_err(|_| Error::Invalid(format!("not a 32-byte hex hash: {text}")))?;
    bytes
        .try_into()
        .map_err(|_| Error::Invalid(format!("not a 32-byte hex hash: {text}")))
}

/// A module name an `app.` key can carry: 1..=28 bytes of `a-z 0-9 - _ .` (an LNS text key).
#[must_use]
pub fn is_module_name(module: &str) -> bool {
    !module.is_empty()
        && module.len() <= MAX_MODULE
        && module
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
}

/// A JSON object whose keys are all different. `serde_json` (like `JSON.parse`) keeps the last
/// of two equal keys, and other readers may keep the first, so a record with one is refused.
struct UniqueObject(Map<String, Value>);

impl<'de> serde::Deserialize<'de> for UniqueObject {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visit;
        impl<'de> serde::de::Visitor<'de> for Visit {
            type Value = UniqueObject;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> std::result::Result<UniqueObject, A::Error> {
                let mut m = Map::new();
                while let Some((k, v)) = a.next_entry::<String, Value>()? {
                    if m.contains_key(&k) {
                        return Err(serde::de::Error::custom(format!("the key {k:?} appears twice")));
                    }
                    m.insert(k, v);
                }
                Ok(UniqueObject(m))
            }
        }
        d.deserialize_map(Visit)
    }
}

impl AppRecord {
    /// Parses an `app.<module>` value: `{"v":1, "root"|"roots", "version"?, "catalog"?,
    /// "sha256"?, "cid"?, "programs"?}`. A key given twice is refused.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() > MAX_VALUE {
            return Err(Error::Invalid("an app record is at most 512 bytes".into()));
        }
        let o = serde_json::from_str::<UniqueObject>(value)
            .map_err(|e| Error::Invalid(format!("an app record is a JSON object with each key once: {e}")))?
            .0;
        if o.get("v").and_then(Value::as_u64) != Some(1) {
            return Err(Error::Invalid("an app record has \"v\": 1".into()));
        }
        let mut roots = Vec::new();
        if let Some(root) = o.get("root").and_then(Value::as_str) {
            roots.push(hex32(root)?);
        }
        for root in o.get("roots").and_then(Value::as_array).into_iter().flatten() {
            let root = hex32(root.as_str().unwrap_or_default())?;
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        if roots.is_empty() || roots.len() > MAX_ROOTS {
            return Err(Error::Invalid(format!(
                "an app record names 1 to {MAX_ROOTS} package roots"
            )));
        }
        let text = |key: &str| o.get(key).and_then(Value::as_str).map(str::to_owned);
        let catalog = text("catalog");
        if catalog.as_deref().is_some_and(|c| !c.starts_with("https://")) {
            return Err(Error::Invalid("an app record's catalog is an https:// URL".into()));
        }
        let sha256 = match o.get("sha256") {
            Some(h) => Some(hex32(h.as_str().ok_or_else(|| Error::Invalid("an app record's sha256 is hex".into()))?)?),
            None => None,
        };
        let cid = text("cid");
        if cid.as_deref().is_some_and(|c| !is_cid(c)) {
            return Err(Error::Invalid("an app record's cid is not a Logos Storage CID".into()));
        }
        let mut programs = Vec::new();
        for p in o.get("programs").and_then(Value::as_array).into_iter().flatten() {
            let id = p
                .as_str()
                .and_then(AccountId::from_base58)
                .ok_or_else(|| Error::Invalid("an app record's programs are base58 ids".into()))?;
            programs.push(id);
        }
        if programs.len() > MAX_PROGRAMS {
            return Err(Error::Invalid(format!(
                "an app record lists at most {MAX_PROGRAMS} programs"
            )));
        }
        Ok(Self {
            roots,
            version: text("version"),
            catalog,
            sha256,
            cid,
            programs,
        })
    }

    /// The value to publish as `app.<module>`, refused if it is over 512 bytes.
    pub fn to_value(&self) -> Result<String> {
        if self.roots.is_empty() || self.roots.len() > MAX_ROOTS {
            return Err(Error::Invalid(format!(
                "an app record names 1 to {MAX_ROOTS} package roots"
            )));
        }
        let mut o = Map::new();
        o.insert("v".into(), json!(1));
        o.insert("root".into(), json!(hex::encode(self.roots[0])));
        if self.roots.len() > 1 {
            o.insert(
                "roots".into(),
                json!(self.roots[1..].iter().map(hex::encode).collect::<Vec<_>>()),
            );
        }
        if let Some(v) = &self.version {
            o.insert("version".into(), json!(v));
        }
        if let Some(c) = &self.catalog {
            o.insert("catalog".into(), json!(c));
        }
        if let Some(h) = &self.sha256 {
            o.insert("sha256".into(), json!(hex::encode(h)));
        }
        if let Some(c) = &self.cid {
            o.insert("cid".into(), json!(c));
        }
        if !self.programs.is_empty() {
            o.insert(
                "programs".into(),
                json!(self.programs.iter().map(AccountId::to_base58).collect::<Vec<_>>()),
            );
        }
        let value = Value::Object(o).to_string();
        if value.len() > MAX_VALUE {
            return Err(Error::Invalid(format!(
                "the app record is {} bytes; an LNS text entry holds {MAX_VALUE}",
                value.len()
            )));
        }
        Ok(value)
    }
}

impl AppRecord {
    /// Whether `bytes` (a downloaded `.lgx`) are the file this record vouches for: `None` when
    /// the record names no `sha256`.
    #[must_use]
    pub fn matches_download(&self, bytes: &[u8]) -> Option<bool> {
        self.sha256.map(|want| lezreg_types::pda::sha256(bytes) == want)
    }
}

/// A Logos Storage CID as text: multibase base58btc (`z…`) or another plain alphanumeric
/// encoding, 10 to 100 characters.
#[must_use]
pub fn is_cid(text: &str) -> bool {
    (10..=100).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The SHA-256 a CID itself names for its content, where it names one: a raw-codec sha2-256
/// CID. A Logos Storage CID (codec `0xcd01`) hashes Storage's manifest of the content's block
/// tree, not the bytes, so it can't be checked against downloaded bytes here: `None`.
#[must_use]
pub fn cid_content_sha256(cid: &str) -> Option<[u8; 32]> {
    crate::cid::parse(cid).ok().and_then(|c| crate::cid::raw_sha256(&c))
}

/// Checks a downloaded `.lgx` against the record that names it, before anything is written:
/// the record must give `sha256` (the root alone leaves the manifest unchecked, so a download
/// without it is refused as unverifiable), the bytes must match it and the CID where the CID
/// names a content hash, the package's recomputed root must be its manifest's and one the record
/// vouches for, and it must be `module`.
pub fn check_download(module: &str, record: &AppRecord, bytes: &[u8]) -> Result<Contents> {
    let want = record.sha256.ok_or_else(|| {
        Error::Invalid(format!(
            "unverifiable download: the record for {module} gives no sha256 of the .lgx, and the package root doesn't cover its manifest"
        ))
    })?;
    let have = lezreg_types::pda::sha256(bytes);
    if have != want {
        return Err(Error::Invalid(format!(
            "the file is not the one the record vouches for (sha256 {}, the record says {})",
            hex::encode(have),
            hex::encode(want)
        )));
    }
    if record.cid.as_deref().and_then(cid_content_sha256).is_some_and(|named| named != have) {
        return Err(Error::Invalid("the file is not the content its CID names".into()));
    }
    let contents = Contents::from_lgx(bytes)?;
    if !contents.root_matches() {
        return Err(Error::Invalid(format!(
            "the package's files don't match its manifest (recomputed root {}, the manifest says {})",
            hex::encode(contents.root),
            hex::encode(contents.manifest.root)
        )));
    }
    if contents.manifest.name != module || !record.roots.contains(&contents.root) {
        return Err(Error::Invalid(format!(
            "the package is not the {module} the record vouches for (module {}, root {})",
            contents.manifest.name,
            hex::encode(contents.root)
        )));
    }
    Ok(contents)
}

/// Every `app.<module>` entry of a profile, parsed (a malformed one is kept as its error; a
/// module whose key is there more than once is one error: the record is ambiguous).
#[must_use]
pub fn apps_of(profile: &Profile) -> Vec<(String, Result<AppRecord>)> {
    let mut out: Vec<(String, Result<AppRecord>)> = Vec::new();
    for (k, v) in &profile.texts {
        let Some(module) = k.strip_prefix(APP_KEY_PREFIX) else { continue };
        if let Some(seen) = out.iter_mut().find(|(m, _)| m == module) {
            seen.1 = Err(Error::Invalid(format!("ambiguous record: {k} is set more than once")));
        } else {
            out.push((module.to_owned(), AppRecord::parse(v)));
        }
    }
    out
}

/// The parts of a package's `manifest.json` that verification reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub author: String,
    /// The root the manifest claims. Never trusted alone: [`Contents`] recomputes it.
    pub root: [u8; 32],
}

impl Manifest {
    /// From a `manifest.json` (an installed module's, or the one inside a `.lgx`).
    pub fn parse(manifest: &Value) -> Result<Self> {
        let text = |k: &str| manifest.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        let root = manifest
            .pointer("/hashes/root")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Invalid("the manifest has no hashes.root".into()))?;
        let name = text("name");
        if name.is_empty() {
            return Err(Error::Invalid("the manifest has no name".into()));
        }
        Ok(Self {
            name,
            version: text("version"),
            author: text("author"),
            root: hex32(root)?,
        })
    }

    /// The LNS name the package says publishes it: `author` = `lns:<name>`, normalised as
    /// logos-names SPEC.md §2 says (`lns:Forumer`, `lns:@forumer`, `lns:forumer.lez` all name
    /// `forumer`). `None` (unclaimed) without the `lns:` prefix, or when the rest is not a name.
    #[must_use]
    pub fn publisher(&self) -> Option<String> {
        self.claim().ok().flatten()
    }

    /// [`Manifest::publisher`], with why a `lns:` claim is not a name.
    pub fn claim(&self) -> std::result::Result<Option<String>, NameError> {
        let trimmed = self.author.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\u{0b}');
        if !trimmed.get(..AUTHOR_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(AUTHOR_PREFIX)) {
            return Ok(None);
        }
        // parse_name removes the one leading `lns:` itself: `lns:lns:x` stays invalid.
        lns::parse_name(&self.author).map(Some)
    }
}

/// What checking a package against its publisher's name found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The name vouches for these files. `whole_file`: the record's `sha256` also matched the
    /// `.lgx` file, so its manifest is the publisher's too; otherwise only the files (which is
    /// all an installed package can show) are proven.
    Verified { name: String, record: AppRecord, whole_file: bool },
    /// The name publishes this module, but not these files: a copy, or a release the name
    /// hasn't vouched for yet.
    Mismatch { name: String, record: AppRecord },
    /// The name vouches for these files but the `.lgx` file is not the one it names (`sha256`):
    /// the manifest, or how it was packed, was changed.
    FileMismatch { name: String, record: AppRecord },
    /// The package's files are not the ones its own manifest names: refused before any lookup.
    RootMismatch,
    /// The name's records can be read two ways (a key set twice, or a forward whose whitespace
    /// LNS clients disagree on), so they vouch for nothing.
    Ambiguous { name: String, why: String },
    /// The name exists but says nothing about this module (or its record is malformed).
    NotPublished { name: String },
    /// The package names a publisher that isn't a registered LNS name.
    NoSuchName { name: String },
    /// The package names no LNS publisher.
    Unclaimed,
}

impl Verdict {
    /// One word for the verdict: `verified`, `mismatch`, `file_mismatch`, `root_mismatch`,
    /// `ambiguous`, `not_published`, `no_such_name`, `unclaimed`.
    #[must_use]
    pub const fn status(&self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified",
            Self::Mismatch { .. } => "mismatch",
            Self::FileMismatch { .. } => "file_mismatch",
            Self::RootMismatch => "root_mismatch",
            Self::Ambiguous { .. } => "ambiguous",
            Self::NotPublished { .. } => "not_published",
            Self::NoSuchName { .. } => "no_such_name",
            Self::Unclaimed => "unclaimed",
        }
    }

    /// The verdict as JSON: `{status, publisher, module, version, root, manifest_root,
    /// whole_file, not_covered, why, record}`. `root` is the recomputed root.
    #[must_use]
    pub fn to_json(&self, contents: &Contents) -> Value {
        let (publisher, record) = match self {
            Self::Verified { name, record, .. } | Self::Mismatch { name, record } | Self::FileMismatch { name, record } => {
                (Some(name.clone()), Some(record))
            }
            Self::NotPublished { name } | Self::NoSuchName { name } | Self::Ambiguous { name, .. } => (Some(name.clone()), None),
            Self::RootMismatch | Self::Unclaimed => (None, None),
        };
        let manifest = &contents.manifest;
        let why = match self {
            Self::Ambiguous { why, .. } => Some(why.clone()),
            _ => None,
        };
        json!({
            "status": self.status(),
            "publisher": publisher,
            "module": manifest.name,
            "version": manifest.version,
            "root": hex::encode(contents.root),
            "manifest_root": hex::encode(manifest.root),
            "whole_file": matches!(self, Self::Verified { whole_file: true, .. }),
            "not_covered": contents.not_covered,
            "why": why,
            "record": record.map(|r| json!({
                "version": r.version,
                "catalog": r.catalog,
                "sha256": r.sha256.map(hex::encode),
                "cid": r.cid,
                "programs": r.programs.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
            })),
        })
    }
}

/// Checks a package against the records of `name` (already resolved to `profile`): no
/// forwarding here.
fn check_record(contents: &Contents, name: String, profile: &Profile) -> Verdict {
    let key = format!("{APP_KEY_PREFIX}{}", contents.manifest.name);
    let value = match profile.entry(&key) {
        Entry::Absent => return Verdict::NotPublished { name },
        Entry::Duplicate => return Verdict::Ambiguous { name, why: format!("{key} is set more than once") },
        Entry::One(v) => v,
    };
    // A malformed record vouches for nothing.
    let Ok(record) = AppRecord::parse(value) else {
        return Verdict::NotPublished { name };
    };
    if !record.roots.contains(&contents.root) {
        return Verdict::Mismatch { name, record };
    }
    match (record.sha256, contents.file_sha256()) {
        (Some(want), Some(have)) if want != have => Verdict::FileMismatch { name, record },
        (Some(_), Some(_)) => Verdict::Verified { name, record, whole_file: true },
        _ => Verdict::Verified { name, record, whole_file: false },
    }
}

/// Checks a package against what its claimed publisher's name says, without following a
/// `forward`. `profile_of` reads a registered name's profile (`Ok(None)`: not registered).
pub fn check(
    contents: &Contents,
    profile_of: impl FnOnce(&str) -> Result<Option<Profile>>,
) -> Result<Verdict> {
    if !contents.root_matches() {
        return Ok(Verdict::RootMismatch);
    }
    let Some(name) = contents.manifest.publisher() else {
        return Ok(Verdict::Unclaimed);
    };
    let Some(profile) = profile_of(&name)? else {
        return Ok(Verdict::NoSuchName { name });
    };
    Ok(check_record(contents, name, &profile))
}

/// The LNS text entry that points a name to another (logos-names SPEC.md §7, Forwarding).
pub const FORWARD_KEY: &str = "forward";

/// Where a name's profile forwards, by SPEC.md §7: no entry, an entry that isn't a valid
/// name (an account ID included), or the name itself, is no forward; otherwise the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forward {
    None,
    To(String),
    /// Two `forward` entries, or one whose surrounding whitespace LNS clients trim differently
    /// (so one client would follow it and another wouldn't): refused.
    Ambiguous(String),
}

/// Where `profile` (of `name`) forwards to.
#[must_use]
pub fn forward_of(name: &str, profile: &Profile) -> Forward {
    match profile.entry(FORWARD_KEY) {
        Entry::Absent => Forward::None,
        Entry::Duplicate => Forward::Ambiguous("forward is set more than once".into()),
        Entry::One(value) => match lns::parse_name(value) {
            Ok(target) if target != name => Forward::To(target),
            Ok(_) | Err(NameError::Invalid | NameError::AccountId) => Forward::None,
            Err(NameError::Ambiguous) => Forward::Ambiguous(format!("forward {value:?} has whitespace LNS clients trim differently")),
        },
    }
}

/// [`check`], following the publisher's `forward` one hop (SPEC.md §7): a package naming
/// `lns:nameswallet` is checked against `names` when `nameswallet` forwards there. Returns the
/// verdict (naming the name whose records decided it) and the name it was forwarded from.
pub fn check_following(
    contents: &Contents,
    mut profile_of: impl FnMut(&str) -> Result<Option<Profile>>,
) -> Result<(Verdict, Option<String>)> {
    if !contents.root_matches() {
        return Ok((Verdict::RootMismatch, None));
    }
    let Some(name) = contents.manifest.publisher() else {
        return Ok((Verdict::Unclaimed, None));
    };
    let Some(profile) = profile_of(&name)? else {
        return Ok((Verdict::NoSuchName { name }, None));
    };
    match forward_of(&name, &profile) {
        Forward::None => Ok((check_record(contents, name, &profile), None)),
        Forward::Ambiguous(why) => Ok((Verdict::Ambiguous { name, why }, None)),
        Forward::To(target) => {
            let Some(forwarded) = profile_of(&target)? else {
                return Ok((Verdict::NoSuchName { name: target }, Some(name)));
            };
            Ok((check_record(contents, target, &forwarded), Some(name)))
        }
    }
}

/// Reads LNS names from a sequencer for [`check`], keeping each name's (slow) derivation.
pub struct Names<R> {
    rpc: R,
    lns: AccountId,
    derived: std::collections::HashMap<String, Derived>,
}

impl<R: Rpc> Names<R> {
    pub fn new(rpc: R, lns: AccountId) -> Self {
        Self {
            rpc,
            lns,
            derived: std::collections::HashMap::new(),
        }
    }

    /// A registered name's profile at `now_ms`, or `None`. `name` is normalised first (SPEC.md
    /// §2: `Forumer`, `@forumer`, `lns:forumer`, `forumer.lez`); text that isn't a name, or is
    /// an account ID, is an error. A name with a record this key can't open counts as
    /// registered with nothing set.
    pub fn profile(&mut self, name: &str, now_ms: u64) -> Result<Option<Profile>> {
        let name = lns::parse_name(name).map_err(|e| Error::Invalid(format!("{name:?}: {e}")))?;
        let derived = self
            .derived
            .entry(name.clone())
            .or_insert_with(|| lns::derive(&name))
            .clone();
        Ok(
            lns::resolve(&self.rpc, &self.lns, &name, &derived, now_ms)?.map(|record| {
                record.profile.unwrap_or_else(|| Profile {
                    name: name.clone(),
                    texts: Vec::new(),
                })
            }),
        )
    }

    /// [`check_following`] against the chain: the verdict, and the name it forwarded from.
    pub fn verify(&mut self, contents: &Contents, now_ms: u64) -> Result<(Verdict, Option<String>)> {
        check_following(contents, |name| self.profile(name, now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lgx::tests::{FILES, genuine, genuine_with, lgx};

    const ROOT: &str = "0a9d5c4ac2011029aabbccddeeff00112233445566778899aabbccddeeff0011";

    /// A genuine package (correct root) claiming `author`, with its root.
    fn package(author: &str) -> (Contents, String) {
        let (bytes, root) = genuine(author, FILES);
        (Contents::from_lgx(&bytes).unwrap(), root)
    }

    /// The same publisher's files, a different build: another root.
    fn other_build(author: &str) -> Contents {
        let mut files = FILES.to_vec();
        files[0].1 = b"another build";
        Contents::from_lgx(&genuine(author, &files).0).unwrap()
    }

    fn record(root: &str) -> String {
        format!(r#"{{"v":1,"root":"{root}","version":"1.2.0"}}"#)
    }

    fn profile(name: &str, entries: &[(&str, String)]) -> Profile {
        Profile {
            name: name.into(),
            texts: entries.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect(),
        }
    }

    /// A name's record as the LNS program stores it, with `texts` sealed under the name's key,
    /// the same way logos-names seals a profile (SPEC.md §5).
    fn sealed_record(name: &str, texts: &[(&str, String)]) -> Vec<u8> {
        use aes_gcm::{
            Aes256Gcm, KeyInit as _, Nonce,
            aead::{Aead as _, Payload},
        };
        let derived = crate::lns::derive(name);
        let mut plain = Vec::new();
        let string = |out: &mut Vec<u8>, s: &str| {
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        };
        string(&mut plain, name);
        plain.extend([0, 0]); // no account, no npk
        plain.extend(0_u32.to_le_bytes()); // no vpk
        plain.extend((texts.len() as u32).to_le_bytes());
        for (k, v) in texts {
            string(&mut plain, k);
            string(&mut plain, v);
        }
        plain.resize(plain.len().div_ceil(256) * 256, 0);
        let nonce = [9_u8; 12];
        let sealed = Aes256Gcm::new_from_slice(&derived.key)
            .unwrap()
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: &plain, aad: b"logos-names/v5/record" })
            .unwrap();
        let sealed = [nonce.as_slice(), &sealed].concat();
        let mut record = vec![6];
        record.extend([1_u8; 32]); // owner tag
        record.extend(u64::MAX.to_le_bytes()); // held by a deposit
        record.extend((sealed.len() as u32).to_le_bytes());
        record.extend(&sealed);
        record.push(0);
        record
    }

    /// The whole path against a (mocked) chain: derive, read the record, open it, check.
    #[test]
    fn a_package_is_checked_against_the_name_on_chain() {
        let lns = AccountId([7; 32]);
        let rpc = crate::rpc::MockRpc::new();
        let (real, root) = package("lns:forumer");
        let good = format!(r#"{{"v":1,"root":"{root}","version":"1.2.0","programs":["{}"]}}"#, AccountId([5; 32]).to_base58());
        let put = |name: &str, texts: &[(&str, String)]| {
            rpc.set(crate::lns::record_address(&lns, &crate::lns::derive(name).id), lns, sealed_record(name, texts));
        };
        put("forumer", &[("app.forumer_ui", good)]);
        let mut names = Names::new(&rpc, lns);
        let (verdict, via) = names.verify(&real, 0).unwrap();
        assert_eq!((verdict.status(), via), ("verified", None));
        assert_eq!(verdict.to_json(&real)["record"]["programs"][0], AccountId([5; 32]).to_base58());
        assert_eq!(names.verify(&other_build("lns:forumer"), 0).unwrap().0.status(), "mismatch");
        assert_eq!(names.verify(&package("lns:nobody").0, 0).unwrap().0.status(), "no_such_name");
        // The author is normalised as SPEC.md §2 says.
        for author in ["lns:Forumer", " lns:@forumer ", "LNS:forumer.lez"] {
            assert_eq!(names.verify(&package(author).0, 0).unwrap().0.status(), "verified", "{author}");
        }
        // A brand name forwarding to the publisher: checked against the publisher's records.
        put("forumerapp", &[("forward", "forumer".into())]);
        let (verdict, via) = names.verify(&package("lns:forumerapp").0, 0).unwrap();
        assert_eq!((verdict.status(), via.as_deref()), ("verified", Some("forumerapp")));
        assert_eq!(verdict.to_json(&real)["publisher"], "forumer");
        // The legacy suffix in a forward is normalised away (the Names CLI follows it too).
        put("oldfwd", &[("forward", "Forumer.lez".into()), ("app.forumer_ui", record(&"ee".repeat(32)))]);
        let (verdict, via) = names.verify(&package("lns:oldfwd").0, 0).unwrap();
        assert_eq!((verdict.status(), via.as_deref()), ("verified", Some("oldfwd")));
        // A record sealed for another name (copied to this address) opens as nothing set.
        put("copycat", &[]);
        rpc.set(crate::lns::record_address(&lns, &crate::lns::derive("copycat").id), lns, sealed_record("forumer", &[("app.forumer_ui", record(ROOT))]));
        assert_eq!(names.verify(&package("lns:copycat").0, 0).unwrap().0.status(), "not_published");
        // Text that is an account ID is never read as a name.
        assert!(names.profile("11111111111111111111111111111112", 0).is_err());
    }

    /// The reviewer's case A: the genuine manifest.json next to other files.
    #[test]
    fn a_copied_manifest_over_other_files_is_refused_before_any_lookup() {
        let (genuine_bytes, root) = genuine("lns:forumer", FILES);
        let real = Contents::from_lgx(&genuine_bytes).unwrap();
        let manifest = serde_json::json!({
            "name": "forumer_ui", "version": "1.2.0", "author": "lns:forumer", "hashes": {"root": root}
        })
        .to_string();
        let evil = Contents::from_lgx(&lgx(&[
            ("manifest.json", Some(manifest.as_bytes())),
            ("variants/darwin-arm64/forumer_ui_plugin.dylib", Some(b"MALICIOUS")),
        ]))
        .unwrap();
        assert_eq!(evil.manifest.root, real.root, "it claims the genuine root");
        let no_lookup = |_: &str| -> Result<Option<Profile>> { panic!("no lookup for a package whose files don't match its manifest") };
        assert_eq!(check(&evil, no_lookup).unwrap(), Verdict::RootMismatch);
        assert_eq!(check_following(&evil, no_lookup).unwrap(), (Verdict::RootMismatch, None));
    }

    #[test]
    fn only_the_vouched_files_are_verified() {
        let (real, root) = package("lns:forumer");
        let named = |entries: Vec<(&'static str, String)>| {
            move |name: &str| {
                assert_eq!(name, "forumer");
                Ok(Some(profile(name, &entries)))
            }
        };
        let v = check(&real, named(vec![("app.forumer_ui", record(&root))])).unwrap();
        assert_eq!(v, Verdict::Verified { name: "forumer".into(), record: AppRecord::parse(&record(&root)).unwrap(), whole_file: false });
        assert_eq!(v.to_json(&real)["publisher"], "forumer");
        assert_eq!(v.to_json(&real)["not_covered"], json!(["manifest.json"]));

        // Another build claiming the same publisher: its root isn't vouched for.
        let v = check(&other_build("lns:forumer"), named(vec![("app.forumer_ui", record(&root))])).unwrap();
        assert_eq!(v.status(), "mismatch");

        // The name publishes something else, or a malformed record: vouches for nothing.
        assert_eq!(check(&real, named(vec![("app.other", record(&root))])).unwrap().status(), "not_published");
        assert_eq!(check(&real, named(vec![("app.forumer_ui", "{".into())])).unwrap().status(), "not_published");
        assert_eq!(check(&real, |_| Ok(None)).unwrap().status(), "no_such_name");
        // Case D: the key twice (Rust readers took the first, JS ones the last): refused.
        for pair in [[record(&root), record(&"ee".repeat(32))], [record(&"ee".repeat(32)), record(&root)]] {
            let v = check(&real, named(vec![("app.forumer_ui", pair[0].clone()), ("app.forumer_ui", pair[1].clone())])).unwrap();
            assert_eq!(v.status(), "ambiguous");
        }
        // A key twice inside the record's JSON: malformed, vouches for nothing.
        let twice = format!(r#"{{"v":1,"root":"{}","root":"{root}"}}"#, "ee".repeat(32));
        assert!(AppRecord::parse(&twice).unwrap_err().to_string().contains("twice"));
        assert_eq!(check(&real, named(vec![("app.forumer_ui", twice)])).unwrap().status(), "not_published");

        for author in ["", "forumer", "lns:", "lns:Not_Valid", "dns:forumer", "lns:lns:forumer", "lns:11111111111111111111111111111112", "lns:\u{85}forumer"] {
            let (c, _) = package(author);
            assert_eq!(check(&c, |_| panic!("no lookup")).unwrap(), Verdict::Unclaimed, "{author:?}");
        }
        let p = profile("x", &[("app.a", "x".into()), ("chat", "hi".into()), ("app.b", record(ROOT)), ("app.c", record(ROOT)), ("app.c", record(ROOT))]);
        let apps = apps_of(&p);
        assert_eq!(apps.len(), 3);
        assert!(apps[0].1.is_err() && apps[1].1.is_ok());
        assert!(apps[2].1.as_ref().unwrap_err().to_string().contains("ambiguous"));
        assert!(is_module_name("forumer_ui") && !is_module_name(&"a".repeat(29)) && !is_module_name("Bad"));
    }

    /// The root doesn't cover manifest.json: a record's sha256 does, for a .lgx.
    #[test]
    fn the_file_hash_covers_the_manifest() {
        let (bytes, root) = genuine("lns:forumer", FILES);
        let real = Contents::from_lgx(&bytes).unwrap();
        let sha = hex::encode(lezreg_types::pda::sha256(&bytes));
        let with_sha = format!(r#"{{"v":1,"root":"{root}","sha256":"{sha}"}}"#);
        let named = |v: String| move |n: &str| Ok(Some(profile(n, &[("app.forumer_ui", v)])));
        let v = check(&real, named(with_sha.clone())).unwrap();
        assert!(matches!(v, Verdict::Verified { whole_file: true, .. }));
        // The same files, a manifest with another `main`: the same root, another file.
        let (edited, edited_root) = genuine_with("lns:forumer", FILES, |m| m["main"]["darwin-arm64"] = json!("qml/Main.qml"));
        assert_eq!(edited_root, root, "the root does not cover the manifest");
        let edited = Contents::from_lgx(&edited).unwrap();
        assert_eq!(check(&edited, named(with_sha)).unwrap().status(), "file_mismatch");
        // Without a sha256 in the record only the files can be proven, and the verdict says so.
        let v = check(&edited, named(record(&root))).unwrap();
        assert!(matches!(v, Verdict::Verified { whole_file: false, .. }));
    }

    /// SPEC.md §7 forwarding, and the cases the Names clients disagree on (case C, D).
    #[test]
    fn forwards_follow_the_spec() {
        let (pkg, root) = package("lns:brand");
        let names = |entries: Vec<(&'static str, Vec<(&'static str, String)>)>| {
            move |name: &str| Ok(entries.iter().find(|(n, _)| *n == name).map(|(n, t)| profile(n, t)))
        };
        let forumer = ("forumer", vec![("app.forumer_ui", record(&root))]);
        let with_forward = |value: &str| names(vec![("brand", vec![("forward", value.to_owned())]), forumer.clone()]);
        for value in ["forumer", "Forumer", "@forumer", "lns:forumer", "forumer.lez", " forumer\n"] {
            let (v, via) = check_following(&pkg, with_forward(value)).unwrap();
            assert_eq!((v.status(), via.as_deref()), ("verified", Some("brand")), "{value:?}");
        }
        // Not a name, an account ID, or the name itself: ignored, `brand` is checked as itself.
        for value in ["", "not a name", "11111111111111111111111111111112", "brand", "jef.eth"] {
            let (v, via) = check_following(&pkg, with_forward(value)).unwrap();
            assert_eq!((v.status(), via), ("not_published", None), "{value:?}");
        }
        // Whitespace one client trims and another doesn't: refused.
        for value in ["\u{85}forumer", "\u{feff}forumer", "forumer\u{a0}"] {
            let (v, _) = check_following(&pkg, with_forward(value)).unwrap();
            assert_eq!(v.status(), "ambiguous", "{value:?}");
        }
        // Two forward entries: refused, whichever way round.
        let two = names(vec![("brand", vec![("forward", "forumer".into()), ("forward", "bobby".into())]), forumer.clone()]);
        assert_eq!(check_following(&pkg, two).unwrap().0.status(), "ambiguous");
        // A forward to a name that isn't registered: refused, naming it.
        let (v, via) = check_following(&pkg, names(vec![("brand", vec![("forward", "gone".into())])])).unwrap();
        assert_eq!((v, via.as_deref()), (Verdict::NoSuchName { name: "gone".into() }, Some("brand")));
        // One hop only: the target's own forward is not followed.
        let chain = names(vec![("brand", vec![("forward", "middle".into())]), ("middle", vec![("forward", "forumer".into())]), forumer]);
        assert_eq!(check_following(&pkg, chain).unwrap().0.status(), "not_published");
    }

    #[test]
    fn a_download_needs_its_sha256() {
        let (bytes, root) = genuine("lns:forumer", FILES);
        let sha = lezreg_types::pda::sha256(&bytes);
        let mut r = AppRecord { roots: vec![hex32(&root).unwrap()], sha256: Some(sha), ..AppRecord::default() };
        assert!(check_download("forumer_ui", &r, &bytes).is_ok());
        assert!(check_download("other_ui", &r, &bytes).is_err());
        // An edited manifest over the same files: same root, refused by the file hash.
        let (edited, _) = genuine_with("lns:forumer", FILES, |m| m["version"] = json!("9.9.9"));
        assert!(check_download("forumer_ui", &r, &edited).unwrap_err().to_string().contains("sha256"));
        // A raw sha2-256 CID names the bytes, so it is checked too.
        r.cid = Some(crate::cid::to_string(&crate::cid::raw_cid(b"something else")));
        assert!(check_download("forumer_ui", &r, &bytes).unwrap_err().to_string().contains("CID"));
        r.cid = Some(crate::cid::to_string(&crate::cid::raw_cid(&bytes)));
        assert!(check_download("forumer_ui", &r, &bytes).is_ok());
        // No sha256: unverifiable, even though the root would match.
        r.sha256 = None;
        assert!(check_download("forumer_ui", &r, &bytes).unwrap_err().to_string().contains("unverifiable"));
    }

    #[test]
    fn a_record_round_trips_and_its_limits_hold() {
        let record = AppRecord {
            roots: vec![[1; 32], [2; 32]],
            version: Some("1.2.0".into()),
            catalog: Some("https://raw.githubusercontent.com/o/r/main/logos-repo.json".into()),
            sha256: Some(lezreg_types::pda::sha256(b"the package")),
            cid: Some("zDvZRwzm8K4nJ2a7q9WTqN1kZt5v7wHcX1yYbP3sQ4rT6uE8f".into()),
            programs: vec![AccountId([7; 32])],
        };
        let value = record.to_value().unwrap();
        assert_eq!(AppRecord::parse(&value).unwrap(), record);
        assert!(value.len() <= MAX_VALUE, "{} bytes", value.len());
        assert_eq!(record.matches_download(b"the package"), Some(true));
        assert_eq!(record.matches_download(b"a copy"), Some(false));
        assert_eq!(AppRecord { sha256: None, ..record.clone() }.matches_download(b"x"), None);
        assert!(AppRecord::parse(&format!(r#"{{"v":1,"root":"{ROOT}","cid":"not a cid!"}}"#)).is_err());
        assert!(AppRecord::parse(&format!(r#"{{"v":1,"root":"{ROOT}","sha256":"00"}}"#)).is_err());
        assert!(AppRecord::parse(&format!(r#"{{"v":1,"root":"{ROOT}","sha256":7}}"#)).is_err());
        for bad in [
            r#"{"v":2,"root":"00"}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"root":"zz"}"#,
            &format!(r#"{{"v":1,"root":"{ROOT}","catalog":"http://x"}}"#),
            &format!(r#"{{"v":1,"root":"{ROOT}","programs":["not base58!"]}}"#),
            &format!(r#"{{"v":1,"v":1,"root":"{ROOT}"}}"#),
            "not json",
            "[1]",
        ] {
            assert!(AppRecord::parse(bad).is_err(), "{bad}");
        }
        let too_big = AppRecord {
            roots: vec![[1; 32]; 4],
            programs: vec![AccountId([7; 32]); 8],
            ..record
        };
        assert!(too_big.to_value().is_err(), "over 512 bytes");
    }
}
