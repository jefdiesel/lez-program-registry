//! IDLs on Logos Storage: publish, and the fetch ladder with graceful degradation
//! (DESIGN §5.2–5.4).
//!
//! Every transport is untrusted. Bytes from any source (cache, a directory the caller trusts,
//! the user's Storage node) are kept only if their sha256 and size match the on-chain
//! [`ContentRef`]; anything else is discarded unparsed. `resolve` never fails because of an
//! IDL: callers get an [`IdlStatus`] instead.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lezreg_types::ContentRef;
use lezreg_types::pda::sha256;
use serde_json::Value;

use crate::cid;
use crate::storage::{MAX_IDL, StorageClient, StorageError};

/// Deepest JSON nesting an IDL may have. SPEL IDLs nest a handful of levels; this only stops
/// a hostile file from exhausting the parser's stack.
pub const MAX_DEPTH: usize = 64;

/// What became of an entry's IDL.
#[derive(Debug, Clone, PartialEq)]
pub enum IdlStatus {
    /// Fetched, hash-checked and parsed. The value is the IDL JSON.
    Ok(Value),
    /// Not fetched by anything yet (a caller that fetches in the background shows this first).
    Pending,
    Unavailable(Unavailable),
    /// Bytes arrived but were not the content the chain names. They were discarded unparsed.
    HashMismatch {
        expected: [u8; 32],
        got: [u8; 32],
    },
    /// The content is the right bytes but not a usable IDL.
    ParseError(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// No Storage node is configured or it does not answer.
    NoStorageNode(String),
    /// The node is up but could not find the content.
    NotFound(String),
    Timeout(String),
    /// The on-chain size, or what arrived, is over [`MAX_IDL`].
    TooLarge,
    /// The entry's CID is not one this client can ask a node for.
    BadCid(String),
}

impl std::fmt::Display for IdlStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ok(_) => write!(f, "ok"),
            Self::Pending => write!(f, "pending"),
            Self::Unavailable(u) => match u {
                Unavailable::NoStorageNode(m) => write!(f, "unavailable: no Storage node ({m})"),
                Unavailable::NotFound(m) => write!(f, "unavailable: not found ({m})"),
                Unavailable::Timeout(m) => write!(f, "unavailable: timed out ({m})"),
                Unavailable::TooLarge => write!(f, "unavailable: larger than {MAX_IDL} bytes"),
                Unavailable::BadCid(m) => write!(f, "unavailable: {m}"),
            },
            Self::HashMismatch { expected, got } => {
                write!(
                    f,
                    "hash mismatch: the chain names {}, the bytes hash to {}",
                    hex::encode(expected),
                    hex::encode(got)
                )
            }
            Self::ParseError(m) => write!(f, "not a usable IDL: {m}"),
        }
    }
}

/// A peer to dial when a fetch stalls: a pinning node, written as its libp2p id and addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub addrs: Vec<String>,
}

/// Where and how to look for content. [`FetchConfig::default`] is DESIGN §5.3's ladder against
/// Storage's default local API, with no pin peers and no extra directories.
#[derive(Debug, Clone)]
pub struct FetchConfig {
    /// The user's Storage node API, or `None` for cache and directories only.
    pub storage_api: Option<String>,
    /// Blob cache directory (`<dir>/<sha256 hex>`); `None` disables it.
    pub cache_dir: Option<PathBuf>,
    /// Directories whose files may hold the content (bring-your-own bytes); hash-checked.
    pub byo_dirs: Vec<PathBuf>,
    /// Pinning nodes to dial after the entry's own `peer_hint`. Works empty.
    pub pin_peers: Vec<Peer>,
    /// Pauses between attempts against the node; its length plus one is the attempt count.
    pub backoff: Vec<Duration>,
    pub timeouts: crate::storage::Timeouts,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            storage_api: Some(crate::storage::DEFAULT_API.to_owned()),
            cache_dir: default_blob_dir(),
            byo_dirs: Vec::new(),
            pin_peers: Vec::new(),
            backoff: vec![
                Duration::from_secs(2),
                Duration::from_secs(8),
                Duration::from_secs(20),
            ],
            timeouts: crate::storage::Timeouts::default(),
        }
    }
}

