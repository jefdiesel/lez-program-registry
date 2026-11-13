//! Writes (DESIGN §7.1, §7.3): `prepare_*` builds a registry transaction — the bare instruction
//! and every account it must declare — and returns it as a *change* for a wallet to sign. The
//! module holds no keys. A change is handed to a signer by `submit` (`bridge.rs`).
//!
//! This builds without any LEZ dependency (the module crate can't link the zkVM-side program
//! crate), so the instruction enum and the actor derivation are mirrored here. They are pinned
//! to the real program by `testdata/write-vectors.json`, which
//! `sdk/rust/tests/module_vectors.rs` writes from `lezreg_program::Instruction` and
//! `actors_to_declare`; the tests below rebuild every vector from its input.

use borsh::BorshSerialize;
use lezreg_sdk::{
    cid,
    types::{self as t, AccountId, BuildConfig, ContentRef, EnvVar, SourceRef, pda::addr, validate},
};
use serde_json::{Value, json};

/// The change format `lezreg module send` reads.
pub const FORMAT: &str = "lezreg-change/1";

/// `lez_program_registry::Instruction` (program/src/lib.rs, wire tags in program/wire-tags.lock):
/// variants in instruction order, fields in argument order. `DeployAndRegister` (tag 5) is
/// last and not built here: it needs segment writes first (`lezreg deploy`/`publish`).
#[derive(BorshSerialize, Debug, Clone, PartialEq)]
pub enum Instruction {
    RegisterDeployer {
        name: String,
        version: String,
        author_label: String,
        description: String,
        tags: Vec<String>,
        idl: ContentRef,
        source: SourceRef,
        build: BuildConfig,
        image_id: [u8; 32],
        author_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    },
    RegisterThirdParty {
        program: [u8; 32],
        name: String,
        version: String,
        author_label: String,
        description: String,
        tags: Vec<String>,
        idl: ContentRef,
        source: SourceRef,
        build: BuildConfig,
        image_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        now: u64,
    },
    Update {
        entry_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        version: Option<String>,
        description: Option<String>,
        tags: Option<Vec<String>>,
        author_label: Option<String>,
        idl: Option<ContentRef>,
        source: Option<SourceRef>,
        build: Option<BuildConfig>,
        image_id: Option<[u8; 32]>,
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    },
    TransferAuthor {
        entry_id: [u8; 32],
        new_author_id: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    },
    Freeze {
        entry_id: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    },
}

/// Why a change could not be prepared: a stable `code` for programs, a sentence for people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn input(message: impl Into<String>) -> Self {
        Self::new("bad_input", message)
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::new("error", message)
    }
}

pub type Res<T> = Result<T, Failure>;

/// A built transaction: what the change JSON carries.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub what: String,
    pub root: AccountId,
    pub ix: Instruction,
    /// Every actor to declare, root first, deduplicated (all of the registry's program).
    pub accounts: Vec<AccountId>,
    pub signers: Vec<AccountId>,
    /// The entry this writes or changes.
    pub entry: AccountId,
}

fn push(v: &mut Vec<AccountId>, a: AccountId) {
    if !v.contains(&a) {
        v.push(a);
    }
}

impl Built {
    fn new(what: impl Into<String>, root: AccountId, ix: Instruction, rest: &[AccountId], signers: &[AccountId], entry: AccountId) -> Self {
        let mut accounts = vec![root];
        for a in rest {
            push(&mut accounts, *a);
        }
        let mut s = Vec::new();
        for a in signers {
            push(&mut s, *a);
        }
        Self { what: what.into(), root, ix, accounts, signers: s, entry }
    }

    pub fn message(&self) -> Vec<u8> {
        borsh::to_vec(&self.ix).expect("borsh into a Vec")
    }

    /// The change JSON: everything a signer needs, in the order `lez_core`'s
    /// `send_generic_public_transaction` takes it (accounts with their program and whether they
    /// sign, the root's index, the message).
    pub fn change(&self, sequencer: &str, registry: &AccountId, now: u64, summary: Value) -> Value {
        json!({
            "format": FORMAT,
            "what": self.what,
            "summary": summary,
            "sequencer": sequencer,
            "registry": registry.to_base58(),
            "root": 0,
            "accounts": self.accounts.iter().map(|a| json!({
                "account": a.to_base58(), "program": registry.to_base58(), "signs": self.signers.contains(a)
            })).collect::<Vec<_>>(),
            "signers": self.signers.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
            "message": hex::encode(self.message()),
            "entry": self.entry.to_base58(),
            "now": now,
            "expires_at": now.saturating_add(t::TIME_WINDOW_MS),
        })
    }
}

