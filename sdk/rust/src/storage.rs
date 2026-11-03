//! Logos Storage node REST client (API v0.5.3), for the small blobs the registry points at:
//! IDLs (at most [`MAX_IDL`]) and, later, source archives.
//!
//! Ported from `logos-mirror`'s `mirror-sdk/src/storage.rs` (ureq 2 → ureq 3) and cut down to
//! what the registry needs. Talks only to the node the user configured: redirects are refused
//! and proxies are ignored unless `LEZREG_USE_PROXY=1` (as for the sequencer, §7.7).

use std::time::{Duration, Instant};

use serde_json::Value;

/// The node's API when the user configures nothing: Storage's own default.
pub const DEFAULT_API: &str = "http://127.0.0.1:8080/api/storage/v1";

/// IDLs above this are refused before upload and before parsing (DESIGN §5.2): below the
/// relay allowance, so an IDL travels even when the hosting node is only reachable through a
/// relay.
pub const MAX_IDL: usize = 120 * 1024;

/// The mimetype every registry upload uses. The manifest CID depends on bytes, filename and
/// mimetype, so a fixed mimetype keeps CIDs reproducible.
pub const IDL_MIME: &str = "application/json";

/// A Storage failure, said in a way the CLI can print as is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// The node could not be reached at all (connection refused, DNS, timeout before a reply).
    Unreachable(String),
    /// The node answered with an HTTP error.
    Status { op: &'static str, code: u16 },
    /// The node answered, but not with what the API promises.
    BadReply { op: &'static str, detail: String },
    /// The content is larger than the caller allowed.
    TooLarge { limit: usize },
    /// The read timed out before the content arrived.
    Timeout { op: &'static str },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "Storage node unreachable: {m}"),
            Self::Status { op, code } => write!(f, "Storage {op}: HTTP {code}"),
            Self::BadReply { op, detail } => write!(f, "Storage {op}: unexpected reply: {detail}"),
            Self::TooLarge { limit } => write!(f, "content is larger than {limit} bytes"),
            Self::Timeout { op } => write!(f, "Storage {op}: timed out"),
        }
    }
}

impl std::error::Error for StorageError {}

pub type SResult<T> = std::result::Result<T, StorageError>;

/// What a node says about itself (`GET /debug/info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub id: String,
    pub addrs: Vec<String>,
}

/// Timeouts for one fetch. The defaults are DESIGN §5.3's; tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub manifest: Duration,
    pub stream: Duration,
    pub dial: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            manifest: Duration::from_secs(15),
            stream: Duration::from_secs(30),
            dial: Duration::from_secs(10),
        }
    }
}

pub struct StorageClient {
    base: String,
    agent: ureq::Agent,
    t: Timeouts,
}

impl StorageClient {
    /// A client for the node whose API root is `base` (for example
    /// `http://127.0.0.1:8080/api/storage/v1`).
    #[must_use]
    pub fn new(base: &str) -> Self {
        Self::with_timeouts(base, Timeouts::default())
    }

