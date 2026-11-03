//! `lezreg.toml` and `lezreg init` (DESIGN §7.2, milestone M8).
//!
//! `lezreg.toml` holds what does not change from commit to commit: the entry's metadata, the
//! IDL's path, the repository and the build tuple, and, once `publish` has deployed it, the
//! program id. The commit and the `Cargo.lock` hash are read when publishing, so the file
//! survives commits unchanged and a release always records the tree it was built from.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::args::DEFAULT_IMAGE_DIGEST;

pub const FILE: &str = "lezreg.toml";

/// A risc0-build version and the canonical builder that goes with it (DESIGN §4.1). The digest
/// is the one LEZ's own CI pins; the host Rust is the rzup toolchain that version ships with.
pub struct Builder {
    pub risc0_build: &'static str,
    pub image_tag: &'static str,
    pub image_digest: &'static str,
    pub host_rust: &'static str,
}

pub const BUILDERS: &[Builder] = &[Builder {
    risc0_build: "3.0.5",
    image_tag: "r0.1.91.1",
    image_digest: DEFAULT_IMAGE_DIGEST,
    host_rust: "1.91.1",
}];

pub fn builder_for(risc0_build: &str) -> Option<&'static Builder> {
    BUILDERS.iter().find(|b| b.risc0_build == risc0_build)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub program: ProgramSection,
    #[serde(default)]
    pub source: SourceSection,
    pub build: BuildSection,
    #[serde(default)]
    pub deploy: DeploySection,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct ProgramSection {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// The IDL file, relative to this file's directory.
    #[serde(default)]
    pub idl: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceSection {
    /// https URL of the public repository (`verify` clones it).
    #[serde(default)]
    pub repo: String,
    /// The build context, relative to the repository root.
    #[serde(default = "dot")]
    pub docker_root: String,
}

impl Default for SourceSection {
    fn default() -> Self {
        Self {
            repo: String::new(),
            docker_root: ".".into(),
        }
    }
}

fn dot() -> String {
    ".".into()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct BuildSection {
    pub risc0_build: String,
    pub image_tag: String,
    /// sha256 of `risczero/risc0-guest-builder` (hex).
    pub image_digest: String,
    pub host_rust: String,
    /// The guest's Cargo.toml, relative to the docker root.
    pub manifest_path: String,
    pub binary: String,
    #[serde(default)]
    pub features: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct DeploySection {
    /// The program id, written by `publish` after the first deploy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// The proven author (co-signs, the only account that can update). Default: the
    /// program's header key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default)]
    pub immutable: bool,
}

/// A loaded `lezreg.toml` with where it lives.
pub struct Project {
    pub path: PathBuf,
    pub dir: PathBuf,
    pub m: Manifest,
}

impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "cannot read {} (run `lezreg init --detect` to make one)",
                path.display()
            )
        })?;
        let m: Manifest =
            toml::from_str(&text).with_context(|| format!("{} is not valid", path.display()))?;
        let dir = path
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            })
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self {
            path: path.to_path_buf(),
            dir,
            m,
        })
    }

    /// The IDL file, or a hint on making one.
    pub fn idl_path(&self) -> Result<PathBuf> {
        if self.m.program.idl.is_empty() {
            bail!(
                "{} names no IDL: set [program] idl to your program's IDL JSON (SPEL: `spel generate-idl`; see `lezreg idl push --help`)",
                self.path.display()
            );
        }
        Ok(self.dir.join(&self.m.program.idl))
    }

    /// The git repository root.
    pub fn git_top(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(git(
            &self.dir,
            &["rev-parse", "--show-toplevel"],
        )?))
    }

    /// The docker root on disk.
    pub fn docker_root_dir(&self) -> Result<PathBuf> {
        Ok(self.git_top()?.join(&self.m.source.docker_root))
    }

    /// The program id `publish` deployed or adopted, if any.
    pub fn program(&self) -> Option<&str> {
        self.m.deploy.program.as_deref().filter(|p| !p.is_empty())
    }

    /// Records the program id in the file, keeping its comments and layout.
    pub fn save_program(&mut self, program: &str) -> Result<()> {
        let text = std::fs::read_to_string(&self.path)?;
        let mut doc: toml_edit::DocumentMut = text
            .parse()
            .with_context(|| format!("{} is not valid TOML", self.path.display()))?;
        if !doc.contains_key("deploy") {
            doc["deploy"] = toml_edit::table();
        }
        doc["deploy"]["program"] = toml_edit::value(program);
        std::fs::write(&self.path, doc.to_string())
            .with_context(|| format!("cannot write {}", self.path.display()))?;
        self.m.deploy.program = Some(program.to_owned());
        Ok(())
    }
}