/// The most gas a change of each kind may declare when the module sends it: about twice what
/// each needs on LEZ rc3 (measured: RegisterDeployer 174k cycles, RegisterThirdParty 231k).
/// `lezreg module send` refuses a change whose simulated limit is above it, so a confirm card's
/// "at most" fee holds.
pub const fn max_gas(ix: &Instruction) -> u64 {
    match ix {
        Instruction::RegisterDeployer { .. } => 400_000,
        Instruction::RegisterThirdParty { .. } => 500_000,
        Instruction::Update { .. } => 300_000,
        Instruction::TransferAuthor { .. } => 300_000,
        Instruction::Freeze { .. } => 200_000,
    }
}

fn content_json(c: &ContentRef) -> Value {
    json!({"cid": cid::to_string(&c.cid), "sha256": hex::encode(c.sha256), "size": c.size, "peer_hint": c.peer_hint.as_ref().map(hex::encode)})
}

fn source_json(s: &SourceRef) -> Value {
    json!({"repo": s.repo, "commit": hex::encode(&s.commit), "docker_root": s.docker_root, "archive": s.archive.as_ref().map(content_json)})
}

fn build_json(b: &BuildConfig) -> Value {
    json!({"builder": b.builder, "image_digest": hex::encode(b.image_digest), "image_tag": b.image_tag, "risc0_build": b.risc0_build,
           "host_risc0_rust": b.host_risc0_rust, "manifest_path": b.manifest_path, "binary": b.binary, "features": b.features,
           "lock_sha256": hex::encode(b.lock_sha256),
           "docker_env": b.docker_env.iter().map(|e| json!({"key": e.key, "value": e.value})).collect::<Vec<_>>()})
}

impl Instruction {
    /// Everything this instruction writes, decoded from the instruction itself (never from what
    /// a caller said it would write): what a confirm card shows. Absent fields don't change.
    pub fn values(&self) -> Value {
        match self {
            Self::RegisterDeployer { name, version, author_label, description, tags, idl, source, build, image_id, author_id, .. } => json!({
                "name": name, "version": version, "author_label": author_label, "description": description, "tags": tags,
                "idl": content_json(idl), "source": source_json(source), "build": build_json(build),
                "image_id": hex::encode(image_id), "author": AccountId(*author_id).to_base58(),
            }),
            Self::RegisterThirdParty { program, name, version, author_label, description, tags, idl, source, build, image_id, .. } => json!({
                "program": AccountId(*program).to_base58(), "name": name, "version": version, "author_label": author_label,
                "description": description, "tags": tags, "idl": content_json(idl), "source": source_json(source),
                "build": build_json(build), "image_id": hex::encode(image_id),
            }),
            Self::Update { revision, version, description, tags, author_label, idl, source, build, image_id, .. } => {
                let mut v = serde_json::Map::new();
                v.insert("revision".into(), json!(revision));
                let mut put = |k: &str, x: Option<Value>| {
                    if let Some(x) = x {
                        v.insert(k.into(), x);
                    }
                };
                put("version", version.as_ref().map(|x| json!(x)));
                put("description", description.as_ref().map(|x| json!(x)));
                put("tags", tags.as_ref().map(|x| json!(x)));
                put("author_label", author_label.as_ref().map(|x| json!(x)));
                put("idl", idl.as_ref().map(content_json));
                put("source", source.as_ref().map(source_json));
                put("build", build.as_ref().map(build_json));
                put("image_id", image_id.as_ref().map(hex::encode).map(|x| json!(x)));
                Value::Object(v)
            }
            Self::TransferAuthor { new_author_id, .. } => json!({"new_author": AccountId(*new_author_id).to_base58()}),
            Self::Freeze { .. } => json!({"frozen": true}),
        }
    }
}

// ---- input parsing: the same JSON shapes the read methods return (sdk/rust/src/json.rs) ----