/// `$XDG_CACHE_HOME/lez-registry/blobs` (or `~/.cache/lez-registry/blobs`).
#[must_use]
pub fn default_blob_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("lez-registry").join("blobs"))
}

/// Gets the bytes `r` names, trying in order: the cache, the caller's directories, the user's
/// Storage node (with retries, dialing `peer_hint` and the pin peers between attempts). Bytes
/// are returned only if they match `r.sha256` and `r.size`; a match from the network is cached.
pub fn fetch_content(r: &ContentRef, cfg: &FetchConfig) -> Result<Vec<u8>, IdlStatus> {
    if usize::try_from(r.size).map_or(true, |s| s > MAX_IDL) {
        return Err(IdlStatus::Unavailable(Unavailable::TooLarge));
    }
    if let Some(bytes) = cfg.cache_dir.as_deref().and_then(|d| from_cache(d, r)) {
        return Ok(bytes);
    }
    for dir in &cfg.byo_dirs {
        if let Some(bytes) = from_dir(dir, r) {
            remember(cfg, r, &bytes);
            return Ok(bytes);
        }
    }
    let Some(api) = cfg.storage_api.as_deref() else {
        return Err(IdlStatus::Unavailable(Unavailable::NoStorageNode(
            "none configured".into(),
        )));
    };
    let node = StorageClient::with_timeouts(api, cfg.timeouts);
    let cid_text = cid::parts(&r.cid)
        .map(|_| cid::to_string(&r.cid))
        .map_err(|e| IdlStatus::Unavailable(Unavailable::BadCid(e)))?;
    let mut peers: Vec<Peer> = Vec::new();
    if let Some(hint) = r.peer_hint.as_deref().filter(|h| !h.is_empty()) {
        peers.push(Peer {
            id: bs58::encode(hint).into_string(),
            addrs: Vec::new(),
        });
    }
    peers.extend(cfg.pin_peers.iter().cloned());

    let attempts = cfg.backoff.len() + 1;
    let mut last = StorageError::Unreachable("not tried".into());
    for attempt in 0..attempts {
        if attempt > 0 {
            std::thread::sleep(cfg.backoff[attempt - 1]);
            // A stalled lookup is often cleared by dialing a known provider (kill-check, §5.3).
            for p in &peers {
                let _ = node.connect(&p.id, &p.addrs);
            }
        }
        match node.fetch(&cid_text, MAX_IDL, false) {
            Ok(bytes) => {
                let got = sha256(&bytes);
                if got != r.sha256 || bytes.len() != r.size as usize {
                    return Err(IdlStatus::HashMismatch {
                        expected: r.sha256,
                        got,
                    });
                }
                remember(cfg, r, &bytes);
                return Ok(bytes);
            }
            // Refusals that retrying won't change.
            Err(StorageError::TooLarge { .. }) => {
                return Err(IdlStatus::Unavailable(Unavailable::TooLarge));
            }
            Err(e @ StorageError::Unreachable(_)) if attempt == 0 && node.info().is_err() => {
                return Err(IdlStatus::Unavailable(Unavailable::NoStorageNode(
                    e.to_string(),
                )));
            }
            Err(e) => last = e,
        }
    }
    Err(IdlStatus::Unavailable(match last {
        StorageError::Timeout { .. } => Unavailable::Timeout(format!("{attempts} tries")),
        StorageError::Unreachable(m) => Unavailable::NoStorageNode(m),
        other => Unavailable::NotFound(format!("{other} after {attempts} tries")),
    }))
}

/// Fetches and checks an entry's IDL. Never fails: the outcome is the status.
#[must_use]
pub fn fetch_idl(r: &ContentRef, cfg: &FetchConfig) -> IdlStatus {
    match fetch_content(r, cfg) {
        Ok(bytes) => parse_idl(&bytes),
        Err(status) => status,
    }
}

/// Parses IDL bytes (already hash-checked) and checks they are a SPEL spec-0.1.0 IDL.
#[must_use]
pub fn parse_idl(bytes: &[u8]) -> IdlStatus {
    let v = match parse_limited(bytes) {
        Ok(v) => v,
        Err(e) => return IdlStatus::ParseError(e),
    };
    let Some(obj) = v.as_object() else {
        return IdlStatus::ParseError("not a JSON object".into());
    };
    for key in ["name", "version", "instructions"] {
        if !obj.contains_key(key) {
            return IdlStatus::ParseError(format!("missing `{key}`"));
        }
    }
    if !obj["instructions"].is_array() {
        return IdlStatus::ParseError("`instructions` is not a list".into());
    }
    IdlStatus::Ok(v)
}

