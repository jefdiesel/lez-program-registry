//! Offline-mode CLI checks that need no Docker, no forge and no sequencer.

use std::process::Command;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_lezreg"))
        .args(args)
        .output()
        .expect("run lezreg");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn verify_help_discloses_every_third_party_service() {
    let (ok, stdout, _) = run(&["verify", "--help"]);
    assert!(ok);
    assert!(stdout.contains("Docker Hub"), "{}", stdout);
    assert!(stdout.contains("crates.io"), "{}", stdout);
    assert!(stdout.contains("git forge"), "{}", stdout);
    assert!(stdout.contains("--offline"), "{}", stdout);
}

#[test]
fn build_help_discloses_its_services() {
    let (ok, stdout, _) = run(&["build", "--help"]);
    assert!(ok);
    assert!(stdout.contains("Docker Hub"), "{}", stdout);
    assert!(stdout.contains("crates.io"), "{}", stdout);
}

#[test]
fn offline_without_inputs_fails_before_any_work() {
    let (ok, _, stderr) = run(&[
        "--registry",
        "7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82",
        "--rpc",
        "http://127.0.0.1:1/",
        "verify",
        "abc",
        "--offline",
    ]);
    assert!(!ok);
    assert!(stderr.contains("--image-tar"), "{}", stderr);
    assert!(stderr.contains("--vendor-tar"), "{}", stderr);
}