pub fn account(v: &Value, what: &str) -> Res<AccountId> {
    let s = v.as_str().ok_or_else(|| Failure::input(format!("{what} is missing")))?;
    AccountId::from_base58(s.trim().strip_prefix("Public/").unwrap_or(s.trim()))
        .ok_or_else(|| Failure::input(format!("{what} '{s}' is not an account id")))
}

fn opt_account(v: &Value, what: &str) -> Res<Option<AccountId>> {
    if v.is_null() { Ok(None) } else { account(v, what).map(Some) }
}

fn text(v: &Value, what: &str) -> Res<String> {
    v.as_str().map(str::to_owned).ok_or_else(|| Failure::input(format!("{what} must be text")))
}

fn opt_text(v: &Value, what: &str) -> Res<Option<String>> {
    if v.is_null() { Ok(None) } else { text(v, what).map(Some) }
}

fn hex_bytes(v: &Value, what: &str) -> Res<Vec<u8>> {
    let s = text(v, what)?;
    hex::decode(s.trim().strip_prefix("0x").unwrap_or(s.trim())).map_err(|_| Failure::input(format!("{what} is not hex")))
}

fn hex32(v: &Value, what: &str) -> Res<[u8; 32]> {
    hex_bytes(v, what)?.try_into().map_err(|_| Failure::input(format!("{what} must be 32 bytes (64 hex characters)")))
}

fn strings(v: &Value, what: &str) -> Res<Vec<String>> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(a) => a.iter().map(|s| text(s, what)).collect(),
        _ => Err(Failure::input(format!("{what} must be a list of text"))),
    }
}

/// `{cid, sha256, size, peer_hint?}`: `cid` is a CID as text (`raw` = the raw-codec CID of the
/// sha256), `peer_hint` hex.
pub fn content(v: &Value, what: &str) -> Res<ContentRef> {
    if !v.is_object() {
        return Err(Failure::input(format!("{what} must be {{cid, sha256, size}}")));
    }
    let sha256 = hex32(&v["sha256"], &format!("{what}.sha256"))?;
    let size = v["size"].as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| Failure::input(format!("{what}.size must be a number")))?;
    let cid_text = text(&v["cid"], &format!("{what}.cid"))?;
    let cid = if cid_text == "raw" {
        let mut c = vec![0x01, 0x55, 0x12, 0x20];
        c.extend_from_slice(&sha256);
        c
    } else {
        cid::parse(&cid_text).map_err(|e| Failure::input(format!("{what}.cid: {e}")))?
    };
    let peer_hint = if v["peer_hint"].is_null() { None } else { Some(hex_bytes(&v["peer_hint"], &format!("{what}.peer_hint"))?) };
    Ok(ContentRef { cid, sha256, size, peer_hint })
}

/// `{repo, commit (hex), docker_root, archive?}`.
pub fn source(v: &Value) -> Res<SourceRef> {
    if !v.is_object() {
        return Err(Failure::input("source must be {repo, commit, docker_root, archive?}"));
    }
    Ok(SourceRef {
        repo: opt_text(&v["repo"], "source.repo")?.unwrap_or_default(),
        commit: if v["commit"].is_null() { Vec::new() } else { hex_bytes(&v["commit"], "source.commit")? },
        docker_root: opt_text(&v["docker_root"], "source.docker_root")?.unwrap_or_else(|| ".".into()),
        archive: if v["archive"].is_null() { None } else { Some(content(&v["archive"], "source.archive")?) },
    })
}

/// The build tuple, as `resolve_program` shows it (`schema` and `builder` default to 1).
pub fn build(v: &Value) -> Res<BuildConfig> {
    if !v.is_object() {
        return Err(Failure::input("build must be an object (see resolve_program's deployer_entry.build)"));
    }
    let small = |k: &str| v[k].as_u64().map_or(Ok(1), |n| u8::try_from(n).map_err(|_| Failure::input(format!("build.{k} is out of range"))));
    let env = match &v["docker_env"] {
        Value::Null => Vec::new(),
        Value::Array(a) => a.iter().map(|e| Ok(EnvVar { key: text(&e["key"], "docker_env key")?, value: text(&e["value"], "docker_env value")? })).collect::<Res<_>>()?,
        _ => return Err(Failure::input("build.docker_env must be a list of {key, value}")),
    };
    Ok(BuildConfig {
        schema: small("schema")?,
        builder: small("builder")?,
        image_digest: hex32(&v["image_digest"], "build.image_digest")?,
        image_tag: opt_text(&v["image_tag"], "build.image_tag")?.unwrap_or_default(),
        risc0_build: opt_text(&v["risc0_build"], "build.risc0_build")?.unwrap_or_default(),
        host_risc0_rust: opt_text(&v["host_risc0_rust"], "build.host_risc0_rust")?.unwrap_or_default(),
        manifest_path: text(&v["manifest_path"], "build.manifest_path")?,
        binary: text(&v["binary"], "build.binary")?,
        features: strings(&v["features"], "build.features")?,
        lock_sha256: if v["lock_sha256"].is_null() { [0; 32] } else { hex32(&v["lock_sha256"], "build.lock_sha256")? },
        docker_env: env,
    })
}

