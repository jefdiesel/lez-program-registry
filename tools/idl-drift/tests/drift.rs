//! IDL drift tests (DESIGN §6.3):
//! 1. the committed generator output equals a fresh run of the released `spel generate-idl`
//!    v0.7.0 (mandatory; `LEZREG_SKIP_SPEL=1` skips it explicitly), and the
//!    committed IDL equals `lezreg-idl-extend` of it, which only adds;
//! 2. `SpelIdl` parse: separate workspace `tools/idl-check` (SPEL v0.7.0 pins LEZ 0.2.5);
//! 3. our IDL-driven borsh codec reproduces `borsh::to_vec(&Instruction)` for every variant,
//!    and decodes every record and event;
//! 4. every IDL `pda.seeds`, derived with SPEL's rule, equals `actors_to_declare`.

use lee_core::{
    account::{AccountId, Actor},
    program::PROGRAM_LOADER_ACCOUNT_ID,
};
use lezreg_idl::codec::{Idl, hex};
use lezreg_program::{INSTRUCTION_NAMES, Instruction, actors_to_declare};
use lezreg_types::{self as t, pda};
use serde_json::{Value, json};

const SPEL_JSON: &str = include_str!("../../../idl/lez_program_registry.spel.json");
const IDL_JSON: &str = include_str!("../../../idl/lez_program_registry.idl.json");

fn idl() -> Value {
    serde_json::from_str(IDL_JSON).unwrap()
}

#[test]
fn committed_idl_is_the_extension_of_the_generator_output() {
    let spel: Value = serde_json::from_str(SPEL_JSON).unwrap();
    let extended = lezreg_idl::extend(&spel).unwrap();
    assert_eq!(lezreg_idl::to_pretty(&extended), IDL_JSON, "idl/ is stale: run scripts/idl.sh");
}

#[test]
fn extend_only_adds() {
    let stripped = lezreg_idl::strip(&idl());
    assert_eq!(lezreg_idl::to_pretty(&stripped), SPEL_JSON, "extend changed a generator field");
    // And it refuses input that already has a field it would add.
    let mut spel: Value = serde_json::from_str(SPEL_JSON).unwrap();
    spel["spec"] = json!("x");
    assert!(lezreg_idl::extend(&spel).is_err());
}

#[test]
fn regenerate_with_released_spel() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let bin = std::env::var("SPEL_BIN").unwrap_or_else(|_| format!("{root}/_deps/spel-root/bin/spel"));
    // This is the only check that catches drift in records and instructions, so a missing
    // generator fails unless the skip is explicit.
    if !std::path::Path::new(&bin).exists() {
        assert!(
            std::env::var("LEZREG_SKIP_SPEL").is_ok_and(|v| v == "1"),
            "no spel v0.7.0 binary at {bin}: run scripts/idl.sh to install it, or set LEZREG_SKIP_SPEL=1 to skip this check"
        );
        eprintln!("SKIPPED (LEZREG_SKIP_SPEL=1): no spel v0.7.0 binary at {bin}");
        return;
    }
    if std::env::var("SPEL_BIN").is_err() {
        let record = std::fs::read_to_string(format!("{root}/_deps/spel-root/.crates.toml")).unwrap_or_default();
        assert!(record.contains("\"spel 0.7.0 ("), "_deps/spel-root is not spel 0.7.0:\n{record}");
    }
    let out = std::process::Command::new(&bin).args(["generate-idl", "program/src/lib.rs"]).current_dir(root).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8(out.stdout).unwrap(), SPEL_JSON, "the released generator now gives a different IDL: run scripts/idl.sh");
}

