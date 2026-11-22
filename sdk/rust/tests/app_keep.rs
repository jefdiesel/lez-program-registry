//! Keeping the newest release of an app on a Storage node (`apps::keep_copy`, `lezreg app sync`)
//! and the downgrade guard (`apps::check_not_older`), against a local fake Storage node.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use lezreg_sdk::{
    StorageClient,
    apps::{self, AppRecord, Kept, Release},
    lgx::Contents,
};

const CID: &str = "zDvZRwzkxEWHPy52PzMMPhZyWAR4CAJpSvACtXhGSaksUj4eqR7f";

fn review3(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/apps/review3/{name}.lgx", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn release(bytes: &[u8]) -> Release {
    let c = Contents::from_lgx(bytes).unwrap();
    Release { root: c.root, sha256: Some(lezreg_types::pda::sha256(bytes)), manifest: Some(c.manifest_sha256), version: Some(c.manifest.version) }
}

/// A fake node: `local` is what `GET /data/{cid}` holds (None: 404), `network` what a network
/// stream delivers. Every request line is logged.
struct Node {
    local: Option<Vec<u8>>,
    network: Option<Vec<u8>>,
    log: Vec<String>,
}

fn fake_node(local: Option<Vec<u8>>, network: Option<Vec<u8>>) -> (String, Arc<Mutex<Node>>) {
    let state = Arc::new(Mutex::new(Node { local, network, log: Vec::new() }));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/api/storage/v1", listener.local_addr().unwrap());
    let s = state.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_owned();
            let mut node = s.lock().unwrap();
            node.log.push(line.clone());
            let body = if line.starts_with("GET ") && line.contains("/network/stream") {
                let got = node.network.clone();
                if got.is_some() {
                    node.local = got.clone();
                }
                got
            } else if line.starts_with("GET ") && line.contains(&format!("/data/{CID} ")) {
                node.local.clone()
            } else if line.starts_with("DELETE ") {
                node.local = None;
                Some(Vec::new())
            } else if line.starts_with("POST ") && line.contains("/advertise") {
                Some(Vec::new())
            } else {
                None
            };
            let mut out = match &body {
                Some(b) => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len()).into_bytes(),
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            };
            out.extend_from_slice(body.as_deref().unwrap_or_default());
            let _ = stream.write_all(&out);
        }
    });
    (url, state)
}

fn record_of(bytes: &[u8]) -> AppRecord {
    AppRecord { releases: vec![release(bytes)], cid: Some(CID.into()), ..AppRecord::default() }
}

#[test]
fn a_release_not_on_the_node_is_fetched_checked_and_announced() {
    let new = review3("new");
    let (url, node) = fake_node(None, Some(new.clone()));
    let (kept, c) = apps::keep_copy(&StorageClient::new(&url), "forumer_ui", &["forumer"], &record_of(&new)).unwrap();
    assert_eq!(kept, Kept::Fetched);
    assert_eq!(c.manifest.name, "forumer_ui");
    let log = node.lock().unwrap().log.clone();
    assert!(log.iter().any(|l| l.contains("/network/stream?advertise=true")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("/advertise?advertise=true")), "announced: {log:?}");
}

#[test]
fn a_release_already_held_is_checked_not_fetched() {
    let new = review3("new");
    let (url, node) = fake_node(Some(new.clone()), None);
    let (kept, _) = apps::keep_copy(&StorageClient::new(&url), "forumer_ui", &["forumer"], &record_of(&new)).unwrap();
    assert_eq!(kept, Kept::AlreadyThere);
    let log = node.lock().unwrap().log.clone();
    assert!(!log.iter().any(|l| l.contains("/network/stream")), "no network fetch: {log:?}");
}

#[test]
fn a_download_that_is_not_the_release_is_dropped_and_not_announced() {
    let (new, old) = (review3("new"), review3("old"));
    // The network hands out the old package under the CID the record gives for the new one.
    let (url, node) = fake_node(None, Some(old));
    let e = apps::keep_copy(&StorageClient::new(&url), "forumer_ui", &["forumer"], &record_of(&new)).unwrap_err().to_string();
    assert!(e.contains("not the one the record vouches for"), "{e}");
    let node = node.lock().unwrap();
    assert!(node.local.is_none(), "the bad copy is dropped");
    assert!(node.log.iter().any(|l| l.starts_with("DELETE ")));
    assert!(!node.log.iter().any(|l| l.contains("/advertise")), "nothing announced: {:?}", node.log);
}

#[test]
fn a_record_without_cid_or_sha256_keeps_nothing() {
    let new = review3("new");
    let (url, node) = fake_node(None, Some(new.clone()));
    let node_client = StorageClient::new(&url);
    let mut r = record_of(&new);
    r.cid = None;
    assert!(apps::keep_copy(&node_client, "forumer_ui", &["forumer"], &r).unwrap_err().to_string().contains("no Logos Storage CID"));
    let mut r = record_of(&new);
    r.releases[0].sha256 = None;
    assert!(apps::keep_copy(&node_client, "forumer_ui", &["forumer"], &r).unwrap_err().to_string().contains("no sha256"));
    assert!(node.lock().unwrap().log.is_empty(), "the node is not asked");
}

#[test]
fn an_older_release_than_installed_needs_saying_so() {
    assert!(apps::check_not_older("vesting_ui", Some("0.6.3"), Some("0.6.3"), false).is_ok());
    assert!(apps::check_not_older("vesting_ui", Some("0.6.10"), Some("0.6.9"), false).is_ok(), "numbers, not text");
    assert!(apps::check_not_older("vesting_ui", Some("0.6.3"), None, false).is_ok(), "nothing installed");
    let e = apps::check_not_older("vesting_ui", Some("0.6.2"), Some("0.6.3"), false).unwrap_err().to_string();
    assert!(e.contains("0.6.2") && e.contains("older than the installed 0.6.3"), "{e}");
    assert!(apps::check_not_older("vesting_ui", Some("0.6.2"), Some("0.6.3"), true).is_ok(), "allowed explicitly");
    assert!(apps::check_not_older("vesting_ui", Some("1.0.0-rc.1"), Some("1.0.0"), false).is_err(), "a pre-release is older");
}