fn invalid(m: &str) -> Failure {
    Failure::input(m)
}

/// The fields of a new entry.
struct Fields {
    name: String,
    version: String,
    author_label: String,
    description: String,
    tags: Vec<String>,
    idl: ContentRef,
    source: SourceRef,
    build: BuildConfig,
}

fn fields(v: &Value) -> Res<Fields> {
    let f = Fields {
        name: text(&v["name"], "name")?,
        version: text(&v["version"], "version")?,
        author_label: opt_text(&v["author_label"], "author_label")?.unwrap_or_default(),
        description: opt_text(&v["description"], "description")?.unwrap_or_default(),
        tags: strings(&v["tags"], "tags")?,
        idl: content(&v["idl"], "idl")?,
        source: source(&v["source"])?,
        build: build(&v["build"])?,
    };
    validate::entry_fields(&f.name, &f.version, &f.author_label, &f.description, &f.tags, &f.idl, &f.source, &f.build).map_err(invalid)?;
    Ok(f)
}

/// The claimed image id: the input's, else the live program's (`live`).
fn image(v: &Value, live: Option<[u8; 32]>) -> Res<[u8; 32]> {
    if v["image_id"].is_null() {
        live.ok_or_else(|| Failure::new("no_program", "no program is deployed at that id, so there is no image id to claim: pass image_id"))
    } else {
        hex32(&v["image_id"], "image_id")
    }
}

/// `RegisterDeployer` at `(program, REG)`: the program's header key signs, and `author` (default:
/// the program itself) co-signs.
pub fn register(reg: &AccountId, v: &Value, now: u64, live_image: Option<[u8; 32]>) -> Res<Built> {
    let x = account(&v["program"], "program")?;
    let author = opt_account(&v["author"], "author")?.unwrap_or(x);
    let f = fields(v)?;
    let ix = Instruction::RegisterDeployer {
        name: f.name.clone(),
        version: f.version,
        author_label: f.author_label,
        description: f.description,
        tags: f.tags,
        idl: f.idl,
        source: f.source,
        build: f.build,
        image_id: image(v, live_image)?,
        author_id: author.0,
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now,
        via_pda_seed: None,
    };
    let rest = [
        author,
        addr::release(reg, &x, 1),
        addr::global_head(reg),
        addr::name_head(reg, &f.name),
        addr::author_head(reg, &author),
        addr::link(reg, &x),
    ];
    Ok(Built::new("RegisterDeployer", x, ix, &rest, &[x, author], x))
}

/// `RegisterThirdParty`: a community entry for `program` by `publisher`, at `T(program, publisher)`.
pub fn register_third_party(reg: &AccountId, v: &Value, now: u64, live_image: Option<[u8; 32]>) -> Res<Built> {
    let program = account(&v["program"], "program")?;
    let publisher = account(&v["publisher"], "publisher")?;
    if program == publisher {
        return Err(invalid("a third-party entry cannot be for the publisher's own account"));
    }
    let f = fields(v)?;
    let at = addr::third_party_entry(reg, &program, &publisher);
    let ix = Instruction::RegisterThirdParty {
        program: program.0,
        name: f.name.clone(),
        version: f.version,
        author_label: f.author_label,
        description: f.description,
        tags: f.tags,
        idl: f.idl,
        source: f.source,
        build: f.build,
        image_id: image(v, live_image)?,
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now,
    };
    let rest = [
        at,
        addr::release(reg, &at, 1),
        addr::global_head(reg),
        addr::name_head(reg, &f.name),
        addr::author_head(reg, &publisher),
        addr::program_head(reg, &program),
        addr::link(reg, &at),
    ];
    Ok(Built::new("RegisterThirdParty", publisher, ix, &rest, &[publisher], at))
}

