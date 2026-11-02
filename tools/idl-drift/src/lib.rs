//! The registry's IDL tooling (DESIGN §5.1, §6.3).
//!
//! - [`extend`]: `lezreg-idl-extend`. Takes the released `spel generate-idl` v0.7.0 output and
//!   only **adds**: the `x-lez04` block, and the fields v0.7.0 leaves out (F1b): each
//!   instruction's `discriminator` (our wire tag: one byte), `execution` and `variant`, plus
//!   top-level `spec` and `metadata`. It never changes a field the generator wrote.
//! - [`codec`]: an IDL-driven borsh encoder/decoder over the JSON IDL, with no dependency on
//!   the program's Rust types. The drift tests check it against `borsh::to_vec(&Instruction)`;
//!   the SDKs reuse it (M2/M7).

use serde_json::{Map, Value, json};

pub mod codec;

pub const IDL_EXT_SPEC: &str = "lezreg-idl-ext/0.1";
pub const LEZ_VERSION: &str = "v0.4.0-rc1";

/// Keys `extend` may add to an instruction.
pub const ADDED_IX_KEYS: [&str; 3] = ["discriminator", "execution", "variant"];
/// Top-level keys `extend` may add.
pub const ADDED_TOP_KEYS: [&str; 3] = ["spec", "metadata", "x-lez04"];

