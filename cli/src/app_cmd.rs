//! `lezreg app`: apps found by LNS name and verified by publisher (docs/APPS.md).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Subcommand;
use lezreg_sdk::{
    HttpRpc, Registry,
    apps::{self, AppRecord, Names, Release, Verdict},
    lgx::Contents,
    lns::Entry,
};
use lezreg_types::AccountId;
use serde_json::{Value, json};

use crate::out::Out;

#[derive(Subcommand)]
pub enum AppCmd {
    /// The apps an LNS name publishes (its `app.<module>` entries), each with the source status of
    /// the programs it calls. Only someone who knows the name can read them.
    Show { name: String },
    /// Check a package (`.lgx`, or an installed module's directory or its `manifest.json`) against
    /// the LNS name its manifest names as publisher. The root is recomputed from the package's
    /// files first: root_mismatch if they aren't the files its manifest names. Then verified (exit
    /// 0: the whole package, by the record's sha256 for a .lgx or its manifest hash for an
    /// installed copy), files_verified (exit 3: the files only, the manifest unproven),
    /// mismatch, file_mismatch, manifest_mismatch, ambiguous, not published, no such name, or
    /// unclaimed (exit 1). With --json, key on `status` only.
    Verify { file: PathBuf },
    /// The `app.<module>` entry a publisher sets on their name for a package, printed as the
    /// `names set-text` command to run. Takes the `.lgx`; refused unless its files match its
    /// manifest's root, it is laid out exactly as lgx writes a package, and it names the
    /// publisher (`"author": "lns:<name>"` in metadata.json).
    Record {
        file: PathBuf,
        /// The catalog's logos-repo.json URL (https).
        #[arg(long)]
        catalog: Option<String>,
        /// A program id the app calls (repeatable).
        #[arg(long = "program")]
        programs: Vec<String>,
        /// An older release's `.lgx` to keep vouching for while installs upgrade (repeatable):
        /// its root, file and manifest hashes and version go into the record's `keep`.
        #[arg(long = "keep")]
        keep: Vec<PathBuf>,
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
    /// the record (`sha256` of the file, required; the root recomputed from its files), and only
    /// then write it to OUT.
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

/// A package to check, its root recomputed from its files: a `.lgx`, an installed package's
/// directory, or an installed package's `manifest.json` (its directory is checked).
pub fn read_contents(file: &Path) -> Result<Contents> {
    if file.is_dir() {
        return Contents::from_installed(file).with_context(|| format!("checking the installed package in {}", file.display()));
    }
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return Contents::from_lgx(&bytes).with_context(|| format!("reading {}", file.display()));
    }
    if serde_json::from_slice::<Value>(&bytes).is_ok() {
        // A bare manifest.json names a root but has no files: check the install it sits in.
        let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        return Contents::from_installed(dir).with_context(|| {
            format!("{} is a manifest, so the installed package around it was checked, and {} is not one (a manifest alone proves nothing: its files are what the root covers)", file.display(), dir.display())
        });
    }
    bail!("{} is not a .lgx package, an installed package's directory, or its manifest.json", file.display())
}

/// A `.lgx` a publisher may vouch for: its files hash to its manifest's root, it is laid out
/// exactly as lgx writes a package (so no reader can read it differently), and it names its
/// publisher. Returns the bytes, the contents and the publisher's name.
fn vouchable(file: &Path) -> Result<(Vec<u8>, Contents, String)> {
    let bytes = std::fs::read(file).with_context(|| format!("cannot read {}", file.display()))?;
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        bail!("{} is not a .lgx package: a record vouches for a package's files and the .lgx's sha256, so it is made from the .lgx", file.display());
    }
    let contents = Contents::from_lgx(&bytes).with_context(|| format!("reading {}", file.display()))?;
    if !contents.root_matches() {
        bail!(
            "{}'s files are not the ones its manifest names (recomputed root {}, the manifest says {}); rebuild it, nothing to vouch for",
            file.display(),
            hex::encode(contents.root),
            hex::encode(contents.manifest.root)
        );
    }
    lezreg_sdk::lgx::check_canonical_layout(&bytes).with_context(|| {
        format!("{} is not laid out exactly as lgx writes a package; vouch only for a package lgx built (rebuild it with lgx)", file.display())
    })?;
    let name = match contents.manifest.claim() {
        Ok(Some(name)) => name,
        Ok(None) => bail!("{} names no publisher: set \"author\": \"lns:<name>\" in its metadata.json and rebuild", file.display()),
        Err(why) => bail!("{}'s author {:?} is {why}", file.display(), contents.manifest.author),
    };
    Ok((bytes, contents, name))
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
            // As normalised (SPEC.md §2): `Forumer`, `lns:forumer` and `forumer.lez` are `forumer`.
            let name = profile.name.clone();
            match apps::forward_of(&name, &profile) {
                apps::Forward::To(target) => out.text(&format!("{name} forwards to {target}: apps it publishes are checked against {target}'s records")),
                apps::Forward::Ambiguous(why) => out.text(&format!("{name}'s forward is ambiguous ({why}): packages naming {name} fail the check")),
                apps::Forward::None => {}
            }
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
                            record.newest().version.as_deref().unwrap_or("-"),
                            hex::encode(record.newest().root),
                            record.catalog.as_deref().map(|c| format!("\n  catalog {c}")).unwrap_or_default()
                        ));
                        for p in &programs {
                            out.text(&format!("  program {}  {}", p["program"].as_str().unwrap_or_default(), p["status"].as_str().unwrap_or_default()));
                        }
                        shown.push(json!({
                            "module": module,
                            "version": record.newest().version,
                            "roots": record.roots().iter().map(hex::encode).collect::<Vec<_>>(),
                            "catalog": record.catalog,
                            "programs": programs,
                        }));
                    }
                    Err(error) => {
                        out.text(&format!("{module}  (vouches for nothing: {error})"));
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
            let contents = read_contents(&file)?;
            let manifest = &contents.manifest;
            let (verdict, via) = if !contents.root_matches() {
                (Verdict::RootMismatch, None)
            } else if contents.manifest.publisher().is_none() {
                (Verdict::Unclaimed, None)
            } else {
                let lns = lns_program(lns)?;
                let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
                Names::new(HttpRpc::new(&rpc), lns).verify(&contents, now_ms())?
            };
            if let Some(from) = &via {
                out.text(&format!("{from} forwards to {}", verdict.to_json(&contents)["claimed"]["name_checked"].as_str().unwrap_or("?")));
            }
            // Never the manifest's version as the published one: only the release's, when proven.
            let line = match &verdict {
                Verdict::Verified { name, release, .. } => format!(
                    "verified: {} {} is published by {name} ({})",
                    manifest.name,
                    release.version.as_deref().unwrap_or("(no version in the record)"),
                    if contents.file_sha256().is_some() { "the whole .lgx file" } else { "the installed files and manifest.json" }
                ),
                Verdict::FilesVerified { name, .. } => format!(
                    "FILES ONLY: {name} vouches for these files (root {}), but its record names nothing that covers {}, so the manifest (its version, dependencies, the rest) is not proven; not verified",
                    hex::encode(contents.root),
                    if contents.file_sha256().is_some() { "the .lgx file (no sha256)" } else { "the installed manifest.json (no manifest hash)" }
                ),
                Verdict::Mismatch { name, .. } => format!("MISMATCH: {name} publishes {} but does not vouch for these files (root {})", manifest.name, hex::encode(contents.root)),
                Verdict::FileMismatch { name, .. } => format!(
                    "FILE MISMATCH: {name} vouches for these files but not for this .lgx (sha256 differs): its manifest or packing was changed (root {})",
                    hex::encode(contents.root)
                ),
                Verdict::ManifestMismatch { name, why, .. } => format!("MANIFEST MISMATCH: {name} vouches for these files, but not this manifest: {why}"),
                Verdict::RootMismatch => format!(
                    "ROOT MISMATCH: the package's files are not the ones its manifest names (recomputed root {}, the manifest says {}); refused",
                    hex::encode(contents.root),
                    hex::encode(manifest.root)
                ),
                Verdict::Ambiguous { name, why } => format!("AMBIGUOUS: {name}'s records can be read two ways ({why}); they vouch for nothing"),
                Verdict::NotPublished { name } => format!("not published: {name} says nothing about {}", manifest.name),
                Verdict::NoSuchName { name } => format!("no such name: the package claims {name}, which is not registered"),
                Verdict::Unclaimed => match manifest.claim() {
                    Err(why) => format!("unclaimed: {} names {:?}, which is {why}", manifest.name, manifest.author),
                    Ok(_) => format!("unclaimed: {} does not name an LNS publisher", manifest.name),
                },
            };
            out.text(&line);
            let mut shown = verdict.to_json(&contents);
            shown["forwarded_from"] = json!(via);
            out.json(shown);
            match verdict.exit_code() {
                0 => {}
                1 => crate::FAILED.store(true, std::sync::atomic::Ordering::Relaxed),
                code => crate::EXIT_CODE.store(code, std::sync::atomic::Ordering::Relaxed),
            }
        }
        AppCmd::Get { name, module, out: path, storage_api } => {
            let lns = lns_program(lns)?;
            let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
            let profile = Names::new(HttpRpc::new(&rpc), lns)
                .profile(&name, now_ms())?
                .ok_or_else(|| anyhow!("{name} is not a registered LNS name"))?;
            let name = profile.name.clone();
            let key = format!("{}{module}", apps::APP_KEY_PREFIX);
            let value = match profile.entry(&key) {
                Entry::One(v) => v,
                Entry::Absent => bail!("{name} doesn't publish {module}"),
                Entry::Duplicate => bail!("ambiguous record: {name} sets {key} more than once, so it vouches for nothing; nothing downloaded"),
            };
            let record = AppRecord::parse(value)?;
            if record.newest().sha256.is_none() {
                bail!("unverifiable download: {name}'s record for {module} gives no sha256 of the .lgx, and the package root doesn't cover the manifest; nothing downloaded");
            }
            let cid = record.cid.clone().ok_or_else(|| anyhow!("{name} gives no Logos Storage CID for {module}; download it from its catalog ({})", record.catalog.as_deref().unwrap_or("none given")))?;
            eprintln!("fetching {module} ({cid}) from Logos Storage through {storage_api}");
            let bytes = lezreg_sdk::StorageClient::new(&storage_api)
                .fetch(&cid, MAX_PACKAGE, false)
                .map_err(|e| anyhow!("Logos Storage didn't give {cid}: {e}"))?;
            let contents = apps::check_download(&module, &record, &bytes).map_err(|e| anyhow!("{e}; nothing written"))?;
            std::fs::write(&path, &bytes).with_context(|| format!("cannot write {}", path.display()))?;
            let version = record.newest().version.clone();
            out.text(&format!(
                "{module} {} from {name}: checked (sha256 matches, files match the root), written to {}",
                version.as_deref().unwrap_or("(no version in the record)"),
                path.display()
            ));
            out.json(json!({"name": name, "module": module, "version": version, "cid": cid, "bytes": bytes.len(), "root": hex::encode(contents.root), "out": path}));
        }
        AppCmd::Record { file, catalog, programs, keep, cid, upload, storage_api } => {
            let (bytes, contents, name) = vouchable(&file)?;
            let manifest = &contents.manifest;
            if !apps::is_module_name(&manifest.name) {
                bail!("module name {} is longer than {} bytes or has characters an LNS key can't hold", manifest.name, apps::MAX_MODULE);
            }
            let release_of = |bytes: &[u8], contents: &Contents| Release {
                root: contents.root,
                sha256: Some(lezreg_types::pda::sha256(bytes)),
                manifest: Some(contents.manifest_sha256),
                version: Some(contents.manifest.version.clone()),
            };
            let mut releases = vec![release_of(&bytes, &contents)];
            for k in &keep {
                let (old_bytes, old, old_name) = vouchable(k)?;
                if old.manifest.name != manifest.name || old_name != name {
                    bail!("{} is {} by {old_name}, not {} by {name}", k.display(), old.manifest.name, manifest.name);
                }
                let release = release_of(&old_bytes, &old);
                if !releases.contains(&release) {
                    releases.push(release);
                }
            }
            let programs = programs
                .iter()
                .map(|p| AccountId::from_base58(p.trim()).ok_or_else(|| anyhow!("--program is not a base58 id: {p}")))
                .collect::<Result<Vec<_>>>()?;
            let cid = if upload {
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
            if cid.as_deref().and_then(apps::cid_content_sha256).is_some_and(|named| Some(named) != releases[0].sha256) {
                bail!("--cid names other content than {}", file.display());
            }
            let record = AppRecord { releases, catalog, cid, programs };
            let value = record.to_value()?;
            let key = format!("{}{}", apps::APP_KEY_PREFIX, manifest.name);
            out.text(&format!("names set-text {name} {key} '{value}'"));
            out.json(json!({ "name": name, "key": key, "value": value, "root": hex::encode(contents.root) }));
        }
    }
    Ok(())
}
