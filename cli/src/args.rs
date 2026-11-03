//! Entry fields on the command line. IDL and source archive pointers are given as a Storage
//! CID plus the content's sha256 and size (or a local file to hash), or for the IDL as
//! `--idl-cid push --idl-file f`, which uploads it to the user's Storage node first.

use anyhow::{Context as _, Result, anyhow, bail};
use lee_core::account::AccountId;
use lezreg_sdk::{
    cid,
    tx::{EntryInput, PatchInput},
};
use lezreg_types::{self as t, BuildConfig, ContentRef, Entry, SourceRef};

/// The `risczero/risc0-guest-builder:r0.1.91.1` digest LEZ's own CI pins (DESIGN §4.1).
pub const DEFAULT_IMAGE_DIGEST: &str =
    "fafb377a44e1cfca415577c48d2f7012bda99ed36f2fae27f9a663b9fe6048f0";

/// An account as the wallet prints it (`Public/<id>`) or bare base58.
pub fn parse_account(s: &str) -> Result<AccountId> {
    let bare = s.strip_prefix("Public/").unwrap_or(s);
    if s.starts_with("Private/") {
        bail!("{s}: the registry is public-only; use a public account");
    }
    bare.parse()
        .map_err(|_| anyhow!("'{s}' is not an account id"))
}

pub fn hex32(s: &str, what: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.strip_prefix("0x").unwrap_or(s))
        .with_context(|| format!("{what} is not hex"))?;
    v.try_into()
        .map_err(|_| anyhow!("{what} must be 32 bytes (64 hex characters)"))
}

fn content(
    cid_text: &str,
    sha: Option<&str>,
    size: Option<u32>,
    file: Option<&std::path::Path>,
    what: &str,
) -> Result<ContentRef> {
    // `--idl-cid push --idl-file f`: canonicalize the IDL and upload it to the user's Storage
    // node first (DESIGN §5.3); the reference then carries the node as `peer_hint`.
    if cid_text == "push" {
        if what != "idl" {
            bail!("`push` works for --idl-cid only; upload the archive and pass its CID");
        }
        let Some(f) = file else {
            bail!("--idl-cid push needs --idl-file <the IDL JSON>")
        };
        if sha.is_some() || size.is_some() {
            bail!("--idl-cid push hashes the canonical IDL itself; drop --idl-sha256/--idl-size");
        }
        let raw = std::fs::read(f).with_context(|| format!("cannot read {}", f.display()))?;
        let prepared =
            lezreg_sdk::idl::prepare_idl(&raw).map_err(|e| anyhow!("{}: {e}", f.display()))?;
        let api = std::env::var("LEZREG_STORAGE_API")
            .unwrap_or_else(|_| lezreg_sdk::storage::DEFAULT_API.to_owned());
        eprintln!(
            "uploading {} ({} B canonical) to {api}",
            prepared.filename,
            prepared.bytes.len()
        );
        let node = lezreg_sdk::StorageClient::new(&api);
        return lezreg_sdk::idl::publish(&node, &prepared, std::time::Duration::from_secs(60))
            .map_err(|e| anyhow!("IDL upload: {e}"));
    }
    let (sha256, size) = match (file, sha, size) {
        (Some(f), None, None) => {
            let b = std::fs::read(f).with_context(|| format!("cannot read {}", f.display()))?;
            (
                t::pda::sha256(&b),
                u32::try_from(b.len()).context("file too large")?,
            )
        }
        (None, Some(s), Some(n)) => (hex32(s, &format!("{what} sha256"))?, n),
        _ => bail!(
            "{what}: give the content's sha256 and size (--{what}-sha256 and --{what}-size), or --{what}-file to hash a local copy"
        ),
    };
    // `--idl-cid raw` (or `--archive-cid raw`): the raw-codec sha2-256 CID of the content.
    let cid = if cid_text == "raw" {
        let mut c = vec![0x01, 0x55, 0x12, 0x20];
        c.extend_from_slice(&sha256);
        c
    } else {
        cid::parse(cid_text).map_err(|e| anyhow!("{what} CID: {e}"))?
    };
    if let Some(named) = cid::raw_sha256(&cid)
        && named != sha256
    {
        bail!(
            "the {what} CID is a raw sha2-256 CID of other bytes: it names sha256 {}, but the content's is {}",
            hex::encode(named),
            hex::encode(sha256)
        );
    }
    Ok(ContentRef {
        cid,
        sha256,
        size,
        peer_hint: None,
    })
}