    #[must_use]
    pub fn with_timeouts(base: &str, t: Timeouts) -> Self {
        let proxy = if std::env::var("LEZREG_USE_PROXY").is_ok_and(|v| v == "1") {
            ureq::Proxy::try_from_env()
        } else {
            None
        };
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(t.dial))
            .max_redirects(0)
            .http_status_as_error(false)
            .proxy(proxy)
            .build();
        Self {
            base: base.trim_end_matches('/').to_owned(),
            agent: config.into(),
            t,
        }
    }

    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, tail: &str) -> String {
        format!("{}{tail}", self.base)
    }

    /// Uploads `bytes` as `filename` and returns the manifest CID text (`zDv…`).
    pub fn upload(&self, filename: &str, bytes: &[u8]) -> SResult<String> {
        if filename.is_empty() || filename.contains(['"', '\r', '\n', '/', '\\']) {
            return Err(StorageError::BadReply {
                op: "upload",
                detail: format!("unusable filename {filename:?}"),
            });
        }
        let mut resp = self
            .agent
            .post(&self.url("/data"))
            .config()
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .header("Content-Type", IDL_MIME)
            .header(
                "Content-Disposition",
                &format!("attachment; filename=\"{filename}\""),
            )
            .send(bytes)
            .map_err(|e| transport("upload", &e))?;
        let code = resp.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(StorageError::Status { op: "upload", code });
        }
        let cid = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| transport("upload", &e))?
            .trim()
            .to_owned();
        if cid.is_empty() || !cid.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(StorageError::BadReply {
                op: "upload",
                detail: format!("not a CID: {cid:?}"),
            });
        }
        Ok(cid)
    }

    /// Asks the node to find the manifest of `cid` on the network (or locally). `advertise=false`
    /// keeps a reader from announcing itself as a provider of content it may soon drop.
    pub fn manifest(&self, cid: &str) -> SResult<Value> {
        check_cid(cid)?;
        let mut resp = self
            .agent
            .get(&self.url(&format!("/data/{cid}/network/manifest")))
            .query("advertise", "false")
            .config()
            .timeout_global(Some(self.t.manifest))
            .build()
            .call()
            .map_err(|e| transport("manifest", &e))?;
        let code = resp.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(StorageError::Status {
                op: "manifest",
                code,
            });
        }
        let text = resp
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_string()
            .map_err(|e| transport("manifest", &e))?;
        serde_json::from_str(&text).map_err(|e| StorageError::BadReply {
            op: "manifest",
            detail: e.to_string(),
        })
    }

    /// Streams `cid` from the network, refusing more than `limit` bytes. `advertise` is passed to
    /// the node, but Storage v0.5.3 does not announce a streamed copy either way: a pinner must
    /// call [`StorageClient::reannounce`] afterwards.
    pub fn fetch(&self, cid: &str, limit: usize, advertise: bool) -> SResult<Vec<u8>> {
        check_cid(cid)?;
        let mut resp = self
            .agent
            .get(&self.url(&format!("/data/{cid}/network/stream")))
            .query("advertise", if advertise { "true" } else { "false" })
            .config()
            .timeout_global(Some(self.t.stream))
            .build()
            .call()
            .map_err(|e| transport("fetch", &e))?;
        let code = resp.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(StorageError::Status { op: "fetch", code });
        }
        let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(limit_u64.saturating_add(1))
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => StorageError::TooLarge { limit },
                other => transport("fetch", &other),
            })?;
        if bytes.len() > limit {
            return Err(StorageError::TooLarge { limit });
        }
        Ok(bytes)
    }

    /// `GET /debug/info`: the node's peer id and the addresses it advertises.
    pub fn info(&self) -> SResult<NodeInfo> {
        let mut resp = self
            .agent
            .get(&self.url("/debug/info"))
            .config()
            .timeout_global(Some(self.t.manifest))
            .build()
            .call()
            .map_err(|e| transport("info", &e))?;
        let code = resp.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(StorageError::Status { op: "info", code });
        }
        let text = resp
            .body_mut()
            .with_config()
            .limit(256 * 1024)
            .read_to_string()
            .map_err(|e| transport("info", &e))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| StorageError::BadReply {
            op: "info",
            detail: e.to_string(),
        })?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if id.is_empty() {
            return Err(StorageError::BadReply {
                op: "info",
                detail: "no peer id".into(),
            });
        }
        let addrs = v
            .get("addrs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(NodeInfo { id, addrs })
    }

    /// Polls `debug/info` until the node advertises an address. Announcing before that makes
    /// the first announcement unreachable (the announce-after-address gotcha).
    pub fn wait_reachable(&self, timeout: Duration) -> SResult<NodeInfo> {
        let start = Instant::now();
        loop {
            if let Ok(info) = self.info()
                && !info.addrs.is_empty()
            {
                return Ok(info);
            }
            if start.elapsed() >= timeout {
                return Err(StorageError::Timeout {
                    op: "wait for an address",
                });
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// Announces `cid` (advertise off, then on): for content uploaded before the node had an
    /// address, and for a pinned copy, which the node keeps but does not announce on its own.
    pub fn reannounce(&self, cid: &str) -> SResult<()> {
        self.set_advertise(cid, false)?;
        self.set_advertise(cid, true)
    }

    pub fn set_advertise(&self, cid: &str, on: bool) -> SResult<()> {
        check_cid(cid)?;
        let resp = self
            .agent
            .post(&self.url(&format!("/data/{cid}/advertise")))
            .query("advertise", if on { "true" } else { "false" })
            .config()
            .timeout_global(Some(self.t.manifest))
            .build()
            .send_empty()
            .map_err(|e| transport("advertise", &e))?;
        let code = resp.status().as_u16();
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(StorageError::Status {
                op: "advertise",
                code,
            })
        }
    }

    /// Asks the node to dial `peer` (at `addrs`, if known). The peer id comes from chain data
    /// anyone can write, so it must be plain base58 before it goes into a URL path.
    pub fn connect(&self, peer: &str, addrs: &[String]) -> SResult<()> {
        if peer.is_empty() || peer.len() > 64 || !peer.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(StorageError::BadReply {
                op: "connect",
                detail: format!("not a peer id: {peer:?}"),
            });
        }
        let mut req = self.agent.get(&self.url(&format!("/connect/{peer}")));
        for a in addrs {
            req = req.query("addrs", a);
        }
        let resp = req
            .config()
            .timeout_global(Some(self.t.dial))
            .build()
            .call()
            .map_err(|e| transport("connect", &e))?;
        let code = resp.status().as_u16();
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(StorageError::Status {
                op: "connect",
                code,
            })
        }
    }
}

/// A CID goes into a URL path; only plain multibase text is allowed there.
fn check_cid(cid: &str) -> SResult<()> {
    if cid.is_empty() || cid.len() > 128 || !cid.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(StorageError::BadReply {
            op: "cid",
            detail: format!("not a CID: {cid:?}"),
        });
    }
    Ok(())
}

fn transport(op: &'static str, e: &ureq::Error) -> StorageError {
    match e {
        ureq::Error::Timeout(_) => StorageError::Timeout { op },
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            StorageError::Timeout { op }
        }
        other => StorageError::Unreachable(format!("{op}: {other}")),
    }
}
