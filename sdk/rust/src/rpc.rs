//! The two sequencer reads discovery needs (DESIGN §2.5): `getAccount` (the whole account,
//! every actor state) and `getAccountView` (one actor). Plain JSON-RPC over HTTP with no LEZ
//! dependency, so read-only users link only this crate and `lezreg-types`.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    /// Many actor states at once, in order. The default reads them one by one;
    /// [`HttpRpc`] sends them as JSON-RPC batches.
    fn get_actor_states(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        actors
            .iter()
            .map(|(a, p)| self.get_actor_state(a, p))
            .collect()
    }
}

impl<T: Rpc + ?Sized> Rpc for &T {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        (**self).get_account(id)
    }
    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        (**self).get_actor_state(account, program)
    }
    fn get_actor_states(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        (**self).get_actor_states(actors)
    }
}

/// How many reads go in one JSON-RPC batch.
pub const BATCH: usize = 100;

/// How many single calls run at once when an endpoint refuses batches.
pub const SINGLE_CONCURRENCY: usize = 4;

/// HTTP statuses with which a filter in front of a sequencer refuses a batch (a body or batch
/// cap, a method allowlist, a front end that serves single calls only). Anything else that is not
/// a success, a 5xx above all, is an error, never a reason to retry one by one.
const BATCH_REFUSED_STATUSES: [u16; 4] = [400, 403, 405, 413];

/// What became of one JSON-RPC batch.
enum BatchOutcome {
    Done(Vec<Value>),
    /// The endpoint refused the batch as such; the same calls may go one by one.
    Refused(String),
}

/// JSON-RPC over HTTP to a LEZ sequencer. Talks to the sequencer the user chose and nothing
/// else: redirects are refused, and `HTTP(S)_PROXY`/`ALL_PROXY` are ignored unless
/// `LEZREG_USE_PROXY=1` is set (a proxy the user did not pick for lezreg is someone else
/// seeing which programs they look up).
pub struct HttpRpc {
    url: String,
    agent: ureq::Agent,
    calls: AtomicUsize,
    /// Set once this endpoint has refused a batch: later reads go one by one at once.
    batches_refused: AtomicBool,
}

impl HttpRpc {
    #[must_use]
    pub fn new(url: &str) -> Self {
        let proxy = if std::env::var("LEZREG_USE_PROXY").is_ok_and(|v| v == "1") {
            ureq::Proxy::try_from_env()
        } else {
            None
        };
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .max_redirects(0)
            .proxy(proxy)
            // Statuses are read here: a refused batch (400/403/405/413) is told apart from an
            // error.
            .http_status_as_error(false)
            .build();
        Self {
            url: url.to_owned(),
            agent: config.into(),
            calls: AtomicUsize::new(0),
            batches_refused: AtomicBool::new(false),
        }
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

    /// Whether this endpoint has refused a batch during this session.
    #[must_use]
    pub fn batches_refused(&self) -> bool {
        self.batches_refused.load(Ordering::Relaxed)
    }

    fn transport(&self, message: String) -> Error {
        Error::Transport {
            url: self.url.clone(),
            message,
        }
    }

    /// POSTs `body`; the HTTP status and the body text. Errors only when nothing usable came back.
    fn post_raw(&self, body: &Value) -> Result<(u16, String)> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut response = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| self.transport(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| self.transport(e.to_string()))?;
        Ok((status, text))
    }

    fn parse(&self, text: &str) -> Result<Value> {
        serde_json::from_str(text)
            .map_err(|_| self.transport(format!("not JSON: {}", truncate(text, 200))))
    }

    fn post(&self, body: &Value) -> Result<Value> {
        let (status, text) = self.post_raw(body)?;
        if !(200..300).contains(&status) {
            return Err(self.transport(format!("HTTP {status}: {}", truncate(&text, 200))));
        }
        self.parse(&text)
    }

