//! `lezreg publish`, `lezreg adopt`, `lezreg release prepare` and `publish --from-release`
//! (DESIGN §7.2, §7.6, milestone M8).
//!
//! `publish` is the one command: deploy and register if nothing is deployed yet
//! (`DeployAndRegister`), register if deployed but unregistered, or update if registered,
//! and do nothing if the chain already says what `lezreg.toml` says. The writes are the SDK's
//! (`tx::Writer::{deploy, register_deployer, update}`), the IDL upload is `--idl-cid push`'s.
//!
//! The GitHub Action builds in CI and attaches an unsigned payload (`lezreg-release.json`,
//! `program.bin`, `idl.json`) to the release; `publish --from-release` re-checks every hash in
//! it and signs here. Header and author keys never enter CI.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use lee_core::account::AccountId;
use lezreg_sdk::{
    idl::{PreparedIdl, prepare_idl},
    tx::{EntryInput, PatchInput, Receipt, tid},
    verify,
};
use lezreg_types::{BuildConfig, ContentRef, Entry, SourceRef, pda::sha256};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    WriteOpts,
    args::{self, BuildArgs, parse_account},
    config::Resolved as Net,
    out::Out,
    project::{self, Manifest, Project},
};

pub const RELEASE_SCHEMA: &str = "lezreg-release/1";
pub const RELEASE_FILE: &str = "lezreg-release.json";
pub const BIN_FILE: &str = "program.bin";
pub const IDL_FILE: &str = "idl.json";
/// What `--from-release` downloads at most, per file.
const MAX_BIN: usize = 16 << 20;
const MAX_PAYLOAD: usize = 256 << 10;

/// Everything a publish registers, from `lezreg.toml` or from a release payload.
#[derive(Debug, Clone)]
pub struct Plan {
    pub name: String,
    pub version: String,
    pub label: String,
    pub description: String,
    pub tags: Vec<String>,
    /// The IDL JSON (canonicalized before use).
    pub idl: PathBuf,
    pub source: SourceRef,
    pub build: BuildConfig,
    /// The program binary (`.bin`), when deploying or checking against the live program.
    pub bin: Option<Vec<u8>>,
    pub program: Option<AccountId>,
    pub author: Option<AccountId>,
    pub immutable: bool,
}

/// The full build tuple from `lezreg.toml` (`BuildArgs` over lezreg's defaults).
pub fn build_config(m: &Manifest, lock_sha256: &[u8; 32]) -> Result<BuildConfig> {
    let b = &m.build;
    BuildArgs {
        image_digest: Some(b.image_digest.clone()),
        image_tag: Some(b.image_tag.clone()),
        risc0_build: Some(b.risc0_build.clone()),
        host_rust: Some(b.host_rust.clone()),
        manifest_path: Some(b.manifest_path.clone()),
        binary: Some(b.binary.clone()),
        features: b.features.clone(),
        lock_sha256: Some(hex::encode(lock_sha256)),
        lock_file: None,
    }
    .apply(args::default_build(&b.binary)?)
}

fn commit_bytes(hex_commit: &str) -> Result<Vec<u8>> {
    let v = hex::decode(hex_commit).context("the commit is not hex")?;
    if !matches!(v.len(), 20 | 32) {
        bail!("the commit must be a 40- or 64-character hex object id");
    }
    Ok(v)
}

/// The guest's `Cargo.lock` hash, as `verify` will find it (walking up from the manifest).
fn lock_sha(p: &Project) -> Result<[u8; 32]> {
    let root = p.docker_root_dir()?;
    let lock = verify::guest_lock(&root, &p.m.build.manifest_path).ok_or_else(|| {
        anyhow!(
            "no Cargo.lock above {} in {}: commit one, or nobody can reproduce the build",
            p.m.build.manifest_path,
            root.display()
        )
    })?;
    Ok(sha256(&std::fs::read(&lock).with_context(|| {
        format!("cannot read {}", lock.display())
    })?))
}

