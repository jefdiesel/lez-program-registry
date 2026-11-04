//! Apps found by LNS name and verified by publisher (docs/APPS.md).
//!
//! An app (a Basecamp module `M`) published by the LNS name `N` is **verified** when all three
//! agree: `N`'s profile has the text entry `app.M` naming the package's root hash, the package's
//! manifest says `"author": "lns:N"`, and the installed package's `hashes.root` is that root.
//! Only `N`'s holder can change `N`'s profile, so a copy can claim `lns:N` but can't make `N`
//! vouch for its hash. `N` may be held privately: the publisher stays anonymous and is still
//! provably the same publisher across updates.

use lezreg_types::AccountId;
use serde_json::{Map, Value, json};

use crate::{
    error::{Error, Result},
    lns::{self, Derived, Profile},
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
    /// The catalog's `logos-repo.json` URL (https).
    pub catalog: Option<String>,
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

impl AppRecord {
    /// Parses an `app.<module>` value: `{"v":1, "root"|"roots", "version"?, "catalog"?, "programs"?}`.
    pub fn parse(value: &str) -> Result<Self> {
        if value.len() > MAX_VALUE {
            return Err(Error::Invalid("an app record is at most 512 bytes".into()));
        }
        let v: Value = serde_json::from_str(value)
            .map_err(|_| Error::Invalid("an app record is not JSON".into()))?;
        let o = v
            .as_object()
            .ok_or_else(|| Error::Invalid("an app record is a JSON object".into()))?;
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

/// Every `app.<module>` entry of a profile, parsed (a malformed one is kept as its error).
#[must_use]
pub fn apps_of(profile: &Profile) -> Vec<(String, Result<AppRecord>)> {
    profile
        .texts
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix(APP_KEY_PREFIX)
                .map(|m| (m.to_owned(), AppRecord::parse(v)))
        })
        .collect()
}

/// The parts of a package's `manifest.json` that verification reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub author: String,
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

    /// The LNS name the package says publishes it: `author` = `lns:<name>`.
    #[must_use]
    pub fn publisher(&self) -> Option<String> {
        let name = self.author.trim().strip_prefix(AUTHOR_PREFIX)?;
        lns::is_valid_name(name).then(|| name.to_owned())
    }
}

/// What checking a package against its publisher's name found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The name vouches for this exact package.
    Verified { name: String, record: AppRecord },
    /// The name publishes this module, but not this package: a copy, or a release the name
    /// hasn't vouched for yet.
    Mismatch { name: String, record: AppRecord },
    /// The name exists but says nothing about this module.
    NotPublished { name: String },
    /// The package names a publisher that isn't a registered LNS name.
    NoSuchName { name: String },
    /// The package names no LNS publisher.
    Unclaimed,
}

impl Verdict {
    /// One word for the verdict: `verified`, `mismatch`, `not_published`, `no_such_name`,
    /// `unclaimed`.
    #[must_use]
    pub const fn status(&self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified",
            Self::Mismatch { .. } => "mismatch",
            Self::NotPublished { .. } => "not_published",
            Self::NoSuchName { .. } => "no_such_name",
            Self::Unclaimed => "unclaimed",
        }
    }

    /// The verdict as JSON: `{status, publisher, module, version, root, record}`.
    #[must_use]
    pub fn to_json(&self, manifest: &Manifest) -> Value {
        let (publisher, record) = match self {
            Self::Verified { name, record } | Self::Mismatch { name, record } => {
                (Some(name.clone()), Some(record))
            }
            Self::NotPublished { name } | Self::NoSuchName { name } => (Some(name.clone()), None),
            Self::Unclaimed => (None, None),
        };
        json!({
            "status": self.status(),
            "publisher": publisher,
            "module": manifest.name,
            "version": manifest.version,
            "root": hex::encode(manifest.root),
            "record": record.map(|r| json!({
                "version": r.version,
                "catalog": r.catalog,
                "programs": r.programs.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
            })),
        })
    }
}