/// Runs git in `dir` and returns its trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("cannot run git")?;
    if !out.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Tracked files with changes, other than `lezreg.toml` itself (it does not affect the build,
/// and `publish` writes the program id into it).
pub fn dirty_files(dir: &Path) -> Result<Vec<String>> {
    Ok(
        git(dir, &["status", "--porcelain", "--untracked-files=no"])?
            .lines()
            .map(|l| l.get(3..).unwrap_or(l).to_owned())
            .filter(|f| !f.ends_with(FILE))
            .collect(),
    )
}

/// Refuses a tree with uncommitted changes: a recorded commit must be what was built.
pub fn check_clean(dir: &Path, allow_dirty: bool) -> Result<()> {
    let dirty = dirty_files(dir)?;
    if !dirty.is_empty() {
        let list = dirty.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        if allow_dirty {
            eprintln!(
                "warning: uncommitted changes ({list}): the commit recorded is not what is in this tree"
            );
        } else {
            bail!(
                "uncommitted changes ({list}{}): commit them first, since the entry records the commit and `verify` rebuilds exactly that (--allow-dirty goes ahead anyway)",
                if dirty.len() > 5 { ", …" } else { "" }
            );
        }
    }
    Ok(())
}

/// An https URL to record for a git remote: `git@host:o/r.git` and `ssh://git@host/o/r` become
/// `https://host/o/r`; any user name or token in the URL is dropped, never put on chain.
pub fn https_repo(remote: &str) -> Result<String> {
    let r = remote.trim();
    let (host, path) = if let Some(rest) = r
        .strip_prefix("https://")
        .or_else(|| r.strip_prefix("ssh://"))
        .or_else(|| r.strip_prefix("git+ssh://"))
    {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
        let (host, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("'{remote}' has no path"))?;
        // ssh://host:22/o/r: drop the port.
        let host = if r.starts_with("https://") {
            host
        } else {
            host.split(':').next().unwrap_or(host)
        };
        (host.to_owned(), path.to_owned())
    } else if let Some((user_host, path)) = r.split_once(':')
        && !user_host.contains('/')
        && user_host.contains('@')
    {
        let host = user_host.rsplit_once('@').map_or(user_host, |(_, h)| h);
        (host.to_owned(), path.to_owned())
    } else {
        bail!("cannot turn the git remote '{remote}' into an https URL: pass --repo https://…");
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() || host.contains(char::is_whitespace) {
        bail!("cannot turn the git remote '{remote}' into an https URL: pass --repo https://…");
    }
    Ok(format!("https://{host}/{path}"))
}

fn read_toml(p: &Path) -> Option<toml::Table> {
    toml::from_str(&std::fs::read_to_string(p).ok()?).ok()
}

/// A `[package]` string field, following `field.workspace = true` up to the workspace root.
fn package_field(manifest: &Path, field: &str) -> Option<String> {
    let t = read_toml(manifest)?;
    let v = t.get("package")?.get(field)?;
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    if v.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        let mut dir = manifest.parent()?.to_path_buf();
        while dir.pop() {
            let root = dir.join("Cargo.toml");
            if let Some(rt) = read_toml(&root)
                && let Some(ws) = rt.get("workspace")
            {
                return ws.get("package")?.get(field)?.as_str().map(str::to_owned);
            }
        }
    }
    None
}

/// `Cargo.toml` files of risc0 guests under `root`: the directories a methods crate lists in
/// `[package.metadata.risc0] methods`, plus any `guest/Cargo.toml` with a package.
pub fn find_guests(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if depth < 5
                    && !name.starts_with('.')
                    && !matches!(name.as_str(), "target" | "node_modules" | "_deps")
                {
                    stack.push((p, depth + 1));
                }
            } else if name == "Cargo.toml"
                && let Some(t) = read_toml(&p)
            {
                if let Some(methods) = t
                    .get("package")
                    .and_then(|x| x.get("metadata"))
                    .and_then(|x| x.get("risc0"))
                    .and_then(|x| x.get("methods"))
                    .and_then(toml::Value::as_array)
                {
                    for m in methods.iter().filter_map(toml::Value::as_str) {
                        let g = dir.join(m).join("Cargo.toml");
                        if g.is_file() {
                            found.push(g);
                        }
                    }
                } else if dir.file_name().is_some_and(|n| n == "guest")
                    && t.get("package").is_some()
                {
                    found.push(p.clone());
                }
            }
        }
    }
    let mut found: Vec<PathBuf> = found
        .into_iter()
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The guest's bin targets: `[[bin]]` names, else `src/bin/*.rs`, else the package name
/// (with `src/main.rs`).
pub fn guest_binaries(manifest: &Path) -> Vec<String> {
    let Some(t) = read_toml(manifest) else {
        return vec![];
    };
    let dir = manifest.parent().unwrap_or(Path::new("."));
    let mut bins: Vec<String> = t
        .get("bin")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("name").and_then(toml::Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if bins.is_empty()
        && let Ok(rd) = std::fs::read_dir(dir.join("src/bin"))
    {
        bins = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                n.strip_suffix(".rs").map(str::to_owned)
            })
            .collect();
        bins.sort();
    }
    if bins.is_empty()
        && dir.join("src/main.rs").is_file()
        && let Some(n) = t
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
    {
        bins.push(n.to_owned());
    }
    bins
}