#[test]
fn instruction_order_is_wire_order() {
    let v = idl();
    let names: Vec<&str> = v["instructions"].as_array().unwrap().iter().map(|i| i["name"].as_str().unwrap()).collect();
    assert_eq!(names, INSTRUCTION_NAMES);
    for (i, ix) in v["instructions"].as_array().unwrap().iter().enumerate() {
        assert_eq!(ix["discriminator"], json!([i]));
    }
    assert_eq!(v["instruction_type"], "Instruction");
    assert_eq!(v["name"], t::PROGRAM_NAME);
    let internal: Vec<String> = v["x-lez04"]["internal"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
    assert_eq!(internal.len(), lezreg_program::EFFECT_NAMES.len());
}

fn content(n: u8) -> t::ContentRef {
    t::ContentRef { cid: vec![1, 0x55, n], sha256: [n; 32], size: 77 + u32::from(n), peer_hint: Some(vec![9, n]) }
}
fn source() -> t::SourceRef {
    t::SourceRef { repo: "https://x.example/r".into(), commit: vec![0xC0; 20], docker_root: "sub".into(), archive: Some(content(4)) }
}
fn build() -> t::BuildConfig {
    t::BuildConfig {
        schema: 1,
        builder: 1,
        image_digest: [0xD1; 32],
        image_tag: "r0.1.91.1".into(),
        risc0_build: "3.0.5".into(),
        host_risc0_rust: "1.91.1".into(),
        manifest_path: "m/Cargo.toml".into(),
        binary: "g".into(),
        features: vec!["f1".into()],
        lock_sha256: [0x10; 32],
        docker_env: vec![t::EnvVar { key: "K".into(), value: "v".into() }],
    }
}

/// One of each instruction, with distinct values.
fn samples() -> Vec<(AccountId, Instruction)> {
    let x = AccountId::new([0x11; 32]);
    let a = AccountId::new([0x22; 32]);
    let rs = pda::revision_seed;
    vec![
        (x, Instruction::RegisterDeployer {
            name: "token".into(), version: "1.0.0".into(), author_label: "Acme".into(), description: "d".into(),
            tags: vec!["t1".into(), "t2".into()], idl: content(1), source: source(), build: build(), image_id: [0x33; 32],
            author_id: *a.value(), revision: 1, revision_seed: rs(1), now: 1_800_000_000_000, via_pda_seed: Some([0x44; 32]),
        }),
        (a, Instruction::RegisterThirdParty {
            program: *x.value(), name: "tp".into(), version: "0.1".into(), author_label: String::new(), description: String::new(),
            tags: vec![], idl: content(2), source: source(), build: build(), image_id: [0x55; 32], revision: 1, revision_seed: rs(1), now: 5,
        }),
        (a, Instruction::Update {
            entry_id: *x.value(), revision: 3, revision_seed: rs(3), version: Some("3".into()), description: None,
            tags: Some(vec!["z".into()]), author_label: None, idl: Some(content(3)), source: None, build: Some(build()),
            image_id: Some([0x66; 32]), now: 6, via_pda_seed: None,
        }),
        (a, Instruction::TransferAuthor { entry_id: *x.value(), new_author_id: [0x77; 32], now: 7, via_pda_seed: None }),
        (a, Instruction::Freeze { entry_id: *x.value(), now: 8, via_pda_seed: Some([0x88; 32]) }),
        (x, Instruction::DeployAndRegister {
            first_segment: [0x99; 32], immutable: true, name: "dar".into(), version: "1".into(), author_label: "L".into(),
            description: String::new(), tags: vec![], idl: content(5), source: source(), build: build(), image_id: [0xAA; 32],
            author_id: *x.value(), revision: 1, revision_seed: rs(1), now: 9,
        }),
    ]
}

#[test]
fn idl_codec_matches_borsh_for_every_instruction() {
    let v = idl();
    let codec = Idl::new(&v);
    let samples = samples();
    assert_eq!(samples.len(), INSTRUCTION_NAMES.len(), "a sample per instruction");
    for (_, ix) in &samples {
        let bytes = borsh::to_vec(ix).unwrap();
        let (name, args) = codec.decode_instruction(&bytes).unwrap();
        assert_eq!(name, ix.name());
        assert_eq!(codec.encode_instruction(&name, &args).unwrap(), bytes, "{name}: IDL encoding differs from borsh");
    }
    // Field names and values land where the Rust enum puts them.
    let (_, a) = codec.decode_instruction(&borsh::to_vec(&samples[0].1).unwrap()).unwrap();
    assert_eq!(a["name"], "token");
    assert_eq!(a["author_id"], hex(&[0x22; 32]));
    assert_eq!(a["revision"], 1);
    assert_eq!(a["now"], 1_800_000_000_000u64);
    assert_eq!(a["via_pda_seed"], hex(&[0x44; 32]));
    assert_eq!(a["build"]["docker_env"][0]["key"], "K");
    assert_eq!(a["source"]["archive"]["size"], 81);
    let (_, u) = codec.decode_instruction(&borsh::to_vec(&samples[2].1).unwrap()).unwrap();
    assert_eq!((u["description"].clone(), u["tags"][0].clone()), (Value::Null, json!("z")));
    // Hand-written JSON encodes to the same bytes as the Rust value.
    let freeze = json!({"entry_id": hex(&[0x11; 32]), "now": 8, "via_pda_seed": hex(&[0x88; 32])});
    assert_eq!(codec.encode_instruction("freeze", &freeze).unwrap(), borsh::to_vec(&samples[4].1).unwrap());
}

#[test]
fn idl_codec_reads_records_and_events() {
    let v = idl();
    let codec = Idl::new(&v);
    let id = |b: u8| t::AccountId([b; 32]);
    let entry = t::Entry {
        schema: 1, kind: 1, flags: 3, program: id(1), author: id(2), registrant: id(3), name: "n".into(), version: "v".into(),
        author_label: "l".into(), description: "d".into(), tags: vec!["a".into()], idl: content(1), source: source(), build: build(),
        image_id: [4; 32], registered_at: 5, updated_at: 6, revision: 7,
    };
    let release = t::Release {
        schema: 1, entry: id(1), revision: 2, version: "v".into(), idl: content(2), source: source(), build: build(),
        image_id: [3; 32], build_hash: [4; 32], at: 5,
    };
    let link = t::Link {
        schema: 1, seq: 9, kind: 0, program: id(1), name: "n".into(), author: id(2), prev_global: Some(id(3)), prev_name: None,
        prev_author: Some(id(4)), prev_program: None,
    };
    let head = t::Head { schema: 1, count: 2, newest: Some(id(5)) };
    let node = t::AuthorNode { schema: 1, entry: id(6), prev: None };
    let records: Vec<(&str, Vec<u8>)> = vec![
        ("Entry", borsh::to_vec(&entry).unwrap()),
        ("Release", borsh::to_vec(&release).unwrap()),
        ("Link", borsh::to_vec(&link).unwrap()),
        ("Head", borsh::to_vec(&head).unwrap()),
        ("AuthorNode", borsh::to_vec(&node).unwrap()),
        ("Registered", borsh::to_vec(&t::events::Registered {
            entry: id(1), program: id(2), kind: 0, author: id(3), name: "n".into(), seq: 4, image_id: [5; 32], idl_sha256: [6; 32], born_with_deploy: true,
        }).unwrap()),
        ("Updated", borsh::to_vec(&t::events::Updated { entry: id(1), revision: 2, version: "v".into(), image_id: [3; 32], idl_sha256: [4; 32], build_hash: [5; 32] }).unwrap()),
        ("AuthorTransferred", borsh::to_vec(&t::events::AuthorTransferred { entry: id(1), from: id(2), to: id(3) }).unwrap()),
        ("Frozen", borsh::to_vec(&t::events::Frozen { entry: id(1) }).unwrap()),
    ];
    for (name, bytes) in records {
        let j = codec.decode_named(name, &bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(codec.encode_named(name, &j).unwrap(), bytes, "{name}");
    }
    // Every field by name, with distinct values: a reordered Rust field (records or events)
    // lands under the wrong IDL name and fails here even when the byte widths still match.
    let b58 = |b: u8| json!(id(b).to_base58());
    let h32 = |b: u8| json!(hex(&[b; 32]));
    let named = |name: &str, bytes: Vec<u8>, expect: Value| {
        let j = codec.decode_named(name, &bytes).unwrap();
        for (k, v) in expect.as_object().unwrap() {
            assert_eq!(j[k], *v, "{name}.{k}");
        }
        assert_eq!(j.as_object().unwrap().len(), expect.as_object().unwrap().len(), "{name}: field count");
    };
    named("Entry", borsh::to_vec(&entry).unwrap(), json!({
        "schema": 1, "kind": 1, "flags": 3, "program": b58(1), "author": b58(2), "registrant": b58(3), "name": "n",
        "version": "v", "author_label": "l", "description": "d", "tags": ["a"],
        "idl": codec.decode_named("ContentRef", &borsh::to_vec(&content(1)).unwrap()).unwrap(),
        "source": codec.decode_named("SourceRef", &borsh::to_vec(&source()).unwrap()).unwrap(),
        "build": codec.decode_named("BuildConfig", &borsh::to_vec(&build()).unwrap()).unwrap(),
        "image_id": h32(4), "registered_at": 5, "updated_at": 6, "revision": 7,
    }));
    named("Release", borsh::to_vec(&release).unwrap(), json!({
        "schema": 1, "entry": b58(1), "revision": 2, "version": "v",
        "idl": codec.decode_named("ContentRef", &borsh::to_vec(&content(2)).unwrap()).unwrap(),
        "source": codec.decode_named("SourceRef", &borsh::to_vec(&source()).unwrap()).unwrap(),
        "build": codec.decode_named("BuildConfig", &borsh::to_vec(&build()).unwrap()).unwrap(),
        "image_id": h32(3), "build_hash": h32(4), "at": 5,
    }));
    named("Link", borsh::to_vec(&link).unwrap(), json!({
        "schema": 1, "seq": 9, "kind": 0, "program": b58(1), "name": "n", "author": b58(2), "prev_global": b58(3),
        "prev_name": null, "prev_author": b58(4), "prev_program": null,
    }));
    named("Head", borsh::to_vec(&head).unwrap(), json!({"schema": 1, "count": 2, "newest": b58(5)}));
    named("AuthorNode", borsh::to_vec(&node).unwrap(), json!({"schema": 1, "entry": b58(6), "prev": null}));
    named("ContentRef", borsh::to_vec(&content(1)).unwrap(), json!({"cid": hex(&[1, 0x55, 1]), "sha256": h32(1), "size": 78, "peer_hint": hex(&[9, 1])}));
    named("Registered", borsh::to_vec(&t::events::Registered {
        entry: id(1), program: id(2), kind: 7, author: id(3), name: "n".into(), seq: 4, image_id: [5; 32], idl_sha256: [6; 32], born_with_deploy: true,
    }).unwrap(), json!({
        "entry": b58(1), "program": b58(2), "kind": 7, "author": b58(3), "name": "n", "seq": 4, "image_id": h32(5), "idl_sha256": h32(6),
        "born_with_deploy": true,
    }));
    named("Updated", borsh::to_vec(&t::events::Updated { entry: id(1), revision: 2, version: "v".into(), image_id: [3; 32], idl_sha256: [4; 32], build_hash: [5; 32] }).unwrap(),
        json!({"entry": b58(1), "revision": 2, "version": "v", "image_id": h32(3), "idl_sha256": h32(4), "build_hash": h32(5)}));
    named("AuthorTransferred", borsh::to_vec(&t::events::AuthorTransferred { entry: id(1), from: id(2), to: id(3) }).unwrap(),
        json!({"entry": b58(1), "from": b58(2), "to": b58(3)}));
    named("Frozen", borsh::to_vec(&t::events::Frozen { entry: id(1) }).unwrap(), json!({"entry": b58(1)}));
    // The IDL's event layouts are the Rust structs' fields, in order.
    for (name, _, fields) in t::events::ALL {
        let def = v["x-lez04"]["event_types"].as_array().unwrap().iter().find(|d| d["name"] == name).unwrap();
        let idl_names: Vec<&str> = def["fields"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(idl_names, fields.iter().map(|(f, _)| *f).collect::<Vec<_>>(), "{name}: IDL event fields");
    }
    // Event selectors in x-lez04 are the ones the program emits.
    for e in v["x-lez04"]["events"].as_array().unwrap() {
        let sel = t::events::selector(e["selector_of"].as_str().unwrap());
        assert_eq!(e["selector"], hex(&sel));
    }
}

fn seed_part(codec_args: &Value, ixdef: &Value, s: &Value, ids: &[(String, AccountId)]) -> [u8; 32] {
    let path = s.get("path").and_then(Value::as_str).unwrap_or_default();
    match s["kind"].as_str().unwrap() {
        "const" => pda::seed_str(s["value"].as_str().unwrap()),
        "account" => *ids.iter().find(|(n, _)| n == path).unwrap_or_else(|| panic!("account seed {path}")).1.value(),
        "arg" => {
            let ty = &ixdef["args"].as_array().unwrap().iter().find(|a| a["name"] == path).unwrap()["type"];
            let val = &codec_args[path];
            if ty == "string" {
                pda::seed_str(val.as_str().unwrap())
            } else {
                assert_eq!(*ty, json!({"array": ["u8", 32]}), "arg seeds are String or [u8;32] (F3)");
                let s = val.as_str().unwrap();
                let b: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
                b.try_into().unwrap()
            }
        }
        k => panic!("seed kind {k}"),
    }
}

#[test]
fn idl_pda_seeds_match_actors_to_declare() {
    let v = idl();
    let codec = Idl::new(&v);
    let reg = AccountId::new([0xEE; 32]);
    let treg = t::AccountId(*reg.value());
    let ext = &v["x-lez04"];
    for (root, ix) in samples() {
        let bytes = borsh::to_vec(&ix).unwrap();
        let (name, args) = codec.decode_instruction(&bytes).unwrap();
        let (_, ixdef) = codec.instruction(&name).unwrap();
        let loader_actors = &ext["instructions"][&name]["loader_actors"];
        let mut ids: Vec<(String, AccountId)> = Vec::new();
        let mut derived: Vec<Actor> = Vec::new();
        for (i, acc) in ixdef["accounts"].as_array().unwrap().iter().enumerate() {
            let an = acc["name"].as_str().unwrap().to_string();
            let actor = if i == 0 {
                Actor::new(root, reg)
            } else if let Some(p) = acc.get("pda") {
                let parts: Vec<[u8; 32]> = p["seeds"].as_array().unwrap().iter().map(|s| seed_part(&args, ixdef, s, &ids)).collect();
                let at = pda::for_public_pda(&treg, &pda::combine(&parts));
                Actor::new(AccountId::new(at.0), reg)
            } else if let Some(l) = loader_actors.get(&an) {
                let of = l["account"].as_str().unwrap();
                assert_eq!(l["program"], "program_loader");
                Actor::new(ids.iter().find(|(n, _)| n == of).unwrap().1, PROGRAM_LOADER_ACCOUNT_ID)
            } else {
                let pair = ext["account_id_args"]["pairs"][&an].as_str().unwrap_or_else(|| panic!("{name}.{an}: no <name>_id pair"));
                assert_eq!(pair, format!("{an}_id"));
                let s = args[pair].as_str().unwrap();
                let b: Vec<u8> = (0..64).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect();
                Actor::new(AccountId::new(b.try_into().unwrap()), reg)
            };
            ids.push((an, actor.account_id));
            if !derived.contains(&actor) {
                derived.push(actor);
            }
        }
        assert_eq!(derived, actors_to_declare(root, reg, &ix), "{name}: IDL-derived actors differ from actors_to_declare");
    }
}

#[test]
fn x_lez04_describes_every_instruction() {
    let v = idl();
    for ix in v["instructions"].as_array().unwrap() {
        let n = ix["name"].as_str().unwrap();
        let e = &v["x-lez04"]["instructions"][n];
        let accounts = ix["accounts"].as_array().unwrap();
        assert_eq!(e["root"]["account"], accounts[0]["name"], "{n}: root");
        // signers == the accounts marked signer, both ways.
        let mut listed: Vec<&str> = e["signers"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        let mut marked: Vec<&str> = accounts.iter().filter(|a| a["signer"] == true).map(|a| a["name"].as_str().unwrap()).collect();
        listed.sort_unstable();
        marked.sort_unstable();
        assert_eq!(listed, marked, "{n}: x-lez04.signers vs accounts[].signer");
        // root_only <=> no via_pda_seed arg; pda_header <=> a via_pda_seed arg.
        let has_via = ix["args"].as_array().unwrap().iter().any(|a| a["name"] == "via_pda_seed");
        assert_eq!(e["root_only"] == true, !has_via, "{n}: root_only");
        assert_eq!(e.get("pda_header").is_some(), has_via, "{n}: pda_header");
        // loader_actors == the non-root accounts that are neither PDAs nor <name>_id pairs.
        let pairs = &v["x-lez04"]["account_id_args"]["pairs"];
        let mut other: Vec<&str> = accounts[1..]
            .iter()
            .filter(|a| a.get("pda").is_none_or(Value::is_null) && pairs.get(a["name"].as_str().unwrap()).is_none())
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        let mut loader: Vec<&str> = e.get("loader_actors").and_then(Value::as_object).map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
        other.sort_unstable();
        loader.sort_unstable();
        assert_eq!(other, loader, "{n}: loader_actors");
        assert_eq!(e["time"]["arg"], "now");
        assert!(ix["args"].as_array().unwrap().iter().any(|a| a["name"] == "now"), "{n} is time-bounded");
    }
}
