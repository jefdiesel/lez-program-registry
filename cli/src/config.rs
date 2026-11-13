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
    /// Registry images the user trusts beyond the committed build (`registry_images` in the
    /// config file, and `LEZREG_REGISTRY_IMAGE`, comma-separated hex): writes are signed only for
    /// a registry running one of these or `lezreg_sdk::REGISTRY_IMAGE`.
    pub registry_images: Vec<[u8; 32]>,
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
    let dir = std::env::var_os("LEZREG_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(|d| PathBuf::from(d).join("lezreg"))
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| PathBuf::from(h).join(".config").join("lezreg"))
                })
        })?;
    Some(dir.join("networks.json"))
}

fn load() -> Result<Value> {
    let Some(p) = path() else {
        return Ok(json!({}));
    };
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("{} is not JSON", p.display())),
        Err(_) => Ok(json!({})),
    }
}

pub fn parse_image(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim().strip_prefix("0x").unwrap_or(s.trim())).map_err(|_| anyhow!("registry image '{s}' is not hex"))?;
    v.try_into().map_err(|_| anyhow!("a registry image is 32 bytes (64 hex characters)"))
}

pub fn parse_registry(s: &str) -> Result<AccountId> {
    let s = s.strip_prefix("Public/").unwrap_or(s);
    AccountId::from_base58(s).ok_or_else(|| anyhow!("'{s}' is not an account id"))
}

pub fn resolve(name: &str, rpc: Option<&str>, registry: Option<&str>) -> Result<Resolved> {
    let file = load()?;
    let saved = file.get(name);
    let builtin = lezreg_sdk::builtin_network(name);
    if builtin.is_none() && saved.is_none() {
        bail!(
            "unknown network '{name}': use local or testnet, or define it with `lezreg network set {name} --rpc <url> --registry <id>`"
        );
    }
    let mut out = Resolved {
        name: name.to_owned(),
        rpc: builtin.as_ref().and_then(|b| b.rpc.clone()),
        registry: builtin.and_then(|b| b.registry),
        registry_images: Vec::new(),
    };
    if let Some(s) = saved {
        if let Some(r) = s.get("rpc").and_then(Value::as_str) {
            out.rpc = Some(r.to_owned());
        }
        if let Some(r) = s.get("registry").and_then(Value::as_str) {
            out.registry = Some(parse_registry(r)?);
        }
        for i in s.get("registry_images").and_then(Value::as_array).into_iter().flatten() {
            out.registry_images.push(parse_image(i.as_str().unwrap_or_default())?);
        }
    }
    if let Ok(env) = std::env::var("LEZREG_REGISTRY_IMAGE") {
        for i in env.split(',').filter(|i| !i.trim().is_empty()) {
            out.registry_images.push(parse_image(i)?);
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

pub fn set(name: &str, rpc: Option<&str>, registry: Option<&str>, registry_image: Option<&str>) -> Result<PathBuf> {
    let p = path().ok_or_else(|| anyhow!("no config directory: set LEZREG_CONFIG_DIR or HOME"))?;
    let mut file = load()?;
    let entry = file
        .as_object_mut()
        .ok_or_else(|| anyhow!("{} is not a JSON object", p.display()))?
        .entry(name)
        .or_insert_with(|| json!({}));
    if let Some(r) = rpc {
        entry["rpc"] = json!(r);
    }
    if let Some(r) = registry {
        entry["registry"] = json!(parse_registry(r)?.to_base58());
    }
    if let Some(i) = registry_image {
        let image = hex::encode(parse_image(i)?);
        let list = entry["registry_images"].as_array().cloned().unwrap_or_default();
        if !list.iter().any(|v| v == &json!(image)) {
            let mut list = list;
            list.push(json!(image));
            entry["registry_images"] = json!(list);
        }
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&p, serde_json::to_vec_pretty(&file)?)?;
    Ok(p)
}
