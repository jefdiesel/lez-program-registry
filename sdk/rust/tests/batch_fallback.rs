//! `HttpRpc` against a mock endpoint that refuses JSON-RPC batches the way a public filter in
//! front of a sequencer does: the same reads go one by one, the endpoint is remembered as
//! batch-refusing for the session, and a network error or a 5xx is never taken for a refusal.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use lezreg_sdk::{Error, rpc::HttpRpc, rpc::Rpc};
use lezreg_types::AccountId;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Batches {
    /// Answered like a sequencer: an array of results.
    Serve,
    /// HTTP 400 with a JSON-RPC error body (the kit filter with --max-batch 0, an oversize batch).
    Refuse400,
    /// HTTP 413.
    Refuse413,
    /// HTTP 200 with one JSON-RPC error object instead of an array.
    RefuseObject,
    /// HTTP 502 (the front end can't reach the sequencer).
    Fail502,
    /// The connection closes without an answer.
    Drop,
}

struct Mock {
    url: String,
    batches: Arc<AtomicUsize>,
    singles: Arc<AtomicUsize>,
}

const PROGRAM: AccountId = AccountId([0xEE; 32]);

fn account(n: u8) -> AccountId {
    AccountId([n; 32])
}

/// A `getAccountView` result whose actor state at PROGRAM is `[n]`.
fn view(account_b58: &str) -> Value {
    let n = AccountId::from_base58(account_b58).expect("account id").0[0];
    json!({"nonce": 0, "data": {"actor_states": {PROGRAM.to_base58(): [n]}}})
}

fn answer(call: &Value) -> Value {
    let account = call["params"][0]["account_id"].as_str().expect("account_id");
    json!({"jsonrpc": "2.0", "id": call["id"], "result": view(account)})
}

fn mock(mode: Batches) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/", listener.local_addr().expect("addr"));
    let batches = Arc::new(AtomicUsize::new(0));
    let singles = Arc::new(AtomicUsize::new(0));
    let (b, s) = (batches.clone(), singles.clone());
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (b, s) = (b.clone(), s.clone());
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let lower = line.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).expect("body");
                let request: Value = serde_json::from_slice(&body).expect("json");
                let (status, reply) = if let Value::Array(calls) = &request {
                    b.fetch_add(1, Ordering::SeqCst);
                    let refusal = json!({"jsonrpc": "2.0", "id": null,
                        "error": {"code": -32600, "message": "batches are not served; send one request"}});
                    match mode {
                        Batches::Serve => (200, Value::Array(calls.iter().map(answer).collect())),
                        Batches::Refuse400 => (400, refusal),
                        Batches::Refuse413 => (413, refusal),
                        Batches::RefuseObject => (200, refusal),
                        Batches::Fail502 => (502, json!({"error": "sequencer unavailable"})),
                        Batches::Drop => return,
                    }
                } else {
                    s.fetch_add(1, Ordering::SeqCst);
                    (200, answer(&request))
                };
                let text = reply.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
    Mock {
        url,
        batches,
        singles,
    }
}

fn actors(n: u8) -> Vec<(AccountId, AccountId)> {
    (1..=n).map(|i| (account(i), PROGRAM)).collect()
}

fn expected(n: u8) -> Vec<Vec<u8>> {
    (1..=n).map(|i| vec![i]).collect()
}

#[test]
fn batches_are_used_when_served() {
    let m = mock(Batches::Serve);
    let rpc = HttpRpc::new(&m.url);
    assert_eq!(rpc.get_actor_states(&actors(7)).unwrap(), expected(7));
    assert_eq!(m.batches.load(Ordering::SeqCst), 1);
    assert_eq!(m.singles.load(Ordering::SeqCst), 0);
    assert!(!rpc.batches_refused());
}

#[test]
fn refused_batches_fall_back_to_single_calls_and_are_remembered() {
    for mode in [Batches::Refuse400, Batches::Refuse413, Batches::RefuseObject] {
        let m = mock(mode);
        let rpc = HttpRpc::new(&m.url);
        assert_eq!(rpc.get_actor_states(&actors(9)).unwrap(), expected(9));
        assert_eq!(m.batches.load(Ordering::SeqCst), 1);
        assert_eq!(m.singles.load(Ordering::SeqCst), 9);
        assert!(rpc.batches_refused());
        // The session remembers: no second batch is tried.
        assert_eq!(rpc.get_actor_states(&actors(3)).unwrap(), expected(3));
        assert_eq!(m.batches.load(Ordering::SeqCst), 1);
        assert_eq!(m.singles.load(Ordering::SeqCst), 12);
    }
}

#[test]
fn a_5xx_or_a_network_error_is_not_a_refusal() {
    for mode in [Batches::Fail502, Batches::Drop] {
        let m = mock(mode);
        let rpc = HttpRpc::new(&m.url);
        let err = rpc.get_actor_states(&actors(4)).unwrap_err();
        assert!(matches!(err, Error::Transport { .. }), "{err:?}");
        assert_eq!(m.singles.load(Ordering::SeqCst), 0);
        assert!(!rpc.batches_refused());
    }
}