/// The entry an author-side change targets: `entry` (an entry address), or `program` for its
/// deployer entry, which lives at the program's own account.
pub fn target(v: &Value) -> Res<AccountId> {
    if v["entry"].is_null() { account(&v["program"], "entry (or program)") } else { account(&v["entry"], "entry") }
}

/// What the chain says about the entry being changed.
pub struct Stored {
    pub revision: u32,
    pub author: AccountId,
    pub registrant: AccountId,
    pub frozen: bool,
}

/// The signing author: the input's `author`, which must be the stored one.
fn author_of(v: &Value, stored: &Stored, entry: &AccountId) -> Res<AccountId> {
    let author = opt_account(&v["author"], "author")?.unwrap_or(stored.author);
    if author != stored.author {
        return Err(Failure::new("not_author", format!("{} is not the author of {}; its author is {}", author.to_base58(), entry.to_base58(), stored.author.to_base58())));
    }
    if stored.frozen {
        return Err(Failure::new("frozen", format!("{} is frozen: it can't be changed any more", entry.to_base58())));
    }
    Ok(author)
}

/// `Update` by the entry's author. Only the fields given change; `name` never does.
pub fn update(reg: &AccountId, v: &Value, now: u64, stored: &Stored) -> Res<Built> {
    let entry = target(v)?;
    let author = author_of(v, stored, &entry)?;
    for fixed in ["name", "kind", "publisher"] {
        if !v[fixed].is_null() {
            return Err(invalid(&format!("{fixed} can't be changed")));
        }
    }
    let revision = stored.revision.checked_add(1).ok_or_else(|| invalid("revision overflow"))?;
    let version = opt_text(&v["version"], "version")?;
    let description = opt_text(&v["description"], "description")?;
    let tags = if v["tags"].is_null() { None } else { Some(strings(&v["tags"], "tags")?) };
    let author_label = opt_text(&v["author_label"], "author_label")?;
    let idl = if v["idl"].is_null() { None } else { Some(content(&v["idl"], "idl")?) };
    let src = if v["source"].is_null() { None } else { Some(source(&v["source"])?) };
    let bld = if v["build"].is_null() { None } else { Some(build(&v["build"])?) };
    let image_id = if v["image_id"].is_null() { None } else { Some(hex32(&v["image_id"], "image_id")?) };
    if let Some(x) = &version { validate::version(x).map_err(invalid)?; }
    if let Some(x) = &description { validate::description(x).map_err(invalid)?; }
    if let Some(x) = &tags { validate::tags(x).map_err(invalid)?; }
    if let Some(x) = &author_label { validate::author_label(x).map_err(invalid)?; }
    if let Some(x) = &idl { validate::content_ref(x).map_err(invalid)?; }
    if let Some(x) = &src { validate::source_ref(x).map_err(invalid)?; }
    if let Some(x) = &bld { validate::build_config(x).map_err(invalid)?; }
    if version.is_none() && description.is_none() && tags.is_none() && author_label.is_none() && idl.is_none() && src.is_none() && bld.is_none() && image_id.is_none() {
        return Err(invalid("nothing to change: give at least one of version, description, tags, author_label, idl, source, build, image_id"));
    }
    let ix = Instruction::Update {
        entry_id: entry.0,
        revision,
        revision_seed: t::pda::revision_seed(revision),
        version,
        description,
        tags,
        author_label,
        idl,
        source: src,
        build: bld,
        image_id,
        now,
        via_pda_seed: None,
    };
    let rest = [entry, addr::release(reg, &entry, revision)];
    Ok(Built::new(format!("Update (revision {revision})"), author, ix, &rest, &[author], entry))
}

/// `TransferAuthor`: the current author and `new_author` both sign, in one transaction.
pub fn transfer(reg: &AccountId, v: &Value, now: u64, stored: &Stored) -> Res<Built> {
    let entry = target(v)?;
    let author = author_of(v, stored, &entry)?;
    let to = account(&v["new_author"], "new_author")?;
    if to == stored.author {
        return Err(invalid(&format!("{} is already the author", to.to_base58())));
    }
    if to == stored.registrant {
        return Err(Failure::new("transfer_back", "a transfer back to a previous author is refused (an entry joins each author's list once)"));
    }
    let ix = Instruction::TransferAuthor { entry_id: entry.0, new_author_id: to.0, now, via_pda_seed: None };
    let rest = [entry, to, addr::author_head(reg, &to), addr::author_node(reg, &to, &entry)];
    Ok(Built::new("TransferAuthor", author, ix, &rest, &[author, to], entry))
}