    /// One JSON-RPC call.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let reply =
            self.post(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))?;
        result_of(method, &reply)
    }

    /// Several calls of one method in a JSON-RPC batch; the results in order. An endpoint that
    /// refuses batches gives an [`Error::Rpc`] (see [`Rpc::get_actor_states`] for the fallback).
    pub fn batch(&self, method: &str, params: &[Value]) -> Result<Vec<Value>> {
        match self.try_batch(method, params)? {
            BatchOutcome::Done(results) => Ok(results),
            BatchOutcome::Refused(message) => Err(Error::Rpc {
                method: method.into(),
                message: format!("batch refused: {message}"),
            }),
        }
    }

    fn try_batch(&self, method: &str, params: &[Value]) -> Result<BatchOutcome> {
        if params.is_empty() {
            return Ok(BatchOutcome::Done(Vec::new()));
        }
        let body: Vec<Value> = params
            .iter()
            .enumerate()
            .map(|(i, p)| json!({"jsonrpc": "2.0", "id": i, "method": method, "params": p}))
            .collect();
        let (status, text) = self.post_raw(&Value::Array(body))?;
        if BATCH_REFUSED_STATUSES.contains(&status) {
            return Ok(BatchOutcome::Refused(format!(
                "HTTP {status}: {}",
                truncate(&text, 200)
            )));
        }
        if !(200..300).contains(&status) {
            return Err(self.transport(format!("HTTP {status}: {}", truncate(&text, 200))));
        }
        let reply = self.parse(&text)?;
        let Value::Array(replies) = reply else {
            // A sequencer without batch support answers with one error object.
            return Ok(BatchOutcome::Refused(match reply.get("error") {
                Some(error) => error.to_string(),
                None => "batch answered with one object".into(),
            }));
        };
        let mut out = vec![None; params.len()];
        for r in &replies {
            let i = r
                .get("id")
                .and_then(Value::as_u64)
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| *i < params.len());
            let Some(i) = i else {
                return Err(Error::Rpc {
                    method: method.into(),
                    message: "batch reply with an unknown id".into(),
                });
            };
            out[i] = Some(result_of(method, r)?);
        }
        out.into_iter()
            .map(|v| {
                v.ok_or_else(|| Error::Rpc {
                    method: method.into(),
                    message: "batch reply missing an item".into(),
                })
            })
            .collect::<Result<Vec<Value>>>()
            .map(BatchOutcome::Done)
    }

    /// The same reads one by one, [`SINGLE_CONCURRENCY`] at a time, in order.
    fn actor_states_one_by_one(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::with_capacity(actors.len());
        for group in actors.chunks(SINGLE_CONCURRENCY) {
            let results: Vec<Result<Vec<u8>>> = std::thread::scope(|scope| {
                let handles: Vec<_> = group
                    .iter()
                    .map(|(a, p)| scope.spawn(move || self.get_actor_state(a, p)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join().unwrap_or_else(|_| {
                            Err(self.transport("a read thread panicked".into()))
                        })
                    })
                    .collect()
            });
            for r in results {
                out.push(r?);
            }
        }
        Ok(out)
    }

    fn note_batches_refused(&self, why: &str) {
        if !self.batches_refused.swap(true, Ordering::Relaxed) {
            eprintln!(
                "note: {} refuses JSON-RPC batches ({}); reading one call at a time, so listing is slower on this endpoint",
                self.url,
                truncate(why, 120)
            );
        }
    }
}