/// The tree's commit, refusing uncommitted changes (unless allowed), with a warning when the
/// commit is on no remote branch (verifiers could not fetch it).
fn head_commit(p: &Project, allow_dirty: bool) -> Result<String> {
    project::check_clean(&p.dir, allow_dirty)?;
    let commit = project::git(&p.dir, &["rev-parse", "HEAD"])?;
    if project::git(&p.dir, &["branch", "-r", "--contains", "HEAD"])
        .map(|s| s.is_empty())
        .unwrap_or(false)
    {
        eprintln!(
            "warning: commit {} is on no remote branch: push it, or `verify` cannot fetch it",
            &commit[..commit.len().min(12)]
        );
    }
    Ok(commit)
}

/// A plan from `lezreg.toml` and the tree it sits in.
pub fn plan_from_project(
    p: &Project,
    bin: Option<Vec<u8>>,
    program: Option<AccountId>,
    allow_dirty: bool,
) -> Result<Plan> {
    if p.m.source.repo.is_empty() {
        bail!(
            "{}: [source] repo is empty: set it to the public https URL of the repository",
            p.path.display()
        );
    }
    let commit = head_commit(p, allow_dirty)?;
    let lock = lock_sha(p)?;
    let program = match program {
        Some(x) => Some(x),
        None => p.program().map(parse_account).transpose()?,
    };
    Ok(Plan {
        name: p.m.program.name.clone(),
        version: p.m.program.version.clone(),
        label: p.m.program.label.clone(),
        description: p.m.program.description.clone(),
        tags: p.m.program.tags.clone(),
        idl: p.idl_path()?,
        source: SourceRef {
            repo: p.m.source.repo.clone(),
            commit: commit_bytes(&commit)?,
            docker_root: p.m.source.docker_root.clone(),
            archive: None,
        },
        build: build_config(&p.m, &lock)?,
        bin,
        program,
        author: p
            .m
            .deploy
            .author
            .as_deref()
            .map(parse_account)
            .transpose()?,
        immutable: p.m.deploy.immutable,
    })
}

/// Reads a program binary or guest ELF (`--elf`) as a deployable `.bin`.
pub fn read_bin(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    verify::program_bin(&bytes).map_err(|e| anyhow!("{}: {e}", path.display()))
}

/// The entry fields of a plan, with the IDL reference given.
fn entry_input(plan: &Plan, idl: ContentRef) -> EntryInput {
    EntryInput {
        name: plan.name.clone(),
        version: plan.version.clone(),
        author_label: plan.label.clone(),
        description: plan.description.clone(),
        tags: plan.tags.clone(),
        idl,
        source: plan.source.clone(),
        build: plan.build.clone(),
        image_id: None,
    }
}

/// The on-chain IDL reference: uploaded to the user's Storage node (`--idl-cid push`), or
/// for a dry run the raw-codec CID of the canonical bytes, uploading nothing.
fn idl_ref(prepared: &PreparedIdl, path: &Path, dry_run: bool) -> Result<ContentRef> {
    if dry_run {
        args::content(
            "raw",
            Some(&hex::encode(prepared.sha256)),
            Some(u32::try_from(prepared.bytes.len()).context("IDL too large")?),
            None,
            "idl",
        )
    } else {
        args::content("push", None, None, Some(path), "idl")
    }
}

/// What an `Update` must change for the entry to say what the plan says.
#[derive(Debug, Default)]
pub struct Changes {
    pub patch: PatchInput,
    /// The IDL changed (its reference is filled in after uploading).
    pub idl: bool,
    pub fields: Vec<&'static str>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// Compares a stored entry with the plan (the IDL by its canonical sha256) and the live
/// program's image.
pub fn diff(stored: &Entry, plan: &Plan, idl_sha256: &[u8; 32], live: [u8; 32]) -> Changes {
    let mut c = Changes::default();
    let mut set = |f: &'static str, changed: bool| {
        if changed {
            c.fields.push(f);
        }
        changed
    };
    if set("version", stored.version != plan.version) {
        c.patch.version = Some(plan.version.clone());
    }
    if set("label", stored.author_label != plan.label) {
        c.patch.author_label = Some(plan.label.clone());
    }
    if set("description", stored.description != plan.description) {
        c.patch.description = Some(plan.description.clone());
    }
    if set("tags", stored.tags != plan.tags) {
        c.patch.tags = Some(plan.tags.clone());
    }
    c.idl = set("idl", stored.idl.sha256 != *idl_sha256);
    let s = &stored.source;
    if set(
        "source",
        s.repo != plan.source.repo
            || s.commit != plan.source.commit
            || s.docker_root != plan.source.docker_root,
    ) {
        c.patch.source = Some(plan.source.clone());
    }
    if set("build", stored.build != plan.build) {
        c.patch.build = Some(plan.build.clone());
    }
    if set("image_id", stored.image_id != live) {
        c.patch.image_id = Some(live);
    }
    c
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Publish,
    Adopt,
}

