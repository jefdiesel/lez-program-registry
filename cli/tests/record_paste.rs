//! Review 3 (B-3 and its delta): the `names set-text` line `lezreg app record` prints is pasted
//! into a shell by the publisher. This runs the real binary, takes the line it printed, pastes
//! it into `sh` with `names` stubbed, and checks that nothing ran and that the value `names`
//! received is the `--json` record byte for byte. A catalog that isn't a plain URL is refused
//! before anything is printed.
use std::process::Command;

const PACKAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../sdk/rust/tests/fixtures/apps/review3/canon-ok.lgx");

fn lezreg(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lezreg")).env("HOME", home).env_remove("LEZREG_LNS").args(args).output().unwrap()
}

#[test]
fn the_printed_record_line_pastes_as_the_record() {
    let dir = std::env::temp_dir().join(format!("lezreg-paste-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("PWNED");
    let m = marker.display().to_string();
    let accepted = ["https://example.org/logos-repo.json", "https://x/a?b=1&c=(2);d=*%27%24!#[x]@+,="];
    let hostile = [
        format!("https://x/'$(id>{m})'"),
        "https://x/a\"b".to_owned(),
        format!("https://x/`id>{m}`"),
        "https://x/a\nb".to_owned(),
        format!("https://x/$(id>{m})"),
        "https://x/a\\b".to_owned(),
        format!("https://x/'\\''$(id>{m})'"),
        "https://x/\u{1b}[2K\rverified".to_owned(),
        "https://x/a b".to_owned(),
        "https://x/\u{202e}evil".to_owned(),
    ];
    for catalog in accepted.iter().map(|s| (*s).to_owned()).chain(hostile.iter().cloned()) {
        let text = lezreg(&dir, &["app", "record", PACKAGE, "--catalog", &catalog]);
        let json = lezreg(&dir, &["--json", "app", "record", PACKAGE, "--catalog", &catalog]);
        if accepted.contains(&catalog.as_str()) {
            assert!(text.status.success(), "{catalog:?}: {}", String::from_utf8_lossy(&text.stderr));
            let line = String::from_utf8(text.stdout).unwrap();
            assert_eq!(line.lines().count(), 1, "{line:?}");
            let want = serde_json::from_slice::<serde_json::Value>(&json.stdout).unwrap()["value"].as_str().unwrap().to_owned();
            let pasted = Command::new("sh")
                .current_dir(&dir)
                .arg("-c")
                .arg(format!("names() {{ printf %s \"$4\"; }}\n{line}"))
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(pasted.stdout).unwrap(), want, "{catalog:?}");
            println!("accepted {catalog:?}: pasted value == --json value");
        } else {
            assert!(!text.status.success() && text.stdout.is_empty(), "{catalog:?} was printed: {}", String::from_utf8_lossy(&text.stdout));
            assert!(!json.status.success(), "{catalog:?}");
            let err = String::from_utf8(text.stderr).unwrap();
            assert!(!err.trim_end_matches('\n').chars().any(lezreg_sdk::text::is_unsafe_char), "{err:?}");
            println!("refused  {catalog:?}: {}", err.trim());
        }
        assert!(!marker.exists(), "{catalog:?} ran code");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