/// Checks a package's manifest against what its claimed publisher's name says. `profile_of`
/// reads a registered name's profile (`Ok(None)`: not registered).
pub fn check(
    manifest: &Manifest,
    profile_of: impl FnOnce(&str) -> Result<Option<Profile>>,
) -> Result<Verdict> {
    let Some(name) = manifest.publisher() else {
        return Ok(Verdict::Unclaimed);
    };
    let Some(profile) = profile_of(&name)? else {
        return Ok(Verdict::NoSuchName { name });
    };
    let Some(value) = profile.text(&format!("{APP_KEY_PREFIX}{}", manifest.name)) else {
        return Ok(Verdict::NotPublished { name });
    };
    // A malformed record vouches for nothing.
    let Ok(record) = AppRecord::parse(value) else {
        return Ok(Verdict::NotPublished { name });
    };
    Ok(if record.roots.contains(&manifest.root) {
        Verdict::Verified { name, record }
    } else {
        Verdict::Mismatch { name, record }
    })
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

    /// A registered name's profile at `now_ms`, or `None`. A name with a record this key
    /// can't open counts as registered with nothing set.
    pub fn profile(&mut self, name: &str, now_ms: u64) -> Result<Option<Profile>> {
        if !lns::is_valid_name(name) {
            return Err(Error::Invalid(format!("not an LNS name: {name}")));
        }
        let derived = self
            .derived
            .entry(name.to_owned())
            .or_insert_with(|| lns::derive(name))
            .clone();
        Ok(
            lns::resolve(&self.rpc, &self.lns, name, &derived, now_ms)?.map(|record| {
                record.profile.unwrap_or_else(|| Profile {
                    name: name.to_owned(),
                    texts: Vec::new(),
                })
            }),
        )
    }

    /// [`check`] against the chain.
    pub fn verify(&mut self, manifest: &Manifest, now_ms: u64) -> Result<Verdict> {
        check(manifest, |name| self.profile(name, now_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "0a9d5c4ac2011029aabbccddeeff00112233445566778899aabbccddeeff0011";

    fn manifest(author: &str, root: &str) -> Manifest {
        Manifest::parse(&json!({
            "name": "forumer_ui", "version": "1.2.0", "author": author, "hashes": { "root": root }
        }))
        .unwrap()
    }

    fn profile(entries: &[(&str, &str)]) -> Profile {
        Profile {
            name: "forumer".into(),
            texts: entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn a_record_round_trips_and_its_limits_hold() {
        let record = AppRecord {
            roots: vec![[1; 32], [2; 32]],
            version: Some("1.2.0".into()),
            catalog: Some("https://raw.githubusercontent.com/o/r/main/logos-repo.json".into()),
            programs: vec![AccountId([7; 32])],
        };
        let value = record.to_value().unwrap();
        assert_eq!(AppRecord::parse(&value).unwrap(), record);
        assert!(value.len() <= MAX_VALUE);
        for bad in [
            r#"{"v":2,"root":"00"}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"root":"zz"}"#,
            &format!(r#"{{"v":1,"root":"{ROOT}","catalog":"http://x"}}"#),
            &format!(r#"{{"v":1,"root":"{ROOT}","programs":["not base58!"]}}"#),
            "not json",
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

    #[test]
    fn only_the_vouched_package_is_verified() {
        let good = format!(r#"{{"v":1,"root":"{ROOT}","version":"1.2.0"}}"#);
        let named = |entries: Vec<(&'static str, String)>| {
            move |name: &str| {
                assert_eq!(name, "forumer");
                Ok(Some(Profile {
                    name: name.into(),
                    texts: entries.iter().map(|(k, v)| ((*k).into(), v.clone())).collect(),
                }))
            }
        };
        let real = manifest("lns:forumer", ROOT);
        let v = check(&real, named(vec![("app.forumer_ui", good.clone())])).unwrap();
        assert_eq!(v.status(), "verified");
        assert_eq!(v.to_json(&real)["publisher"], "forumer");

        // A copy claiming the same publisher: its root isn't vouched for.
        let copy = manifest("lns:forumer", &"ff".repeat(32));
        let v = check(&copy, named(vec![("app.forumer_ui", good.clone())])).unwrap();
        assert_eq!(v.status(), "mismatch");

        // The name publishes something else, or a malformed record: vouches for nothing.
        assert_eq!(
            check(&real, named(vec![("app.other", good.clone())])).unwrap().status(),
            "not_published"
        );
        assert_eq!(
            check(&real, named(vec![("app.forumer_ui", "{".into())])).unwrap().status(),
            "not_published"
        );
        assert_eq!(
            check(&real, |_| Ok(None)).unwrap().status(),
            "no_such_name"
        );
        for author in ["", "forumer", "lns:", "lns:Not-Valid", "dns:forumer"] {
            assert_eq!(
                check(&manifest(author, ROOT), |_| panic!("no lookup")).unwrap(),
                Verdict::Unclaimed,
                "{author}"
            );
        }
        let p = profile(&[("app.a", "x"), ("chat", "hi"), ("app.b", &good)]);
        let apps = apps_of(&p);
        assert_eq!(apps.len(), 2);
        assert!(apps[0].1.is_err() && apps[1].1.is_ok());
        assert!(is_module_name("forumer_ui") && !is_module_name(&"a".repeat(29)) && !is_module_name("Bad"));
    }
}
