//! Network settings: built-in defaults (`lezreg_sdk::builtin_network`), overlaid by the config
//! file `networks.json` in `$LEZREG_CONFIG_DIR` (default `$XDG_CONFIG_HOME/lezreg` or
//! `~/.config/lezreg`), overlaid by `--rpc`/`--registry`.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use lezreg_types::AccountId;
use serde_json::{Value, json};

pub struct Resolved {
    pub name: String,
    pub rpc: Option<String>,
    pub registry: Option<AccountId>,
}

impl Resolved {
    pub fn rpc(&self) -> Result<String> {
        self.rpc.clone().ok_or_else(|| {
            if self.name == "testnet" {
                anyhow!("LEZ testnet 0.4 has not launched yet, so lezreg has no testnet RPC. Use --network local, or pass --rpc and --registry")
            } else {
                anyhow!("network '{}' has no RPC URL: pass --rpc or run `lezreg network set {} --rpc <url>`", self.name, self.name)
            }
        })
    }

    pub fn registry(&self) -> Result<AccountId> {
        self.registry.ok_or_else(|| {
            if self.name == "testnet" {
                anyhow!("the registry is not deployed on LEZ testnet 0.4 yet (it launches on the testnet's first day). Use --network local, or pass --registry")
            } else {
                anyhow!(
                    "no registry id for network '{}': deploy one with chain/deploy.sh, or run `lezreg network set {} --registry <id>`, or pass --registry",
                    self.name,
                    self.name
                )
            }
        })
    }
}

pub fn path() -> Option<PathBuf> {
    let dir = std::env::var_os("LEZREG_CONFIG_DIR").map(PathBuf::from).or_else(|| {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(|d| PathBuf::from(d).join("lezreg"))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("lezreg")))
    })?;
    Some(dir.join("networks.json"))
}

fn load() -> Result<Value> {
    let Some(p) = path() else { return Ok(json!({})) };
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{} is not JSON", p.display())),
        Err(_) => Ok(json!({})),
    }
}

fn parse_registry(s: &str) -> Result<AccountId> {
    let s = s.strip_prefix("Public/").unwrap_or(s);
    AccountId::from_base58(s).ok_or_else(|| anyhow!("'{s}' is not an account id"))
}

pub fn resolve(name: &str, rpc: Option<&str>, registry: Option<&str>) -> Result<Resolved> {
    let file = load()?;
    let saved = file.get(name);
    let builtin = lezreg_sdk::builtin_network(name);
    if builtin.is_none() && saved.is_none() {
        bail!("unknown network '{name}': use local or testnet, or define it with `lezreg network set {name} --rpc <url> --registry <id>`");
    }
    let mut out = Resolved {
        name: name.to_owned(),
        rpc: builtin.as_ref().and_then(|b| b.rpc.clone()),
        registry: builtin.and_then(|b| b.registry),
    };
    if let Some(s) = saved {
        if let Some(r) = s.get("rpc").and_then(Value::as_str) {
            out.rpc = Some(r.to_owned());
        }
        if let Some(r) = s.get("registry").and_then(Value::as_str) {
            out.registry = Some(parse_registry(r)?);
        }
    }
    if let Some(r) = rpc {
        out.rpc = Some(r.to_owned());
    }
    if let Some(r) = registry {
        out.registry = Some(parse_registry(r)?);
    }
    Ok(out)
}

pub fn set(name: &str, rpc: Option<&str>, registry: Option<&str>) -> Result<PathBuf> {
    let p = path().ok_or_else(|| anyhow!("no config directory: set LEZREG_CONFIG_DIR or HOME"))?;
    let mut file = load()?;
    let entry = file.as_object_mut().ok_or_else(|| anyhow!("{} is not a JSON object", p.display()))?.entry(name).or_insert_with(|| json!({}));
    if let Some(r) = rpc {
        entry["rpc"] = json!(r);
    }
    if let Some(r) = registry {
        entry["registry"] = json!(parse_registry(r)?.to_base58());
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(&file)?)?;
    Ok(p)
}
