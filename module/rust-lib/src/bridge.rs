//! Signing a prepared change. The module holds no keys, so `submit` hands the change to a
//! wallet:
//!
//! - **`cli`** (the default, and the only one that works today): the person's own `lezreg`
//!   command, as `lezreg module send` with the change on stdin. The CLI re-derives the
//!   transaction from the message and refuses anything that is not exactly the registry change
//!   it claims to be, then simulates it, sizes its gas, signs it with its wallet
//!   (`LEE_WALLET_HOME_DIR`, default `~/.lee/wallet`), sends it and waits for its block. This is
//!   how the Names module signs on LEZ 0.4 (its `names module send`).
//! - **`lez_core`**: the Logos wallet module's `send_generic_public_transaction`, which takes
//!   exactly the change's accounts, root and message. Not usable yet (see [`LEZ_CORE_BLOCKED`]).
//!
//! A send outlasts a module call (a block, sometimes minutes), so it runs as a job in the
//! background: `submit` answers at once with the job's id, `submit_status` says how it is
//! going, and the `change_settled` event fires when it ends.

use std::{
    collections::BTreeMap,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::writes::{FORMAT, Failure, Res};

/// Why `lez_core` can't sign yet.
pub const LEZ_CORE_BLOCKED: &str = "the lez_core wallet module can't sign registry changes yet: its generic public send (logos-execution-zone-module 0.6.0, merged 2026-11-13) has no release, is built with a newer module builder than Basecamp 0.3.x loads, and is pinned to LEZ v0.4.0-rc1, whose wallet sends with a fixed gas limit and no simulation (this registry runs on rc3, whose wallet measures gas first). Use the lezreg CLI signer (the default)";

/// A send that takes longer than this is stopped.
const PATIENCE: Duration = Duration::from_secs(10 * 60);
/// How much of the CLI's progress a status keeps.
const TAIL_LINES: usize = 20;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub what: String,
    pub entry: String,
    pub started: u64,
    pub state: &'static str,
    pub receipt: Value,
    pub error: Option<String>,
    pub code: Option<&'static str>,
    pub log: Vec<String>,
    /// The app that submitted it (or approved it for another).
    pub requested_by: String,
}

impl Job {
    pub fn json(&self) -> Value {
        json!({"job": self.id, "what": self.what, "entry": self.entry, "started": self.started, "state": self.state,
               "receipt": self.receipt, "error": self.error, "code": self.code, "log_tail": self.log, "requested_by": self.requested_by})
    }
}

static JOBS: Mutex<BTreeMap<String, Job>> = Mutex::new(BTreeMap::new());

pub fn status(id: &str) -> Option<Job> {
    JOBS.lock().ok()?.get(id).cloned()
}

pub fn jobs() -> Vec<Value> {
    JOBS.lock().map(|j| j.values().rev().take(20).map(Job::json).collect()).unwrap_or_default()
}

fn set(id: &str, f: impl FnOnce(&mut Job)) {
    if let Ok(mut j) = JOBS.lock() {
        if let Some(job) = j.get_mut(id) {
            f(job);
        }
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The `lezreg` to run: the configured one, else on PATH, else where its installers put it.
/// Basecamp is started by the desktop, whose PATH rarely has `~/.cargo/bin` in it.
pub fn find_cli(configured: Option<&Path>) -> Res<PathBuf> {
    if let Some(p) = configured {
        return if is_executable(p) { Ok(p.to_owned()) } else { Err(Failure::new("no_signer", format!("the configured lezreg ({}) is not an executable file", p.display()))) };
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).map(|d| d.join("lezreg")).collect()).unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.extend([home.join(".local/bin/lezreg"), home.join(".cargo/bin/lezreg")]);
    }
    candidates.push(PathBuf::from("/usr/local/bin/lezreg"));
    candidates.into_iter().find(|p| is_executable(p)).ok_or_else(|| {
        Failure::new("no_signer", "no lezreg command found to sign with: install lezreg (it brings the LEZ wallet), or call config_set with {\"cli\": \"/path/to/lezreg\"}")
    })
}

