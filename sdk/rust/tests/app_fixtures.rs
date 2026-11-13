//! Regression fixtures for the app check (docs/APPS.md): an outside reviewer's packages that
//! `lezreg app verify` used to pass (`mkpkgs-reviewer.py`), and packages with a correct root
//! (`mkfix.py`). Each is checked against a profile that vouches for the genuine root, the way
//! a name's `app.forumer_ui` entry would.

use lezreg_sdk::{
    apps::{self, Verdict},
    lgx::Contents,
    lns::Profile,
};

const GENUINE_ROOT: &str = "6e099b91e08bf8aeebf386a5118ee9da534af0682fad4d51379dea53c0737d98";
/// What the reviewer's packages claim, and the record they were checked against.
const CLAIMED_ROOT: &str = "0a9d5c4ac2011029aabbccddeeff00112233445566778899aabbccddeeff0011";

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
    for (name, via) in [("genuine", None), ("genuine-upper", None)] {
        let (v, from) = verdict(name);
        assert_eq!((v.status(), from.as_deref()), ("verified", via), "{name}");
        assert_eq!(hex::encode(contents(name).root), GENUINE_ROOT);
    }
    // With the record's sha256, the whole file (manifest included) is proven.
    assert!(matches!(verdict("genuine-sealed").0, Verdict::Verified { whole_file: true, .. }));
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