/// Records the program id in `lezreg.toml` (only if it has none: never repoints a project).
fn remember(project: Option<&mut Project>, program: AccountId) {
    let Some(p) = project else { return };
    match p.program() {
        None => match p.save_program(&program.to_string()) {
            Ok(()) => eprintln!("recorded program {program} in {}", p.path.display()),
            Err(e) => eprintln!(
                "warning: could not record program {program} in {}: {e:#}",
                p.path.display()
            ),
        },
        Some(have) if have != program.to_string() => eprintln!(
            "note: {} names program {have}; left unchanged",
            p.path.display()
        ),
        Some(_) => {}
    }
}

fn share_line(net: &Net, program: &AccountId) -> String {
    let flag = if net.name == "local" {
        String::new()
    } else {
        format!(" --network {}", net.name)
    };
    format!("lezreg{flag} show {program}")
}

fn done(
    out: &mut Out,
    net: &Net,
    action: &str,
    plan: &Plan,
    program: AccountId,
    receipt: Option<&Receipt>,
    changed: &[&str],
) {
    if let Some(r) = receipt {
        crate::render::receipt(out, r);
    }
    let dry = receipt.is_some_and(|r| r.hash.is_none());
    let share = share_line(net, &program);
    out.text(&format!(
        "{}{} {} {} at {program}{}\nshare: {share}",
        if dry { "dry run: would have " } else { "" },
        action,
        plan.name,
        plan.version,
        if changed.is_empty() {
            String::new()
        } else {
            format!(" (changed: {})", changed.join(", "))
        }
    ));
    out.json(json!({
        "action": action,
        "dry_run": dry,
        "program": program.to_string(),
        "name": plan.name,
        "version": plan.version,
        "changed": changed,
        "receipt": receipt.map(Receipt::json),
        "share": share,
    }));
}

