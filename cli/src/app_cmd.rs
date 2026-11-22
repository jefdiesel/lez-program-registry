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

use lezreg_sdk::text::shown;

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
    /// unclaimed (exit 1). With --json, key on `status` only, and treat every JSON string as
    /// untrusted (it comes from the package or the name): never print one raw. Control and
    /// bidi characters in `claimed.author`, catalogs and errors are escaped already.
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
    ///
    /// The newest release is never older than the copy installed in Basecamp without saying so:
    /// that is refused unless --allow-older.
    Get {
        name: String,
        module: String,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
        storage_api: String,
        /// Basecamp's data folder, to compare with the installed copy (default: where Basecamp
        /// keeps it).
        #[arg(long, env = "LEZREG_BASECAMP_DATA")]
        basecamp_data: Option<PathBuf>,
        /// Accept a release older than the installed copy (going back on purpose).
        #[arg(long)]
        allow_older: bool,
    },
    /// Keep the newest published packages of apps on your own Logos Storage node, so it serves
    /// them to others too. For each app: read its name's newest record, fetch the record's `cid`
    /// onto the node unless the node already holds it, check it exactly as `get` does (the
    /// record's sha256 and version, the files hashed to its root, the module and publisher), and
    /// announce it. A copy that fails is dropped from the node again and nothing is announced.
    /// With no TARGET: the apps installed in Basecamp that name an LNS publisher.
    Sync {
        /// `<name>` (every app the name publishes) or `<name>:<module>`. Repeatable.
        targets: Vec<String>,
        #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
        storage_api: String,
        /// Basecamp's data folder, for the installed apps (default: where Basecamp keeps it).
        #[arg(long, env = "LEZREG_BASECAMP_DATA")]
        basecamp_data: Option<PathBuf>,
        /// Repeat every SECS seconds (at least 60), forever: new releases reach the node by
        /// themselves.
        #[arg(long)]
        every: Option<u64>,
    },
}

/// Fetching a package: Storage's 30 s default is for IDLs; a package of tens of MB over the
/// network takes longer.
fn package_node(api: &str) -> lezreg_sdk::StorageClient {
    let t = lezreg_sdk::storage::Timeouts::default();
    lezreg_sdk::StorageClient::with_timeouts(api, lezreg_sdk::storage::Timeouts { stream: std::time::Duration::from_secs(900), ..t })
}

/// A name's profile, following its `forward` one hop as `verify` does (SPEC.md §7): the name
/// asked for (normalised), the publisher's name, and its profile.
fn publisher_profile(names: &mut Names<HttpRpc>, asked: &str) -> Result<(String, String, lezreg_sdk::lns::Profile)> {
    let profile = names
        .profile(asked, now_ms())?
        .ok_or_else(|| anyhow!("{} is not a registered LNS name", shown(asked)))?;
    let asked = profile.name.clone();
    match apps::forward_of(&asked, &profile) {
        apps::Forward::None => Ok((asked.clone(), asked, profile)),
        apps::Forward::Ambiguous(why) => bail!("{asked}'s forward is ambiguous ({why}); nothing fetched"),
        apps::Forward::To(target) => {
            let p = names.profile(&target, now_ms())?.ok_or_else(|| anyhow!("{asked} forwards to {target}, which isn't registered; nothing fetched"))?;
            eprintln!("{asked} forwards to {target}");
            Ok((asked, target, p))
        }
    }
}

/// The one record `profile` holds for `module`.
fn record_for(name: &str, profile: &lezreg_sdk::lns::Profile, module: &str) -> Result<AppRecord> {
    let key = format!("{}{module}", apps::APP_KEY_PREFIX);
    match profile.entry(&key) {
        Entry::One(v) => Ok(AppRecord::parse(v)?),
        Entry::Absent => bail!("{name} doesn't publish {module}"),
        Entry::Duplicate => bail!("ambiguous record: {name} sets {key} more than once, so it vouches for nothing"),
    }
}

