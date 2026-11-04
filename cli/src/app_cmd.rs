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
        /// The package's Logos Storage CID, if it is already there.
        #[arg(long, conflicts_with = "upload")]
        cid: Option<String>,
        /// Upload the package to your Logos Storage node and record its CID: a download that
        /// depends on no web host.
        #[arg(long)]
        upload: bool,
        #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
        storage_api: String,
    },
    /// Download an app a name publishes from Logos Storage (its record's `cid`), check it against
    /// the record (`sha256` of the file, the package root), and only then write it to OUT.
    Get {
        name: String,
        module: String,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
        storage_api: String,
    },
}

/// The most a package may be: a download larger than any Basecamp module is refused.
const MAX_PACKAGE: usize = 256 * 1024 * 1024;

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
    manifest_of(&bytes, &file.display().to_string())
}

/// Whether `bytes` are a `.lgx` package (gzip), not a bare `manifest.json`.
fn is_package(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

fn manifest_of(bytes: &[u8], what: &str) -> Result<Manifest> {
    let file = what;
    let json: Value = if is_package(bytes) {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
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
        found.ok_or_else(|| anyhow!("{file} has no manifest.json"))?
    } else {
        serde_json::from_slice(bytes).context("not a .lgx package or a manifest.json")?
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
        AppCmd::Get { name, module, out: path, storage_api } => {
            let lns = lns_program(lns)?;
            let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
            let profile = Names::new(HttpRpc::new(&rpc), lns)
                .profile(&name, now_ms())?
                .ok_or_else(|| anyhow!("{name} is not a registered LNS name"))?;
            let value = profile
                .text(&format!("{}{module}", apps::APP_KEY_PREFIX))
                .ok_or_else(|| anyhow!("{name} doesn't publish {module}"))?;
            let record = AppRecord::parse(value)?;
            let cid = record.cid.clone().ok_or_else(|| anyhow!("{name} gives no Logos Storage CID for {module}; download it from its catalog ({})", record.catalog.as_deref().unwrap_or("none given")))?;
            eprintln!("fetching {module} ({cid}) from Logos Storage through {storage_api}");
            let bytes = lezreg_sdk::StorageClient::new(&storage_api)
                .fetch(&cid, MAX_PACKAGE, false)
                .map_err(|e| anyhow!("Logos Storage didn't give {cid}: {e}"))?;
            if record.matches_download(&bytes) == Some(false) {
                bail!("the file Storage gave is not the one {name} vouches for (sha256 differs); nothing written");
            }
            let manifest = manifest_of(&bytes, &cid)?;
            if manifest.name != module || !record.roots.contains(&manifest.root) {
                bail!("the package inside is not the {module} {name} vouches for (module {} root {}); nothing written", manifest.name, hex::encode(manifest.root));
            }
            std::fs::write(&path, &bytes).with_context(|| format!("cannot write {}", path.display()))?;
            out.text(&format!("{module} {} from {name}: checked (sha256 {}, root), written to {}", manifest.version,
                record.sha256.map_or_else(|| "not given".to_owned(), |_| "matches".to_owned()), path.display()));
            out.json(json!({"name": name, "module": module, "version": manifest.version, "cid": cid, "bytes": bytes.len(), "out": path}));
        }
        AppCmd::Record { file, catalog, programs, keep, cid, upload, storage_api } => {
            let bytes = std::fs::read(&file).with_context(|| format!("cannot read {}", file.display()))?;
            let manifest = manifest_of(&bytes, &file.display().to_string())?;
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
            // A package file is hashed as downloaded; a bare manifest.json has no file to hash.
            let sha256 = is_package(&bytes).then(|| lezreg_types::pda::sha256(&bytes));
            let cid = if upload {
                if sha256.is_none() {
                    bail!("--upload takes the .lgx package, not a manifest.json");
                }
                let name_on_storage = format!("{}-{}.lgx", manifest.name, manifest.version);
                let cid = lezreg_sdk::StorageClient::new(&storage_api)
                    .upload_as(&name_on_storage, "application/octet-stream", &bytes)
                    .map_err(|e| anyhow!("uploading to {storage_api}: {e}"))?;
                eprintln!("uploaded {name_on_storage} to Logos Storage: {cid} (keep your node online, or pin it elsewhere, so others can fetch it)");
                Some(cid)
            } else {
                cid
            };
            if cid.as_deref().is_some_and(|c| !apps::is_cid(c)) {
                bail!("--cid is not a Logos Storage CID");
            }
            let record = AppRecord { roots, version: Some(manifest.version.clone()), catalog, sha256, cid, programs };
            let value = record.to_value()?;
            let key = format!("{}{}", apps::APP_KEY_PREFIX, manifest.name);
            out.text(&format!("names set-text {name} {key} '{value}'"));
            out.json(json!({ "name": name, "key": key, "value": value }));
        }
    }
    Ok(())
}