fn camel(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

fn hex8(b: &[u8; 8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Per-instruction 0.4 facts the SPEL 0.1.0 schema cannot express.
fn instruction_ext(name: &str) -> Value {
    let time = json!({"arg": "now", "window_ms": lezreg_types::TIME_WINDOW_MS});
    let pda_path = json!({
        "arg": "via_pda_seed",
        "rule": "None: must be the transaction's root message. Some(s): the sender's program P must satisfy for_public_pda(P, s) == root account, and the root must be authorized (P's grant); every signer account must then be the root itself (a co-signer's signature there only proves it signed some transaction that reached P)"
    });
    let evidence = "none: registry PDAs self-admit via pda_seeds; signers are always admitted";
    match name {
        "register_deployer" => json!({
            "root": {"account": "program", "program": "self"},
            "signers": ["program", "author"],
            "signers_note": "author may equal program: declare and sign once. The root's signature may be replaced by the PDA-header path, which requires author == program",
            "pda_header": pda_path,
            "evidence": evidence,
            "time": time,
            "creates": "Entry at (program, self); Release; Link"
        }),
        "register_third_party" => json!({
            "root": {"account": "publisher", "program": "self"},
            "signers": ["publisher"],
            "root_only": true,
            "evidence": evidence,
            "time": time,
            "creates": "Entry at (entry, self) = pda[tp, program, publisher]; Release; Link"
        }),
        "update" => json!({
            "root": {"account": "author", "program": "self"},
            "signers": ["author"],
            "inline_if": "author == entry (the root actor is the entry: patched in place)",
            "pda_header": pda_path,
            "evidence": evidence,
            "time": time,
            "revision": "must equal the stored revision + 1; revision_seed = revision as 32 little-endian bytes"
        }),
        "transfer_author" => json!({
            "root": {"account": "author", "program": "self"},
            "signers": ["author", "new_author"],
            "signers_note": "if new_author == entry, no probe is sent: the entry's own delivery checks new_author signed. Not possible on the PDA-header path (new_author would be a non-root signer)",
            "inline_if": "author == entry",
            "pda_header": pda_path,
            "evidence": evidence,
            "time": time
        }),
        "freeze" => json!({
            "root": {"account": "author", "program": "self"},
            "signers": ["author"],
            "inline_if": "author == entry",
            "pda_header": pda_path,
            "evidence": evidence,
            "time": time,
            "irreversible": true
        }),
        "deploy_and_register" => json!({
            "root": {"account": "program", "program": "self"},
            "signers": ["program", "author"],
            "signers_note": "author may equal program. Key-held headers only (no PDA path)",
            "root_only": true,
            "evidence": evidence,
            "time": time,
            "loader_actors": {"program_header": {"account": "program", "program": "program_loader"}},
            "first_message": "program_loader CreateHeader{first_segment, immutable} to (program, program_loader); segment accounts are not declared",
            "creates": "loader header at program; Entry with born_with_deploy; Release; Link"
        }),
        _ => json!({}),
    }
}

/// IDL type of an event field, from its Rust type token.
fn idl_type(rust: &str) -> Value {
    match rust {
        "AccountId" => json!("account_id"),
        "String" => json!("string"),
        "[u8; 32]" => json!({"array": ["u8", 32]}),
        "u8" | "u32" | "u64" | "bool" => json!(rust),
        other => panic!("event field type {other} has no IDL mapping"),
    }
}

/// Event payload layouts in SPEL `IdlTypeDef` form, derived from `lezreg_types::events`.
fn event_types() -> Value {
    let defs: Vec<Value> = lezreg_types::events::ALL
        .iter()
        .map(|(name, _, fields)| {
            let fields: Vec<Value> = fields.iter().map(|(f, t)| json!({"name": f, "type": idl_type(t)})).collect();
            json!({"name": name, "kind": "struct", "fields": fields})
        })
        .collect();
    Value::Array(defs)
}

/// The `x-lez04` block.
#[must_use]
pub fn x_lez04(instruction_names: &[&str]) -> Value {
    use lezreg_types::events as ev;
    let events: Vec<Value> = ev::ALL
        .iter()
        .map(|(n, full, _)| json!({"name": n, "selector": hex8(&ev::selector(full)), "selector_of": full, "type": n}))
        .collect();
    let mut ixs = Map::new();
    for n in instruction_names {
        ixs.insert((*n).to_string(), instruction_ext(n));
    }
    json!({
        "spec": IDL_EXT_SPEC,
        "lez": LEZ_VERSION,
        "wire": {
            "root": "bare Instruction: u8 variant index (= instruction order = discriminator) + borsh args; not the spel v0.7.0 CLI's risc0-serde words",
            "discriminator": "one byte, the borsh variant index; not lssa-lang's sha256(\"global:<name>\")[..8]",
            "effect_tag_min": 192,
            "require_signer_tag": 255,
            "append_only": "new instructions are only appended (wire-tags.lock)"
        },
        "account_id_args": {
            "rule": "<name>_id: [u8;32] supplies the id of each non-root, non-PDA account <name> (a 0.4 guest sees only its receiver)",
            "pairs": {"author": "author_id", "new_author": "new_author_id", "entry": "entry_id"}
        },
        "accounts_rule": "every account listed is an actor to declare: (account, self) unless loader_actors says otherwise; the first account is the root; pda accounts derive under self by pda_rule; signer accounts sign",
        "instructions": Value::Object(ixs),
        "internal": lezreg_program::EFFECT_NAMES.iter().map(|n| camel(n)).collect::<Vec<_>>(),
        "events": events,
        "event_types": event_types(),
        "state": {
            "(X, self) where X is the program's header account": "Entry (kind 0, deployer)",
            "(pda[lezreg/v1/tp, program, publisher], self)": "Entry (kind 1, third party)",
            "(pda[lezreg/v1/rel, entry, revision_seed], self)": "Release",
            "(pda[lezreg/v1/link, entry], self)": "Link",
            "(pda[lezreg/v1/head], self)": "Head (global list)",
            "(pda[lezreg/v1/name, name], self)": "Head (per name)",
            "(pda[lezreg/v1/author, author], self)": "Head (per author)",
            "(pda[lezreg/v1/tphead, program], self)": "Head (third-party entries per program)",
            "(pda[lezreg/v1/anode, new_author, entry], self)": "AuthorNode"
        },
        "lists": "Head.newest -> Link(entry).prev_global | prev_name | prev_author (or AuthorNode(author, entry).prev if Link.author != author) | prev_program",
        "pda_rule": "spel-compute_pda: 32-byte components (strings zero-padded, account ids raw); one component is the seed, several are sha256(c1||c2||..); address = for_public_pda(self, seed). No integer arg seeds",
        "program": {"account_id": null, "image_id": null}
    })
}

/// Adds to the generator's IDL; never changes what it wrote. Errors if the generator's output
/// already has one of the keys we add (so "only adds" holds by construction).
pub fn extend(spel: &Value) -> Result<Value, String> {
    let mut v = spel.clone();
    let top = v.as_object_mut().ok_or("IDL must be an object")?;
    for k in ADDED_TOP_KEYS {
        if top.get(k).is_some_and(|x| !x.is_null()) {
            return Err(format!("generator output already has `{k}`"));
        }
    }
    let name = top.get("name").and_then(Value::as_str).ok_or("IDL has no name")?.to_string();
    let ixs = top.get_mut("instructions").and_then(Value::as_array_mut).ok_or("IDL has no instructions")?;
    let mut names = Vec::new();
    for (i, ix) in ixs.iter_mut().enumerate() {
        let o = ix.as_object_mut().ok_or("instruction must be an object")?;
        for k in ADDED_IX_KEYS {
            if o.get(k).is_some_and(|x| !x.is_null()) {
                return Err(format!("generator output already has instructions[{i}].{k}"));
            }
        }
        let n = o.get("name").and_then(Value::as_str).ok_or("instruction has no name")?.to_string();
        o.insert("discriminator".into(), json!([i]));
        o.insert("execution".into(), json!({"public": true, "private_owned": false}));
        o.insert("variant".into(), json!(camel(&n)));
        names.push(n);
    }
    let expected: Vec<&str> = lezreg_program::INSTRUCTION_NAMES.to_vec();
    if names.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        return Err(format!("IDL instruction order {names:?} differs from the wire order {expected:?}"));
    }
    top.insert("spec".into(), json!("0.1.0"));
    top.insert("metadata".into(), json!({"name": name, "version": env!("CARGO_PKG_VERSION")}));
    top.insert("x-lez04".into(), x_lez04(&expected));
    Ok(v)
}

/// The inverse of [`extend`]: what the generator wrote.
#[must_use]
pub fn strip(idl: &Value) -> Value {
    let mut v = idl.clone();
    if let Some(top) = v.as_object_mut() {
        for k in ADDED_TOP_KEYS {
            top.remove(k);
        }
        if let Some(ixs) = top.get_mut("instructions").and_then(Value::as_array_mut) {
            for ix in ixs {
                if let Some(o) = ix.as_object_mut() {
                    for k in ADDED_IX_KEYS {
                        o.remove(k);
                    }
                }
            }
        }
    }
    v
}

/// Pretty JSON with a trailing newline (the committed form).
#[must_use]
pub fn to_pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("json");
    s.push('\n');
    s
}
