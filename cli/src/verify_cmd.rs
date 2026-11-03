//! `lezreg build` and `lezreg verify` (DESIGN §4.2, milestone M4).
//!
//! Both need Docker with buildx. The builder image is amd64; on Apple silicon it runs emulated
//! and needs about 8 GB of Docker memory, which is why authors are pointed at the GitHub Action.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use lezreg_sdk::{
    HttpRpc, Registry,
    tx::tid,
    verify::{self, Verdict},
};
use lezreg_types::{BuildConfig, SourceRef};
use serde_json::json;

use crate::{args::parse_account, out::Out};

fn workdir(given: Option<&Path>, tag: &str) -> Result<PathBuf> {
    let d = match given {
        Some(d) => d.to_path_buf(),
        None => std::env::temp_dir().join(format!("lezreg-{tag}-{}", std::process::id())),
    };
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

/// The tuple to rebuild: the entry's current revision, or a recorded release.
struct Tuple {
    name: String,
    revision: u32,
    source: SourceRef,
    build: BuildConfig,
    claimed: [u8; 32],
}

/// `lezreg verify <program> [--revision N]`: rebuild the recorded source and compare.
#[allow(clippy::too_many_arguments)]
pub fn verify(
    out: &mut Out,
    r: &Registry<HttpRpc>,
    network: &str,
    target: &str,
    revision: Option<u32>,
    work: Option<&Path>,
    allow_network: bool,
    mark: bool,
) -> Result<()> {
    let program = parse_account(target).map_err(|_| anyhow!("verify takes a program id"))?;
    let resolved = r.resolve(&tid(&program))?;
    let entry = resolved
        .deployer
        .clone()
        .ok_or_else(|| anyhow!("{target} has no deployer entry to verify"))?;
    let live = resolved.header.as_ref().map(|h| h.image_id);
    let t = match revision {
        None => Tuple {
            name: entry.name.clone(),
            revision: entry.revision,
            source: entry.source.clone(),
            build: entry.build.clone(),
            claimed: entry.image_id,
        },
        Some(n) => {
            let rel = r
                .release(&tid(&program), n)?
                .ok_or_else(|| anyhow!("{target} has no release #{n}"))?;
            Tuple {
                name: entry.name.clone(),
                revision: n,
                source: rel.source,
                build: rel.build,
                claimed: rel.image_id,
            }
        }
    };
    let work = workdir(work, "verify")?;
    eprintln!(
        "verifying {} revision {}: {} @ {}",
        t.name,
        t.revision,
        t.source.repo,
        hex::encode(&t.source.commit)
    );
    if t.source.repo.is_empty() {
        bail!(
            "this entry has only a Storage archive; archive sources come with the source pinner (DESIGN §4.4)"
        );
    }
    let src = verify::source_from_git(
        &t.source.repo,
        &t.source.commit,
        &t.source.docker_root,
        &work,
    )?;
    if let Some(a) = &t.source.archive
        && a.sha256 != src.archive_sha256
    {
        bail!(
            "the git archive ({}) differs from the archive the entry records ({}): the repo moved",
            hex::encode(src.archive_sha256),
            hex::encode(a.sha256)
        );
    }
    let lock = verify::guest_lock(&src.dir, &t.build.manifest_path)
        .ok_or_else(|| anyhow!("no Cargo.lock above {}", t.build.manifest_path))?;
    let lock_sha = lezreg_types::pda::sha256(&std::fs::read(&lock)?);
    if t.build.lock_sha256 != [0; 32] && lock_sha != t.build.lock_sha256 {
        bail!(
            "Cargo.lock at that commit hashes to {}, the entry records {}",
            hex::encode(lock_sha),
            hex::encode(t.build.lock_sha256)
        );
    }
    eprintln!("building in Docker (network off for the compile)…");
    let (elf, network_denied) = match verify::docker_build(&src.dir, &t.build, true, &work) {
        Ok(elf) => (elf, true),
        Err(e) if allow_network => {
            eprintln!("offline build failed ({e}); retrying with the network on (--allow-network)");
            (
                verify::docker_build(&src.dir, &t.build, false, &work)?,
                false,
            )
        }
        Err(e) => {
            let v = Verdict::NotReproducible(e.to_string());
            out.json(json!({"verdict": v.label(), "error": e.to_string()}));
            bail!("NotReproducible: {e}");
        }
    };
    let rebuilt = verify::image_id_of_elf(&elf)?;
    let v = Verdict::of(rebuilt, t.claimed, live);
    let h = |x: &[u8; 32]| hex::encode(x);
    out.text(&format!(
        "{}{}\n  rebuilt  {}\n  claimed  {}  (revision {})\n  live     {}\n  source   {} @ {}  (git archive sha256 {})\n  elf      sha256 {}",
        v.label(),
        if network_denied { "" } else { " (built WITH network: weaker)" },
        h(&rebuilt),
        h(&t.claimed),
        t.revision,
        live.as_ref().map_or_else(|| "(nothing deployed)".into(), h),
        t.source.repo,
        hex::encode(&t.source.commit),
        h(&src.archive_sha256),
        h(&lezreg_types::pda::sha256(&elf))
    ));
    out.json(json!({"verdict": v.label(), "rebuilt": h(&rebuilt), "claimed": h(&t.claimed), "live": live.map(|l| h(&l)), "revision": t.revision, "network_denied": network_denied, "archive_sha256": h(&src.archive_sha256)}));
    if mark && v == Verdict::Match {
        let key = mark_key(network, target, &rebuilt, &t.build, &src.archive_sha256);
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let p = write_mark(
            key,
            json!({"program": target, "name": t.name, "revision": t.revision, "verdict": v.label(), "image_id": h(&rebuilt), "elf_sha256": h(&lezreg_types::pda::sha256(&elf)), "image_digest": h(&t.build.image_digest), "network_denied": network_denied, "at": format!("unix:{at}"), "tool": env!("CARGO_PKG_VERSION")}),
        )?;
        eprintln!("marked locally verified in {}", p.display());
    }
    match v {
        Verdict::Match | Verdict::MatchStale { .. } => Ok(()),
        other => bail!("{}", other.label()),
    }
}

/// `lezreg build`: the canonical build of a local source tree (an author checks what the
/// registry will verify before deploying).
pub fn build(out: &mut Out, dir: &Path, b: &BuildConfig, work: Option<&Path>) -> Result<()> {
    let work = workdir(work, "build")?;
    let elf = verify::docker_build(dir, b, true, &work)?;
    let id = verify::image_id_of_elf(&elf)?;
    let bin_path = work.join(format!("{}.elf", b.binary));
    std::fs::write(&bin_path, &elf)?;
    out.text(&format!(
        "image id {}\nelf      {} (sha256 {})",
        hex::encode(id),
        bin_path.display(),
        hex::encode(lezreg_types::pda::sha256(&elf))
    ));
    out.json(json!({"image_id": hex::encode(id), "elf": bin_path.display().to_string()}));
    Ok(())
}

/// Where local "verified" marks live (DESIGN §4.3); the Basecamp module reads the same file.
pub fn marks_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let home = PathBuf::from(std::env::var_os("HOME")?);
            Some(if cfg!(target_os = "macos") {
                home.join("Library/Application Support")
            } else {
                home.join(".local/share")
            })
        })?;
    Some(base.join("lez-registry").join("verified.json"))
}

