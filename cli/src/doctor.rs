//! `lezreg doctor`: is this machine ready to publish? (DESIGN §7.2.) Checks the sequencer, the
//! registry id, the Storage node, the risc0 executor writes need, the wallet, Docker (for
//! `build`/`verify`) and the clock. Nothing is sent anywhere but the user's own sequencer and
//! Storage node.

use std::{path::PathBuf, process::Command, time::Duration};

use lezreg_sdk::{HttpRpc, Rpc as _, StorageClient, storage::Timeouts, tx::tid};
use serde_json::{Value, json};

use crate::{config::Resolved as Net, out::Out};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "FAIL",
        }
    }
}

pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
}

fn check(name: &'static str, level: Level, detail: impl Into<String>) -> Check {
    Check {
        name,
        level,
        detail: detail.into(),
    }
}

/// How far this machine's clock may be from the chain's before it matters: entries carry the
/// chain time and the program accepts `[now, now + 10 min)` (DESIGN §10).
pub fn skew_level(skew_ms: i128) -> Level {
    match skew_ms.unsigned_abs() {
        s if s <= 120_000 => Level::Ok,
        s if s <= 600_000 => Level::Warn,
        _ => Level::Fail,
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(bin))
            .find(|p| p.is_file())
    })
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn now_ms() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i128::try_from(d.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

/// The sequencer's tip and the chain clock's time.
fn sequencer(rpc: &str) -> Vec<Check> {
    let h = HttpRpc::new(rpc);
    let mut v = Vec::new();
    match h.call("getLastBlockId", json!([])) {
        Ok(b) => v.push(check("sequencer", Level::Ok, format!("{rpc}, block {b}"))),
        Err(e) => {
            v.push(check("sequencer", Level::Fail, format!("{rpc}: {e}")));
            return v;
        }
    }
    let clock = h.get_actor_state(
        &tid(&clock_core::CLOCK_01_PROGRAM_ACCOUNT_ID),
        &tid(&clock_core::clock_account_id()),
    );
    match clock.map(|s| borsh::from_slice::<clock_core::ClockAccountData>(&s)) {
        Ok(Ok(c)) => {
            let skew = now_ms() - i128::from(c.timestamp);
            v.push(check(
                "clock",
                skew_level(skew),
                format!(
                    "this machine is {:+.1} s from the chain clock (block {})",
                    skew as f64 / 1000.0,
                    c.block_id
                ),
            ));
        }
        _ => v.push(check(
            "clock",
            Level::Warn,
            "cannot read the chain clock; writes use it, so this only matters for your own logs",
        )),
    }
    v
}

pub fn run_checks(net: &Net, storage_api: &str) -> Vec<Check> {
    let mut v = vec![check(
        "network",
        Level::Ok,
        format!(
            "{} (config {})",
            net.name,
            crate::config::path().map_or_else(|| "none".into(), |p| p.display().to_string())
        ),
    )];
    match net.rpc() {
        Ok(rpc) => {
            v.extend(sequencer(&rpc));
            let up = v
                .iter()
                .all(|c| c.name != "sequencer" || c.level == Level::Ok);
            match net.registry() {
                Ok(reg) if up => match crate::check_registry(&rpc, &reg) {
                    Ok(()) => v.push(check("registry", Level::Ok, reg.to_base58())),
                    Err(e) => v.push(check("registry", Level::Fail, format!("{e:#}"))),
                },
                Ok(reg) => v.push(check(
                    "registry",
                    Level::Warn,
                    format!("{} (not checked: no sequencer)", reg.to_base58()),
                )),
                Err(e) => v.push(check("registry", Level::Fail, format!("{e:#}"))),
            }
        }
        Err(e) => v.push(check("sequencer", Level::Fail, format!("{e:#}"))),
    }
    let node = StorageClient::with_timeouts(
        storage_api,
        Timeouts {
            manifest: Duration::from_secs(5),
            stream: Duration::from_secs(5),
            dial: Duration::from_secs(5),
        },
    );
    match node.info() {
        Ok(i) => v.push(check(
            "storage",
            if i.addrs.is_empty() { Level::Warn } else { Level::Ok },
            format!(
                "{storage_api}: peer {}{}",
                i.id,
                if i.addrs.is_empty() {
                    ", no announced address yet (others cannot fetch your IDL)"
                } else {
                    ""
                }
            ),
        )),
        Err(e) => v.push(check(
            "storage",
            Level::Warn,
            format!(
                "{storage_api}: {e}. Publishing uploads the IDL there: start one with `lezreg storage up` or set LEZREG_STORAGE_API"
            ),
        )),
    }
    let r0vm = std::env::var_os("RISC0_SERVER_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| which("r0vm"));
    v.push(match r0vm {
        Some(p) => check("r0vm", Level::Ok, p.display().to_string()),
        None => check(
            "r0vm",
            Level::Fail,
            "not on PATH and RISC0_SERVER_PATH unset: every write is simulated with it first (rzup install)",
        ),
    });
    v.push(match wallet::utils::fetch_persistent_storage_path() {
        Ok(p) if p.exists() => check("wallet", Level::Ok, p.display().to_string()),
        Ok(p) => check(
            "wallet",
            Level::Warn,
            format!(
                "none at {} yet: `lezreg wallet account new public`",
                p.display()
            ),
        ),
        Err(e) => check("wallet", Level::Warn, e.to_string()),
    });
    v.push(match run("docker", &["version", "--format", "{{.Server.Version}}"]) {
        None => check(
            "docker",
            Level::Warn,
            "not available: needed only for `lezreg build`/`verify`; the GitHub Action builds without local Docker",
        ),
        Some(ver) => {
            let buildx = run("docker", &["buildx", "version"]).is_some();
            let mem = run("docker", &["info", "--format", "{{.MemTotal}}"])
                .and_then(|m| m.parse::<u64>().ok())
                .unwrap_or(0);
            let arm = cfg!(target_arch = "aarch64");
            let low = arm && mem > 0 && mem < 8 << 30;
            let level = if !buildx || low {
                Level::Warn
            } else {
                Level::Ok
            };
            check(
                "docker",
                level,
                format!(
                    "server {ver}, buildx {}, memory {:.1} GiB{}",
                    if buildx { "yes" } else { "MISSING" },
                    mem as f64 / f64::from(1u32 << 30),
                    if low {
                        " (the builder runs emulated on arm64: give Docker at least 8 GiB)"
                    } else {
                        ""
                    }
                ),
            )
        }
    });
    v
}

/// Prints the checks; returns whether none failed.
pub fn report(out: &mut Out, checks: &[Check]) -> bool {
    let mut j: Vec<Value> = Vec::new();
    for c in checks {
        out.text(&format!(
            "{:<4}  {:<9}  {}",
            c.level.label(),
            c.name,
            c.detail
        ));
        j.push(
            json!({"check": c.name, "level": c.level.label().to_lowercase(), "detail": c.detail}),
        );
    }
    let ok = checks.iter().all(|c| c.level != Level::Fail);
    out.json(json!({"ok": ok, "checks": j}));
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skew_levels() {
        assert_eq!(skew_level(0), Level::Ok);
        assert_eq!(skew_level(-90_000), Level::Ok);
        assert_eq!(skew_level(300_000), Level::Warn);
        assert_eq!(skew_level(-700_000), Level::Fail);
    }

    #[test]
    fn nothing_reachable_fails_cleanly() {
        let net = Net {
            name: "x".into(),
            rpc: Some("http://127.0.0.1:9".into()),
            registry: None,
        };
        let checks = run_checks(&net, "http://127.0.0.1:9/api/storage/v1");
        let get = |n: &str| checks.iter().find(|c| c.name == n).map(|c| c.level);
        assert_eq!(get("sequencer"), Some(Level::Fail));
        assert_eq!(get("registry"), Some(Level::Fail), "no registry id");
        assert_eq!(get("storage"), Some(Level::Warn));
        let mut out = Out::new(false);
        assert!(!report(&mut out, &checks));
        // testnet before launch: says so instead of guessing an RPC.
        let checks = run_checks(
            &Net {
                name: "testnet".into(),
                rpc: None,
                registry: None,
            },
            "http://127.0.0.1:9/api/storage/v1",
        );
        assert!(checks.iter().any(|c| c.detail.contains("not launched")));
    }
}
