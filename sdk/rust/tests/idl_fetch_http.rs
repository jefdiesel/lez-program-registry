//! Four IDL failure cases against a local fake Storage node. No live chain,
//! no Logos Storage process, no Docker. These are the offline equivalents of
//! demo rows R1.a, R1.b, R1.c and R1.d.

use lezreg_sdk::{FetchConfig, IdlStatus, Unavailable, cid, idl, json as sdk_json, storage};
use lezreg_types::ContentRef;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

#[derive(Clone)]
enum Reply {
    NotFound,
    Bytes(Vec<u8>),
}

fn fake_node(reply: Reply) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/api/storage/v1", listener.local_addr().expect("addr"));
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]).to_string();
            let (status, text) = if request.contains("/debug/info") {
                (200, json!({"id": "12D3KooWFake", "addrs": []}).to_string())
            } else if request.contains("/network/stream") {
                match reply {
                    Reply::NotFound => (404, "not found".to_string()),
                    Reply::Bytes(ref bytes) => {
                        let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: ".to_vec();
                        response.extend_from_slice(bytes.len().to_string().as_bytes());
                        response.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
                        response.extend_from_slice(&bytes);
                        let _ = stream.write_all(&response);
                        continue;
                    }
                }
            } else {
                (404, "not found".to_string())
            };
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    url
}

fn config(api: String) -> FetchConfig {
    FetchConfig {
        storage_api: Some(api),
        cache_dir: None,
        byo_dirs: Vec::new(),
        pin_peers: Vec::new(),
        backoff: Vec::new(),
        timeouts: storage::Timeouts {
            manifest: Duration::from_millis(500),
            stream: Duration::from_millis(500),
            dial: Duration::from_millis(500),
        },
    }
}

fn reference(bytes: &[u8]) -> ContentRef {
    ContentRef {
        cid: cid::raw_cid(bytes),
        sha256: lezreg_types::pda::sha256(bytes),
        size: bytes.len() as u32,
        peer_hint: None,
    }
}

#[test]
fn r1a_no_storage_node_is_unavailable() {
    // Port 1 is closed on this machine; a fresh node also fails its info call.
    let status = idl::fetch_idl(&reference(b"{}"), &config("http://127.0.0.1:1/".into()));
    assert_eq!(status.label(), "unavailable");
    assert!(matches!(
        status,
        IdlStatus::Unavailable(Unavailable::NoStorageNode(_))
    ));
    assert_eq!(sdk_json::idl_status(&status)["status"], json!("unavailable"));
}

#[test]
fn r1b_unknown_cid_is_unavailable() {
    let url = fake_node(Reply::NotFound);
    let status = idl::fetch_idl(&reference(b"{\"version\":\"2\"}"), &config(url));
    assert_eq!(status.label(), "unavailable");
    assert!(matches!(
        status,
        IdlStatus::Unavailable(Unavailable::NotFound(_))
    ));
}

#[test]
fn r1c_not_json_is_parse_error() {
    let bytes = b"this is not an IDL".to_vec();
    let url = fake_node(Reply::Bytes(bytes.clone()));
    let status = idl::fetch_idl(&reference(&bytes), &config(url));
    assert_eq!(status.label(), "unparseable");
    assert!(matches!(status, IdlStatus::ParseError(_)));
    assert_eq!(sdk_json::idl_status(&status)["status"], json!("unparseable"));
}

#[test]
fn r1d_hash_mismatch_is_reported_before_parsing() {
    let bytes = b"{\"version\":\"2\",\"messages\":[]}".to_vec();
    let url = fake_node(Reply::Bytes(bytes.clone()));
    let mut r = reference(&bytes);
    r.sha256 = [0u8; 32]; // the chain records someone else's hash
    let status = idl::fetch_idl(&r, &config(url));
    assert_eq!(status.label(), "hash-mismatch");
    assert!(matches!(status, IdlStatus::HashMismatch { .. }));
    assert_eq!(sdk_json::idl_status(&status)["status"], json!("hash-mismatch"));
}
