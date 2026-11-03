//! IDLs on Logos Storage from the command line (DESIGN §5.3, milestone M3): `idl get`,
//! `idl push`, `pin` and `pin-all`.
//!
//! Reads go to the user's own Storage node only. `pin`/`pin-all` fetch with `advertise=true`,
//! which turns the node into one more provider of registry content: that is what keeps an
//! outside author's IDL resolvable after their own node goes away (logos-storage-nim#1556 makes
//! extra providers matter).

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use lezreg_sdk::{
    HttpRpc, Registry, cid,
    idl::{self, FetchConfig, IdlStatus, Peer},
    storage::{MAX_IDL, StorageClient},
    tx::tid,
};
use lezreg_types::{ContentRef, Entry};
use serde_json::json;

use crate::{args::parse_account, out::Out};

/// Where the user's Storage node and pin peers are.
#[derive(clap::Args, Clone)]
pub struct StorageOpts {
    /// The Logos Storage node's REST API.
    #[arg(long, env = "LEZREG_STORAGE_API", default_value = lezreg_sdk::storage::DEFAULT_API)]
    pub storage_api: String,
    /// A pinning node to dial when a fetch stalls: `<peer id>` or `<peer id>@<multiaddr>`.
    /// Repeatable. Fetching works with none.
    #[arg(long = "pin-peer", env = "LEZREG_PIN_PEERS", value_delimiter = ',')]
    pub pin_peers: Vec<String>,
    /// A directory whose files may hold the IDL (used only if the hash matches).
    #[arg(long = "dir")]
    pub dirs: Vec<std::path::PathBuf>,
    /// Attempts against the node (backoff 2 s, 8 s, 20 s between them).
    #[arg(long, default_value_t = 4)]
    pub tries: usize,
}

impl StorageOpts {
    fn fetch_config(&self) -> Result<FetchConfig> {
        let backoff = [2u64, 8, 20]
            .iter()
            .map(|s| Duration::from_secs(*s))
            .take(self.tries.saturating_sub(1))
            .collect();
        let pin_peers = self
            .pin_peers
            .iter()
            .map(|p| parse_peer(p))
            .collect::<Result<_>>()?;
        Ok(FetchConfig {
            storage_api: Some(self.storage_api.clone()),
            byo_dirs: self.dirs.clone(),
            pin_peers,
            backoff,
            ..FetchConfig::default()
        })
    }
}

fn parse_peer(s: &str) -> Result<Peer> {
    let (id, addr) = s.split_once('@').map_or((s, None), |(i, a)| (i, Some(a)));
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        bail!("'{s}' is not a peer: expected <peer id> or <peer id>@<multiaddr>");
    }
    Ok(Peer {
        id: id.to_owned(),
        addrs: addr.map(|a| vec![a.to_owned()]).unwrap_or_default(),
    })
}

/// The deployer entry for a program id, or for a name that names exactly one deployer entry.
pub fn deployer_entry(r: &Registry<HttpRpc>, target: &str) -> Result<Entry> {
    if let Ok(program) = parse_account(target) {
        return r
            .resolve(&tid(&program))?
            .deployer
            .ok_or_else(|| anyhow!("{target} has no deployer entry"));
    }
    let found = r.by_name(&target.to_ascii_lowercase(), 10_000)?;
    let mut deployers = found
        .into_iter()
        .filter(|i| i.kind == Some(lezreg_sdk::Kind::Deployer))
        .filter_map(|i| i.entry);
    match (deployers.next(), deployers.next()) {
        (Some(e), None) => Ok(e),
        (None, _) => bail!("'{target}' names no deployer entry; give the program id"),
        (Some(_), Some(_)) => {
            bail!("'{target}' names more than one deployer entry; give the program id")
        }
    }
}

fn status_json(s: &IdlStatus) -> serde_json::Value {
    match s {
        IdlStatus::Ok(_) => json!({"status": "ok"}),
        other => json!({"status": "unavailable", "reason": other.to_string()}),
    }
}

/// `lezreg idl get <program|name>`: fetch, check and print (or save) an entry's IDL.
pub fn get(
    out: &mut Out,
    r: &Registry<HttpRpc>,
    target: &str,
    o: &StorageOpts,
    save: Option<&std::path::Path>,
) -> Result<()> {
    let e = deployer_entry(r, target)?;
    let status = idl::fetch_idl(&e.idl, &o.fetch_config()?);
    match &status {
        IdlStatus::Ok(v) => {
            let pretty = serde_json::to_string_pretty(v)?;
            if let Some(path) = save {
                // The canonical bytes, exactly what the chain hashes, not the pretty print.
                let bytes = idl::fetch_content(
                    &e.idl,
                    &FetchConfig {
                        storage_api: None,
                        ..o.fetch_config()?
                    },
                )
                .map_err(|s| anyhow!("{s}"))?;
                std::fs::write(path, bytes)?;
                out.text(&format!(
                    "saved {} {}'s IDL ({} B, sha256 {}) to {}",
                    e.name,
                    e.version,
                    e.idl.size,
                    hex::encode(e.idl.sha256),
                    path.display()
                ));
            } else {
                out.text(&pretty);
            }
            out.json(json!({"name": e.name, "version": e.version, "idl": v, "cid": cid::to_string(&e.idl.cid), "sha256": hex::encode(e.idl.sha256)}));
            Ok(())
        }
        other => {
            out.json(json!({"name": e.name, "idl": status_json(other), "cid": cid::to_string(&e.idl.cid)}));
            bail!(
                "{} {}: IDL {other}. Try --pin-peer <peer> or ask the author to run `lezreg pin {}`",
                e.name,
                e.version,
                e.name
            )
        }
    }
}

