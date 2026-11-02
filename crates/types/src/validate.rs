//! Field limits and charsets (DESIGN §2.3). The guest panics with these messages, which is a
//! charged revert at the full declared gas limit, so clients run the same checks first.

use crate::{BUILDER_RISC0_DOCKER, BuildConfig, ContentRef, SCHEMA, SourceRef, limits};

pub type Invalid = &'static str;

fn check(ok: bool, msg: Invalid) -> Result<(), Invalid> {
    if ok { Ok(()) } else { Err(msg) }
}

/// Printable ASCII without spaces.
fn is_token(s: &str) -> bool {
    s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// UTF-8 without control characters.
fn is_text(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

/// `[a-z0-9][a-z0-9._-]*`, 1..=32 bytes.
pub fn name(s: &str) -> Result<(), Invalid> {
    check(!s.is_empty() && s.len() <= limits::NAME, "name must be 1..=32 bytes")?;
    let b = s.as_bytes();
    check(b[0].is_ascii_lowercase() || b[0].is_ascii_digit(), "name must start with [a-z0-9]")?;
    check(
        b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')),
        "name may only contain [a-z0-9._-]",
    )
}

pub fn version(s: &str) -> Result<(), Invalid> {
    check(!s.is_empty() && s.len() <= limits::VERSION, "version must be 1..=32 bytes")?;
    check(is_token(s), "version must be printable ASCII without spaces")
}

pub fn author_label(s: &str) -> Result<(), Invalid> {
    check(s.len() <= limits::AUTHOR_LABEL, "author_label must be at most 64 bytes")?;
    check(is_text(s), "author_label must not contain control characters")
}

pub fn description(s: &str) -> Result<(), Invalid> {
    check(s.len() <= limits::DESCRIPTION, "description must be at most 512 bytes")?;
    check(is_text(s), "description must not contain control characters")
}

pub fn tags(tags: &[String]) -> Result<(), Invalid> {
    check(tags.len() <= limits::TAGS, "at most 8 tags")?;
    for t in tags {
        check(!t.is_empty() && t.len() <= limits::TAG, "a tag must be 1..=24 bytes")?;
        check(
            t.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'),
            "a tag may only contain [a-z0-9-]",
        )?;
    }
    Ok(())
}

pub fn content_ref(c: &ContentRef) -> Result<(), Invalid> {
    check(!c.cid.is_empty() && c.cid.len() <= limits::CID, "a CID must be 1..=64 bytes")?;
    if let Some(p) = &c.peer_hint {
        check(!p.is_empty() && p.len() <= limits::PEER_HINT, "peer_hint must be 1..=48 bytes")?;
    }
    Ok(())
}

pub fn source_ref(s: &SourceRef) -> Result<(), Invalid> {
    check(s.repo.len() <= limits::REPO, "source.repo must be at most 160 bytes")?;
    check(is_token(&s.repo), "source.repo must be printable ASCII without spaces")?;
    check(s.docker_root.len() <= limits::DOCKER_ROOT, "source.docker_root must be at most 96 bytes")?;
    check(is_token(&s.docker_root), "source.docker_root must be printable ASCII without spaces")?;
    check(matches!(s.commit.len(), 0 | 20 | 32), "source.commit must be 20 or 32 bytes")?;
    check(s.repo.is_empty() || !s.commit.is_empty(), "source.commit is required with a repo")?;
    check(!s.repo.is_empty() || s.archive.is_some(), "source needs a repo or an archive")?;
    if let Some(a) = &s.archive {
        content_ref(a)?;
    }
    Ok(())
}

pub fn build_config(b: &BuildConfig) -> Result<(), Invalid> {
    check(b.schema == SCHEMA, "build.schema must be 1")?;
    check(b.builder == BUILDER_RISC0_DOCKER, "build.builder must be 1 (risc0-build Docker)")?;
    let short = |s: &str, max: usize, msg: Invalid| check(s.len() <= max && is_token(s), msg);
    short(&b.image_tag, limits::IMAGE_TAG, "build.image_tag: at most 48 printable bytes")?;
    short(&b.risc0_build, limits::RISC0_BUILD, "build.risc0_build: at most 16 printable bytes")?;
    short(&b.host_risc0_rust, limits::HOST_RISC0_RUST, "build.host_risc0_rust: at most 16 printable bytes")?;
    short(&b.manifest_path, limits::MANIFEST_PATH, "build.manifest_path: at most 96 printable bytes")?;
    short(&b.binary, limits::BINARY, "build.binary: at most 48 printable bytes")?;
    check(!b.binary.is_empty() && !b.manifest_path.is_empty(), "build.binary and build.manifest_path are required")?;
    check(b.features.len() <= limits::FEATURES, "at most 4 build.features")?;
    for f in &b.features {
        check(!f.is_empty() && f.len() <= limits::FEATURE && is_token(f), "a build feature must be 1..=32 printable bytes")?;
    }
    check(b.docker_env.len() <= limits::DOCKER_ENV, "at most 4 build.docker_env entries")?;
    for e in &b.docker_env {
        check(!e.key.is_empty() && e.key.len() + e.value.len() <= limits::DOCKER_ENV_PAIR, "a docker_env entry must be 1..=64 bytes")?;
        check(is_token(&e.key) && is_text(&e.value), "docker_env must be printable")?;
    }
    Ok(())
}

/// Every field a registration sets.
#[allow(clippy::too_many_arguments)]
pub fn entry_fields(
    n: &str,
    v: &str,
    label: &str,
    desc: &str,
    t: &[String],
    idl: &ContentRef,
    source: &SourceRef,
    build: &BuildConfig,
) -> Result<(), Invalid> {
    name(n)?;
    version(v)?;
    author_label(label)?;
    description(desc)?;
    tags(t)?;
    content_ref(idl)?;
    source_ref(source)?;
    build_config(build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["a", "token", "my-token_2.0", "0x", &"a".repeat(32)] {
            assert_eq!(name(ok), Ok(()), "{ok}");
        }
        for bad in ["", "-a", ".a", "_a", "A", "a b", "ä", "a/b", &"a".repeat(33)] {
            assert!(name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tag_rules() {
        assert!(tags(&["defi".into(), "token-2".into()]).is_ok());
        assert!(tags(&["Defi".into()]).is_err());
        assert!(tags(&["".into()]).is_err());
        assert!(tags(&["a".repeat(25)]).is_err());
        assert!(tags(&vec!["a".to_string(); 9]).is_err());
    }

    #[test]
    fn text_limits() {
        assert!(version("1.2.0-rc.1+build").is_ok());
        assert!(version("1 2").is_err());
        assert!(version("").is_err());
        assert!(author_label(&"é".repeat(32)).is_ok());
        assert!(author_label(&"é".repeat(33)).is_err());
        assert!(author_label("a\nb").is_err());
        assert!(description(&"x".repeat(512)).is_ok());
        assert!(description(&"x".repeat(513)).is_err());
    }

    #[test]
    fn content_and_source() {
        let c = ContentRef { cid: vec![1; 38], sha256: [0; 32], size: 10, peer_hint: None };
        assert!(content_ref(&c).is_ok());
        assert!(content_ref(&ContentRef { cid: vec![], ..c.clone() }).is_err());
        assert!(content_ref(&ContentRef { cid: vec![1; 65], ..c.clone() }).is_err());
        assert!(content_ref(&ContentRef { peer_hint: Some(vec![1; 49]), ..c.clone() }).is_err());
        let s = SourceRef { repo: "https://github.com/a/b".into(), commit: vec![1; 20], docker_root: ".".into(), archive: None };
        assert!(source_ref(&s).is_ok());
        assert!(source_ref(&SourceRef { commit: vec![1; 21], ..s.clone() }).is_err());
        assert!(source_ref(&SourceRef { commit: vec![], ..s.clone() }).is_err());
        assert!(source_ref(&SourceRef { repo: String::new(), commit: vec![], archive: None, ..s.clone() }).is_err());
        assert!(source_ref(&SourceRef { repo: String::new(), commit: vec![], archive: Some(c), ..s }).is_ok());
    }

    #[test]
    fn build_rules() {
        let b = BuildConfig {
            schema: 1,
            builder: 1,
            image_tag: "r0.1.91.1".into(),
            risc0_build: "3.0.5".into(),
            host_risc0_rust: "1.91.1".into(),
            manifest_path: "methods/guest/Cargo.toml".into(),
            binary: "x".into(),
            ..Default::default()
        };
        assert!(build_config(&b).is_ok());
        assert!(build_config(&BuildConfig { builder: 2, ..b.clone() }).is_err());
        assert!(build_config(&BuildConfig { features: vec!["f".into(); 5], ..b.clone() }).is_err());
        let env = crate::EnvVar { key: "K".into(), value: "v".repeat(63) };
        assert!(build_config(&BuildConfig { docker_env: vec![env.clone()], ..b.clone() }).is_ok());
        let env = crate::EnvVar { value: "v".repeat(64), ..env };
        assert!(build_config(&BuildConfig { docker_env: vec![env], ..b }).is_err());
    }
}
