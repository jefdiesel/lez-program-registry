//! Write vectors for the `program_registry` Logos module (DESIGN §7.3, M6). The module builds
//! registry transactions without any LEZ dependency (its own `Instruction` mirror and actor
//! derivation, `module/rust-lib/src/writes.rs`), so this test pins it to the real program: for
//! each case it gives the module's JSON input and what the real `lezreg_program::Instruction`
//! and `actors_to_declare` make of it. The module's unit tests rebuild every case from the input
//! and must match byte for byte.
//!
//! `LEZREG_WRITE_VECTORS=1 cargo test -p lezreg-sdk --test module_vectors` rewrites
//! `module/rust-lib/testdata/write-vectors.json`; without it the test checks the committed file
//! is current.
#![cfg(feature = "tx")]

use lee_core::account::{AccountId as LeeId, Actor};
use lezreg_program::{Instruction, actors_to_declare};
use lezreg_sdk::{cid, json as js, tx::tid};
use lezreg_types::{
    AccountId, BuildConfig, ContentRef, EnvVar, SourceRef,
    pda::{addr, revision_seed},
};
use serde_json::{Value, json};

fn id(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn lee(a: &AccountId) -> LeeId {
    LeeId::new(a.0)
}

const NOW: u64 = 1_790_000_000_000;

fn idl() -> ContentRef {
    ContentRef {
        cid: cid::raw_cid(b"{\"name\":\"token\"}"),
        sha256: [0x11; 32],
        size: 16,
        peer_hint: Some(vec![0, 36, 8, 1, 18, 32]),
    }
}

fn source() -> SourceRef {
    SourceRef {
        repo: "https://github.com/acme/token".into(),
        commit: vec![0xab; 20],
        docker_root: ".".into(),
        archive: Some(ContentRef {
            cid: cid::raw_cid(b"archive"),
            sha256: [0x22; 32],
            size: 7,
            peer_hint: None,
        }),
    }
}

fn build() -> BuildConfig {
    BuildConfig {
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
    }
}

/// One case: the module's input, the stored revision it reads (update), the root account, the
/// real instruction, and the entry it writes.
fn case(op: &str, input: Value, stored: Option<u32>, root: AccountId, ix: &Instruction, entry: AccountId, reg: AccountId) -> Value {
    let actors: Vec<Actor> = actors_to_declare(lee(&root), lee(&reg), ix);
    let signers: Vec<AccountId> = match ix {
        Instruction::RegisterDeployer { author_id, .. } if *author_id != root.0 => vec![root, AccountId(*author_id)],
        Instruction::TransferAuthor { new_author_id, .. } if *new_author_id != root.0 => vec![root, AccountId(*new_author_id)],
        _ => vec![root],
    };
    json!({
        "op": op,
        "input": input,
        "stored_revision": stored,
        "message": hex::encode(borsh::to_vec(ix).unwrap()),
        "root": root.to_base58(),
        "accounts": actors.iter().map(|a| json!([tid(&a.account_id).to_base58(), tid(&a.program_account_id).to_base58()])).collect::<Vec<_>>(),
        "signers": signers.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
        "entry": entry.to_base58(),
    })
}

fn vectors() -> Value {
    let reg = id(0xEE);
    let (x, a, p, b) = (id(7), id(8), id(10), id(11));
    let fields = |extra: Value| {
        let mut v = json!({
            "name": "token", "version": "1.2.0", "author_label": "Acme Labs", "description": "A token",
            "tags": ["token", "defi"], "idl": js::content(&idl()), "source": js::source(&source()), "build": js::build(&build()),
            "image_id": hex::encode([0x44; 32]),
        });
        for (k, val) in extra.as_object().unwrap() {
            v[k] = val.clone();
        }
        v
    };
    let deployer = |author: AccountId| Instruction::RegisterDeployer {
        name: "token".into(),
        version: "1.2.0".into(),
        author_label: "Acme Labs".into(),
        description: "A token".into(),
        tags: vec!["token".into(), "defi".into()],
        idl: idl(),
        source: source(),
        build: build(),
        image_id: [0x44; 32],
        author_id: author.0,
        revision: 1,
        revision_seed: revision_seed(1),
        now: NOW,
        via_pda_seed: None,
    };
    let third = Instruction::RegisterThirdParty {
        program: x.0,
        name: "token".into(),
        version: "1.2.0".into(),
        author_label: "Acme Labs".into(),
        description: "A token".into(),
        tags: vec!["token".into(), "defi".into()],
        idl: idl(),
        source: source(),
        build: build(),
        image_id: [0x44; 32],
        revision: 1,
        revision_seed: revision_seed(1),
        now: NOW,
    };
    let t_entry = addr::third_party_entry(&reg, &x, &p);
    let full_update = Instruction::Update {
        entry_id: x.0,
        revision: 4,
        revision_seed: revision_seed(4),
        version: Some("1.3.0".into()),
        description: Some("Better".into()),
        tags: Some(vec!["token".into()]),
        author_label: Some(String::new()),
        idl: Some(idl()),
        source: Some(source()),
        build: Some(build()),
        image_id: Some([0x55; 32]),
        now: NOW,
        via_pda_seed: None,
    };
    let version_update = Instruction::Update {
        entry_id: t_entry.0,
        revision: 2,
        revision_seed: revision_seed(2),
        version: Some("1.2.1".into()),
        description: None,
        tags: None,
        author_label: None,
        idl: None,
        source: None,
        build: None,
        image_id: None,
        now: NOW,
        via_pda_seed: None,
    };
    let transfer = |to: AccountId| Instruction::TransferAuthor {
        entry_id: x.0,
        new_author_id: to.0,
        now: NOW,
        via_pda_seed: None,
    };
    let freeze = Instruction::Freeze {
        entry_id: t_entry.0,
        now: NOW,
        via_pda_seed: None,
    };
    let e58 = |e: &AccountId| json!(e.to_base58());
    json!({
        "comment": "Generated by sdk/rust/tests/module_vectors.rs from the real lezreg_program::Instruction and actors_to_declare; do not edit.",
        "registry": reg.to_base58(),
        "now": NOW,
        "clock_account": tid(&clock_core::CLOCK_PROGRAM_ACCOUNT_ID).to_base58(),
        "clock_program": tid(&clock_core::clock_account_id()).to_base58(),
        "loader": tid(&lee_core::program::PROGRAM_LOADER_ACCOUNT_ID).to_base58(),
        "cases": [
            case("register", fields(json!({"program": e58(&x), "author": e58(&a)})), None, x, &deployer(a), x, reg),
            case("register", fields(json!({"program": e58(&x)})), None, x, &deployer(x), x, reg),
            case("register_third_party", fields(json!({"program": e58(&x), "publisher": e58(&p)})), None, p, &third, t_entry, reg),
            case("update", json!({"entry": e58(&x), "author": e58(&a), "version": "1.3.0", "description": "Better", "tags": ["token"],
                                  "author_label": "", "idl": js::content(&idl()), "source": js::source(&source()), "build": js::build(&build()),
                                  "image_id": hex::encode([0x55; 32])}), Some(3), a, &full_update, x, reg),
            case("update", json!({"entry": e58(&t_entry), "author": e58(&p), "version": "1.2.1"}), Some(1), p, &version_update, t_entry, reg),
            case("transfer", json!({"entry": e58(&x), "author": e58(&a), "new_author": e58(&b)}), None, a, &transfer(b), x, reg),
            case("transfer", json!({"entry": e58(&x), "author": e58(&a), "new_author": e58(&x)}), None, a, &transfer(x), x, reg),
            case("freeze", json!({"entry": e58(&t_entry), "author": e58(&p)}), None, p, &freeze, t_entry, reg),
        ],
    })
}

#[test]
fn module_write_vectors_are_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../module/rust-lib/testdata/write-vectors.json");
    let text = serde_json::to_string_pretty(&vectors()).unwrap() + "\n";
    if std::env::var_os("LEZREG_WRITE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(committed == text, "{} is stale: rerun with LEZREG_WRITE_VECTORS=1", path.display());
}
