//! Rust ↔ JS vectors (DESIGN §7.1, M7): the JS SDK (`sdk/js`) must derive the same addresses
//! and decode the same records as this crate. `LEZREG_WRITE_VECTORS=1 cargo test -p lezreg-sdk
//! --no-default-features --test js_vectors` rewrites `sdk/js/vectors.json`; without it the test
//! checks the committed file is current.

use lezreg_types::{
    AccountId, BuildConfig, ContentRef, Entry, EnvVar, Head, Link, Release, SourceRef,
    pda::{addr, build_hash, sha256},
};
use serde_json::{Value, json};

fn id(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn entry() -> Entry {
    Entry {
        schema: 1,
        kind: 0,
        flags: 2,
        program: id(7),
        author: id(8),
        registrant: id(9),
        name: "token-é".into(),
        version: "1.2.0".into(),
        author_label: "Acme Labs".into(),
        description: "A fungible token\nwith two lines".into(),
        tags: vec!["token".into(), "defi".into()],
        idl: ContentRef {
            cid: vec![1, 0x55, 0x12, 0x20, 3, 4],
            sha256: [0x11; 32],
            size: 18282,
            peer_hint: Some(vec![0, 36, 8, 2]),
        },
        source: SourceRef {
            repo: "https://github.com/acme/token".into(),
            commit: vec![0xab; 20],
            docker_root: ".".into(),
            archive: Some(ContentRef {
                cid: vec![1, 0x55],
                sha256: [0x22; 32],
                size: 9,
                peer_hint: None,
            }),
        },
        build: BuildConfig {
            schema: 1,
            builder: 1,
            image_digest: [0xfa; 32],
            image_tag: "r0.1.91.1".into(),
            risc0_build: "3.0.5".into(),
            host_risc0_rust: "1.91.1".into(),
            manifest_path: "methods/guest/Cargo.toml".into(),
            binary: "token".into(),
            features: vec!["x".into()],
            lock_sha256: [0x33; 32],
            docker_env: vec![EnvVar {
                key: "K".into(),
                value: "v".into(),
            }],
        },
        image_id: [0x44; 32],
        registered_at: 1_793_000_000_123,
        updated_at: 1_793_000_999_456,
        revision: 3,
    }
}

fn h(b: &[u8]) -> String {
    hex::encode(b)
}

fn vectors() -> Value {
    let reg = id(0xa5);
    let e = entry();
    let link = Link {
        schema: 1,
        seq: 41,
        kind: 1,
        program: id(7),
        name: "token-é".into(),
        author: id(8),
        prev_global: Some(id(1)),
        prev_name: None,
        prev_author: Some(id(2)),
        prev_program: None,
    };
    let head = Head {
        schema: 1,
        count: 42,
        newest: Some(id(3)),
    };
    let release = Release {
        schema: 1,
        entry: id(7),
        revision: 2,
        version: "1.1.0".into(),
        idl: e.idl.clone(),
        source: e.source.clone(),
        build: e.build.clone(),
        image_id: [0x55; 32],
        build_hash: build_hash(&e.build),
        at: 1_793_000_500_000,
    };
    json!({
        "registry": reg.to_base58(),
        "addresses": {
            "global_head": addr::global_head(&reg).to_base58(),
            "name_head:alice": addr::name_head(&reg, "alice").to_base58(),
            "author_head:08": addr::author_head(&reg, &id(8)).to_base58(),
            "program_head:07": addr::program_head(&reg, &id(7)).to_base58(),
            "link:07": addr::link(&reg, &id(7)).to_base58(),
            "release:07:2": addr::release(&reg, &id(7), 2).to_base58(),
            "third_party:07:09": addr::third_party_entry(&reg, &id(7), &id(9)).to_base58(),
        },
        "entry": {"borsh": h(&borsh::to_vec(&e).unwrap()), "program": e.program.to_base58(), "name": e.name, "revision": e.revision,
                  "image_id": h(&e.image_id), "idl_size": e.idl.size, "updated_at": e.updated_at.to_string(), "build_hash": h(&build_hash(&e.build))},
        "link": {"borsh": h(&borsh::to_vec(&link).unwrap()), "seq": link.seq, "prev_global": id(1).to_base58()},
        "head": {"borsh": h(&borsh::to_vec(&head).unwrap()), "count": head.count, "newest": id(3).to_base58()},
        "release": {"borsh": h(&borsh::to_vec(&release).unwrap()), "revision": 2, "at": release.at.to_string()},
        "sha256:abc": h(&sha256(b"abc")),
    })
}

#[test]
fn js_vectors_are_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../js/vectors.json");
    let want = serde_json::to_string_pretty(&vectors()).unwrap() + "\n";
    if std::env::var("LEZREG_WRITE_VECTORS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &want).unwrap();
    }
    let got = std::fs::read_to_string(&path)
        .expect("sdk/js/vectors.json (run with LEZREG_WRITE_VECTORS=1)");
    assert_eq!(
        got, want,
        "sdk/js/vectors.json is stale: rerun with LEZREG_WRITE_VECTORS=1"
    );
}