fn result_of(method: &str, reply: &Value) -> Result<Value> {
    if let Some(error) = reply.get("error") {
        return Err(Error::Rpc {
            method: method.to_owned(),
            message: error.to_string(),
        });
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

fn actor_param(account: &AccountId, program: &AccountId) -> Value {
    json!([{"account_id": account.to_base58(), "program_account_id": program.to_base58()}])
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
    let bad = |m: &str| Error::Rpc {
        method: "getAccount".into(),
        message: format!("unexpected account shape: {m}"),
    };
    let nonce = match v.get("nonce") {
        Some(Value::Number(n)) => n.as_u64().map(u128::from).ok_or_else(|| bad("nonce"))?,
        Some(Value::String(s)) => s.parse().map_err(|_| bad("nonce"))?,
        _ => 0,
    };
    let mut actor_states = BTreeMap::new();
    if let Some(states) = v
        .get("data")
        .and_then(|d| d.get("actor_states"))
        .and_then(Value::as_object)
    {
        for (program, bytes) in states {
            let program = AccountId::from_base58(program).ok_or_else(|| bad("program id"))?;
            let bytes = bytes
                .as_array()
                .ok_or_else(|| bad("state bytes"))?
                .iter()
                .map(|b| {
                    b.as_u64()
                        .and_then(|b| u8::try_from(b).ok())
                        .ok_or_else(|| bad("state byte"))
                })
                .collect::<Result<Vec<u8>>>()?;
            actor_states.insert(program, bytes);
        }
    }
    Ok(Some(AccountStates {
        nonce,
        actor_states,
    }))
}

impl Rpc for HttpRpc {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        parse_account(&self.call("getAccount", json!([id.to_base58()]))?)
    }

    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        let found = parse_account(&self.call("getAccountView", actor_param(account, program))?)?;
        Ok(found.map(|a| a.state(program).to_vec()).unwrap_or_default())
    }

    /// In JSON-RPC batches of [`BATCH`]. An endpoint that refuses a batch (HTTP 400, 403, 405
    /// or 413, or one JSON-RPC error object in place of the batch reply) gets the same calls one
    /// by one, and every later read of this session too; a network error or a 5xx is an error.
    fn get_actor_states(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::with_capacity(actors.len());
        for chunk in actors.chunks(BATCH) {
            if self.batches_refused() {
                out.extend(self.actor_states_one_by_one(chunk)?);
                continue;
            }
            let params: Vec<Value> = chunk.iter().map(|(a, p)| actor_param(a, p)).collect();
            match self.try_batch("getAccountView", &params)? {
                BatchOutcome::Done(results) => {
                    for ((_, p), v) in chunk.iter().zip(results) {
                        out.push(
                            parse_account(&v)?
                                .map(|a| a.state(p).to_vec())
                                .unwrap_or_default(),
                        );
                    }
                }
                BatchOutcome::Refused(why) => {
                    self.note_batches_refused(&why);
                    out.extend(self.actor_states_one_by_one(chunk)?);
                }
            }
        }
        Ok(out)
    }
}

/// An in-memory sequencer for tests: actor states set by hand, every read counted.
#[derive(Default)]
pub struct MockRpc {
    states: Mutex<HashMap<AccountId, AccountStates>>,
    reads: AtomicUsize,
    /// When set, every read fails with this transport error.
    pub fail: Mutex<Option<String>>,
    /// When set, `getAccount` answers with this JSON-RPC error (a sequencer without it).
    pub no_get_account: Mutex<bool>,
}

impl MockRpc {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets `program`'s actor state on `account`.
    pub fn set(&self, account: AccountId, program: AccountId, state: Vec<u8>) {
        let mut s = self.states.lock().expect("lock");
        s.entry(account)
            .or_default()
            .actor_states
            .insert(program, state);
    }

    /// Sets a borsh record as `program`'s actor state on `account`.
    pub fn put<T: borsh::BorshSerialize>(
        &self,
        account: AccountId,
        program: AccountId,
        record: &T,
    ) {
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
            Some(message) => Err(Error::Transport {
                url: "mock".into(),
                message,
            }),
            None => Ok(()),
        }
    }
}

impl Rpc for MockRpc {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        self.check()?;
        if *self.no_get_account.lock().expect("lock") {
            return Err(Error::Rpc {
                method: "getAccount".into(),
                message: "method not found".into(),
            });
        }
        Ok(self.states.lock().expect("lock").get(id).cloned())
    }

    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        self.check()?;
        Ok(self
            .states
            .lock()
            .expect("lock")
            .get(account)
            .map(|a| a.state(program).to_vec())
            .unwrap_or_default())
    }

    /// One read per batch of [`BATCH`], like [`HttpRpc`].
    fn get_actor_states(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for chunk in actors.chunks(BATCH) {
            self.check()?;
            let s = self.states.lock().expect("lock");
            out.extend(
                chunk
                    .iter()
                    .map(|(a, p)| s.get(a).map(|x| x.state(p).to_vec()).unwrap_or_default()),
            );
        }
        Ok(out)
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
        let v: Value =
            serde_json::from_str(r#"{"nonce":1,"data":{"actor_states":{"!!":[1]}}}"#).unwrap();
        assert!(matches!(parse_account(&v), Err(Error::Rpc { .. })));
        let v: Value = serde_json::from_str(
            r#"{"nonce":1,"data":{"actor_states":{"11111111111111111111111111111111":[300]}}}"#,
        )
        .unwrap();
        assert!(matches!(parse_account(&v), Err(Error::Rpc { .. })));
    }
}