#[derive(clap::Args, Clone, Default)]
pub struct EntryArgs {
    /// Name (immutable): [a-z0-9][a-z0-9._-]*, at most 32 bytes.
    #[arg(long)]
    pub name: Option<String>,
    /// Version (printable ASCII, semver recommended).
    #[arg(long = "version", id = "entry_version")]
    pub version: Option<String>,
    /// Free-text author name. Shown as UNPROVEN.
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// A tag ([a-z0-9-]); repeat for more (at most 8).
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// IDL on Logos Storage: its CID (z…, b… or 0x…). `push` (with --idl-file) uploads the
    /// canonical IDL to your Storage node (LEZREG_STORAGE_API) and uses that; `raw` uses the
    /// raw-codec sha2-256 CID of the bytes without uploading.
    #[arg(long)]
    pub idl_cid: Option<String>,
    /// IDL sha256 (hex).
    #[arg(long)]
    pub idl_sha256: Option<String>,
    /// IDL size in bytes.
    #[arg(long)]
    pub idl_size: Option<u32>,
    /// A local copy of the IDL to hash instead of --idl-sha256/--idl-size.
    #[arg(long)]
    pub idl_file: Option<std::path::PathBuf>,
    /// Source repository URL.
    #[arg(long)]
    pub repo: Option<String>,
    /// Source commit (hex, 40 or 64 characters).
    #[arg(long)]
    pub commit: Option<String>,
    /// Build context inside the repo.
    #[arg(long, default_value = ".")]
    pub docker_root: String,
    /// Source archive (`git archive`) on Logos Storage: its CID.
    #[arg(long)]
    pub archive_cid: Option<String>,
    #[arg(long)]
    pub archive_sha256: Option<String>,
    #[arg(long)]
    pub archive_size: Option<u32>,
    #[arg(long)]
    pub archive_file: Option<std::path::PathBuf>,
    #[command(flatten)]
    pub build: BuildArgs,
    /// Claimed image id (hex). Default: the live program header's (or the ELF's for deploy).
    #[arg(long)]
    pub image_id: Option<String>,
}

#[derive(clap::Args, Clone, Default)]
pub struct BuildArgs {
    /// Builder image digest (sha256 hex) of risczero/risc0-guest-builder.
    #[arg(long)]
    pub image_digest: Option<String>,
    /// Builder image tag (informational).
    #[arg(long)]
    pub image_tag: Option<String>,
    /// risc0-build version.
    #[arg(long)]
    pub risc0_build: Option<String>,
    /// Host rzup Rust version.
    #[arg(long)]
    pub host_rust: Option<String>,
    /// Guest Cargo.toml, relative to the docker root.
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Guest binary target name. Default: the entry name.
    #[arg(long)]
    pub binary: Option<String>,
    /// Cargo feature of the guest build; repeat for more.
    #[arg(long = "feature")]
    pub features: Vec<String>,
    /// sha256 of the guest's Cargo.lock (hex).
    #[arg(long)]
    pub lock_sha256: Option<String>,
    /// A Cargo.lock to hash instead of --lock-sha256.
    #[arg(long)]
    pub lock_file: Option<std::path::PathBuf>,
}

impl BuildArgs {
    pub fn any(&self) -> bool {
        self.image_digest.is_some()
            || self.image_tag.is_some()
            || self.risc0_build.is_some()
            || self.host_rust.is_some()
            || self.manifest_path.is_some()
            || self.binary.is_some()
            || !self.features.is_empty()
            || self.lock_sha256.is_some()
            || self.lock_file.is_some()
    }