/// Parses JSON of at most [`MAX_IDL`] bytes and [`MAX_DEPTH`] levels.
pub fn parse_limited(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_IDL {
        return Err(format!("larger than {MAX_IDL} bytes"));
    }
    let depth = max_depth(bytes);
    if depth > MAX_DEPTH {
        return Err(format!("nested {depth} levels deep (limit {MAX_DEPTH})"));
    }
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}

/// Bracket nesting outside strings, without parsing.
fn max_depth(bytes: &[u8]) -> usize {
    let (mut depth, mut max, mut in_str, mut esc) = (0usize, 0usize, false, false);
    for &b in bytes {
        if in_str {
            match (esc, b) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

fn from_cache(dir: &Path, r: &ContentRef) -> Option<Vec<u8>> {
    let bytes = std::fs::read(dir.join(hex::encode(r.sha256))).ok()?;
    (sha256(&bytes) == r.sha256 && bytes.len() == r.size as usize).then_some(bytes)
}

fn from_dir(dir: &Path, r: &ContentRef) -> Option<Vec<u8>> {
    for item in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(meta) = item.metadata() else { continue };
        if !meta.is_file() || meta.len() != u64::from(r.size) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(item.path())
            && sha256(&bytes) == r.sha256
        {
            return Some(bytes);
        }
    }
    None
}

/// Caches content by its hash. Best effort: a cache that can't be written costs a refetch.
fn remember(cfg: &FetchConfig, r: &ContentRef, bytes: &[u8]) {
    let Some(dir) = cfg.cache_dir.as_deref() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = dir.join(hex::encode(r.sha256));
    let tmp = dir.join(format!(".{}.tmp", hex::encode(r.sha256)));
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// An IDL ready to upload: canonical bytes and the name Storage will hold them under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedIdl {
    pub bytes: Vec<u8>,
    pub filename: String,
    pub sha256: [u8; 32],
}

/// Canonicalizes (RFC 8785) an IDL file's bytes and checks it is an IDL within the limits.
/// The upload name is `<name>-<version>.idl.json` from the IDL itself, so identical IDLs get
/// identical CIDs (DESIGN §5.2).
pub fn prepare_idl(raw: &[u8]) -> Result<PreparedIdl, String> {
    let bytes = crate::jcs::canonicalize(raw)?;
    if bytes.len() > MAX_IDL {
        return Err(format!(
            "the IDL is {} bytes canonicalized; the limit is {MAX_IDL}",
            bytes.len()
        ));
    }
    let IdlStatus::Ok(v) = parse_idl(&bytes) else {
        return Err(match parse_idl(&bytes) {
            IdlStatus::ParseError(e) => e,
            other => other.to_string(),
        });
    };
    let part = |k: &str| -> Result<String, String> {
        let s = v.get(k).and_then(Value::as_str).unwrap_or_default();
        if s.is_empty()
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            || s.len() > 64
        {
            return Err(format!(
                "the IDL's `{k}` must be 1–64 characters of a-z, 0-9, '-', '_' or '.'"
            ));
        }
        Ok(s.to_owned())
    };
    let filename = format!("{}-{}.idl.json", part("name")?, part("version")?);
    let sha256 = sha256(&bytes);
    Ok(PreparedIdl {
        bytes,
        filename,
        sha256,
    })
}

/// Uploads a prepared IDL to the user's node, waits for the node to have an address and
/// re-announces, and returns the on-chain reference (with the node as `peer_hint`).
pub fn publish(
    node: &StorageClient,
    idl: &PreparedIdl,
    reachable_within: Duration,
) -> Result<ContentRef, String> {
    let cid_text = node
        .upload(&idl.filename, &idl.bytes)
        .map_err(|e| e.to_string())?;
    let cid_bytes = cid::parse(&cid_text)?;
    let info = node
        .wait_reachable(reachable_within)
        .map_err(|e| e.to_string())?;
    node.reannounce(&cid_text).map_err(|e| e.to_string())?;
    let peer_hint = bs58::decode(&info.id)
        .into_vec()
        .ok()
        .filter(|p| !p.is_empty() && p.len() <= lezreg_types::limits::PEER_HINT);
    Ok(ContentRef {
        cid: cid_bytes,
        sha256: idl.sha256,
        size: u32::try_from(idl.bytes.len()).map_err(|_| "too large")?,
        peer_hint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lezreg-idl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn reference(bytes: &[u8]) -> ContentRef {
        ContentRef {
            cid: cid::raw_cid(bytes),
            sha256: sha256(bytes),
            size: bytes.len() as u32,
            peer_hint: None,
        }
    }

    fn offline(cache: Option<PathBuf>, byo: Vec<PathBuf>) -> FetchConfig {
        FetchConfig {
            storage_api: None,
            cache_dir: cache,
            byo_dirs: byo,
            pin_peers: vec![],
            backoff: vec![],
            timeouts: Default::default(),
        }
    }

    const IDL: &[u8] = br#"{"version":"0.1.0","name":"toy","instructions":[{"name":"go","accounts":[],"args":[]}]}"#;

    #[test]
    fn a_directory_copy_is_used_only_if_its_hash_matches_and_is_then_cached() {
        let byo = tmpdir("byo");
        let cache = tmpdir("cache");
        std::fs::write(byo.join("decoy.json"), br#"{"version":"0.1.0","name":"toz","instructions":[{"name":"go","accounts":[],"args":[]}]}"#).unwrap();
        let r = reference(IDL);
        assert!(matches!(
            fetch_idl(&r, &offline(Some(cache.clone()), vec![byo.clone()])),
            IdlStatus::Unavailable(Unavailable::NoStorageNode(_))
        ));
        std::fs::write(byo.join("toy.idl.json"), IDL).unwrap();
        assert!(matches!(
            fetch_idl(&r, &offline(Some(cache.clone()), vec![byo.clone()])),
            IdlStatus::Ok(_)
        ));
        std::fs::remove_dir_all(&byo).unwrap();
        assert!(
            matches!(
                fetch_idl(&r, &offline(Some(cache), vec![])),
                IdlStatus::Ok(_)
            ),
            "served from the cache"
        );
    }

    #[test]
    fn a_tampered_cache_file_is_ignored() {
        let cache = tmpdir("tamper");
        let r = reference(IDL);
        std::fs::write(cache.join(hex::encode(r.sha256)), b"{\"evil\":1}").unwrap();
        assert!(matches!(
            fetch_idl(&r, &offline(Some(cache), vec![])),
            IdlStatus::Unavailable(_)
        ));
    }

    #[test]
    fn an_oversized_reference_is_refused_without_fetching() {
        let mut r = reference(IDL);
        r.size = (MAX_IDL + 1) as u32;
        assert_eq!(
            fetch_idl(&r, &offline(None, vec![])),
            IdlStatus::Unavailable(Unavailable::TooLarge)
        );
    }

    #[test]
    fn parse_checks_shape_and_depth() {
        assert!(matches!(parse_idl(b"[]"), IdlStatus::ParseError(_)));
        assert!(matches!(
            parse_idl(br#"{"name":"x","version":"1"}"#),
            IdlStatus::ParseError(_)
        ));
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
        assert!(
            matches!(parse_idl(deep.as_bytes()), IdlStatus::ParseError(e) if e.contains("nested"))
        );
        assert!(matches!(parse_idl(IDL), IdlStatus::Ok(_)));
    }

    #[test]
    fn prepare_canonicalizes_and_names_the_file() {
        let pretty =
            b"{\n  \"name\": \"toy\",\n  \"version\": \"0.1.0\",\n  \"instructions\": []\n}";
        let p = prepare_idl(pretty).unwrap();
        assert_eq!(p.filename, "toy-0.1.0.idl.json");
        assert_eq!(
            p.bytes,
            br#"{"instructions":[],"name":"toy","version":"0.1.0"}"#
        );
        assert_eq!(
            prepare_idl(&p.bytes).unwrap(),
            p,
            "canonical input is a fixed point"
        );
        assert!(prepare_idl(br#"{"name":"../x","version":"1","instructions":[]}"#).is_err());
    }
}
