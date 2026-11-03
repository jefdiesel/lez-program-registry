//! Source verification (DESIGN §4.2): rebuild a recorded build tuple the way `cargo risczero
//! build` (risc0-build 3.0.5) does, with the compile step's network off, and compare image ids
//! three ways (rebuilt, claimed, live header).
//!
//! The Dockerfile is the one risc0-build 3.0.5 writes (`docker.rs` `create_dockerfile`, flags
//! from `lib.rs` `encode_rust_flags`), with two changes: `FROM` pins the image by digest, and
//! the build `RUN` gets `--network=none`. The fetch step keeps the network, so dependencies come
//! only from `Cargo.lock` checksums; every `build.rs` and proc-macro then runs offline.
//! `.github/workflows/repro.yml` proves on every push that this gives the same bytes as the
//! stock build.

use std::path::{Path, PathBuf};
use std::process::Command;

use lezreg_types::{BuildConfig, EnvVar};

use crate::error::{Error, Result};

/// risc0-build's target triple.
pub const TARGET: &str = "riscv32im-risc0-zkvm-elf";
/// `risc0_zkvm_platform::memory::TEXT_START` for non-kernel guests.
const TEXT_START: u32 = 0x0020_0800;

/// The outcome of a verification (DESIGN §4.2 step 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Rebuilt = claimed = live header.
    Match,
    /// Rebuilt = claimed, but the live header runs another image (repointed since, or nothing
    /// deployed): the claim is true, the running code is different.
    MatchStale { live: Option<[u8; 32]> },
    /// The rebuild gives a different image from the one claimed.
    Mismatch {
        rebuilt: [u8; 32],
        claimed: [u8; 32],
    },
    /// The build failed.
    NotReproducible(String),
}

impl Verdict {
    /// The verdict for a successful rebuild.
    #[must_use]
    pub fn of(rebuilt: [u8; 32], claimed: [u8; 32], live: Option<[u8; 32]>) -> Self {
        if rebuilt != claimed {
            Self::Mismatch { rebuilt, claimed }
        } else if live == Some(claimed) {
            Self::Match
        } else {
            Self::MatchStale { live }
        }
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Match => "Match",
            Self::MatchStale { .. } => "MatchStale",
            Self::Mismatch { .. } => "Mismatch",
            Self::NotReproducible(_) => "NotReproducible",
        }
    }
}

/// `encode_rust_flags(meta, escape = true)`: the elements joined by 0x1f, each
/// `escape_default`ed. `host_rust` is the host's rzup Rust version (`1.91.1`); above 1.81 LLVM's
/// pass is `lower-atomic`. `extra` is the guest's `[package.metadata.risc0] rustc-flags`.
pub fn encoded_rustflags(host_rust: &str, extra: &[String]) -> Result<String> {
    let lower_atomic = if version_above_1_81(host_rust)? {
        "passes=lower-atomic"
    } else {
        "passes=loweratomic"
    };
    let text = format!("link-arg=-Ttext={TEXT_START:#010x}");
    let fixed = [
        "-C",
        lower_atomic,
        "-C",
        &text,
        "-C",
        "link-arg=--fatal-warnings",
        "-C",
        "panic=abort",
        "--cfg",
        "getrandom_backend=\"custom\"",
    ];
    let all: Vec<String> = extra
        .iter()
        .map(String::as_str)
        .chain(fixed)
        .map(|s| s.escape_default().to_string())
        .collect();
    Ok(all.join("\u{1f}"))
}