/// The version of `package` in a Cargo.lock.
pub fn locked_version(lock: &Path, package: &str) -> Option<String> {
    let t = read_toml(lock)?;
    t.get("package")?
        .as_array()?
        .iter()
        .find(|p| p.get("name").and_then(toml::Value::as_str) == Some(package))?
        .get("version")?
        .as_str()
        .map(str::to_owned)
}

/// An entry name from a crate name: lower case, `[a-z0-9._-]`, at most 32 bytes.
pub fn entry_name(s: &str) -> String {
    let n: String = s
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let n = n.trim_start_matches(['.', '_', '-']);
    n.chars().take(32).collect()
}

/// TOML-quotes a string.
fn q(s: &str) -> String {
    toml::Value::String(s.to_owned()).to_string()
}

fn q_list(v: &[String]) -> String {
    format!(
        "[{}]",
        v.iter().map(|s| q(s)).collect::<Vec<_>>().join(", ")
    )
}

/// The file `init` writes: the manifest, with comments.
pub fn render(m: &Manifest) -> String {
    let p = &m.program;
    let b = &m.build;
    let mut s = format!(
        "# lezreg.toml: what `lezreg publish` registers (LEZ Program Registry).\n\
         # Written by `lezreg init`. Edit freely; bump `version` and rerun `lezreg publish` to\n\
         # record a new release. The commit and the Cargo.lock hash are read at publish time.\n\
         \n\
         [program]\n\
         name = {}            # immutable once registered: [a-z0-9][a-z0-9._-]*, at most 32 bytes\n\
         version = {}\n\
         label = {}           # free-text author name, shown as UNPROVEN\n\
         description = {}\n\
         tags = {}            # at most 8, [a-z0-9-]\n\
         idl = {}             # the program's IDL JSON, relative to this file\n\
         \n\
         [source]\n\
         repo = {}\n\
         docker_root = {}     # build context, relative to the repository root\n\
         \n\
         # The canonical build (`lezreg build`, `lezreg verify`). risc0-build {} builds in\n\
         # risczero/risc0-guest-builder:{} (pinned by digest).\n\
         [build]\n\
         risc0_build = {}\n\
         image_tag = {}\n\
         image_digest = {}\n\
         host_rust = {}\n\
         manifest_path = {}   # the guest's Cargo.toml, relative to docker_root\n\
         binary = {}\n\
         features = {}\n\
         \n\
         [deploy]\n",
        q(&p.name),
        q(&p.version),
        q(&p.label),
        q(&p.description),
        q_list(&p.tags),
        q(&p.idl),
        q(&m.source.repo),
        q(&m.source.docker_root),
        b.risc0_build,
        b.image_tag,
        q(&b.risc0_build),
        q(&b.image_tag),
        q(&b.image_digest),
        q(&b.host_rust),
        q(&b.manifest_path),
        q(&b.binary),
        q_list(&b.features),
    );
    match &m.deploy.author {
        Some(a) => s.push_str(&format!(
            "author = {}          # the proven author; keep the header key cold\n",
            q(a)
        )),
        None => s.push_str(
            "# author = \"<account>\"   # the proven author (default: the program's header key)\n",
        ),
    }
    s.push_str(&format!("immutable = {}\n", m.deploy.immutable));
    if let Some(prog) = &m.deploy.program {
        s.push_str(&format!("program = {}\n", q(prog)));
    } else {
        s.push_str("# program = \"<id>\"     # set by `lezreg publish` after the first deploy\n");
    }
    s
}