/// `lezreg idl push <file>`: canonicalize, upload to the user's node, print the reference.
pub fn push(out: &mut Out, file: &std::path::Path, o: &StorageOpts) -> Result<ContentRef> {
    let raw = std::fs::read(file).map_err(|e| anyhow!("cannot read {}: {e}", file.display()))?;
    let prepared = idl::prepare_idl(&raw).map_err(|e| anyhow!("{}: {e}", file.display()))?;
    let node = StorageClient::new(&o.storage_api);
    eprintln!(
        "uploading {} ({} B canonical) to {}",
        prepared.filename,
        prepared.bytes.len(),
        o.storage_api
    );
    let r = idl::publish(&node, &prepared, Duration::from_secs(60))
        .map_err(|e| anyhow!("upload: {e}"))?;
    let cid_text = cid::to_string(&r.cid);
    let peer = r
        .peer_hint
        .as_deref()
        .map(|p| bs58::encode(p).into_string());
    out.text(&format!(
        "cid      {cid_text}\nsha256   {}\nsize     {} B\npeer     {}\n\nUse with register/update:\n  --idl-cid {cid_text} --idl-sha256 {} --idl-size {}",
        hex::encode(r.sha256),
        r.size,
        peer.as_deref().unwrap_or("(none)"),
        hex::encode(r.sha256),
        r.size
    ));
    out.json(json!({"cid": cid_text, "sha256": hex::encode(r.sha256), "size": r.size, "peer": peer, "filename": prepared.filename}));
    Ok(r)
}

/// Fetches one content ref with `advertise=true`, so this node becomes a provider.
fn pin_ref(node: &StorageClient, r: &ContentRef) -> std::result::Result<(), String> {
    if r.size as usize > MAX_IDL {
        return Err(format!("{} B is over the {MAX_IDL} B limit", r.size));
    }
    let text = cid::to_string(&r.cid);
    let bytes = node
        .fetch(&text, MAX_IDL, true)
        .map_err(|e| e.to_string())?;
    if lezreg_types::pda::sha256(&bytes) != r.sha256 {
        return Err("the bytes do not match the on-chain sha256".into());
    }
    // Storage v0.5.3 keeps a streamed copy but does not announce it, whatever `advertise` the
    // stream asked for (measured: the flag stayed false, and a fresh node got 404 until it was
    // set). Announcing explicitly is what makes this node a provider others can discover.
    node.reannounce(&text)
        .map_err(|e| format!("kept, but could not announce it: {e}"))
}

/// `lezreg pin <program|name>`.
pub fn pin(out: &mut Out, r: &Registry<HttpRpc>, target: &str, o: &StorageOpts) -> Result<()> {
    let e = deployer_entry(r, target)?;
    let node = StorageClient::new(&o.storage_api);
    for p in &o.fetch_config()?.pin_peers {
        let _ = node.connect(&p.id, &p.addrs);
    }
    pin_ref(&node, &e.idl).map_err(|m| anyhow!("pin {}: {m}", e.name))?;
    out.text(&format!(
        "pinned {} {}'s IDL {} on {}",
        e.name,
        e.version,
        cid::to_string(&e.idl.cid),
        o.storage_api
    ));
    out.json(json!({"pinned": [cid::to_string(&e.idl.cid)], "failed": []}));
    Ok(())
}

/// `lezreg pin-all`: walk the whole registry and pin every entry's IDL, once or every
/// `every` seconds. Its cost is set by the walk interval, not by how many people read.
pub fn pin_all(
    out: &mut Out,
    r: &Registry<HttpRpc>,
    o: &StorageOpts,
    every: Option<u64>,
) -> Result<()> {
    let node = StorageClient::new(&o.storage_api);
    loop {
        for p in &o.fetch_config()?.pin_peers {
            let _ = node.connect(&p.id, &p.addrs);
        }
        let (mut pinned, mut failed) = (Vec::new(), Vec::new());
        let mut seen = std::collections::HashSet::new();
        let mut after = None;
        loop {
            let page = r.list(after.as_ref(), 50)?;
            for item in &page.items {
                let Some(e) = &item.entry else { continue };
                if !seen.insert(e.idl.sha256) {
                    continue;
                }
                match pin_ref(&node, &e.idl) {
                    Ok(()) => pinned.push(cid::to_string(&e.idl.cid)),
                    Err(m) => {
                        eprintln!("  {} {}: {m}", e.name, e.version);
                        failed.push(
                            json!({"name": e.name, "cid": cid::to_string(&e.idl.cid), "error": m}),
                        );
                    }
                }
            }
            match page.next {
                Some(n) => after = Some(n),
                None => break,
            }
        }
        r.cache().save().ok();
        eprintln!("pin-all: {} pinned, {} failed", pinned.len(), failed.len());
        let Some(secs) = every else {
            out.text(&format!(
                "pinned {} IDLs, {} failed",
                pinned.len(),
                failed.len()
            ));
            out.json(json!({"pinned": pinned, "failed": failed}));
            return Ok(());
        };
        std::thread::sleep(Duration::from_secs(secs.max(60)));
    }
}