fn version_above_1_81(v: &str) -> Result<bool> {
    let parts: Vec<u64> = v
        .split('.')
        .map(|p| p.parse::<u64>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| Error::Invalid(format!("host rust version '{v}' is not x.y.z")))?;
    let (a, b, c) = (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    Ok((a, b, c) > (1, 81, 0))
}

/// A value for a Dockerfile `ENV k="v"` line.
fn quoted(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The Dockerfile for `b`. `rustc_flags`: the guest's `[package.metadata.risc0] rustc-flags`.
/// `offline_compile`: `RUN --network=none` on the build step (the canonical verification);
/// without it the build matches `cargo risczero build` line for line apart from the pinned
/// `FROM` (the `--allow-network` retry).
pub fn dockerfile(
    b: &BuildConfig,
    rustc_flags: &[String],
    offline_compile: bool,
) -> Result<String> {
    check_build_config(b)?;
    let flags = encoded_rustflags(&b.host_risc0_rust, rustc_flags)?;
    let common = format!("--locked --target {TARGET} --manifest-path $CARGO_MANIFEST_PATH");
    let features = if b.features.is_empty() {
        String::new()
    } else {
        format!(" --features {}", b.features.join(","))
    };
    let mut f = String::new();
    f.push_str(&format!(
        "FROM risczero/risc0-guest-builder:{}@sha256:{} AS build\n",
        b.image_tag,
        hex::encode(b.image_digest)
    ));
    f.push_str("WORKDIR /src\nCOPY . .\n");
    f.push_str(&format!(
        "ENV CARGO_MANIFEST_PATH={}\n",
        quoted(&b.manifest_path)
    ));
    f.push_str(&format!("ENV CARGO_ENCODED_RUSTFLAGS={}\n", quoted(&flags)));
    f.push_str("ENV CARGO_TARGET_DIR=\"target\"\nENV RISC0_FEATURE_bigint2=\"\"\n");
    f.push_str(
        "ENV CC_riscv32im_risc0_zkvm_elf=\"/root/.risc0/cpp/bin/riscv32-unknown-elf-gcc\"\n",
    );
    f.push_str("ENV CFLAGS_riscv32im_risc0_zkvm_elf=\"-march=rv32im -nostdlib\"\n");
    for EnvVar { key, value } in &b.docker_env {
        f.push_str(&format!("ENV {key}={}\n", quoted(value)));
    }
    if offline_compile {
        // Fetch for every platform: `--target` skips crates only host-side build scripts need,
        // which the network-off compile then cannot download (repro.yml, 2026-11-03).
        f.push_str("RUN cargo +risc0 fetch --locked --manifest-path $CARGO_MANIFEST_PATH\n");
        f.push_str(&format!(
            "RUN --network=none cargo +risc0 build --release {common}{features}\n"
        ));
    } else {
        f.push_str(&format!("RUN cargo +risc0 fetch {common}\n"));
        f.push_str(&format!(
            "RUN cargo +risc0 build --release {common}{features}\n"
        ));
    }
    f.push_str(&format!(
        "FROM scratch AS export\nCOPY --from=build /src/target/{TARGET}/release /\n"
    ));
    Ok(f)
}

/// risc0-build's `.dockerignore`.
pub const DOCKERIGNORE: &str = "**/Dockerfile\n**/.git\n**/node_modules\n**/target\n**/tmp\n";

/// The values go into a Dockerfile and a cargo command line, from chain data anyone can write:
/// only what a real build config can hold.
fn check_build_config(b: &BuildConfig) -> Result<()> {
    let bad = |what: &str, v: &str| {
        Err(Error::Invalid(format!(
            "the entry's build {what} '{v}' is not usable"
        )))
    };
    let plain = |s: &str, extra: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
    };
    if b.builder != lezreg_types::BUILDER_RISC0_DOCKER {
        return Err(Error::Invalid(format!(
            "builder {} is not risc0-build Docker (1)",
            b.builder
        )));
    }
    if !plain(&b.image_tag, "._-") {
        return bad("image tag", &b.image_tag);
    }
    if !plain(&b.manifest_path, "._-/")
        || b.manifest_path.contains("..")
        || b.manifest_path.starts_with('/')
    {
        return bad("manifest path", &b.manifest_path);
    }
    if !plain(&b.binary, "_-") {
        return bad("binary", &b.binary);
    }
    for feat in &b.features {
        if !plain(feat, "_-/") {
            return bad("feature", feat);
        }
    }
    for e in &b.docker_env {
        if !plain(&e.key, "_") || e.value.contains(['\n', '\r']) {
            return bad("docker env", &e.key);
        }
    }
    version_above_1_81(&b.host_risc0_rust).map(|_| ())
}

/// A source tree ready to build, and the sha256 of the `git archive` it came from.
pub struct Source {
    pub dir: PathBuf,
    pub archive_sha256: [u8; 32],
}

fn run(cmd: &mut Command, what: &str) -> Result<Vec<u8>> {
    let out = cmd
        .output()
        .map_err(|e| Error::Invalid(format!("{what}: cannot run: {e}")))?;
    if !out.status.success() {
        // The cause is usually well above the last lines (docker prints the failing step's
        // output, then the Dockerfile excerpt), so keep a generous tail.
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(40).collect();
        return Err(Error::Invalid(format!(
            "{what} failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )));
    }
    Ok(out.stdout)
}

/// Clones `repo` into `work`, and extracts `git archive <commit>:<docker_root>` into
/// `work/src`. The repo URL and commit come from chain data: only `https://` URLs and hex
/// commits are accepted, and git never runs a command the URL names.
pub fn source_from_git(
    repo: &str,
    commit: &[u8],
    docker_root: &str,
    work: &Path,
) -> Result<Source> {
    if !repo.starts_with("https://") || repo.contains(char::is_whitespace) {
        return Err(Error::Invalid(format!(
            "source repo '{repo}' is not an https:// URL"
        )));
    }
    if !matches!(commit.len(), 20 | 32) {
        return Err(Error::Invalid("the entry has no usable commit".into()));
    }
    if docker_root.contains("..")
        || docker_root.starts_with('/')
        || docker_root.contains(char::is_whitespace)
    {
        return Err(Error::Invalid(format!(
            "docker root '{docker_root}' is not a relative path"
        )));
    }
    let commit = hex::encode(commit);
    let git = work.join("git");
    std::fs::create_dir_all(work).map_err(|e| Error::Invalid(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&git);
    run(
        Command::new("git")
            .args([
                "-c",
                "protocol.allow=never",
                "-c",
                "protocol.https.allow=always",
                "clone",
                "--quiet",
                "--no-checkout",
                "--",
                repo,
            ])
            .arg(&git),
        "git clone",
    )?;
    let root = if docker_root.is_empty() || docker_root == "." {
        String::new()
    } else {
        format!(":{}", docker_root.trim_end_matches('/'))
    };
    let tar = run(
        Command::new("git").arg("-C").arg(&git).args([
            "archive",
            "--format=tar",
            &format!("{commit}{root}"),
        ]),
        "git archive",
    )?;
    let archive_sha256 = lezreg_types::pda::sha256(&tar);
    let dir = work.join("src");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Invalid(e.to_string()))?;
    let tar_path = work.join("source.tar");
    std::fs::write(&tar_path, &tar).map_err(|e| Error::Invalid(e.to_string()))?;
    run(
        Command::new("tar")
            .arg("-xf")
            .arg(&tar_path)
            .arg("-C")
            .arg(&dir),
        "tar",
    )?;
    Ok(Source {
        dir,
        archive_sha256,
    })
}

/// The `Cargo.lock` the guest build uses: the first one found walking up from the guest
/// manifest's directory to the build root.
#[must_use]
pub fn guest_lock(dir: &Path, manifest_path: &str) -> Option<PathBuf> {
    let mut p = dir.join(manifest_path);
    while p.pop() {
        let lock = p.join("Cargo.lock");
        if lock.is_file() {
            return Some(lock);
        }
        if p == dir {
            break;
        }
    }
    None
}

/// `[package.metadata.risc0] rustc-flags` from the guest manifest, if any.
#[must_use]
pub fn guest_rustc_flags(dir: &Path, manifest_path: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(manifest_path)) else {
        return Vec::new();
    };
    let Ok(v) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    v.get("package")
        .and_then(|p| p.get("metadata"))
        .and_then(|m| m.get("risc0"))
        .and_then(|r| r.get("rustc-flags"))
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Runs the build in Docker (buildx) and returns the guest ELF. `log` receives docker's output.
pub fn docker_build(
    dir: &Path,
    b: &BuildConfig,
    offline_compile: bool,
    out_dir: &Path,
) -> Result<Vec<u8>> {
    let flags = guest_rustc_flags(dir, &b.manifest_path);
    let ctx = out_dir.join("ctx");
    std::fs::create_dir_all(&ctx).map_err(|e| Error::Invalid(e.to_string()))?;
    let df = ctx.join("Dockerfile");
    std::fs::write(&df, dockerfile(b, &flags, offline_compile)?)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(ctx.join("Dockerfile.dockerignore"), DOCKERIGNORE)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let export = out_dir.join("export");
    let _ = std::fs::remove_dir_all(&export);
    run(
        Command::new("docker")
            .args(["buildx", "build", "--progress=plain", "-f"])
            .arg(&df)
            .arg("--output")
            .arg(format!("type=local,dest={}", export.display()))
            .arg(dir),
        "docker buildx build",
    )?;
    std::fs::read(export.join(&b.binary))
        .map_err(|e| Error::Invalid(format!("the build produced no {}: {e}", b.binary)))
}

/// The image id LEZ's loader computes for a guest ELF: `Program::id()` of the
/// `ProgramBinary(user ELF, V1COMPAT)`, as 32 little-endian bytes.
pub fn image_id_of_elf(elf: &[u8]) -> Result<[u8; 32]> {
    let bin = risc0_binfmt::ProgramBinary::new(elf, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
    image_id_of_bin(&bin)
}

/// The image id of a risc0 program binary (`.bin`).
pub fn image_id_of_bin(bin: &[u8]) -> Result<[u8; 32]> {
    let id = lee::program::Program::new(bin.to_vec().into())
        .map_err(|e| Error::Invalid(format!("not a loadable program: {e}")))?
        .id();
    Ok(crate::tx::image_bytes(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BuildConfig {
        BuildConfig {
            schema: lezreg_types::SCHEMA,
            builder: lezreg_types::BUILDER_RISC0_DOCKER,
            image_digest: [0xfa; 32],
            image_tag: "r0.1.91.1".into(),
            risc0_build: "3.0.5".into(),
            host_risc0_rust: "1.91.1".into(),
            manifest_path: "methods/guest/Cargo.toml".into(),
            binary: "lez_program_registry".into(),
            features: vec![],
            lock_sha256: [0; 32],
            docker_env: vec![],
        }
    }

    #[test]
    fn rustflags_match_risc0_build_3_0_5() {
        // The exact string repro.yml (built by hand from risc0-build 3.0.5) uses, which CI
        // proved byte-identical to `cargo risczero build`.
        let us = '\u{1f}';
        let want = format!(
            "-C{us}passes=lower-atomic{us}-C{us}link-arg=-Ttext=0x00200800{us}-C{us}link-arg=--fatal-warnings{us}-C{us}panic=abort{us}--cfg{us}getrandom_backend=\\\"custom\\\""
        );
        assert_eq!(encoded_rustflags("1.91.1", &[]).unwrap(), want);
        assert!(
            encoded_rustflags("1.81.0", &[])
                .unwrap()
                .contains("passes=loweratomic")
        );
    }

    #[test]
    fn the_dockerfile_pins_the_digest_and_takes_the_network_off_the_compile() {
        let d = dockerfile(&config(), &[], true).unwrap();
        assert!(d.starts_with(&format!(
            "FROM risczero/risc0-guest-builder:r0.1.91.1@sha256:{} AS build\n",
            "fa".repeat(32)
        )));
        assert!(d.contains("\nRUN --network=none cargo +risc0 build --release --locked --target riscv32im-risc0-zkvm-elf --manifest-path $CARGO_MANIFEST_PATH\n"));
        assert!(
            d.contains("\nRUN cargo +risc0 fetch --locked --manifest-path $CARGO_MANIFEST_PATH\n")
        );
        assert!(d.ends_with("COPY --from=build /src/target/riscv32im-risc0-zkvm-elf/release /\n"));
    }

    #[test]
    fn hostile_build_configs_are_refused() {
        for f in [
            |b: &mut BuildConfig| b.manifest_path = "../../etc/x/Cargo.toml".into(),
            |b: &mut BuildConfig| b.image_tag = "x\nRUN curl evil".into(),
            |b: &mut BuildConfig| b.features = vec!["a b".into()],
            |b: &mut BuildConfig| b.binary = "x;rm".into(),
            |b: &mut BuildConfig| {
                b.docker_env = vec![EnvVar {
                    key: "A".into(),
                    value: "1\nRUN evil".into(),
                }]
            },
            |b: &mut BuildConfig| b.host_risc0_rust = "latest".into(),
        ] {
            let mut b = config();
            f(&mut b);
            assert!(dockerfile(&b, &[], true).is_err());
        }
    }

    #[test]
    fn verdicts() {
        let (a, b) = ([1; 32], [2; 32]);
        assert_eq!(Verdict::of(a, a, Some(a)), Verdict::Match);
        assert_eq!(
            Verdict::of(a, a, Some(b)),
            Verdict::MatchStale { live: Some(b) }
        );
        assert_eq!(Verdict::of(a, a, None), Verdict::MatchStale { live: None });
        assert_eq!(
            Verdict::of(a, b, Some(b)),
            Verdict::Mismatch {
                rebuilt: a,
                claimed: b
            }
        );
    }

    #[test]
    fn git_sources_must_be_https_and_relative() {
        let w = std::env::temp_dir();
        assert!(source_from_git("ext::sh -c evil", &[0; 20], ".", &w).is_err());
        assert!(source_from_git("file:///etc", &[0; 20], ".", &w).is_err());
        assert!(source_from_git("https://x/y", &[0; 20], "../up", &w).is_err());
        assert!(source_from_git("https://x/y", &[], ".", &w).is_err());
    }
}