/// `lezreg --version`, to show which signer is configured.
pub fn cli_version(cli: &Path) -> Res<String> {
    let out = Command::new(cli).arg("--version").stdin(Stdio::null()).output().map_err(|e| Failure::new("no_signer", format!("cannot run {}: {e}", cli.display())))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// What a failed send's output says, as a code.
fn code_of(text: &str) -> &'static str {
    let t = text.to_ascii_lowercase();
    if t.contains("holds no key") || t.contains("no key for") || t.contains("both signatures") {
        "missing_key"
    } else if t.contains("outside the registry's 10-minute window") {
        "expired"
    } else if t.contains("there is no wallet") || t.contains("cannot open the wallet") {
        "no_wallet"
    } else if t.contains("holds") && t.contains("more") {
        "insufficient_funds"
    } else if t.contains("reverted") {
        "reverted"
    } else if t.contains("not a url") || t.contains("not on this machine") || t.contains("--yes") {
        "remote_sequencer"
    } else if t.contains("simulat") || t.contains("refused") {
        "refused"
    } else {
        "signer_failed"
    }
}

/// Starts signing and sending `change` with the `lezreg` CLI at `cli`. `allow_remote` passes
/// the CLI's `--yes` (spend through a sequencer that is not on this machine); `payer` its
/// `--payer`. `settled(job)` runs when it ends.
pub fn start(cli: &Path, change: &Value, now_ms: u64, requested_by: &str, settled: impl FnOnce(&Job) + Send + 'static) -> Res<Job> {
    if change["format"] != FORMAT {
        return Err(Failure::input("not a change: pass what a prepare_* method returned"));
    }
    if let Some(exp) = change["expires_at"].as_u64() {
        if now_ms >= exp {
            return Err(Failure::new("expired", "this change was prepared more than 10 minutes ago and the registry would refuse it: prepare it again"));
        }
    }
    let seq = change["sequencer"].as_str().ok_or_else(|| Failure::input("the change names no sequencer"))?.to_owned();
    let reg = change["registry"].as_str().ok_or_else(|| Failure::input("the change names no registry"))?.to_owned();
    let mut args = vec!["--json".to_owned(), "--rpc".into(), seq, "--registry".into(), reg, "module".into(), "send".into()];
    if let Some(p) = change["payer"].as_str() {
        args.extend(["--payer".into(), p.to_owned()]);
    }
    if change["allow_remote"] == true {
        args.push("--yes".into());
    }
    let mut child = Command::new(cli)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Failure::new("no_signer", format!("cannot run {}: {e}", cli.display())))?;
    let input = change.to_string();
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).map_err(|e| Failure::new("signer_failed", format!("cannot hand the change to lezreg: {e}")))?;
    }
    // Unique even for two submits in the same millisecond.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let id = format!("{}-{}-{n}", change["what"].as_str().unwrap_or("change").split_whitespace().next().unwrap_or("change").to_ascii_lowercase(), now_ms);
    let job = Job {
        id: id.clone(),
        what: change["what"].as_str().unwrap_or_default().to_owned(),
        entry: change["entry"].as_str().unwrap_or_default().to_owned(),
        started: now_ms,
        state: "running",
        receipt: Value::Null,
        error: None,
        code: None,
        log: Vec::new(),
        requested_by: requested_by.to_owned(),
    };
    if let Ok(mut j) = JOBS.lock() {
        if j.len() > 100 {
            let oldest: Vec<String> = j.keys().take(50).cloned().collect();
            for k in oldest {
                j.remove(&k);
            }
        }
        j.insert(id.clone(), job.clone());
    }
    std::thread::spawn(move || {
        let mut stderr = child.stderr.take();
        let err_id = id.clone();
        let reader = std::thread::spawn(move || {
            let mut all = String::new();
            if let Some(e) = stderr.as_mut() {
                let mut buf = [0u8; 4096];
                while let Ok(n) = e.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    all.push_str(&String::from_utf8_lossy(&buf[..n]));
                    let tail: Vec<String> = all.lines().rev().take(TAIL_LINES).map(str::to_owned).collect::<Vec<_>>().into_iter().rev().collect();
                    set(&err_id, |j| j.log = tail);
                }
            }
            all
        });
        let mut out = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = o.read_to_string(&mut out);
        }
        let begun = Instant::now();
        let exit = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if begun.elapsed() > PATIENCE => {
                    let _ = child.kill();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(_) => break None,
            }
        };
        let err_text = reader.join().unwrap_or_default();
        // `--json` prints one (pretty) JSON object on stdout and nothing else.
        let answer: Value = serde_json::from_str(out.trim()).ok().or_else(|| out.lines().rev().find_map(|l| serde_json::from_str(l).ok())).unwrap_or(Value::Null);
        let ok = exit.is_some_and(|s| s.success()) && answer["tx"].is_string();
        set(&id, |j| {
            if ok {
                j.state = "applied";
                j.receipt = answer.clone();
            } else {
                j.state = "failed";
                let why = answer["error"]["message"].as_str().or_else(|| answer["error"].as_str()).map(str::to_owned).unwrap_or_else(|| {
                    if exit.is_none() { format!("lezreg did not finish within {} minutes", PATIENCE.as_secs() / 60) } else { err_text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("lezreg failed").to_owned() }
                });
                j.code = Some(code_of(&format!("{why}\n{err_text}")));
                j.error = Some(why);
                j.receipt = answer.clone();
            }
        });
        if let Some(job) = status(&id) {
            settled(&job);
        }
    });
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_get_codes() {
        assert_eq!(code_of("this wallet holds no key for 7Pq"), "missing_key");
        assert_eq!(code_of("it is outside the registry's 10-minute window; prepare it again"), "expired");
        assert_eq!(code_of("block 9: REVERTED"), "reverted");
        assert_eq!(code_of("something else"), "signer_failed");
    }

    /// A stand-in `lezreg` that answers like `lezreg --json module send` does.
    fn fake_cli(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("lezreg");
        std::fs::write(&p, format!("#!/bin/sh\ncat > \"$(dirname \"$0\")/stdin.json\"\necho \"$@\" > \"$(dirname \"$0\")/args.txt\"\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn change() -> Value {
        json!({"format": FORMAT, "what": "Freeze", "sequencer": "http://127.0.0.1:3062", "registry": "Reg", "entry": "E",
               "expires_at": 2_000, "message": "04"})
    }

    fn wait(id: &str) -> Job {
        for _ in 0..100 {
            let j = status(id).unwrap();
            if j.state != "running" {
                return j;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("job {id} never settled");
    }

    #[test]
    fn a_change_is_sent_through_the_cli_and_settles() {
        let dir = std::env::temp_dir().join(format!("lezreg-bridge-ok-{}", std::process::id()));
        let cli = fake_cli(&dir, "echo 'simulated: OK' >&2\necho '{\"tx\":\"abc\",\"block\":7}'");
        let (tx, rx) = std::sync::mpsc::channel();
        let job = start(&cli, &change(), 1_000, "test", move |j| tx.send(j.state).unwrap()).unwrap();
        assert_eq!(job.state, "running");
        let done = wait(&job.id);
        assert_eq!((done.state, done.receipt["block"].as_u64()), ("applied", Some(7)));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "applied", "the settled callback ran");
        assert_eq!(std::fs::read_to_string(dir.join("args.txt")).unwrap().trim(), "--json --rpc http://127.0.0.1:3062 --registry Reg module send");
        let sent: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("stdin.json")).unwrap()).unwrap();
        assert_eq!(sent, change(), "the change arrives on stdin verbatim");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_refusal_comes_back_with_a_code_and_remote_spending_needs_consent() {
        let dir = std::env::temp_dir().join(format!("lezreg-bridge-no-{}", std::process::id()));
        let cli = fake_cli(&dir, "printf '{\\n  \"error\": \"this wallet holds no key for B\",\\n  \"kind\": \"no_key\"\\n}\\n'\nexit 1");
        let mut c = change();
        c["allow_remote"] = json!(true);
        c["payer"] = json!("P");
        let job = start(&cli, &c, 1_000, "test", |_| {}).unwrap();
        let done = wait(&job.id);
        assert_eq!((done.state, done.code), ("failed", Some("missing_key")));
        assert!(std::fs::read_to_string(dir.join("args.txt")).unwrap().contains("--payer P --yes"));
        assert_eq!(start(&cli, &change(), 5_000, "test", |_| {}).unwrap_err().code, "expired");
        assert_eq!(start(&cli, &json!({"format": "x"}), 1, "test", |_| {}).unwrap_err().code, "bad_input");
        assert_eq!(find_cli(Some(&dir.join("nope"))).unwrap_err().code, "no_signer");
        let _ = std::fs::remove_dir_all(dir);
    }
}