/// `Freeze`: irreversible.
pub fn freeze(_reg: &AccountId, v: &Value, now: u64, stored: &Stored) -> Res<Built> {
    let entry = target(v)?;
    let author = author_of(v, stored, &entry)?;
    let ix = Instruction::Freeze { entry_id: entry.0, now, via_pda_seed: None };
    Ok(Built::new("Freeze", author, ix, &[entry], &[author], entry))
}

/// The clock the registry's time window is checked against: actor `(clock account, clock
/// program)` (LEZ rc3 has one clock account). Pinned by the vectors (`clock_account`,
/// `clock_program`).
pub fn clock_actor() -> (AccountId, AccountId) {
    let clock_account = AccountId::new(*b"/LEZ/ClockProgramAccount/0000000");
    (clock_account, clock_program())
}

/// `clock_core::clock_account_id()` (`AccountId::from_builtin_program_name(b"clock")`) on LEZ
/// v0.4.0-rc3, checked against the vectors file.
fn clock_program() -> AccountId {
    AccountId::from_base58(CLOCK_PROGRAM).expect("a valid id")
}

pub const CLOCK_PROGRAM: &str = "2hwaRW8sMnyKLLRbDRw8k68tiv9bnfq98oxcdtcp157Z";

/// The chain's time from the clock account's state (`block_id: u64, timestamp: u64`, borsh).
pub fn clock_time(state: &[u8]) -> Option<u64> {
    let ts: [u8; 8] = state.get(8..16)?.try_into().ok()?;
    Some(u64::from_le_bytes(ts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../testdata/write-vectors.json")).expect("vectors JSON")
    }

    fn id(v: &Value) -> AccountId {
        AccountId::from_base58(v.as_str().unwrap()).unwrap()
    }

    /// Every vector, rebuilt from its input, gives the real program's bytes, actors and signers.
    #[test]
    fn every_write_vector_is_rebuilt_exactly() {
        let all = vectors();
        let reg = id(&all["registry"]);
        let now = all["now"].as_u64().unwrap();
        let cases = all["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 8);
        for c in cases {
            let input = &c["input"];
            let stored = |author: &str| Stored {
                revision: c["stored_revision"].as_u64().map_or(1, |r| r as u32),
                author: AccountId::from_base58(author).unwrap(),
                registrant: AccountId::new([0x99; 32]),
                frozen: false,
            };
            let author = input["author"].as_str().unwrap_or_default();
            let built = match c["op"].as_str().unwrap() {
                "register" => register(&reg, input, now, None),
                "register_third_party" => register_third_party(&reg, input, now, None),
                "update" => update(&reg, input, now, &stored(author)),
                "transfer" => transfer(&reg, input, now, &stored(author)),
                "freeze" => freeze(&reg, input, now, &stored(author)),
                other => panic!("unknown op {other}"),
            }
            .unwrap_or_else(|e| panic!("{}: {e:?}", c["op"]));
            assert_eq!(hex::encode(built.message()), c["message"].as_str().unwrap(), "{} message", c["op"]);
            assert_eq!(built.root, id(&c["root"]), "{} root", c["op"]);
            let accounts: Vec<String> = built.accounts.iter().map(AccountId::to_base58).collect();
            let want: Vec<String> = c["accounts"].as_array().unwrap().iter().map(|p| {
                assert_eq!(p[1], all["registry"], "every actor is the registry's");
                p[0].as_str().unwrap().to_owned()
            }).collect();
            assert_eq!(accounts, want, "{} accounts", c["op"]);
            let signers: Vec<String> = built.signers.iter().map(AccountId::to_base58).collect();
            let want: Vec<String> = c["signers"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect();
            assert_eq!(signers, want, "{} signers", c["op"]);
            assert_eq!(built.entry, id(&c["entry"]), "{} entry", c["op"]);
            // The change a signer gets lists the same accounts, root first, with who signs.
            let change = built.change("http://127.0.0.1:3062", &reg, now, Value::Null);
            assert_eq!(change["accounts"].as_array().unwrap().len(), want_len(c));
            assert_eq!(change["expires_at"].as_u64().unwrap(), now + t::TIME_WINDOW_MS);
        }
    }

    fn want_len(c: &Value) -> usize {
        c["accounts"].as_array().unwrap().len()
    }

    /// A card shows what is signed: the values come from the instruction, not the input.
    #[test]
    fn values_are_decoded_from_the_instruction() {
        let all = vectors();
        let reg = id(&all["registry"]);
        let c = &all["cases"][0];
        let built = register(&reg, &c["input"], 1, None).unwrap();
        let v = built.ix.values();
        assert_eq!((v["name"].as_str(), v["version"].as_str()), (Some("token"), Some("1.2.0")));
        assert_eq!(v["image_id"], "44".repeat(32));
        assert_eq!(v["build"]["binary"], "token");
        assert_eq!(v["author"], c["input"]["author"]);
        let a = AccountId::new([8; 32]);
        let stored = Stored { revision: 3, author: a, registrant: a, frozen: false };
        let u = update(&reg, &json!({"program": AccountId::new([7; 32]).to_base58(), "version": "2.0.0"}), 1, &stored).unwrap();
        assert_eq!(u.ix.values(), json!({"revision": 4, "version": "2.0.0"}));
        assert!(max_gas(&u.ix) >= 200_000 && max_gas(&built.ix) >= 300_000);
    }

    #[test]
    fn the_clock_ids_match_the_chain() {
        let all = vectors();
        let (acct, prog) = clock_actor();
        assert_eq!(acct.to_base58(), all["clock_account"].as_str().unwrap());
        assert_eq!(prog.to_base58(), all["clock_program"].as_str().unwrap());
        let mut state = 7u64.to_le_bytes().to_vec();
        state.extend_from_slice(&1_790_000_000_000u64.to_le_bytes());
        assert_eq!(clock_time(&state), Some(1_790_000_000_000));
        assert_eq!(clock_time(&[1, 2]), None);
    }

    #[test]
    fn refusals_are_coded_before_anything_is_built() {
        let all = vectors();
        let reg = id(&all["registry"]);
        let reg_input = &all["cases"][0]["input"];
        let mut bad = reg_input.clone();
        bad["name"] = json!("Bad Name");
        assert_eq!(register(&reg, &bad, 1, None).unwrap_err().code, "bad_input");
        let mut no_image = reg_input.clone();
        no_image["image_id"] = Value::Null;
        assert_eq!(register(&reg, &no_image, 1, None).unwrap_err().code, "no_program");
        assert!(register(&reg, &no_image, 1, Some([1; 32])).is_ok(), "the live image is claimed by default");
        let a = AccountId::new([8; 32]);
        let stored = Stored { revision: 1, author: a, registrant: a, frozen: false };
        let x = json!({"entry": AccountId::new([7; 32]).to_base58(), "author": AccountId::new([5; 32]).to_base58(), "version": "2"});
        assert_eq!(update(&reg, &x, 1, &stored).unwrap_err().code, "not_author");
        let frozen = Stored { frozen: true, ..stored };
        let x = json!({"program": AccountId::new([7; 32]).to_base58(), "version": "2"});
        assert_eq!(update(&reg, &x, 1, &frozen).unwrap_err().code, "frozen");
        let stored = Stored { frozen: false, ..frozen };
        assert_eq!(update(&reg, &json!({"program": AccountId::new([7; 32]).to_base58()}), 1, &stored).unwrap_err().code, "bad_input");
        assert_eq!(update(&reg, &json!({"program": AccountId::new([7; 32]).to_base58(), "name": "x"}), 1, &stored).unwrap_err().code, "bad_input");
        let back = json!({"program": AccountId::new([7; 32]).to_base58(), "new_author": a.to_base58()});
        assert!(transfer(&reg, &back, 1, &stored).is_err(), "already the author");
        let stored = Stored { registrant: AccountId::new([3; 32]), ..stored };
        let back = json!({"program": AccountId::new([7; 32]).to_base58(), "new_author": AccountId::new([3; 32]).to_base58()});
        assert_eq!(transfer(&reg, &back, 1, &stored).unwrap_err().code, "transfer_back");
    }
}
