//! Minimal `names-network/2` reader for the module (audit rows 438, 440).
//!
//! Parse-only: one whole file, no merging, no network fetch. Unknown keys are
//! ignored as in v1. The built-in profile is `network-rc3.json` (our LEZ v0.4.0-rc3
//! chain); the host can name another file with the `network_file` setting.

use lezreg_sdk::types::AccountId;
use serde_json::Value;

pub const BUILTIN_RAW: &str = include_str!("../network-rc3.json");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkFile {
    pub name: String,
    pub chain_id: String,
    pub rpc: String,
    /// `programs.lns_registry.id` (v1 mirror: `registry`).
    pub lns: Option<AccountId>,
    /// `programs.program_registry.id`, when this chain has one.
    pub registry: Option<AccountId>,
    /// `programs.program_registry.image_id`, a trusted pin when the build's own
    /// pin does not match (audit row 437).
    pub registry_image: Option<[u8; 32]>,
    pub storage_api: Option<String>,
}

fn program_id(value: &Value, field: &str) -> Result<Option<AccountId>, String> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => AccountId::from_base58(text)
            .map(Some)
            .ok_or_else(|| format!("{field} is not an account id: {text}")),
        _ => Err(format!("{field} is not a string")),
    }
}

pub fn parse(text: &str) -> Result<NetworkFile, String> {
    if text.len() > 64 * 1024 {
        return Err("network file is larger than 64 KiB".into());
    }
    let file: Value = serde_json::from_str(text).map_err(|e| format!("bad network file: {e}"))?;
    let format = file.get("format").and_then(Value::as_str).unwrap_or_default();
    if format != "names-network/1" && format != "names-network/2" {
        return Err(format!("this build cannot read format '{format}'"));
    }
    if file.get("lez").and_then(Value::as_str).unwrap_or_default() != "0.4" {
        return Err("this build cannot use this LEZ line".into());
    }
    let chain = file
        .get("chain_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if chain.len() != 64 || !chain.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err("network file has no 64-hex lowercase chain_id".into());
    }
    let rpc = file
        .get("rpc")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // https, or http to this machine for a `kind: local` file (spec: http only for local chains).
    let local = file.get("kind").and_then(Value::as_str) == Some("local");
    let loopback = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|p| rpc.strip_prefix(p).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/'])));
    if !(rpc.starts_with("https://") && rpc.len() > 8) && !(local && loopback) {
        return Err("network file RPC must be https (http only to this machine, for kind \"local\")".into());
    }
    let programs = file.get("programs");
    let lns = match programs.and_then(|p| p.get("lns_registry")) {
        Some(value) => program_id(value, "id")?,
        None => program_id(&file, "registry")?,
    };
    let registry = match programs.and_then(|p| p.get("program_registry")) {
        None => None,
        Some(Value::Null) => None,
        Some(value) => program_id(value, "id")?,
    };
    let registry_image = match programs
        .and_then(|p| p.get("program_registry"))
        .and_then(|p| p.get("image_id"))
        .and_then(Value::as_str)
    {
        None => None,
        Some(text) => {
            let bytes = hex::decode(text.trim_start_matches("0x"))
                .map_err(|_| "program_registry.image_id is not hex".to_string())?;
            let mut image = [0u8; 32];
            if bytes.len() != 32 {
                return Err("program_registry.image_id must be 32 bytes".into());
            }
            image.copy_from_slice(&bytes);
            Some(image)
        }
    };
    Ok(NetworkFile {
        name: file
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        chain_id: chain,
        rpc,
        lns,
        registry,
        registry_image,
        storage_api: file
            .get("storage_api")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The built-in rc3 profile, so the module works out of the box and offline.
pub fn builtin() -> Result<NetworkFile, String> {
    parse(BUILTIN_RAW)
}

/// Compare a chain's `getChannelId` answer with the file. Both are 64-hex.
pub fn chain_matches(file: &NetworkFile, channel_id: &str) -> bool {
    file.chain_id.eq_ignore_ascii_case(channel_id.trim().trim_start_matches("0x"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_profile_has_ids_and_image() {
        let file = builtin().unwrap();
        assert_eq!(file.chain_id.len(), 64);
        assert!(file.rpc.starts_with("https://"));
        assert!(file.lns.is_some());
        assert_eq!(file.registry_image.unwrap()[0], 0x5f);
        assert!(chain_matches(&file, &file.chain_id));
        assert!(!chain_matches(&file, &"0".repeat(64)));
    }

    #[test]
    fn v1_mirrors_are_mapped() {
        let one = serde_json::json!({
            "format": "names-network/1",
            "name": "v1",
            "lez": "0.4",
            "chain_id": "ab".repeat(32),
            "rpc": "https://example.org/",
            "registry": "6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA"
        })
        .to_string();
        let file = parse(&one).unwrap();
        assert_eq!(file.lns.unwrap().to_base58(), "6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA");
        assert!(file.registry.is_none());
    }

    #[test]
    fn bad_files_are_refused() {
        let mut bad: Value = serde_json::from_str(BUILTIN_RAW).unwrap();
        bad["chain_id"] = Value::String("abc".into());
        assert!(parse(&bad.to_string()).is_err());
        let mut bad: Value = serde_json::from_str(BUILTIN_RAW).unwrap();
        bad["rpc"] = Value::String("http://example.org/".into());
        assert!(parse(&bad.to_string()).is_err());
        let mut bad: Value = serde_json::from_str(BUILTIN_RAW).unwrap();
        bad["format"] = Value::String("names-network/2".into());
        assert!(parse(&bad.to_string()).is_ok()); // v2 readers accept /2 for 0.4 files
    }

    #[test]
    fn plain_http_only_to_this_machine_for_a_local_file() {
        let mut file: Value = serde_json::from_str(BUILTIN_RAW).unwrap();
        file["rpc"] = Value::String("http://127.0.0.1:3290/".into());
        assert!(parse(&file.to_string()).is_err(), "our-test over http");
        file["kind"] = Value::String("local".into());
        assert_eq!(parse(&file.to_string()).unwrap().rpc, "http://127.0.0.1:3290/");
        file["rpc"] = Value::String("http://127.0.0.1.evil.example/".into());
        assert!(parse(&file.to_string()).is_err());
        file["rpc"] = Value::String("http://example.org/".into());
        assert!(parse(&file.to_string()).is_err());
    }
}
