//! The two sequencer reads discovery needs (DESIGN §2.5): `getAccount` (the whole account,
//! every actor state) and `getAccountView` (one actor). Plain JSON-RPC over HTTP with no LEZ
//! dependency, so read-only users link only this crate and `lezreg-types`.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use lezreg_types::AccountId;
use serde_json::{Value, json};

use crate::error::{Error, Result};

/// An account as `getAccount` returns it: its nonce and every program's actor state on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountStates {
    pub nonce: u128,
    pub actor_states: BTreeMap<AccountId, Vec<u8>>,
}

impl AccountStates {
    /// `program`'s state on this account; empty if none.
    #[must_use]
    pub fn state(&self, program: &AccountId) -> &[u8] {
        self.actor_states.get(program).map_or(&[], Vec::as_slice)
    }
}

/// A sequencer the SDK reads from.
pub trait Rpc {
    /// `getAccount(id)`: `None` if the account does not exist.
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>>;
    /// `getAccountView(Actor(account, program))`: that actor's state, empty if none.
    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>>;
}

impl<T: Rpc + ?Sized> Rpc for &T {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        (**self).get_account(id)
    }
    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        (**self).get_actor_state(account, program)
    }
}

/// JSON-RPC over HTTP to a LEZ sequencer. Talks to the sequencer the user chose and nothing
/// else.
pub struct HttpRpc {
    url: String,
    agent: ureq::Agent,
    calls: AtomicUsize,
}

impl HttpRpc {
    #[must_use]
    pub fn new(url: &str) -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build();
        Self { url: url.to_owned(), agent: config.into(), calls: AtomicUsize::new(0) }
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// How many RPC calls this client has made.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    /// One JSON-RPC call.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let transport = |message: String| Error::Transport { url: self.url.clone(), message };
        let mut response = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| transport(e.to_string()))?;
        let text = response.body_mut().read_to_string().map_err(|e| transport(e.to_string()))?;
        let reply: Value = serde_json::from_str(&text).map_err(|_| transport(format!("not JSON: {}", truncate(&text, 200))))?;
        if let Some(error) = reply.get("error") {
            return Err(Error::Rpc { method: method.to_owned(), message: error.to_string() });
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Parses `{"nonce": n, "data": {"actor_states": {"<base58 program>": [bytes…]}}}`.
pub fn parse_account(v: &Value) -> Result<Option<AccountStates>> {
    if v.is_null() {
        return Ok(None);
    }
    let bad = |m: &str| Error::Rpc { method: "getAccount".into(), message: format!("unexpected account shape: {m}") };
    let nonce = match v.get("nonce") {
        Some(Value::Number(n)) => n.as_u64().map(u128::from).ok_or_else(|| bad("nonce"))?,
        Some(Value::String(s)) => s.parse().map_err(|_| bad("nonce"))?,
        _ => 0,
    };
    let mut actor_states = BTreeMap::new();
    if let Some(states) = v.get("data").and_then(|d| d.get("actor_states")).and_then(Value::as_object) {
        for (program, bytes) in states {
            let program = AccountId::from_base58(program).ok_or_else(|| bad("program id"))?;
            let bytes = bytes
                .as_array()
                .ok_or_else(|| bad("state bytes"))?
                .iter()
                .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()).ok_or_else(|| bad("state byte")))
                .collect::<Result<Vec<u8>>>()?;
            actor_states.insert(program, bytes);
        }
    }
    Ok(Some(AccountStates { nonce, actor_states }))
}

impl Rpc for HttpRpc {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        parse_account(&self.call("getAccount", json!([id.to_base58()]))?)
    }

    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        let actor = json!([{"account_id": account.to_base58(), "program_account_id": program.to_base58()}]);
        let found = parse_account(&self.call("getAccountView", actor)?)?;
        Ok(found.map(|a| a.state(program).to_vec()).unwrap_or_default())
    }
}

/// An in-memory sequencer for tests: actor states set by hand, every read counted.
#[derive(Default)]
pub struct MockRpc {
    states: Mutex<HashMap<AccountId, AccountStates>>,
    reads: AtomicUsize,
    /// When set, every read fails with this transport error.
    pub fail: Mutex<Option<String>>,
}

impl MockRpc {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `program`'s actor state on `account`.
    pub fn set(&self, account: AccountId, program: AccountId, state: Vec<u8>) {
        let mut s = self.states.lock().expect("lock");
        s.entry(account).or_default().actor_states.insert(program, state);
    }

    /// Sets a borsh record as `program`'s actor state on `account`.
    pub fn put<T: borsh::BorshSerialize>(&self, account: AccountId, program: AccountId, record: &T) {
        self.set(account, program, borsh::to_vec(record).expect("borsh"));
    }

    /// Reads so far.
    #[must_use]
    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::Relaxed)
    }

    pub fn reset_reads(&self) {
        self.reads.store(0, Ordering::Relaxed);
    }

    fn check(&self) -> Result<()> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        match self.fail.lock().expect("lock").clone() {
            Some(message) => Err(Error::Transport { url: "mock".into(), message }),
            None => Ok(()),
        }
    }
}

impl Rpc for MockRpc {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        self.check()?;
        Ok(self.states.lock().expect("lock").get(id).cloned())
    }

    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        self.check()?;
        Ok(self.states.lock().expect("lock").get(account).map(|a| a.state(program).to_vec()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_rc1_account_shape() {
        // Captured from the rc1 sequencer's getAccount.
        let v: Value = serde_json::from_str(
            r#"{"nonce":22,"data":{"actor_states":{"11111111111111111111111111111111":[16,162,160,231,23,9,0,0,0,0,0,0,0,0,0,0]}}}"#,
        )
        .unwrap();
        let a = parse_account(&v).unwrap().unwrap();
        assert_eq!(a.nonce, 22);
        assert_eq!(a.state(&AccountId([0; 32])).len(), 16);
        assert!(a.state(&AccountId([1; 32])).is_empty());
        assert_eq!(parse_account(&Value::Null).unwrap(), None);
    }

    #[test]
    fn bad_shapes_are_typed_errors() {
        let v: Value = serde_json::from_str(r#"{"nonce":1,"data":{"actor_states":{"!!":[1]}}}"#).unwrap();
        assert!(matches!(parse_account(&v), Err(Error::Rpc { .. })));
        let v: Value = serde_json::from_str(r#"{"nonce":1,"data":{"actor_states":{"11111111111111111111111111111111":[300]}}}"#).unwrap();
        assert!(matches!(parse_account(&v), Err(Error::Rpc { .. })));
    }
}