/// What `init` can be told instead of detecting.
#[derive(clap::Args, Clone, Default)]
pub struct InitOpts {
    /// Detect everything it can from Cargo.toml, Cargo.lock and git (otherwise only the
    /// flags given and the defaults are used).
    #[arg(long)]
    pub detect: bool,
    /// Project directory (where lezreg.toml goes).
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    /// Entry name. Default: the guest package name.
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long = "version", id = "init_version")]
    pub version: Option<String>,
    #[arg(long)]
    pub label: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// The IDL JSON. Default: the one `*.idl.json` (or only JSON) under idl/.
    #[arg(long)]
    pub idl: Option<String>,
    /// Repository URL. Default: git remote origin, as https.
    #[arg(long)]
    pub repo: Option<String>,
    /// The guest's Cargo.toml, relative to the project directory.
    #[arg(long)]
    pub manifest_path: Option<String>,
    #[arg(long)]
    pub binary: Option<String>,
    #[arg(long = "feature")]
    pub features: Vec<String>,
    /// The proven author account for [deploy] author.
    #[arg(long)]
    pub author: Option<String>,
    /// Overwrite an existing lezreg.toml.
    #[arg(long)]
    pub force: bool,
    /// Go ahead although tracked files have uncommitted changes.
    #[arg(long)]
    pub allow_dirty: bool,
    /// Also write .github/workflows/lezreg.yml (the Action in `verify` mode on pull requests,
    /// `prepare` on tags).
    #[arg(long)]
    pub ci: bool,
}

/// The `[program] idl` default: the only `*.idl.json` under idl/, else the only JSON there.
fn detect_idl(dir: &Path) -> Option<String> {
    let rd = std::fs::read_dir(dir.join("idl")).ok()?;
    let mut json: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    json.sort();
    let idl: Vec<String> = json
        .iter()
        .filter(|n| n.ends_with(".idl.json"))
        .cloned()
        .collect();
    match (idl.as_slice(), json.as_slice()) {
        ([one], _) | ([], [one]) => Some(format!("idl/{one}")),
        _ => None,
    }
}

fn rel(path: &Path, base: &Path) -> Result<String> {
    let p = path
        .strip_prefix(base)
        .map_err(|_| anyhow!("{} is not under {}", path.display(), base.display()))?;
    let s = p.to_string_lossy().replace('\\', "/");
    Ok(if s.is_empty() { ".".into() } else { s })
}