    /// `base` with these flags applied.
    pub fn apply(&self, mut b: BuildConfig) -> Result<BuildConfig> {
        if let Some(d) = &self.image_digest {
            b.image_digest = hex32(d, "--image-digest")?;
        }
        if let Some(v) = &self.image_tag {
            b.image_tag.clone_from(v);
        }
        if let Some(v) = &self.risc0_build {
            b.risc0_build.clone_from(v);
        }
        if let Some(v) = &self.host_rust {
            b.host_risc0_rust.clone_from(v);
        }
        if let Some(v) = &self.manifest_path {
            b.manifest_path.clone_from(v);
        }
        if let Some(v) = &self.binary {
            b.binary.clone_from(v);
        }
        if !self.features.is_empty() {
            b.features.clone_from(&self.features);
        }
        if let Some(v) = &self.lock_sha256 {
            b.lock_sha256 = hex32(v, "--lock-sha256")?;
        }
        if let Some(f) = &self.lock_file {
            b.lock_sha256 = t::pda::sha256(
                &std::fs::read(f).with_context(|| format!("cannot read {}", f.display()))?,
            );
        }
        Ok(b)
    }
}

pub fn default_build(name: &str) -> Result<BuildConfig> {
    Ok(BuildConfig {
        schema: t::SCHEMA,
        builder: t::BUILDER_RISC0_DOCKER,
        image_digest: hex32(DEFAULT_IMAGE_DIGEST, "default digest")?,
        image_tag: "r0.1.91.1".into(),
        risc0_build: "3.0.5".into(),
        host_risc0_rust: "1.91.1".into(),
        manifest_path: "methods/guest/Cargo.toml".into(),
        binary: name.replace('-', "_"),
        features: vec![],
        lock_sha256: [0; 32],
        docker_env: vec![],
    })
}

fn commit_bytes(s: &str) -> Result<Vec<u8>> {
    let v = hex::decode(s.strip_prefix("0x").unwrap_or(s)).context("--commit is not hex")?;
    if !matches!(v.len(), 20 | 32) {
        bail!("--commit must be a 40- or 64-character hex object id");
    }
    Ok(v)
}

impl EntryArgs {
    fn archive(&self) -> Result<Option<ContentRef>> {
        self.archive_cid
            .as_deref()
            .map(|c| {
                content(
                    c,
                    self.archive_sha256.as_deref(),
                    self.archive_size,
                    self.archive_file.as_deref(),
                    "archive",
                )
            })
            .transpose()
    }

    /// The fields of a new entry. `elf`: for deploy, whose image id is the claim.
    pub fn input(&self, elf: Option<&[u8]>) -> Result<EntryInput> {
        let _ = elf;
        // Every missing required flag at once.
        let mut missing = vec![];
        if self.name.is_none() {
            missing.push("--name");
        }
        if self.version.is_none() {
            missing.push("--version");
        }
        if self.idl_cid.is_none() {
            missing
                .push("--idl-cid (the IDL's Storage CID, or `push` with --idl-file to upload it)");
        }
        if self.repo.is_none() && self.archive_cid.is_none() {
            missing.push("the source: --repo and --commit, and/or --archive-cid");
        } else if self.repo.is_some() && self.commit.is_none() {
            missing.push("--commit (with --repo)");
        }
        if !missing.is_empty() {
            bail!("missing: {}", missing.join("; "));
        }
        let name = self.name.clone().unwrap_or_default();
        let version = self.version.clone().unwrap_or_default();
        let idl_cid = self.idl_cid.as_deref().unwrap_or_default();
        let idl = content(
            idl_cid,
            self.idl_sha256.as_deref(),
            self.idl_size,
            self.idl_file.as_deref(),
            "idl",
        )?;
        let archive = self.archive()?;
        let repo = self.repo.clone().unwrap_or_default();
        let commit = match &self.commit {
            Some(c) => commit_bytes(c)?,
            None if repo.is_empty() => vec![],
            None => bail!("--repo needs --commit"),
        };
        if repo.is_empty() && archive.is_none() {
            bail!("give the source: --repo and --commit, and/or --archive-cid");
        }
        let source = SourceRef {
            repo,
            commit,
            docker_root: self.docker_root.clone(),
            archive,
        };
        let build = self.build.apply(default_build(&name)?)?;
        Ok(EntryInput {
            name,
            version,
            author_label: self.label.clone().unwrap_or_default(),
            description: self.description.clone().unwrap_or_default(),
            tags: self.tags.clone(),
            idl,
            source,
            build,
            image_id: self
                .image_id
                .as_deref()
                .map(|s| hex32(s, "--image-id"))
                .transpose()?,
        })
    }
}