/// `lezreg app sync`, one round: `(kept, failed)` as JSON rows.
fn sync_round(names: &mut Names<HttpRpc>, node: &lezreg_sdk::StorageClient, targets: &[(String, Option<String>)], out: &mut Out) -> (Vec<Value>, Vec<Value>) {
    let (mut kept, mut failed) = (Vec::new(), Vec::new());
    let mut fail = |out: &mut Out, name: &str, module: &str, e: String| {
        out.text(&format!("{}  {}: FAILED: {}", shown(module), shown(name), shown(&e)));
        failed.push(json!({"name": shown(name), "module": shown(module), "error": shown(&e)}));
    };
    for (asked, module) in targets {
        let (asked, name, profile) = match publisher_profile(names, asked) {
            Ok(p) => p,
            Err(e) => {
                fail(out, asked, module.as_deref().unwrap_or("*"), e.to_string());
                continue;
            }
        };
        let records: Vec<(String, Result<AppRecord>)> = match module {
            Some(m) => vec![(m.clone(), record_for(&name, &profile, m))],
            None => apps::apps_of(&profile).into_iter().map(|(m, r)| (m, r.map_err(anyhow::Error::from))).collect(),
        };
        if records.is_empty() {
            fail(out, &name, "*", format!("{name} publishes no apps"));
        }
        for (module, record) in records {
            let record = match record {
                Ok(r) => r,
                Err(e) => {
                    fail(out, &name, &module, e.to_string());
                    continue;
                }
            };
            let version = record.newest().version.clone().unwrap_or_default();
            match apps::keep_copy(node, &module, &[&asked, &name], &record) {
                Ok((how, _)) => {
                    let cid = record.cid.clone().unwrap_or_default();
                    let state = match how {
                        apps::Kept::AlreadyThere => "already on the node",
                        apps::Kept::Fetched => "fetched",
                    };
                    out.text(&format!("{module}  {}  from {name}: {state}, checked (sha256 and version are the record's) and announced  {cid}", shown(&version)));
                    kept.push(json!({"name": name, "module": module, "version": version, "cid": cid, "state": match how { apps::Kept::AlreadyThere => "already_there", apps::Kept::Fetched => "fetched" }}));
                }
                Err(e) => fail(out, &name, &module, e.to_string()),
            }
        }
    }
    (kept, failed)
}