/// Builds the manifest `init` writes (no file I/O beyond reading the project).
pub fn detect(o: &InitOpts) -> Result<(Manifest, Vec<String>)> {
    let mut notes = Vec::new();
    let dir = o
        .dir
        .canonicalize()
        .with_context(|| format!("no directory {}", o.dir.display()))?;
    let top = PathBuf::from(git(&dir, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        anyhow!(
            "{} is not in a git repository: the registry records the source commit, so commit your program to git first",
            dir.display()
        )
    })?)
    .canonicalize()?;
    check_clean(&dir, o.allow_dirty)?;
    let docker_root = rel(&dir, &top)?;

    // The guest.
    let manifest = match &o.manifest_path {
        Some(m) => dir.join(m),
        None if o.detect => {
            let guests = find_guests(&dir);
            let conventional = dir.join("methods/guest/Cargo.toml").canonicalize().ok();
            match guests.as_slice() {
                [one] => one.clone(),
                [] => bail!(
                    "no risc0 guest found under {} (a methods crate with [package.metadata.risc0] methods, or a guest/Cargo.toml): pass --manifest-path",
                    dir.display()
                ),
                many => match conventional.filter(|c| many.contains(c)) {
                    Some(c) => {
                        notes.push(format!(
                            "{} guests found; using methods/guest/Cargo.toml (pass --manifest-path for another)",
                            many.len()
                        ));
                        c
                    }
                    None => bail!(
                        "several risc0 guests found ({}): pass --manifest-path",
                        many.iter()
                            .map(|g| rel(g, &dir).unwrap_or_default())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                },
            }
        }
        None => dir.join("methods/guest/Cargo.toml"),
    };
    if !manifest.is_file() {
        bail!(
            "{} does not exist: pass --manifest-path (or --detect)",
            manifest.display()
        );
    }
    let manifest = manifest.canonicalize()?;
    let manifest_path = rel(&manifest, &dir)?;
    let binary = match &o.binary {
        Some(b) => b.clone(),
        None => match guest_binaries(&manifest).as_slice() {
            [one] => one.clone(),
            [] => bail!("{} has no bin target: pass --binary", manifest_path),
            many => bail!(
                "{} has several bin targets ({}): pass --binary",
                manifest_path,
                many.join(", ")
            ),
        },
    };
    let name = match &o.name {
        Some(n) => n.clone(),
        None => entry_name(&package_field(&manifest, "name").unwrap_or_else(|| binary.clone())),
    };
    lezreg_types::validate::name(&name).map_err(|e| anyhow!("name '{name}': {e} (pass --name)"))?;
    let root_manifest = dir.join("Cargo.toml");
    let version = o.version.clone().unwrap_or_else(|| {
        package_field(&root_manifest, "version")
            .or_else(|| package_field(&manifest, "version"))
            .unwrap_or_else(|| "0.1.0".into())
    });
    let description = o.description.clone().unwrap_or_else(|| {
        package_field(&root_manifest, "description")
            .or_else(|| package_field(&manifest, "description"))
            .or_else(|| {
                read_toml(&root_manifest)?
                    .get("workspace")?
                    .get("package")?
                    .get("description")?
                    .as_str()
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    });

    // The builder, from the risc0-build the project locks.
    let lock = lezreg_sdk::verify::guest_lock(&dir, &manifest_path);
    let mut locks: Vec<PathBuf> = lock.iter().cloned().collect();
    locks.push(dir.join("Cargo.lock"));
    let risc0 = locks.iter().find_map(|l| locked_version(l, "risc0-build"));
    let builder = match &risc0 {
        Some(v) => builder_for(v).ok_or_else(|| {
            anyhow!(
                "risc0-build {v} has no known canonical builder in this lezreg (known: {}); write [build] by hand",
                BUILDERS
                    .iter()
                    .map(|b| b.risc0_build)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?,
        None => {
            notes.push(format!(
                "no risc0-build in Cargo.lock; assuming {} (LEZ 0.4's)",
                BUILDERS[0].risc0_build
            ));
            &BUILDERS[0]
        }
    };
    if lock.is_none() {
        notes.push(format!(
            "no Cargo.lock above {manifest_path}: commit one, or `verify` cannot reproduce the build"
        ));
    }

    let repo = match &o.repo {
        Some(r) => https_repo(r)?,
        None => match git(&dir, &["remote", "get-url", "origin"]) {
            Ok(r) => https_repo(&r)?,
            Err(_) => {
                notes.push(
                    "no git remote 'origin': set [source] repo to the public https URL before publishing".into(),
                );
                String::new()
            }
        },
    };
    let idl = o
        .idl
        .clone()
        .or_else(|| detect_idl(&dir))
        .unwrap_or_else(|| {
            notes.push(
                "no IDL found under idl/: set [program] idl to your IDL JSON before publishing"
                    .into(),
            );
            String::new()
        });
    if !idl.is_empty() && !dir.join(&idl).is_file() {
        bail!("the IDL {idl} does not exist");
    }
    let m = Manifest {
        program: ProgramSection {
            name,
            version,
            label: o.label.clone().unwrap_or_default(),
            description,
            tags: o.tags.clone(),
            idl,
        },
        source: SourceSection { repo, docker_root },
        build: BuildSection {
            risc0_build: builder.risc0_build.into(),
            image_tag: builder.image_tag.into(),
            image_digest: builder.image_digest.into(),
            host_rust: builder.host_rust.into(),
            manifest_path,
            binary,
            features: o.features.clone(),
        },
        deploy: DeploySection {
            program: None,
            author: o.author.clone(),
            immutable: false,
        },
    };
    lezreg_types::validate::version(&m.program.version)
        .map_err(|e| anyhow!("version '{}': {e}", m.program.version))?;
    lezreg_types::validate::description(&m.program.description).map_err(|e| anyhow!("{e}"))?;
    lezreg_types::validate::author_label(&m.program.label).map_err(|e| anyhow!("{e}"))?;
    lezreg_types::validate::tags(&m.program.tags).map_err(|e| anyhow!("{e}"))?;
    if let Some(a) = &m.deploy.author {
        crate::args::parse_account(a)?;
    }
    Ok((m, notes))
}

/// The workflow `init --ci` writes.
pub fn ci_workflow() -> &'static str {
    "# LEZ Program Registry: rebuild the program canonically on every PR (verify), and on a\n\
     # tag attach the release payload (prepare). Then, on your machine:\n\
     #   lezreg publish --from-release https://github.com/<you>/<repo>/releases/tag/<tag>\n\
     # Header and author keys never enter CI.\n\
     name: lezreg\n\
     on:\n\
     \x20 pull_request:\n\
     \x20 push:\n\
     \x20   tags: [\"v*\"]\n\
     permissions:\n\
     \x20 contents: write\n\
     jobs:\n\
     \x20 registry:\n\
     \x20   runs-on: ubuntu-24.04\n\
     \x20   steps:\n\
     \x20     - uses: actions/checkout@v4\n\
     \x20     - uses: jefdiesel/lez-program-registry/action@main\n\
     \x20       with:\n\
     \x20         mode: ${{ startsWith(github.ref, 'refs/tags/') && 'prepare' || 'verify' }}\n\
     \x20         # rpc: https://<LEZ testnet 0.4 RPC>   # with program-id: compare with the live program\n\
     \x20         # program-id: ${{ vars.PROGRAM_ID }}\n"
}

/// `lezreg init`.
pub fn init(o: &InitOpts) -> Result<(PathBuf, Manifest, Vec<String>)> {
    let path = o.dir.join(FILE);
    if path.exists() && !o.force {
        bail!("{} exists already (--force overwrites it)", path.display());
    }
    let (m, mut notes) = detect(o)?;
    let text = render(&m);
    // What we write must read back as what we meant.
    let back: Manifest = toml::from_str(&text).context("internal: lezreg.toml does not parse")?;
    if back != m {
        bail!("internal: lezreg.toml does not round-trip");
    }
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))?;
    if o.ci {
        let wf = o.dir.join(".github/workflows/lezreg.yml");
        if wf.exists() && !o.force {
            notes.push(format!("{} exists; left alone", wf.display()));
        } else {
            std::fs::create_dir_all(wf.parent().expect("has a parent"))?;
            std::fs::write(&wf, ci_workflow())?;
            notes.push(format!("wrote {}", wf.display()));
        }
    }
    Ok((path, m, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_become_https_without_credentials() {
        for (from, to) in [
            ("git@github.com:acme/tok.git", "https://github.com/acme/tok"),
            (
                "https://github.com/acme/tok.git",
                "https://github.com/acme/tok",
            ),
            (
                "https://github.com/acme/tok/",
                "https://github.com/acme/tok",
            ),
            (
                "https://x-access-token:ghs_SECRET@github.com/acme/tok.git",
                "https://github.com/acme/tok",
            ),
            (
                "ssh://git@gitlab.com:22/a/b/c.git",
                "https://gitlab.com/a/b/c",
            ),
            (
                "ssh://git@github.com/acme/tok",
                "https://github.com/acme/tok",
            ),
        ] {
            assert_eq!(https_repo(from).unwrap(), to, "{from}");
        }
        for bad in [
            "/tmp/repo",
            "file:///tmp/repo",
            "../sibling",
            "https://host",
        ] {
            assert!(https_repo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn entry_names_follow_the_charset() {
        assert_eq!(entry_name("My_Token"), "my_token");
        assert_eq!(entry_name("amm program"), "amm-program");
        assert_eq!(entry_name("-x"), "x");
        assert_eq!(entry_name(&"a".repeat(40)).len(), 32);
        assert!(lezreg_types::validate::name(&entry_name("Weird*Name!")).is_ok());
    }

    #[test]
    fn rendered_manifest_round_trips_and_keeps_comments_on_save() {
        let m = Manifest {
            program: ProgramSection {
                name: "my-token".into(),
                version: "1.0.0".into(),
                label: "Acme \"Labs\"".into(),
                description: "a token\nwith a newline".into(),
                tags: vec!["token".into(), "defi".into()],
                idl: "idl/my_token.idl.json".into(),
            },
            source: SourceSection {
                repo: "https://github.com/acme/tok".into(),
                docker_root: ".".into(),
            },
            build: BuildSection {
                risc0_build: "3.0.5".into(),
                image_tag: "r0.1.91.1".into(),
                image_digest: DEFAULT_IMAGE_DIGEST.into(),
                host_rust: "1.91.1".into(),
                manifest_path: "methods/guest/Cargo.toml".into(),
                binary: "my_token".into(),
                features: vec![],
            },
            deploy: DeploySection::default(),
        };
        let text = render(&m);
        assert_eq!(toml::from_str::<Manifest>(&text).unwrap(), m);
        let d = std::env::temp_dir().join(format!("lezreg-proj-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(FILE);
        std::fs::write(&p, &text).unwrap();
        let mut proj = Project::load(&p).unwrap();
        assert_eq!(proj.program(), None);
        proj.save_program("Abc123").unwrap();
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("# lezreg.toml: what"), "comments kept");
        assert!(after.contains("program = \"Abc123\""));
        assert_eq!(Project::load(&p).unwrap().program(), Some("Abc123"));
        // Unknown keys are refused (a typo would silently do nothing otherwise).
        std::fs::write(&p, text.replace("[source]", "[source]\nrepoo = \"x\"")).unwrap();
        assert!(Project::load(&p).is_err());
        std::fs::remove_dir_all(&d).ok();
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A tiny risc0 project: workspace, methods crate, guest, lock, IDL, git.
    fn sample(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lezreg-init-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("methods/guest/src")).unwrap();
        std::fs::create_dir_all(d.join("idl")).unwrap();
        std::fs::write(
            d.join("Cargo.toml"),
            "[workspace]\nmembers=[\"methods\",\"methods/guest\"]\n[workspace.package]\nversion = \"2.3.4\"\ndescription = \"A sample\"\n",
        )
        .unwrap();
        std::fs::write(
            d.join("methods/Cargo.toml"),
            "[package]\nname=\"m\"\nversion=\"0.0.1\"\n[package.metadata.risc0]\nmethods=[\"guest\"]\n",
        )
        .unwrap();
        std::fs::write(
            d.join("methods/guest/Cargo.toml"),
            "[package]\nname=\"Sample_Guest\"\nversion.workspace = true\n",
        )
        .unwrap();
        std::fs::write(d.join("methods/guest/src/main.rs"), "fn main(){}\n").unwrap();
        std::fs::write(
            d.join("Cargo.lock"),
            "version = 4\n[[package]]\nname = \"risc0-build\"\nversion = \"3.0.5\"\n",
        )
        .unwrap();
        std::fs::write(d.join("idl/sample.idl.json"), "{\"name\":\"s\"}").unwrap();
        sh(&d, &["init", "-q"]);
        sh(
            &d,
            &["remote", "add", "origin", "git@github.com:acme/sample.git"],
        );
        sh(&d, &["add", "."]);
        sh(&d, &["commit", "-qm", "init"]);
        d
    }

    #[test]
    fn init_detects_the_project() {
        let d = sample("detect");
        let o = InitOpts {
            detect: true,
            dir: d.clone(),
            ..Default::default()
        };
        let (path, m, _) = init(&o).unwrap();
        assert_eq!(m.program.name, "sample_guest");
        assert_eq!(m.program.version, "2.3.4", "workspace-inherited version");
        assert_eq!(m.program.description, "A sample");
        assert_eq!(m.program.idl, "idl/sample.idl.json");
        assert_eq!(m.source.repo, "https://github.com/acme/sample");
        assert_eq!(m.source.docker_root, ".");
        assert_eq!(m.build.manifest_path, "methods/guest/Cargo.toml");
        assert_eq!(m.build.binary, "Sample_Guest");
        assert_eq!(m.build.risc0_build, "3.0.5");
        assert_eq!(m.build.image_digest, DEFAULT_IMAGE_DIGEST);
        assert_eq!(Project::load(&path).unwrap().m, m);
        // Not twice without --force.
        assert!(init(&o).is_err());
        // lezreg.toml itself never makes the tree dirty.
        assert!(dirty_files(&d).unwrap().is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn init_refuses_a_dirty_tree_and_unknown_builders() {
        let d = sample("dirty");
        std::fs::write(d.join("methods/guest/src/main.rs"), "fn main(){ }\n").unwrap();
        let mut o = InitOpts {
            detect: true,
            dir: d.clone(),
            ..Default::default()
        };
        let e = init(&o).unwrap_err().to_string();
        assert!(e.contains("uncommitted"), "{e}");
        o.allow_dirty = true;
        init(&o).unwrap();
        // An unknown risc0-build is refused rather than guessed.
        std::fs::write(
            d.join("Cargo.lock"),
            "version = 4\n[[package]]\nname = \"risc0-build\"\nversion = \"9.9.9\"\n",
        )
        .unwrap();
        o.force = true;
        let e = init(&o).unwrap_err().to_string();
        assert!(e.contains("9.9.9"), "{e}");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn several_guests_need_a_choice() {
        let d = sample("many");
        std::fs::create_dir_all(d.join("other/guest/src")).unwrap();
        std::fs::write(d.join("other/guest/Cargo.toml"), "[package]\nname=\"o\"\n").unwrap();
        std::fs::write(d.join("other/guest/src/main.rs"), "").unwrap();
        // methods/guest is the conventional one and wins, with a note.
        let (m, notes) = detect(&InitOpts {
            detect: true,
            dir: d.clone(),
            allow_dirty: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(m.build.manifest_path, "methods/guest/Cargo.toml");
        assert!(notes.iter().any(|n| n.contains("2 guests")), "{notes:?}");
        std::fs::remove_dir_all(&d).ok();
    }
}