/// Which entry a change targets.
#[derive(clap::Args, Clone)]
pub struct Target {
    /// The program id (its deployer entry), or a name.
    #[arg(required_unless_present = "entry")]
    pub target: Option<String>,
    /// The third-party entry this publisher made for the program.
    #[arg(long)]
    pub publisher: Option<String>,
    /// The entry's address directly.
    #[arg(long)]
    pub entry: Option<String>,
}

#[derive(clap::Args, Clone, Default)]
pub struct PatchArgs {
    #[arg(long = "version", id = "patch_version")]
    pub version: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    /// Replace the tags (repeat for more).
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Remove every tag.
    #[arg(long)]
    pub clear_tags: bool,
    /// New IDL: its CID, or `push` with --idl-file to upload it (see `register --help`).
    #[arg(long)]
    pub idl_cid: Option<String>,
    #[arg(long)]
    pub idl_sha256: Option<String>,
    #[arg(long)]
    pub idl_size: Option<u32>,
    #[arg(long)]
    pub idl_file: Option<std::path::PathBuf>,
    #[arg(long)]
    pub repo: Option<String>,
    #[arg(long)]
    pub commit: Option<String>,
    #[arg(long)]
    pub docker_root: Option<String>,
    #[command(flatten)]
    pub build: BuildArgs,
    /// New claimed image id (hex), e.g. after upgrading the program.
    #[arg(long)]
    pub image_id: Option<String>,
}

impl PatchArgs {
    /// The patch, with source/build changes applied on top of the `stored` entry's.
    pub fn input(&self, stored: &Entry) -> Result<PatchInput> {
        let idl = self
            .idl_cid
            .as_deref()
            .map(|c| {
                content(
                    c,
                    self.idl_sha256.as_deref(),
                    self.idl_size,
                    self.idl_file.as_deref(),
                    "idl",
                )
            })
            .transpose()?;
        let source = if self.repo.is_some() || self.commit.is_some() || self.docker_root.is_some() {
            let mut s = stored.source.clone();
            if let Some(r) = &self.repo {
                s.repo.clone_from(r);
            }
            if let Some(c) = &self.commit {
                s.commit = commit_bytes(c)?;
            }
            if let Some(d) = &self.docker_root {
                s.docker_root.clone_from(d);
            }
            Some(s)
        } else {
            None
        };
        let build = if self.build.any() {
            Some(self.build.apply(stored.build.clone())?)
        } else {
            None
        };
        let tags = if self.clear_tags {
            Some(vec![])
        } else if self.tags.is_empty() {
            None
        } else {
            Some(self.tags.clone())
        };
        let p = PatchInput {
            version: self.version.clone(),
            description: self.description.clone(),
            tags,
            author_label: self.label.clone(),
            idl,
            source,
            build,
            image_id: self
                .image_id
                .as_deref()
                .map(|s| hex32(s, "--image-id"))
                .transpose()?,
        };
        if p.version.is_none()
            && p.description.is_none()
            && p.tags.is_none()
            && p.author_label.is_none()
            && p.idl.is_none()
            && p.source.is_none()
            && p.build.is_none()
            && p.image_id.is_none()
        {
            bail!(
                "nothing to update: give at least one field (--version, --description, --idl-cid, --image-id, ...)"
            );
        }
        Ok(p)
    }
}
