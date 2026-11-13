//! `lezreg module send`: the signing bridge for the `program_registry` Logos module
//! (DESIGN §7.3). The module builds a registry transaction (`prepare_*`) but holds no keys; it
//! hands the change to this command on stdin. The change is checked against what this SDK would
//! build (`Writer::send_prepared`) before anything is signed, so a module can't get any other
//! transaction signed through it.
//!
//! The change (`lezreg-change/1`, as the module's `prepare_*` returns it):
//!
//! ```json
//! { "format": "lezreg-change/1", "what": "Freeze", "sequencer": "http://127.0.0.1:3062",
//!   "registry": "<base58>", "root": 0,
//!   "accounts": [{"account": "<base58>", "program": "<base58>", "signs": true}, …],
//!   "message": "<hex>", "entry": "<base58>", "expires_at": 1790000600000 }
//! ```

use anyhow::{Context as _, Result, anyhow, bail};
use lee_core::account::{AccountId, Actor};
use serde_json::Value;

use crate::{WriteOpts, config::Resolved as Net};

#[derive(clap::Subcommand)]
pub enum ModuleCmd {
    /// Read a prepared change (JSON) on stdin, check it, then simulate, sign, send and confirm it.
    Send {
        #[command(flatten)]
        opts: WriteOpts,
    },
}

/// A change as the module prepared it.
pub struct Change {
    pub root: AccountId,
    pub message: Vec<u8>,
    /// Every declared actor after the root, in order.
    pub accounts: Vec<Actor>,
    pub signers: Vec<AccountId>,
    pub entry: Option<AccountId>,
}

fn account(v: &Value, what: &str) -> Result<AccountId> {
    let s = v.as_str().ok_or_else(|| anyhow!("{what} is missing"))?;
    s.parse().map_err(|_| anyhow!("{what} '{s}' is not an account id"))
}

/// Parses `text` as a change for this network: its sequencer and registry must be the ones
/// lezreg is pointed at (`--rpc`/`--registry`, which the module passes).
pub fn parse_change(text: &str, rpc: &str, registry: &AccountId) -> Result<Change> {
    let v: Value = serde_json::from_str(text).context("the change is not JSON")?;
    if v["format"] != "lezreg-change/1" {
        bail!("not a lezreg change (format lezreg-change/1)");
    }
    if account(&v["registry"], "registry")? != *registry {
        bail!("the change is for registry {}, but lezreg is pointed at {registry}", v["registry"]);
    }
    let seq = v["sequencer"].as_str().unwrap_or_default();
    if seq.trim_end_matches('/') != rpc.trim_end_matches('/') {
        bail!("the change was prepared against sequencer {seq}, but lezreg is pointed at {rpc}");
    }
    let list = v["accounts"].as_array().ok_or_else(|| anyhow!("accounts is missing"))?;
    let root_at = usize::try_from(v["root"].as_u64().ok_or_else(|| anyhow!("root is missing"))?)?;
    let mut actors = Vec::new();
    let mut signers = Vec::new();
    for (i, a) in list.iter().enumerate() {
        let id = account(&a["account"], "an account")?;
        let program = account(&a["program"], "an account's program")?;
        if program != *registry {
            bail!("account {i} is mentioned for program {program}, not the registry");
        }
        if a["signs"].as_bool() == Some(true) && !signers.contains(&id) {
            signers.push(id);
        }
        actors.push(Actor::new(id, program));
    }
    let root = actors.get(root_at).ok_or_else(|| anyhow!("root {root_at} is not one of the accounts"))?.account_id;
    actors.remove(root_at);
    let message = hex::decode(v["message"].as_str().unwrap_or_default()).context("message is not hex")?;
    let entry = match &v["entry"] {
        Value::Null => None,
        e => Some(account(e, "entry")?),
    };
    Ok(Change { root, message, accounts: actors, signers, entry })
}

/// Reads the change from stdin.
pub fn read_change(net: &Net) -> Result<Change> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).context("cannot read the change on stdin")?;
    let registry = lezreg_sdk::tx::lid(&net.registry()?);
    parse_change(&text, &net.rpc()?, &registry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_for_another_network_or_program_is_refused() {
        let reg = AccountId::new([9; 32]);
        let a = AccountId::new([1; 32]);
        let good = serde_json::json!({
            "format": "lezreg-change/1", "sequencer": "http://127.0.0.1:3062", "registry": reg.to_string(), "root": 0,
            "accounts": [{"account": a.to_string(), "program": reg.to_string(), "signs": true},
                         {"account": AccountId::new([2; 32]).to_string(), "program": reg.to_string(), "signs": false}],
            "message": "04", "entry": AccountId::new([2; 32]).to_string(),
        });
        let c = parse_change(&good.to_string(), "http://127.0.0.1:3062/", &reg).unwrap();
        assert_eq!((c.root, c.signers.clone(), c.accounts.len(), c.message.clone()), (a, vec![a], 1, vec![4]));
        assert!(parse_change(&good.to_string(), "http://127.0.0.1:9999", &reg).is_err());
        assert!(parse_change(&good.to_string(), "http://127.0.0.1:3062", &a).is_err());
        let mut other = good.clone();
        other["accounts"][1]["program"] = serde_json::json!(a.to_string());
        assert!(parse_change(&other.to_string(), "http://127.0.0.1:3062", &reg).is_err());
        let mut fmt = good;
        fmt["format"] = serde_json::json!("x");
        assert!(parse_change(&fmt.to_string(), "http://127.0.0.1:3062", &reg).is_err());
    }
}
