//! Regression fixtures for the app check (docs/APPS.md): the first outside review's packages
//! that `lezreg app verify` used to pass (`mkpkgs-reviewer.py`), packages with a correct root
//! (`mkfix.py`), and the second review's bypasses rebuilt small (`battery/`, `mkbattery.py`:
//! the B1 polyglot, the archive battery b1-b9, the B2 edited manifest). Each is checked against
//! a profile that vouches for the genuine package, the way a name's `app.<module>` entry would.

use lezreg_sdk::{
    apps::{self, Verdict},
    lgx::{self, Contents},
    lns::Profile,
};

const GENUINE_ROOT: &str = "6e099b91e08bf8aeebf386a5118ee9da534af0682fad4d51379dea53c0737d98";
/// What the reviewer's packages claim, and the record they were checked against.
const CLAIMED_ROOT: &str = "0a9d5c4ac2011029aabbccddeeff00112233445566778899aabbccddeeff0011";

fn battery(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/apps/battery/{name}.lgx", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/apps/{name}.lgx", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn contents(name: &str) -> Contents {
    Contents::from_lgx(&read(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A chain where `forumer` vouches for both roots and `brand` forwards to it; `sealed` also
/// gives the sha256 of `genuine-sealed.lgx`.
fn profile_of(name: &str) -> lezreg_sdk::Result<Option<Profile>> {
    let texts: Vec<(String, String)> = match name {
        "forumer" => vec![("app.forumer_ui".into(), format!(r#"{{"v":1,"root":"{GENUINE_ROOT}","roots":["{CLAIMED_ROOT}"]}}"#))],
        "brand" => vec![("forward".into(), "forumer".into())],
        "oldfwd" => vec![("forward".into(), "bobby.lez".into()), ("app.forumer_ui".into(), format!(r#"{{"v":1,"root":"{}"}}"#, "ee".repeat(32)))],
        "bobby" => vec![],
        "sealed" => {
            let sha = hex::encode(lezreg_types::pda::sha256(&read("genuine-sealed")));
            vec![("app.forumer_ui".into(), format!(r#"{{"v":1,"root":"{GENUINE_ROOT}","sha256":"{sha}"}}"#))]
        }
        _ => return Ok(None),
    };
    Ok(Some(Profile { name: name.into(), texts }))
}

fn verdict(name: &str) -> (Verdict, Option<String>) {
    apps::check_following(&contents(name), profile_of).unwrap()
}

/// Case A: the genuine manifest.json next to another binary. Before: "verified".
#[test]
fn the_reviewers_packages_are_refused() {
    for name in ["evil-copied-manifest", "brand", "oldfwd", "upper"] {
        let c = contents(name);
        assert_ne!(c.root, c.manifest.root, "{name}: its files don't hash to its manifest's root");
        assert_eq!(verdict(name), (Verdict::RootMismatch, None), "{name}");
    }
}

#[test]
fn genuine_packages_still_verify() {
    // forumer's record has no sha256: the files only (W1: never exit-0 verified).
    for name in ["genuine", "genuine-upper"] {
        let (v, from) = verdict(name);
        assert_eq!((v.status(), v.exit_code(), from), ("files_verified", apps::EXIT_FILES_ONLY, None), "{name}");
        assert_eq!(hex::encode(contents(name).root), GENUINE_ROOT);
    }
    // With the record's sha256, the whole file (manifest included) is proven.
    let (v, _) = verdict("genuine-sealed");
    assert_eq!((v.status(), v.exit_code()), ("verified", 0));
}

/// The genuine files with another `main` in the manifest: the same root (the root doesn't
/// cover manifest.json), so only the record's sha256 catches it.
#[test]
fn an_edited_manifest_is_caught_by_the_file_hash() {
    assert_eq!(hex::encode(contents("edited-sealed").root), GENUINE_ROOT);
    assert_eq!(verdict("edited-sealed").0.status(), "file_mismatch");
}

/// A file directly under variants/ is outside lgx's Merkle tree (`uncovered.lgx` hashes to the
/// genuine root), so the package is refused rather than verified.
#[test]
fn a_file_the_root_does_not_cover_is_refused() {
    let e = Contents::from_lgx(&read("uncovered")).unwrap_err().to_string();
    assert!(e.contains("variants/evil.dylib") && e.contains("not covered"), "{e}");
}

/// The record a name would publish for the battery's genuine package (`pub.lgx`).
fn pub_record(fields: &[&str]) -> String {
    let bytes = battery("pub");
    let c = Contents::from_lgx(&bytes).unwrap();
    let mut o = serde_json::Map::new();
    o.insert("v".into(), 1.into());
    o.insert("root".into(), hex::encode(c.root).into());
    for f in fields {
        let v = match *f {
            "sha256" => hex::encode(lezreg_types::pda::sha256(&bytes)),
            "manifest" => hex::encode(c.manifest_sha256),
            "version" => c.manifest.version.clone(),
            _ => unreachable!(),
        };
        o.insert((*f).into(), v.into());
    }
    serde_json::Value::Object(o).to_string()
}

fn against(record: String) -> impl FnMut(&str) -> lezreg_sdk::Result<Option<Profile>> {
    move |name: &str| Ok((name == "forumer").then(|| Profile { name: name.into(), texts: vec![("app.forumer_core".into(), record.clone())] }))
}

/// B1 and the archive battery: a package some reader could read differently is refused outright,
/// whatever the record says (even a record whose sha256 is the polyglot's own).
#[test]
fn the_second_reviews_archives_are_refused() {
    for name in [
        "poly",
        "b1-hider-space-nul",
        "b2-trailing-garbage",
        "b3-two-gzip-members",
        "b4-second-archive",
        "b5-single-zero-block",
        "b6-type7-file",
        "b7-base256-size",
        "b8-checksum-leading-nul",
        "b9-size-junk",
    ] {
        let e = Contents::from_lgx(&battery(name)).err().unwrap_or_else(|| panic!("{name} was read"));
        assert!(!e.to_string().is_empty(), "{name}");
    }
    let pub_bytes = battery("pub");
    let genuine = Contents::from_lgx(&pub_bytes).unwrap();
    assert_eq!(apps::check_following(&genuine, against(pub_record(&["sha256", "manifest", "version"]))).unwrap().0.status(), "verified");
    // The reviewer's tarlib layout is readable but not lgx's own: `record` won't vouch for it.
    assert!(lgx::check_canonical_layout(&pub_bytes).is_err());
}

/// B2: pub's files with an edited manifest (version 999.0.0, an extra dependency).
#[test]
fn an_edited_manifest_is_refused() {
    let edited = Contents::from_lgx(&battery("edited")).unwrap();
    assert_eq!(edited.root, Contents::from_lgx(&battery("pub")).unwrap().root, "same files, same root");
    for (fields, want) in [
        (&["sha256", "manifest", "version"][..], "manifest_mismatch"),
        (&["sha256"][..], "file_mismatch"),
        (&["manifest"][..], "manifest_mismatch"),
        (&["version"][..], "manifest_mismatch"),
        (&[][..], "files_verified"),
    ] {
        let (v, _) = apps::check_following(&edited, against(pub_record(fields))).unwrap();
        assert_eq!(v.status(), want, "{fields:?}");
        assert_ne!(v.exit_code(), 0);
        assert_eq!(v.to_json(&edited)["version"], serde_json::Value::Null, "999.0.0 is never shown as published");
    }
}