/// Options for `lezreg storage up`.
#[derive(clap::Args, Clone)]
pub struct UpOpts {
    /// The Logos Storage node binary (v0.5.3). Default: `logos-storage` on PATH. lezreg never
    /// downloads it for you: get it from the logos-storage-nim releases and check its hash.
    #[arg(long, env = "LEZREG_STORAGE_BIN")]
    pub bin: Option<std::path::PathBuf>,
    /// Data directory. Default: $XDG_DATA_HOME/lez-registry/storage (or ~/.local/share/…).
    #[arg(long)]
    pub data_dir: Option<std::path::PathBuf>,
    #[arg(long, default_value_t = 8090)]
    pub api_port: u16,
    #[arg(long, default_value_t = 8070)]
    pub listen_port: u16,
    /// How long the node keeps blocks, in seconds. Default ten years. 0 is refused: Storage
    /// v0.5.3 expires blocks at once with it, whatever its help says.
    #[arg(long, default_value_t = 315_360_000)]
    pub block_ttl: u64,
    /// A Storage node to join (its SPR, `spr:…`). Default: the network's bootstrap nodes.
    #[arg(long, conflicts_with = "no_bootstrap")]
    pub bootstrap: Option<String>,
    /// Join no network (a local test node). Storage then needs --nat.
    #[arg(long, requires = "nat")]
    pub no_bootstrap: bool,
    /// Passed to the node as `--nat=…`, for example `extip:203.0.113.7` (or `extip:127.0.0.1`
    /// for a local test node).
    #[arg(long)]
    pub nat: Option<String>,
}

/// `lezreg storage up`: start a Logos Storage node in the background for authors without one,
/// wait until it advertises an address, and print the API to use (DESIGN §5.3).
pub fn up(out: &mut Out, o: &UpOpts) -> Result<()> {
    if o.block_ttl == 0 {
        bail!("--block-ttl 0 makes Storage v0.5.3 expire blocks immediately; use a long TTL");
    }
    let bin = o
        .bin
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("logos-storage"));
    let data = match &o.data_dir {
        Some(d) => d.clone(),
        None => std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })
            .ok_or_else(|| anyhow!("no HOME; pass --data-dir"))?
            .join("lez-registry/storage"),
    };
    std::fs::create_dir_all(&data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700))?;
    }
    let api = format!("http://127.0.0.1:{}/api/storage/v1", o.api_port);
    if StorageClient::new(&api).info().is_ok() {
        bail!(
            "a Storage node already answers at {api}; use it (LEZREG_STORAGE_API={api}) or pick another --api-port"
        );
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data.join("node.log"))?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg(format!("--data-dir={}", data.display()))
        .arg(format!("--api-port={}", o.api_port))
        .arg(format!("--listen-port={}", o.listen_port))
        .arg(format!("--block-ttl={}", o.block_ttl));
    if let Some(spr) = &o.bootstrap {
        cmd.arg(format!("--bootstrap-node={spr}"));
    }
    if o.no_bootstrap {
        cmd.arg("--no-bootstrap-node");
    }
    if let Some(nat) = &o.nat {
        cmd.arg(format!("--nat={nat}"));
    }
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .map_err(|e| {
            anyhow!(
                "cannot start {}: {e} (pass --bin <path to logos-storage v0.5.3>)",
                bin.display()
            )
        })?;
    std::fs::write(data.join("node.pid"), child.id().to_string())?;
    eprintln!(
        "started {} (pid {}), waiting for it to advertise an address…",
        bin.display(),
        child.id()
    );
    let info = StorageClient::new(&api)
        .wait_reachable(Duration::from_secs(120))
        .map_err(|e| {
            anyhow!(
                "the node started but {e}; see {}",
                data.join("node.log").display()
            )
        })?;
    out.text(&format!(
        "Storage node up\n  api      {api}\n  peer     {}\n  addrs    {}\n  data     {}\n  stop     kill $(cat {})\n\nexport LEZREG_STORAGE_API={api}",
        info.id,
        info.addrs.join(" "),
        data.display(),
        data.join("node.pid").display()
    ));
    out.json(json!({"api": api, "peer": info.id, "addrs": info.addrs, "data_dir": data.display().to_string(), "pid": child.id()}));
    Ok(())
}
