//! `lezreg app`: apps found by LNS name and verified by publisher (docs/APPS.md).

use std::{
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Subcommand;
use lezreg_sdk::{
    HttpRpc, Registry,
    apps::{self, AppRecord, Manifest, Names, Verdict},
};
use lezreg_types::AccountId;
use serde_json::{Value, json};

use crate::out::Out;

#[derive(Subcommand)]
pub enum AppCmd {
    /// The apps an LNS name publishes (its `app.<module>` entries), each with the source status of
    /// the programs it calls. Only someone who knows the name can read them.
    Show { name: String },
    /// Check a package (`.lgx`, or an installed module's `manifest.json`) against the LNS name its
    /// manifest names as publisher: verified, mismatch (a copy, or a release the name hasn't
    /// vouched for), not published, no such name, or unclaimed.
    Verify { file: PathBuf },
    /// The `app.<module>` entry a publisher sets on their name for a package, printed as the
    /// `names set-text` command to run. The package must already name the publisher
    /// (`"author": "lns:<name>"` in metadata.json).
    Record {
        file: PathBuf,
        /// The catalog's logos-repo.json URL (https).
        #[arg(long)]
        catalog: Option<String>,
        /// A program id the app calls (repeatable).
        #[arg(long = "program")]
        programs: Vec<String>,
        /// An older root to keep vouching for while installs upgrade (repeatable, hex).
        #[arg(long = "keep")]
        keep: Vec<String>,
    },
}

/// The LNS program to read names from: `--lns`, `LEZREG_LNS`.
fn lns_program(lns: Option<&str>) -> Result<AccountId> {
    let text = lns.ok_or_else(|| {
        anyhow!("no LNS program: pass --lns <program id> (or set LEZREG_LNS); LNS launches on LEZ testnet 0.4 with the testnet")
    })?;
    AccountId::from_base58(text.trim()).ok_or_else(|| anyhow!("--lns is not a base58 program id: {text}"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// A package's manifest: the `manifest.json` inside a `.lgx` (a gzipped tar), or the file itself.
pub fn read_manifest(file: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    let json: Value = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let mut found = None;
        for entry in archive.entries().context("not a .lgx package")? {
            let entry = entry.context("a damaged .lgx package")?;
            let path = entry.path()?.into_owned();
            if path.file_name().is_some_and(|n| n == "manifest.json") && path.components().count() <= 2 {
                let mut text = String::new();
                entry.take(1 << 20).read_to_string(&mut text)?;
                found = Some(serde_json::from_str(&text).context("the package's manifest.json is not JSON")?);
                break;
            }
        }
        found.ok_or_else(|| anyhow!("{} has no manifest.json", file.display()))?
    } else {
        serde_json::from_slice(&bytes).context("not a .lgx package or a manifest.json")?
    };
    Ok(Manifest::parse(&json)?)
}

/// A program's registry status in a few words.
fn program_status(registry: Option<&Registry<HttpRpc>>, program: &AccountId) -> Value {
    let Some(registry) = registry else {
        return json!({ "program": program.to_base58(), "status": "registry not configured" });
    };
    match registry.resolve(program) {
        Ok(resolved) => match (&resolved.deployer, resolved.freshness()) {
            (Some(entry), Some(fresh)) => json!({
                "program": program.to_base58(),
                "status": format!("{fresh:?}").to_lowercase(),
                "name": entry.name, "version": entry.version,
            }),
            _ => json!({ "program": program.to_base58(), "status": "unregistered" }),
        },
        Err(error) => json!({ "program": program.to_base58(), "status": "unreadable", "error": error.to_string() }),
    }
}

pub fn run(
    out: &mut Out,
    cmd: AppCmd,
    rpc: Option<String>,
    registry: Option<Registry<HttpRpc>>,
    lns: Option<&str>,
) -> Result<()> {
    match cmd {
        AppCmd::Show { name } => {
            let lns = lns_program(lns)?;
            let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
            let mut names = Names::new(HttpRpc::new(&rpc), lns);
            let profile = names
                .profile(&name, now_ms())?
                .ok_or_else(|| anyhow!("{name} is not a registered LNS name"))?;
            let mut shown = Vec::new();
            for (module, record) in apps::apps_of(&profile) {
                match record {
                    Ok(record) => {
                        let programs: Vec<Value> = record
                            .programs
                            .iter()
                            .map(|p| program_status(registry.as_ref(), p))
                            .collect();
                        out.text(&format!(
                            "{module}  {}  root {}{}",
                            record.version.as_deref().unwrap_or("-"),
                            hex::encode(record.roots[0]),
                            record.catalog.as_deref().map(|c| format!("\n  catalog {c}")).unwrap_or_default()
                        ));
                        for p in &programs {
                            out.text(&format!("  program {}  {}", p["program"].as_str().unwrap_or_default(), p["status"].as_str().unwrap_or_default()));
                        }
                        shown.push(json!({
                            "module": module,
                            "version": record.version,
                            "roots": record.roots.iter().map(hex::encode).collect::<Vec<_>>(),
                            "catalog": record.catalog,
                            "programs": programs,
                        }));
                    }
                    Err(error) => {
                        out.text(&format!("{module}  (malformed record: {error})"));
                        shown.push(json!({ "module": module, "error": error.to_string() }));
                    }
                }
            }
            if shown.is_empty() {
                out.text(&format!("{name} publishes no apps"));
            }
            out.json(json!({ "name": name, "apps": shown }));
        }
        AppCmd::Verify { file } => {
            let manifest = read_manifest(&file)?;
            let (verdict, via) = match manifest.publisher() {
                None => (Verdict::Unclaimed, None),
                Some(_) => {
                    let lns = lns_program(lns)?;
                    let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
                    Names::new(HttpRpc::new(&rpc), lns).verify(&manifest, now_ms())?
                }
            };
            if let Some(from) = &via {
                out.text(&format!("{from} forwards to {}", verdict.to_json(&manifest)["publisher"].as_str().unwrap_or("?")));
            }
            let line = match &verdict {
                Verdict::Verified { name, .. } => format!("verified: {} {} is published by {name}", manifest.name, manifest.version),
                Verdict::Mismatch { name, .. } => format!("MISMATCH: {name} publishes {} but does not vouch for this package (root {})", manifest.name, hex::encode(manifest.root)),
                Verdict::NotPublished { name } => format!("not published: {name} says nothing about {}", manifest.name),
                Verdict::NoSuchName { name } => format!("no such name: the package claims {name}, which is not registered"),
                Verdict::Unclaimed => format!("unclaimed: {} does not name an LNS publisher", manifest.name),
            };
            out.text(&line);
            let mut shown = verdict.to_json(&manifest);
            shown["forwarded_from"] = json!(via);
            out.json(shown);
            if !matches!(verdict, Verdict::Verified { .. }) {
                crate::FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        AppCmd::Record { file, catalog, programs, keep } => {
            let manifest = read_manifest(&file)?;
            let Some(name) = manifest.publisher() else {
                bail!("{} names no publisher: set \"author\": \"lns:<name>\" in its metadata.json and rebuild", file.display());
            };
            if !apps::is_module_name(&manifest.name) {
                bail!("module name {} is longer than {} bytes or has characters an LNS key can't hold", manifest.name, apps::MAX_MODULE);
            }
            let mut roots = vec![manifest.root];
            for k in &keep {
                let root: [u8; 32] = hex::decode(k.trim()).ok().and_then(|b| b.try_into().ok())
                    .ok_or_else(|| anyhow!("--keep takes a 64-hex root: {k}"))?;
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
            let programs = programs
                .iter()
                .map(|p| AccountId::from_base58(p.trim()).ok_or_else(|| anyhow!("--program is not a base58 id: {p}")))
                .collect::<Result<Vec<_>>>()?;
            let record = AppRecord { roots, version: Some(manifest.version.clone()), catalog, programs };
            let value = record.to_value()?;
            let key = format!("{}{}", apps::APP_KEY_PREFIX, manifest.name);
            out.text(&format!("names set-text {name} {key} '{value}'"));
            out.json(json!({ "name": name, "key": key, "value": value }));
        }
    }
    Ok(())
}
