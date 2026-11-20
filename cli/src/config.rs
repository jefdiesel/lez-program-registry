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
    /// The chain id (`getChannelId`) this network belongs to, from the v2 file.
    pub chain_id: Option<String>,
    /// `programs.lns_registry.id` from the v2 file (audit row 439).
    pub lns: Option<AccountId>,
    pub storage_api: Option<String>,
    /// Registry images the user trusts beyond the committed build (`registry_images` in the
    /// config file, and `LEZREG_REGISTRY_IMAGE`, comma-separated hex): writes are signed only for
    /// a registry running one of these or `lezreg_sdk::REGISTRY_IMAGE`.
    pub registry_images: Vec<[u8; 32]>,
}

impl Resolved {
    pub fn rpc(&self) -> Result<String> {
        self.rpc.clone().ok_or_else(|| {
            anyhow!("network '{}' has no RPC URL: pass --rpc or run `lezreg network set {} --rpc <url>`", self.name, self.name)
        })
    }

    pub fn registry(&self) -> Result<AccountId> {
        self.registry.ok_or_else(|| {
            anyhow!(
                "no registry id for network '{}': run `lezreg network set {} --registry <id>`, or pass --registry",
                self.name,
                self.name
            )
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

/// The name of a network saved in the config file with exactly this RPC and registry, if any.
/// `lezreg module send` signs only for such a pair: the module (or an app that set its
/// sequencer) can't send it to a sequencer and registry the user never chose in lezreg.
pub fn saved_network_for(rpc: &str, registry: &AccountId) -> Result<Option<String>> {
    let file = load()?;
    let want = rpc.trim_end_matches('/');
    Ok(file
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(name, v)| {
            let r = v.get("rpc").and_then(Value::as_str)?.trim_end_matches('/');
            let g = v
                .get("registry")
                .and_then(Value::as_str)
                .and_then(|g| parse_registry(g).ok())?;
            (r == want && g == *registry).then(|| name.clone())
        }))
}

pub fn parse_image(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s.trim().strip_prefix("0x").unwrap_or(s.trim()))
        .map_err(|_| anyhow!("registry image '{s}' is not hex"))?;
    v.try_into()
        .map_err(|_| anyhow!("a registry image is 32 bytes (64 hex characters)"))
}

pub fn parse_registry(s: &str) -> Result<AccountId> {
    let s = s.strip_prefix("Public/").unwrap_or(s);
    AccountId::from_base58(s).ok_or_else(|| anyhow!("'{s}' is not an account id"))
}

pub fn resolve(
    name: &str,
    rpc: Option<&str>,
    registry: Option<&str>,
    lns: Option<&str>,
) -> Result<Resolved> {
    let file = load()?;
    let saved = file.get(name);
    let builtin = lezreg_sdk::builtin_network(name);
    if builtin.is_none() && saved.is_none() && std::env::var_os("LEZREG_NETWORK_FILE").is_none() {
        bail!(
            "unknown network '{name}': use local or testnet, or define it with `lezreg network set {name} --rpc <url> --registry <id>`"
        );
    }
    let mut out = Resolved {
        name: name.to_owned(),
        rpc: builtin.as_ref().and_then(|b| b.rpc.clone()),
        registry: builtin.and_then(|b| b.registry),
        chain_id: None,
        lns: None,
        storage_api: None,
        registry_images: Vec::new(),
    };
    // The `testnet` network is a real profile now (audit row 438): it comes
    // from the embedded names-network/2 file, not from placeholder constants.
    if name == "testnet" {
        let profile = lezreg_sdk::network_file::builtin()
            .map_err(|error| anyhow!("built-in network file: {error}"))?;
        out.rpc = Some(profile.rpc);
        out.chain_id = Some(profile.chain_id);
        out.registry = profile.registry;
        out.lns = profile.lns;
        if let Some(image) = profile.registry_image {
            out.registry_images.push(image);
        }
    }
    // An explicit v2 file (flag/env/saved setting) overrides the built-in
    // profile. Whole files are used; fields are never merged across files.
    let file_path = std::env::var("LEZREG_NETWORK_FILE").ok().or_else(|| {
        saved
            .and_then(|s| s.get("network_file"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    if let Some(path) = file_path {
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("network file {} is not readable", path))?;
        let profile = lezreg_sdk::network_file::parse(&text)
            .map_err(|error| anyhow!("network file {}: {error}", path))?;
        out.rpc = Some(profile.rpc);
        out.chain_id = Some(profile.chain_id);
        out.registry = profile.registry;
        out.lns = profile.lns;
        if let Some(image) = profile.registry_image {
            out.registry_images.push(image);
        }
        if let Some(api) = profile.storage_api {
            out.storage_api = Some(api);
        }
    }
    if let Some(s) = saved {
        if let Some(r) = s.get("rpc").and_then(Value::as_str) {
            out.rpc = Some(r.to_owned());
        }
        if let Some(r) = s.get("registry").and_then(Value::as_str) {
            out.registry = Some(parse_registry(r)?);
        }
        if let Some(c) = s.get("chain_id").and_then(Value::as_str) {
            out.chain_id = Some(c.to_string());
        }
        if let Some(l) = s.get("lns").and_then(Value::as_str) {
            out.lns = Some(parse_registry(l)?);
        }
        if let Some(api) = s.get("storage_api").and_then(Value::as_str) {
            out.storage_api = Some(api.to_string());
        }
        for i in s
            .get("registry_images")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.registry_images
                .push(parse_image(i.as_str().unwrap_or_default())?);
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
    if let Ok(l) = std::env::var("LEZREG_LNS") {
        out.lns = Some(parse_registry(&l)?);
    }
    if let Some(l) = lns {
        out.lns = Some(parse_registry(l)?);
    }
    Ok(out)
}

/// Check the RPC's `getChannelId` against the file's chain id (spec §4/§6).
pub fn check_chain(rpc: &str, chain_id: &str) -> Result<()> {
    use lezreg_sdk::rpc::HttpRpc;
    let value = HttpRpc::new(rpc).call("getChannelId", serde_json::json!([]))?;
    let got = value
        .as_str()
        .ok_or_else(|| anyhow!("getChannelId did not return a string"))?;
    if !lezreg_sdk::network_file::chain_matches(
        &lezreg_sdk::network_file::NetworkFile {
            name: String::new(),
            chain_id: chain_id.to_string(),
            rpc: rpc.to_string(),
            lns: None,
            registry: None,
            registry_image: None,
            storage_api: None,
        },
        got,
    ) {
        bail!("the RPC chain id {got} does not match the network file's {chain_id}");
    }
    Ok(())
}

pub fn set(
    name: &str,
    rpc: Option<&str>,
    registry: Option<&str>,
    registry_image: Option<&str>,
    chain_id: Option<&str>,
    lns: Option<&str>,
    network_file: Option<&str>,
) -> Result<PathBuf> {
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
    if let Some(c) = chain_id {
        entry["chain_id"] = json!(c);
    }
    if let Some(l) = lns {
        entry["lns"] = json!(parse_registry(l)?.to_base58());
    }
    if let Some(path) = network_file {
        entry["network_file"] = json!(path);
    }
    if let Some(i) = registry_image {
        let image = hex::encode(parse_image(i)?);
        let list = entry["registry_images"]
            .as_array()
            .cloned()
            .unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Both tests set and remove `LEZREG_CONFIG_DIR`, a process-wide variable; cargo runs tests
    /// in parallel, so they take turns.
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn testnet_resolves_from_the_embedded_network_file() {
        let _env = ENV.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("lezreg-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("LEZREG_CONFIG_DIR", &dir) };
        let net = resolve("testnet", None, None, None).unwrap();
        let profile = lezreg_sdk::network_file::builtin().unwrap();
        assert_eq!(net.rpc.as_deref(), Some(profile.rpc.as_str()));
        assert_eq!(net.chain_id.as_deref(), Some(profile.chain_id.as_str()));
        assert_eq!(net.registry, profile.registry);
        assert_eq!(net.lns, profile.lns);
        assert!(
            net.registry_images
                .contains(&profile.registry_image.unwrap())
        );
        unsafe { std::env::remove_var("LEZREG_CONFIG_DIR") };
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saved_chain_id_and_lns_are_read_and_flag_overrides_lns() {
        let _env = ENV.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("lezreg-net2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unsafe { std::env::set_var("LEZREG_CONFIG_DIR", &dir) };
        let lns = "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN";
        set(
            "mine",
            Some("https://example.org/"),
            None,
            None,
            Some(&"ab".repeat(32)),
            Some(lns),
            None,
        )
        .unwrap();
        let net = resolve("mine", None, None, None).unwrap();
        assert_eq!(net.chain_id.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(net.lns.unwrap().to_base58(), lns);
        let other = "6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA";
        let net = resolve("mine", None, None, Some(other)).unwrap();
        assert_eq!(net.lns.unwrap().to_base58(), other);
        unsafe { std::env::remove_var("LEZREG_CONFIG_DIR") };
        std::fs::remove_dir_all(dir).unwrap();
    }
}