fn load_marks() -> serde_json::Map<String, serde_json::Value> {
    marks_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_marks(m: &serde_json::Map<String, serde_json::Value>) -> Result<PathBuf> {
    let p = marks_path().ok_or_else(|| anyhow!("no data directory for the marks file"))?;
    let dir = p.parent().expect("has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".verified.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    // Which programs someone checked is their business: owner-only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

/// A mark's key: it applies only while the network, program, image, build tuple and source all
/// stay what was verified.
pub fn mark_key(
    network: &str,
    program: &str,
    image: &[u8; 32],
    build: &BuildConfig,
    source_sha: &[u8; 32],
) -> String {
    let build_hash = lezreg_types::pda::sha256(&borsh::to_vec(build).unwrap_or_default());
    format!(
        "{network}/{program}/{}/{}/{}",
        hex::encode(image),
        hex::encode(build_hash),
        hex::encode(source_sha)
    )
}

/// Records a Match. Never published anywhere.
pub fn write_mark(key: String, record: serde_json::Value) -> Result<PathBuf> {
    let mut m = load_marks();
    m.insert(key, record);
    save_marks(&m)
}

/// `lezreg verified list|forget`.
pub fn marks(out: &mut Out, forget: Option<&str>) -> Result<()> {
    let mut m = load_marks();
    if let Some(prefix) = forget {
        let before = m.len();
        m.retain(|k, v| {
            !(k.contains(prefix) || v.get("program").and_then(|p| p.as_str()) == Some(prefix))
        });
        let p = save_marks(&m)?;
        out.text(&format!(
            "forgot {} mark(s); {} left in {}",
            before - m.len(),
            m.len(),
            p.display()
        ));
        out.json(json!({"forgot": before - m.len()}));
        return Ok(());
    }
    if m.is_empty() {
        out.text("no local verification marks");
    }
    for v in m.values() {
        out.text(&format!(
            "{}  {} rev {}  {}  image {}…  {}",
            v["at"].as_str().unwrap_or("?"),
            v["name"].as_str().unwrap_or("?"),
            v["revision"],
            v["verdict"].as_str().unwrap_or("?"),
            v["image_id"].as_str().unwrap_or("").get(..12).unwrap_or(""),
            v["program"].as_str().unwrap_or("?")
        ));
    }
    out.json(json!({"marks": m}));
    Ok(())
}