/// The LNS program to read names from: `--lns`, `LEZREG_LNS`, else the network's.
fn lns_program(lns: Option<&str>) -> Result<AccountId> {
    let text = lns.ok_or_else(|| {
        anyhow!("no LNS program: pass --lns <program id> (or set LEZREG_LNS), or use a network whose file names one (`--network testnet`)")
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
    // A file in a variant named like an installer's own file would be overwritten by it, or
    // overwrite it, once installed (lgpm writes manifest.json, manifest.sig, variant and
    // .lgpm-source into the variant's directory).
    if let Some(p) = contents.files.keys().find(|p| {
        p.strip_prefix("variants/")
            .and_then(|r| r.split_once('/'))
            .is_some_and(|(_, rel)| lezreg_sdk::lgx::INSTALL_SIDECARS.iter().any(|s| s.eq_ignore_ascii_case(rel)))
    }) {
        bail!("{} holds {}, which the installer writes itself; rename it", file.display(), shown(p));
    }
    lezreg_sdk::lgx::check_canonical_layout(&bytes).with_context(|| {
        format!("{} is not laid out exactly as lgx writes a package; vouch only for a package lgx built (rebuild it with lgx)", file.display())
    })?;
    let name = match contents.manifest.claim() {
        Ok(Some(name)) => name,
        Ok(None) => bail!("{} names no publisher: set \"author\": \"lns:<name>\" in its metadata.json and rebuild", file.display()),
        Err(why) => bail!("{}'s author \"{}\" is {why}", file.display(), shown(&contents.manifest.author)),
    };
    Ok((bytes, contents, name))
}

/// `s` as one POSIX shell word: single-quoted, each `'` written `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
            let mut listed = Vec::new();
            for (module, record) in apps::apps_of(&profile) {
                match record {
                    Ok(record) => {
                        let programs: Vec<Value> = record
                            .programs
                            .iter()
                            .map(|p| program_status(registry.as_ref(), p))
                            .collect();
                        out.text(&format!(
                            "{}  {}  root {}{}",
                            shown(&module),
                            shown(record.newest().version.as_deref().unwrap_or("-")),
                            hex::encode(record.newest().root),
                            record.catalog.as_deref().map(|c| format!("\n  catalog {}", shown(c))).unwrap_or_default()
                        ));
                        for p in &programs {
                            out.text(&format!("  program {}  {}", shown(p["program"].as_str().unwrap_or_default()), shown(p["status"].as_str().unwrap_or_default())));
                        }
                        listed.push(json!({
                            "module": module,
                            "version": record.newest().version,
                            "roots": record.roots().iter().map(hex::encode).collect::<Vec<_>>(),
                            "catalog": record.catalog.as_deref().map(shown),
                            "programs": programs,
                        }));
                    }
                    Err(error) => {
                        out.text(&format!("{}  (vouches for nothing: {})", shown(&module), shown(&error.to_string())));
                        listed.push(json!({ "module": shown(&module), "error": shown(&error.to_string()) }));
                    }
                }
            }
            if listed.is_empty() {
                out.text(&format!("{name} publishes no apps"));
            }
            out.json(json!({ "name": name, "apps": listed }));
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
                Verdict::Verified { name, release, record } => format!(
                    "verified: {} {} is published by {name} ({}){}",
                    manifest.name,
                    shown(release.version.as_deref().unwrap_or("(no version in the record)")),
                    if contents.file_sha256().is_some() { "the whole .lgx file" } else { "the installed files and manifest.json" },
                    if record.releases.first() == Some(release) { "" } else { "; an older release the publisher still vouches for, not the newest" }
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
                    Err(why) => format!("unclaimed: {} names \"{}\", which is {why}", manifest.name, shown(&manifest.author)),
                    Ok(_) => format!("unclaimed: {} does not name an LNS publisher", manifest.name),
                },
            };
            out.text(&line);
            let mut result = verdict.to_json(&contents);
            result["forwarded_from"] = json!(via);
            out.json(result);
            match verdict.exit_code() {
                0 => {}
                1 => crate::FAILED.store(true, std::sync::atomic::Ordering::Relaxed),
                code => crate::EXIT_CODE.store(code, std::sync::atomic::Ordering::Relaxed),
            }
        }
        AppCmd::Get { name, module, out: path, storage_api, basecamp_data, allow_older } => {
            let lns = lns_program(lns)?;
            let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
            if !apps::is_module_name(&module) {
                bail!("\"{}\" is not a module name", shown(&module));
            }
            let mut names = Names::new(HttpRpc::new(&rpc), lns);
            // The same forwarding as `verify` (SPEC.md §7): a brand name leads to the publisher.
            let (asked, name, profile) = publisher_profile(&mut names, &name)?;
            let record = record_for(&name, &profile, &module).map_err(|e| anyhow!("{e}; nothing downloaded"))?;
            if record.newest().sha256.is_none() {
                bail!("unverifiable download: {name}'s record for {module} gives no sha256 of the .lgx, and the package root doesn't cover the manifest; nothing downloaded");
            }
            let version = record.newest().version.clone();
            // Never older than the installed copy without saying so (checked before fetching).
            let installed = match basecamp_data.or_else(apps::default_basecamp_data) {
                Some(data) => apps::installed_manifest(&data, &module).map_err(|e| anyhow!("{e}; nothing downloaded"))?.map(|m| m.version),
                None => None,
            };
            apps::check_not_older(&module, version.as_deref(), installed.as_deref(), allow_older).map_err(|e| anyhow!("{e}; nothing downloaded"))?;
            let cid = record.cid.clone().ok_or_else(|| anyhow!("{name} gives no Logos Storage CID for {module}; download it from its catalog ({})", shown(record.catalog.as_deref().unwrap_or("none given"))))?;
            eprintln!("fetching {module} ({cid}) from Logos Storage through {storage_api}");
            let bytes = package_node(&storage_api)
                .fetch(&cid, apps::MAX_PACKAGE, false)
                .map_err(|e| anyhow!("Logos Storage didn't give {cid}: {}", shown(&e.to_string())))?;
            let contents = apps::check_download(&module, &[&asked, &name], &record, &bytes).map_err(|e| anyhow!("{e}; nothing written"))?;
            std::fs::write(&path, &bytes).with_context(|| format!("cannot write {}", path.display()))?;
            let older = matches!((version.as_deref(), installed.as_deref()), (Some(v), Some(i)) if apps::is_older(v, i));
            out.text(&format!(
                "{module} {} from {name}: checked (sha256 matches, files match the root), written to {}{}",
                shown(version.as_deref().unwrap_or("(no version in the record)")),
                path.display(),
                if older { format!("; OLDER than the installed {} (allowed with --allow-older)", shown(installed.as_deref().unwrap_or_default())) } else { String::new() }
            ));
            out.json(json!({"name": name, "module": module, "version": version, "installed": installed, "older_than_installed": older, "cid": cid, "bytes": bytes.len(), "root": hex::encode(contents.root), "out": path}));
        }
        AppCmd::Sync { targets, storage_api, basecamp_data, every } => {
            let lns = lns_program(lns)?;
            let rpc = rpc.ok_or_else(|| anyhow!("no RPC: pass --rpc or --network"))?;
            let mut list: Vec<(String, Option<String>)> = Vec::new();
            for t in &targets {
                let (name, module) = t.split_once(':').map_or((t.as_str(), None), |(n, m)| (n, Some(m)));
                if let Some(m) = module
                    && !apps::is_module_name(m)
                {
                    bail!("\"{}\" is not a module name", shown(m));
                }
                list.push((name.to_owned(), module.map(str::to_owned)));
            }
            if list.is_empty() {
                let data = basecamp_data
                    .or_else(apps::default_basecamp_data)
                    .ok_or_else(|| anyhow!("no Basecamp data folder: pass --basecamp-data, or name the apps (<name> or <name>:<module>)"))?;
                list = apps::installed_published(&data).into_iter().map(|(module, name, _)| (name, Some(module))).collect();
                if list.is_empty() {
                    bail!("no app installed in {} names an LNS publisher; name the apps to keep (<name> or <name>:<module>)", data.display());
                }
                eprintln!("the apps installed in {}: {}", data.display(), list.iter().map(|(n, m)| format!("{n}:{}", m.as_deref().unwrap_or("*"))).collect::<Vec<_>>().join(" "));
            }
            let node = package_node(&storage_api);
            // Announcing before the node has an address makes the announcement unreachable.
            node.wait_reachable(std::time::Duration::from_secs(30))
                .map_err(|e| anyhow!("the Storage node at {storage_api} is not ready to serve ({e}): it must be running and advertise an address"))?;
            let mut names = Names::new(HttpRpc::new(&rpc), lns);
            loop {
                let (kept, failed) = sync_round(&mut names, &node, &list, out);
                eprintln!("app sync: {} kept, {} failed", kept.len(), failed.len());
                let Some(secs) = every else {
                    if !failed.is_empty() {
                        crate::FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    out.json(json!({"storage_api": storage_api, "kept": kept, "failed": failed}));
                    break;
                };
                std::thread::sleep(std::time::Duration::from_secs(secs.max(60)));
                // Read records fresh each round: a new release is what the loop is for.
                names = Names::new(HttpRpc::new(&rpc), lns);
            }
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
            if catalog.as_deref().is_some_and(|c| !apps::is_catalog_url(c)) {
                bail!("--catalog: {}", apps::CATALOG_RULE);
            }
            let programs = programs
                .iter()
                .map(|p| AccountId::from_base58(p.trim()).ok_or_else(|| anyhow!("--program is not a base58 id: {}", shown(p))))
                .collect::<Result<Vec<_>>>()?;
            // The record must fit before anything is uploaded (a Storage CID is about 50
            // characters; 64 leaves room).
            if upload {
                AppRecord { releases: releases.clone(), catalog: catalog.clone(), cid: Some("z".repeat(64)), programs: programs.clone() }
                    .to_value()
                    .context("the record would not fit in an LNS text entry; nothing uploaded")?;
            }
            let cid = if upload {
                let name_on_storage = format!("{}-{}.lgx", manifest.name, manifest.version);
                let cid = lezreg_sdk::StorageClient::new(&storage_api)
                    .upload_as(&name_on_storage, "application/octet-stream", &bytes)
                    .map_err(|e| anyhow!("uploading to {storage_api}: {}", shown(&e.to_string())))?;
                if !apps::is_cid(&cid) {
                    bail!("the Storage node answered with something that is not a CID: \"{}\"", shown(&cid));
                }
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
            // The record as anyone will read it: what is printed must parse back to this.
            AppRecord::parse(&value)?;
            let key = format!("{}{}", apps::APP_KEY_PREFIX, manifest.name);
            // Printed to be pasted into a shell: the value is single-quoted with every quote in it
            // escaped, so no record content can end the quoting (review 3, B-3).
            // name: an LNS name; key: app.<module name>; both of plain characters. The value is
            // JSON of validated fields (hex, base58, SemVer, a plain catalog URL, a CID), quoted.
            out.paste_line(&format!("names set-text {name} {key} {}", shell_quote(&value)));
            out.json(json!({ "name": name, "key": key, "value": value, "root": hex::encode(contents.root) }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review 3, B-3: whatever a record holds, the printed `set-text` line pastes as one word.
    #[test]
    fn hostile_values_stay_one_shell_word() {
        for hostile in [
            r#"{"v":1,"version":"1.2.0'$(id>/tmp/lezreg-pwned)'"}"#,
            "it's",
            "'",
            "''",
            "a'\\''b",
            "$(touch /tmp/lezreg-pwned) `id` ; | & > < * ? ~ \" \\ \n !",
            "",
        ] {
            let out = std::process::Command::new("sh").arg("-c").arg(format!("printf %s {}", shell_quote(hostile))).output().unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), hostile, "{hostile:?}");
        }
        assert!(!std::path::Path::new("/tmp/lezreg-pwned").exists());
    }
}