/// The one command.
pub async fn publish(
    out: &mut Out,
    net: &Net,
    plan: &Plan,
    opts: &WriteOpts,
    mut project: Option<&mut Project>,
    mode: Mode,
) -> Result<()> {
    let so = crate::send_opts(opts)?;
    let raw = std::fs::read(&plan.idl)
        .with_context(|| format!("cannot read the IDL {}", plan.idl.display()))?;
    let prepared = prepare_idl(&raw).map_err(|e| anyhow!("{}: {e}", plan.idl.display()))?;
    lezreg_types::validate::entry_fields(
        &plan.name,
        &plan.version,
        &plan.label,
        &plan.description,
        &plan.tags,
        &ContentRef {
            cid: vec![1, 0x55, 0x12, 0x20],
            sha256: prepared.sha256,
            size: 1,
            peer_hint: None,
        },
        &plan.source,
        &plan.build,
    )
    .map_err(|e| anyhow!("{e}"))?;
    let r = crate::reader(net)?;
    let resolved = plan.program.map(|p| r.resolve(&tid(&p))).transpose()?;
    let header = resolved.as_ref().and_then(|x| x.header);
    let bin_image = plan
        .bin
        .as_deref()
        .map(verify::image_id_of_bin)
        .transpose()?;
    if let (Some(h), Some(img)) = (header, bin_image)
        && h.image_id != img
    {
        bail!(
            "{} runs image {}, but the binary given is image {}: it is not this program's code. Upgrading the code is not something publish does",
            plan.program.expect("has a header"),
            hex::encode(h.image_id),
            hex::encode(img)
        );
    }
    if mode == Mode::Adopt && header.is_none() {
        bail!(
            "nothing is deployed at {}: adopt registers a deployed program (`lezreg publish --elf` deploys and registers)",
            plan.program.map(|p| p.to_string()).unwrap_or_default()
        );
    }
    let mut w = crate::writer(net, opts).await?;
    match (plan.program, header, resolved.and_then(|x| x.deployer)) {
        (given, None, _) => {
            let Some(bin) = &plan.bin else {
                bail!(
                    "{} is not deployed yet: pass --elf <program .bin or guest ELF> (`lezreg build` makes one) to deploy and register it",
                    given.map_or_else(|| plan.name.clone(), |p| p.to_string())
                );
            };
            let input = entry_input(plan, idl_ref(&prepared, &plan.idl, opts.dry_run)?);
            let program = match given {
                Some(p) => p,
                None => w.new_account().await?,
            };
            let author = plan.author.unwrap_or(program);
            if plan.author.is_none() {
                eprintln!(
                    "note: no [deploy] author, so the program's own header key is the entry's author. A separate author account (kept with your other programs) is safer and makes your programs count as one author"
                );
            }
            let result = w
                .deploy(bin, program, plan.immutable, Some((author, &input)), &so)
                .await;
            if !opts.dry_run && (given.is_some() || w.wrote()) {
                out.context("program", json!(program.to_string()));
                remember(project.as_deref_mut(), program);
            }
            let receipt = result?;
            done(
                out,
                net,
                "deployed and registered",
                plan,
                program,
                Some(&receipt),
                &[],
            );
        }
        (Some(program), Some(_), None) => {
            if !w.holds(program) {
                bail!(
                    "{program} is deployed, but this wallet does not hold its header key, so it cannot make the deployer entry. A community entry needs no key: `lezreg register --third-party --program {program} --publisher <your account> …`"
                );
            }
            let author = plan.author.unwrap_or(program);
            // A header account usually holds nothing: the author pays unless --payer says.
            let so = lezreg_sdk::tx::SendOpts {
                payer: so.payer.or(Some(author)),
                ..so.clone()
            };
            let input = entry_input(plan, idl_ref(&prepared, &plan.idl, opts.dry_run)?);
            let receipt = w.register_deployer(program, author, &input, &so).await?;
            if !opts.dry_run {
                remember(project.as_deref_mut(), program);
            }
            done(out, net, "registered", plan, program, Some(&receipt), &[]);
        }
        (Some(program), Some(h), Some(stored)) => {
            if let Some(a) = plan.author
                && tid(&a) != stored.author
            {
                eprintln!(
                    "note: the entry's author is {}, not [deploy] author {a}: the entry stays with its author (`lezreg transfer` hands it over)",
                    stored.author
                );
            }
            let mut ch = diff(&stored, plan, &prepared.sha256, h.image_id);
            if !opts.dry_run {
                remember(project.as_deref_mut(), program);
            }
            if ch.is_empty() {
                out.text(&format!(
                    "{} {} at {program} is up to date (revision {}): nothing to send\nshare: {}",
                    stored.name,
                    stored.version,
                    stored.revision,
                    share_line(net, &program)
                ));
                out.json(json!({"action": "unchanged", "program": program.to_string(), "name": stored.name, "version": stored.version, "revision": stored.revision, "share": share_line(net, &program)}));
                return Ok(());
            }
            if stored.name != plan.name {
                eprintln!(
                    "note: the entry is named {} and names never change; lezreg.toml says {}",
                    stored.name, plan.name
                );
            }
            if ch.idl {
                ch.patch.idl = Some(idl_ref(&prepared, &plan.idl, opts.dry_run)?);
            }
            let receipt = w.update(program, None, &ch.patch, &so).await?;
            done(
                out,
                net,
                "updated",
                plan,
                program,
                Some(&receipt),
                &ch.fields,
            );
        }
        (None, Some(_), _) => unreachable!("a header needs a program id"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Release payloads (the GitHub Action's `prepare` mode, and `publish --from-release`).

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub sha256: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BuildTuple {
    pub image_digest: String,
    pub image_tag: String,
    pub risc0_build: String,
    pub host_rust: String,
    pub manifest_path: String,
    pub binary: String,
    pub features: Vec<String>,
    pub lock_sha256: String,
}

/// `lezreg-release.json`: an unsigned register/update payload with every hash.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePayload {
    pub schema: String,
    pub name: String,
    pub version: String,
    pub label: String,
    pub description: String,
    pub tags: Vec<String>,
    pub repo: String,
    pub commit: String,
    pub docker_root: String,
    pub build: BuildTuple,
    /// The image id of `program.bin`.
    pub image_id: String,
    /// The program, if the project has been deployed.
    pub program: Option<String>,
    pub author: Option<String>,
    pub immutable: bool,
    /// `program.bin` and `idl.json` (canonical).
    pub files: BTreeMap<String, FileRef>,
}

fn file_ref(b: &[u8]) -> FileRef {
    FileRef {
        sha256: hex::encode(sha256(b)),
        size: b.len() as u64,
    }
}

impl ReleasePayload {
    pub fn of(plan: &Plan, bin: &[u8], idl: &[u8], image: [u8; 32]) -> Self {
        let b = &plan.build;
        Self {
            schema: RELEASE_SCHEMA.into(),
            name: plan.name.clone(),
            version: plan.version.clone(),
            label: plan.label.clone(),
            description: plan.description.clone(),
            tags: plan.tags.clone(),
            repo: plan.source.repo.clone(),
            commit: hex::encode(&plan.source.commit),
            docker_root: plan.source.docker_root.clone(),
            build: BuildTuple {
                image_digest: hex::encode(b.image_digest),
                image_tag: b.image_tag.clone(),
                risc0_build: b.risc0_build.clone(),
                host_rust: b.host_risc0_rust.clone(),
                manifest_path: b.manifest_path.clone(),
                binary: b.binary.clone(),
                features: b.features.clone(),
                lock_sha256: hex::encode(b.lock_sha256),
            },
            image_id: hex::encode(image),
            program: plan.program.map(|p| p.to_string()),
            author: plan.author.map(|a| a.to_string()),
            immutable: plan.immutable,
            files: [
                (BIN_FILE.to_owned(), file_ref(bin)),
                (IDL_FILE.to_owned(), file_ref(idl)),
            ]
            .into(),
        }
    }

    /// Checks the files against the payload: sizes, sha256s, the binary's image id, and that
    /// the IDL is canonical. Returns the binary.
    pub fn check(&self, bin: &[u8], idl: &[u8]) -> Result<Vec<u8>> {
        if self.schema != RELEASE_SCHEMA {
            bail!(
                "unknown payload schema '{}' (this lezreg reads {RELEASE_SCHEMA})",
                self.schema
            );
        }
        for (name, bytes) in [(BIN_FILE, bin), (IDL_FILE, idl)] {
            let want = self
                .files
                .get(name)
                .ok_or_else(|| anyhow!("the payload lists no {name}"))?;
            let got = file_ref(bytes);
            if got != *want {
                bail!(
                    "{name} does not match the payload: sha256 {} ({} B), the payload says {} ({} B)",
                    got.sha256,
                    got.size,
                    want.sha256,
                    want.size
                );
            }
        }
        let bin = verify::program_bin(bin).map_err(|e| anyhow!("{BIN_FILE}: {e}"))?;
        let image = hex::encode(verify::image_id_of_bin(&bin)?);
        if image != self.image_id {
            bail!(
                "{BIN_FILE} is image {image}, the payload says {}",
                self.image_id
            );
        }
        let canon = prepare_idl(idl).map_err(|e| anyhow!("{IDL_FILE}: {e}"))?;
        if canon.bytes != idl {
            bail!("{IDL_FILE} is not canonical JSON (RFC 8785)");
        }
        Ok(bin)
    }

    pub fn plan(&self, bin: Vec<u8>, idl: PathBuf) -> Result<Plan> {
        let b = &self.build;
        let build = BuildArgs {
            image_digest: Some(b.image_digest.clone()),
            image_tag: Some(b.image_tag.clone()),
            risc0_build: Some(b.risc0_build.clone()),
            host_rust: Some(b.host_rust.clone()),
            manifest_path: Some(b.manifest_path.clone()),
            binary: Some(b.binary.clone()),
            features: b.features.clone(),
            lock_sha256: Some(b.lock_sha256.clone()),
            lock_file: None,
        }
        .apply(args::default_build(&b.binary)?)?;
        Ok(Plan {
            name: self.name.clone(),
            version: self.version.clone(),
            label: self.label.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            idl,
            source: SourceRef {
                repo: self.repo.clone(),
                commit: commit_bytes(&self.commit)?,
                docker_root: self.docker_root.clone(),
                archive: None,
            },
            build,
            bin: Some(bin),
            program: self.program.as_deref().map(parse_account).transpose()?,
            author: self.author.as_deref().map(parse_account).transpose()?,
            immutable: self.immutable,
        })
    }
}

/// `lezreg release prepare`: the payload for a built binary, written to `out_dir`. With
/// `live`, the program's live image (read from the sequencer) must equal the binary's.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    out: &mut Out,
    net: &Net,
    p: &Project,
    elf: &Path,
    out_dir: &Path,
    program: Option<AccountId>,
    live: bool,
    allow_dirty: bool,
) -> Result<()> {
    let bin = read_bin(elf)?;
    let plan = plan_from_project(p, Some(bin.clone()), program, allow_dirty)?;
    let image = verify::image_id_of_bin(&bin)?;
    let raw = std::fs::read(&plan.idl)
        .with_context(|| format!("cannot read the IDL {}", plan.idl.display()))?;
    let idl = prepare_idl(&raw).map_err(|e| anyhow!("{}: {e}", plan.idl.display()))?;
    let payload = ReleasePayload::of(&plan, &bin, &idl.bytes, image);
    std::fs::create_dir_all(out_dir)?;
    std::fs::write(out_dir.join(BIN_FILE), &bin)?;
    std::fs::write(out_dir.join(IDL_FILE), &idl.bytes)?;
    std::fs::write(
        out_dir.join(RELEASE_FILE),
        serde_json::to_vec_pretty(&payload)?,
    )?;
    let mut live_image = None;
    if live {
        let prog = plan.program.ok_or_else(|| {
            anyhow!("--live needs a program id (--program, or [deploy] program in lezreg.toml)")
        })?;
        let rpc = net.rpc()?;
        // A header read needs no registry id.
        let r = lezreg_sdk::Registry::new(
            lezreg_sdk::HttpRpc::new(&rpc),
            net.registry.unwrap_or_default(),
        );
        let h = r.header(&tid(&prog))?;
        live_image = Some(h.map(|h| hex::encode(h.image_id)));
    }
    let matches = live_image
        .as_ref()
        .map(|l| l.as_deref() == Some(&payload.image_id));
    out.text(&format!(
        "{} {} commit {}\n  image id  {}{}\n  wrote     {}/{{{BIN_FILE}, {IDL_FILE}, {RELEASE_FILE}}}",
        payload.name,
        payload.version,
        payload.commit,
        payload.image_id,
        match &live_image {
            None => String::new(),
            Some(None) => "\n  live      (nothing deployed at the program)".into(),
            Some(Some(l)) => format!(
                "\n  live      {l}  {}",
                if matches == Some(true) { "MATCH" } else { "MISMATCH" }
            ),
        },
        out_dir.display()
    ));
    out.json(json!({
        "name": payload.name,
        "version": payload.version,
        "commit": payload.commit,
        "image_id": payload.image_id,
        "image_digest": payload.build.image_digest,
        "program": payload.program,
        "live_image_id": live_image.clone().flatten(),
        "match": matches,
        "dir": out_dir.display().to_string(),
        "payload": payload,
    }));
    if matches == Some(false) {
        bail!(
            "the live program does not run this build's image ({})",
            payload.image_id
        );
    }
    Ok(())
}

/// Where the payload's files are: a GitHub release tag page becomes its download base; a
/// URL of `lezreg-release.json` becomes its directory.
pub fn release_base(url: &str) -> String {
    let u = url.trim_end_matches('/');
    if let Some((repo, tag)) = u.split_once("/releases/tag/") {
        return format!("{repo}/releases/download/{tag}/");
    }
    if let Some(dir) = u.strip_suffix(RELEASE_FILE) {
        return dir.to_owned();
    }
    format!("{u}/")
}

fn download(agent: &ureq::Agent, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut resp = agent.get(url).call().map_err(|e| anyhow!("{url}: {e}"))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(limit as u64)
        .read_to_vec()
        .map_err(|e| anyhow!("{url}: {e}"))?;
    Ok(bytes)
}

/// The payload's three files, from a directory or a URL (into `scratch`).
pub fn fetch_release(src: &str, scratch: &Path) -> Result<(ReleasePayload, Vec<u8>, PathBuf)> {
    let (payload, bin, idl_path) = if src.starts_with("https://") || src.starts_with("http://") {
        let base = release_base(src);
        let host_ok = url::Url::parse(&base)
            .is_ok_and(|u| u.scheme() == "https" || crate::is_local(&base) && u.scheme() == "http");
        if !host_ok {
            bail!("{src}: only https:// URLs (or http on this machine)");
        }
        // Release assets redirect to the forge's file host; follow at most a few.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .max_redirects(5)
            .build()
            .into();
        eprintln!("fetching the release payload from {base}");
        let payload = download(&agent, &format!("{base}{RELEASE_FILE}"), MAX_PAYLOAD)?;
        let bin = download(&agent, &format!("{base}{BIN_FILE}"), MAX_BIN)?;
        let idl = download(
            &agent,
            &format!("{base}{IDL_FILE}"),
            lezreg_sdk::storage::MAX_IDL,
        )?;
        std::fs::create_dir_all(scratch)?;
        let idl_path = scratch.join(IDL_FILE);
        std::fs::write(&idl_path, &idl)?;
        (payload, bin, idl_path)
    } else {
        let d = Path::new(src);
        let read = |f: &str| {
            std::fs::read(d.join(f)).with_context(|| format!("cannot read {}", d.join(f).display()))
        };
        (read(RELEASE_FILE)?, read(BIN_FILE)?, d.join(IDL_FILE))
    };
    let payload: ReleasePayload =
        serde_json::from_slice(&payload).context("lezreg-release.json is not a release payload")?;
    let idl = std::fs::read(&idl_path)?;
    let bin = payload.check(&bin, &idl)?;
    Ok((payload, bin, idl_path))
}

/// When run inside a checkout of the payload's commit, the local `Cargo.lock` must hash to the
/// payload's: one more hash checked on this machine instead of trusting CI.
pub fn check_against_checkout(payload: &ReleasePayload, p: &Project) -> Result<Option<bool>> {
    let Ok(head) = project::git(&p.dir, &["rev-parse", "HEAD"]) else {
        return Ok(None);
    };
    if head != payload.commit {
        return Ok(None);
    }
    let lock = hex::encode(lock_sha(p)?);
    if lock != payload.build.lock_sha256 {
        bail!(
            "this checkout is at the payload's commit, but its Cargo.lock hashes to {lock}, not the payload's {}",
            payload.build.lock_sha256
        );
    }
    Ok(Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lezreg_types as t;

    fn plan() -> Plan {
        Plan {
            name: "tok".into(),
            version: "1.0.0".into(),
            label: "Acme".into(),
            description: "d".into(),
            tags: vec!["token".into()],
            idl: PathBuf::from("idl.json"),
            source: SourceRef {
                repo: "https://github.com/acme/tok".into(),
                commit: vec![7; 20],
                docker_root: ".".into(),
                archive: None,
            },
            build: args::default_build("tok").unwrap(),
            bin: None,
            program: None,
            author: None,
            immutable: false,
        }
    }

    fn stored(p: &Plan, idl_sha: [u8; 32], image: [u8; 32]) -> Entry {
        Entry {
            schema: 1,
            kind: t::KIND_DEPLOYER,
            flags: 0,
            program: t::AccountId([1; 32]),
            author: t::AccountId([2; 32]),
            registrant: t::AccountId([2; 32]),
            name: p.name.clone(),
            version: p.version.clone(),
            author_label: p.label.clone(),
            description: p.description.clone(),
            tags: p.tags.clone(),
            idl: ContentRef {
                cid: vec![1],
                sha256: idl_sha,
                size: 3,
                peer_hint: Some(vec![1, 2]),
            },
            source: SourceRef {
                archive: None,
                ..p.source.clone()
            },
            build: p.build.clone(),
            image_id: image,
            registered_at: 0,
            updated_at: 0,
            revision: 3,
        }
    }

    #[test]
    fn an_unchanged_project_sends_nothing() {
        let p = plan();
        let e = stored(&p, [9; 32], [5; 32]);
        assert!(diff(&e, &p, &[9; 32], [5; 32]).is_empty());
    }

    #[test]
    fn a_version_bump_is_an_update_of_just_that() {
        let mut p = plan();
        let e = stored(&p, [9; 32], [5; 32]);
        p.version = "1.1.0".into();
        p.source.commit = vec![8; 20];
        let c = diff(&e, &p, &[9; 32], [5; 32]);
        assert_eq!(c.fields, vec!["version", "source"]);
        assert_eq!(c.patch.version.as_deref(), Some("1.1.0"));
        assert_eq!(c.patch.source.as_ref().unwrap().commit, vec![8; 20]);
        assert!(c.patch.build.is_none() && c.patch.idl.is_none() && !c.idl);
        assert!(c.patch.image_id.is_none());
    }

    #[test]
    fn idl_build_and_a_new_live_image_are_noticed() {
        let mut p = plan();
        let e = stored(&p, [9; 32], [5; 32]);
        p.build.features = vec!["x".into()];
        let c = diff(&e, &p, &[10; 32], [6; 32]);
        assert_eq!(c.fields, vec!["idl", "build", "image_id"]);
        assert!(c.idl);
        assert_eq!(c.patch.image_id, Some([6; 32]));
        assert_eq!(
            c.patch.build.as_ref().unwrap().features,
            vec!["x".to_owned()]
        );
        // Clearing every tag is a change too.
        p = plan();
        p.tags.clear();
        assert_eq!(diff(&e, &p, &[9; 32], [5; 32]).patch.tags, Some(vec![]));
    }

    #[test]
    fn release_urls() {
        assert_eq!(
            release_base("https://github.com/acme/tok/releases/tag/v1.2.0"),
            "https://github.com/acme/tok/releases/download/v1.2.0/"
        );
        assert_eq!(
            release_base("https://github.com/acme/tok/releases/download/v1/lezreg-release.json"),
            "https://github.com/acme/tok/releases/download/v1/"
        );
        assert_eq!(
            release_base("https://example.org/rel"),
            "https://example.org/rel/"
        );
    }

    /// The committed registry binary as a release; every tampered file is refused.
    #[test]
    fn payload_hashes_are_rechecked() {
        let bin = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../artifacts/lez_program_registry.bin"
        ))
        .unwrap();
        let idl_raw = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../idl/lez_program_registry.idl.json"
        ))
        .unwrap();
        let idl = prepare_idl(&idl_raw).unwrap().bytes;
        let image = verify::image_id_of_bin(&bin).unwrap();
        let want = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../artifacts/lez_program_registry.imageid"
        ))
        .unwrap();
        assert_eq!(hex::encode(image), want.trim());
        let payload = ReleasePayload::of(&plan(), &bin, &idl, image);
        // Round trip through JSON, then every check passes.
        let payload: ReleasePayload =
            serde_json::from_slice(&serde_json::to_vec(&payload).unwrap()).unwrap();
        assert_eq!(payload.check(&bin, &idl).unwrap(), bin);
        let plan2 = payload.plan(bin.clone(), PathBuf::from("x")).unwrap();
        assert_eq!(plan2.build, plan().build);
        assert_eq!(plan2.source, plan().source);
        // A flipped byte in the binary.
        let mut bad = bin.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(payload.check(&bad, &idl).is_err());
        // A different (but valid) IDL.
        let other = prepare_idl(br#"{"name":"x","version":"1","instructions":[]}"#);
        if let Ok(o) = other {
            assert!(payload.check(&bin, &o.bytes).is_err());
        }
        // The raw (non-canonical) IDL with a payload that claims it.
        if idl_raw != idl {
            let mut p2 = payload.clone();
            p2.files.insert(IDL_FILE.into(), file_ref(&idl_raw));
            let e = p2.check(&bin, &idl_raw).unwrap_err().to_string();
            assert!(e.contains("canonical"), "{e}");
        }
        // A payload that claims another image for the same bytes.
        let mut p3 = payload.clone();
        p3.image_id = hex::encode([0u8; 32]);
        assert!(p3.check(&bin, &idl).is_err());
        let mut p4 = payload;
        p4.schema = "lezreg-release/9".into();
        assert!(p4.check(&bin, &idl).is_err());
    }
}
