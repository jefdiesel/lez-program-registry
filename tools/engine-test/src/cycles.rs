//! The operations docs/CYCLES.md documents, each measured with [`crate::bench::measure`] on the
//! committed registry binary (`artifacts/`), with a fee declared as a real client sends it.
//!
//! Field sets: `min` is every field at its smallest valid value; `typical` and `worst` are the
//! §2.3 size-table fixtures of `program/tests/handlers.rs` (`record_sizes_match_the_design_table`).
//! Every registration below uses a name and an author no earlier registration used, so each
//! one creates its name and author heads (the most a registration writes), except where the
//! label says "taken name".

use std::collections::BTreeMap;

use lee_core::{
    account::{AccountId, Actor},
    program::PROGRAM_LOADER_ACCOUNT_ID,
};
use lezreg_program::{Instruction, actors_to_declare};
use lezreg_types::{self as t, BuildConfig, ContentRef, EnvVar, SourceRef};
use program_loader_core::Message as LoaderMessage;

use crate::{
    Engine, Key, Send, T0, artifact_elf, ballast_elfs, bench, build, forwarder_elf, image_bytes, image_id_of, key, pda,
};

/// Key seed of the registry's program id.
pub const REG_SEED: u8 = 0x70;
/// Key seed of the forwarder (the owner program of the PDA case).
pub const FWD_SEED: u8 = 0x71;

/// What each `Instruction` field set fills in.
#[derive(Debug, Clone)]
pub struct Fields {
    pub name: String,
    pub version: String,
    pub author_label: String,
    pub description: String,
    pub tags: Vec<String>,
    pub idl: ContentRef,
    pub source: SourceRef,
    pub build: BuildConfig,
}

fn min_content() -> ContentRef {
    ContentRef { cid: vec![1], sha256: [2; 32], size: 1, peer_hint: None }
}

fn typical_content() -> ContentRef {
    ContentRef { cid: vec![1; 38], sha256: [2; 32], size: 1000, peer_hint: None }
}

fn max_content() -> ContentRef {
    ContentRef { cid: vec![1; 64], sha256: [2; 32], size: 1000, peer_hint: Some(vec![3; 48]) }
}

fn max_build() -> BuildConfig {
    BuildConfig {
        image_tag: "i".repeat(48),
        risc0_build: "r".repeat(16),
        host_risc0_rust: "h".repeat(16),
        manifest_path: "m".repeat(96),
        binary: "b".repeat(48),
        features: vec!["f".repeat(32); 4],
        docker_env: vec![EnvVar { key: "K".repeat(16), value: "v".repeat(48) }; 4],
        ..build()
    }
}

fn max_source() -> SourceRef {
    SourceRef { repo: "r".repeat(160), commit: vec![3; 32], docker_root: "d".repeat(96), archive: Some(max_content()) }
}

/// Smallest valid fields (`name` is set by the caller).
#[must_use]
pub fn min(name: &str) -> Fields {
    Fields {
        name: name.into(),
        version: "1".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: min_content(),
        source: SourceRef { repo: String::new(), commit: vec![], docker_root: String::new(), archive: Some(min_content()) },
        build: BuildConfig {
            image_tag: String::new(),
            risc0_build: String::new(),
            host_risc0_rust: String::new(),
            manifest_path: "m".into(),
            binary: "b".into(),
            features: vec![],
            docker_env: vec![],
            ..build()
        },
    }
}

/// The §2.3 "typical" fixture: a 13-character name, one sentence, two tags, a 38-byte CID, a
/// source archive.
#[must_use]
pub fn typical(name: &str) -> Fields {
    Fields {
        name: name.into(),
        version: "1.2.3".into(),
        author_label: "Example Labs".into(),
        description: "A fungible token program with mint, burn and transfer, deployed on LEZ.".into(),
        tags: vec!["token".into(), "defi".into()],
        idl: typical_content(),
        source: SourceRef {
            repo: "https://github.com/example-org/example-program.git".into(),
            commit: vec![3; 20],
            docker_root: ".".into(),
            archive: Some(typical_content()),
        },
        build: build(),
    }
}

/// The §2.3 "worst case" fixture: every field at its limit (the name is padded to 32 bytes).
#[must_use]
pub fn worst(name: &str) -> Fields {
    Fields {
        name: format!("{name:a<32}"),
        version: "v".repeat(32),
        author_label: "é".repeat(32),
        description: "d".repeat(512),
        tags: vec!["t".repeat(24); 8],
        idl: max_content(),
        source: max_source(),
        build: max_build(),
    }
}

fn reg_deployer(f: Fields, author: &AccountId, image_id: [u8; 32], via_pda_seed: Option<[u8; 32]>) -> Instruction {
    Instruction::RegisterDeployer {
        name: f.name,
        version: f.version,
        author_label: f.author_label,
        description: f.description,
        tags: f.tags,
        idl: f.idl,
        source: f.source,
        build: f.build,
        image_id,
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
        via_pda_seed,
    }
}

fn reg_third_party(f: Fields, program: &AccountId) -> Instruction {
    Instruction::RegisterThirdParty {
        program: *program.value(),
        name: f.name,
        version: f.version,
        author_label: f.author_label,
        description: f.description,
        tags: f.tags,
        idl: f.idl,
        source: f.source,
        build: f.build,
        image_id: [0x33; 32],
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
    }
}

fn deploy_and_register(f: Fields, author: &AccountId, first_segment: AccountId, image_id: [u8; 32]) -> Instruction {
    Instruction::DeployAndRegister {
        first_segment: *first_segment.value(),
        immutable: false,
        name: f.name,
        version: f.version,
        author_label: f.author_label,
        description: f.description,
        tags: f.tags,
        idl: f.idl,
        source: f.source,
        build: f.build,
        image_id,
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
    }
}

/// `Update` of revision `revision`: `None` fields keep the stored value.
#[derive(Default)]
struct Up {
    version: Option<String>,
    description: Option<String>,
    tags: Option<Vec<String>>,
    author_label: Option<String>,
    idl: Option<ContentRef>,
    source: Option<SourceRef>,
    build: Option<BuildConfig>,
    image_id: Option<[u8; 32]>,
}

fn update(entry: &AccountId, revision: u32, u: Up) -> Instruction {
    Instruction::Update {
        entry_id: *entry.value(),
        revision,
        revision_seed: t::pda::revision_seed(revision),
        version: u.version,
        description: u.description,
        tags: u.tags,
        author_label: u.author_label,
        idl: u.idl,
        source: u.source,
        build: u.build,
        image_id: u.image_id,
        now: T0,
        via_pda_seed: None,
    }
}

/// One measured operation, for CYCLES.md.
#[derive(Debug, Clone)]
pub struct Op {
    /// The operation (`RegisterDeployer`, `WriteSegment`, ...).
    pub op: String,
    /// The case.
    pub case: String,
    pub m: bench::Measured,
    /// Deploy cases: the program binary's size and its user ELF's size.
    pub program_bytes: Option<(usize, usize)>,
}

impl Op {
    /// Data (storage gas) bytes of the charged transaction: borsh `LeeTransaction` = the
    /// `PublicTransaction` + its 1-byte enum tag.
    #[must_use]
    pub fn data_bytes(&self) -> u64 {
        self.m.tx_bytes as u64 + 1
    }

    /// The fee at LEZ's minimum (genesis) base fees, 8 per execution gas and 8 per data byte,
    /// tip 0: `charged × 8 + data_bytes × 8` (`fee_core::assess::fee_actual_base`).
    #[must_use]
    pub fn fee_at_min_base_fees(&self) -> u128 {
        u128::from(self.m.charged) * 8 + u128::from(self.data_bytes()) * 8
    }
}

struct Run {
    e: Engine,
    names: BTreeMap<AccountId, String>,
    ops: Vec<Op>,
}

impl Run {
    fn tx(&mut self, op: &str, case: &str, root: Actor, message: Vec<u8>, actors: Vec<Actor>, signers: &[&Key]) -> &Op {
        let tx = self.e.build_tx(root, message, actors, signers);
        let label = format!("{op} / {case}");
        let (m, _) = bench::measure(&mut self.e, &label, &tx, &self.names).unwrap_or_else(|err| panic!("{err}"));
        self.ops.push(Op { op: op.into(), case: case.into(), m, program_bytes: None });
        self.ops.last().expect("pushed")
    }

    fn ix(&mut self, op: &str, case: &str, root_account: AccountId, ix: &Instruction, signers: &[&Key]) -> &Op {
        let actors = actors_to_declare(root_account, self.e.reg, ix);
        let root = self.e.actor(root_account);
        self.tx(op, case, root, borsh::to_vec(ix).expect("borsh"), actors, signers)
    }

    /// Writes `elf`'s segments tail to head (each signed by its own new key), measuring each.
    fn write_segments(&mut self, what: &str, elf: &[u8], first_key_seed: u8) -> AccountId {
        let user_elf = risc0_binfmt::ProgramBinary::decode(elf).expect("a risc0 binary").user_elf.to_vec();
        let chunks: Vec<&[u8]> = user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).collect();
        let keys: Vec<Key> = (0..chunks.len()).map(|i| key(first_key_seed + u8::try_from(i).expect("few segments"))).collect();
        for i in (0..chunks.len()).rev() {
            let to = Actor::new(keys[i].id, PROGRAM_LOADER_ACCOUNT_ID);
            let m = LoaderMessage::WriteSegment { bytecode: chunks[i].to_vec(), next_segment: keys.get(i + 1).map(|k| k.id) };
            let case = format!("{what}, segment {}/{} ({} bytes{})", i + 1, chunks.len(), chunks[i].len(), if i + 1 < chunks.len() { ", links the next" } else { ", the tail" });
            self.tx("WriteSegment", &case, to, borsh::to_vec(&m).expect("borsh"), vec![to], &[&keys[i]]);
        }
        keys[0].id
    }
}

/// Runs every operation once, in a fixed order on one state, and returns the measurements.
/// Deterministic: the same registry binary and fixtures give the same numbers on any machine.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn run() -> Vec<Op> {
    let (reg, fwd) = (key(REG_SEED), key(FWD_SEED));
    // Funded: alice 3, bob 4, the publisher q 5, carol 6; authors of the deploy cases 0x40..
    let funded: Vec<AccountId> = [3, 4, 5, 6, 0x40, 0x41, 0x42, 0x43].iter().map(|s| key(*s).id).collect();
    let mut e = Engine::with_registry(artifact_elf(), reg.id, &[fwd.id], &funded);
    e.declare_fee = true;
    let names = BTreeMap::from([(reg.id, "registry".to_owned()), (fwd.id, "forwarder".to_owned())]);
    let mut r = Run { e, names, ops: Vec::new() };
    let (alice, bob, q, carol) = (key(3), key(4), key(5), key(6));
    let (x1, x2, x3, x4) = (key(0x10), key(0x11), key(0x12), key(0x13));
    let reg_image = image_bytes(&image_id_of(artifact_elf()));

    // The registry's self-entry, as at launch (also creates the global head every later
    // registration finds).
    let ix = reg_deployer(typical("lez_program_registry"), &reg.id, reg_image, None);
    r.ix("RegisterDeployer", "the registry's own entry at launch (REG signs), typical fields", reg.id, &ix, &[&reg]);

    // RegisterDeployer.
    r.ix("RegisterDeployer", "min fields, author = X (one signature)", x1.id, &reg_deployer(min("a"), &x1.id, [7; 32], None), &[&x1]);
    r.ix("RegisterDeployer", "typical fields, author ≠ X (two signatures)", x2.id, &reg_deployer(typical("token-program"), &alice.id, [8; 32], None), &[&x2, &alice]);
    r.ix("RegisterDeployer", "worst-case fields, author ≠ X", x3.id, &reg_deployer(worst("w"), &bob.id, [9; 32], None), &[&x3, &bob]);
    r.ix("RegisterDeployer", "typical fields, taken name, author = X", x4.id, &reg_deployer(typical("token-program"), &x4.id, [10; 32], None), &[&x4]);

    // RegisterDeployer through the PDA path: owner program P (the forwarder) registers its PDA
    // header X = pda(P, s) (author = X), in a transaction alice signs.
    let s = [0x5E; 32];
    let xp = pda(&fwd.id, s);
    let ix = reg_deployer(typical("pda-program"), &xp, [11; 32], Some(s));
    let sends = vec![Send::to(r.e.actor(xp), borsh::to_vec(&ix).expect("borsh"), vec![s])];
    let actors = actors_to_declare(xp, reg.id, &ix);
    r.tx("RegisterDeployer", "via_pda_seed through an owner program, typical fields", Actor::new(alice.id, fwd.id), borsh::to_vec(&sends).expect("borsh"), actors, &[&alice]);

    // RegisterThirdParty.
    r.ix("RegisterThirdParty", "min fields", q.id, &reg_third_party(min("a3"), &x1.id), &[&q]);
    r.ix("RegisterThirdParty", "typical fields", carol.id, &reg_third_party(typical("token-program-3p"), &x2.id), &[&carol]);
    let q2 = key(0x14);
    r.ix("RegisterThirdParty", "worst-case fields", q2.id, &reg_third_party(worst("w3"), &x1.id), &[&q2]);

    // Update.
    r.ix("Update", "one field (version), author ≠ X (PatchEntry)", alice.id, &update(&x2.id, 2, Up { version: Some("1.2.4".into()), ..Up::default() }), &[&alice]);
    let typical_up = Up {
        version: Some("1.3.0".into()),
        description: Some("A fungible token program with mint, burn and transfer; now with permits.".into()),
        idl: Some(typical_content()),
        ..Up::default()
    };
    r.ix("Update", "typical (version, description, IDL), author ≠ X", alice.id, &update(&x2.id, 3, typical_up), &[&alice]);
    let worst_up = Up {
        version: Some("u".repeat(32)),
        description: Some("e".repeat(512)),
        tags: Some(vec!["u".repeat(24); 8]),
        author_label: Some("ü".repeat(32)),
        idl: Some(max_content()),
        source: Some(max_source()),
        build: Some(max_build()),
        image_id: Some([12; 32]),
    };
    r.ix("Update", "every field at its limit, author ≠ X", bob.id, &update(&x3.id, 2, worst_up), &[&bob]);
    r.ix("Update", "one field (version), author = X (inline)", x1.id, &update(&x1.id, 2, Up { version: Some("2".into()), ..Up::default() }), &[&x1]);
    let tp = t::pda::addr::third_party_entry(&r.e.treg(), &crate::tid(&x2.id), &crate::tid(&carol.id));
    r.ix("Update", "third-party entry, one field (version)", carol.id, &update(&crate::lid(&tp), 2, Up { version: Some("0.2.0".into()), ..Up::default() }), &[&carol]);

    // TransferAuthor (both sign) and Freeze.
    let tr = Instruction::TransferAuthor { entry_id: *x2.id.value(), new_author_id: *carol.id.value(), now: T0, via_pda_seed: None };
    r.ix("TransferAuthor", "alice → carol, both sign", alice.id, &tr, &[&alice, &carol]);
    r.ix("Freeze", "author ≠ X (PatchEntry)", carol.id, &Instruction::Freeze { entry_id: *x2.id.value(), now: T0, via_pda_seed: None }, &[&carol]);
    r.ix("Freeze", "author = X (inline)", x1.id, &Instruction::Freeze { entry_id: *x1.id.value(), now: T0, via_pda_seed: None }, &[&x1]);

    // Deploys: segment writes, then DeployAndRegister (typical fields, author ≠ X, a new name
    // and author each), for four program sizes.
    let [(b430_name, b430), (b1m_name, b1m)] = ballast_elfs();
    let programs: [(&str, &'static [u8]); 4] =
        [("forwarder", forwarder_elf()), (b430_name, b430), ("lez_program_registry", artifact_elf()), (b1m_name, b1m)];
    for (i, (what, elf)) in programs.iter().enumerate() {
        let i8 = u8::try_from(i).expect("four");
        let x = key(0x20 + i8);
        let author = key(0x40 + i8);
        let first = r.write_segments(what, elf, 0x80 + i8 * 0x18);
        let image = image_bytes(&image_id_of(elf));
        // 13-character names, as `typical`'s, so the registry's part is comparable across sizes.
        let ix = deploy_and_register(typical(&format!("deploy-size-{}", i + 1)), &author.id, first, image);
        let user_len = risc0_binfmt::ProgramBinary::decode(elf).expect("a risc0 binary").user_elf.len();
        r.ix("DeployAndRegister", &format!("{what}: {} B binary, {user_len} B user ELF", elf.len()), x.id, &ix, &[&x, &author]);
        r.ops.last_mut().expect("pushed").program_bytes = Some((elf.len(), user_len));
        if i == 2 {
            // The plain loader path on the same segment chain: CreateHeader, then UpdateHeader.
            let h = key(0x30);
            let to = Actor::new(h.id, PROGRAM_LOADER_ACCOUNT_ID);
            let m = LoaderMessage::CreateHeader { first_segment: first, immutable: false };
            r.tx("CreateHeader", &format!("{what} ({user_len} B user ELF), plain loader"), to, borsh::to_vec(&m).expect("borsh"), vec![to], &[&h]);
            r.ops.last_mut().expect("pushed").program_bytes = Some((elf.len(), user_len));
            let m = LoaderMessage::UpdateHeader { first_segment: first, immutable: false };
            r.tx("UpdateHeader", &format!("{what} ({user_len} B user ELF), plain loader"), to, borsh::to_vec(&m).expect("borsh"), vec![to], &[&h]);
            r.ops.last_mut().expect("pushed").program_bytes = Some((elf.len(), user_len));
        }
    }
    r.ops
}

/// The rows as a Markdown table (what `lezreg-cycles` prints).
#[must_use]
pub fn markdown(ops: &[Op]) -> String {
    let mut s = String::from(
        "| Operation | Case | Transitions (user cycles) | Registry guest cycles | Loader gas | Execution gas | Data bytes | Fee at base fees 8/8 |\n|---|---|---|---:|---:|---:|---:|---:|\n",
    );
    for o in ops {
        let rows: Vec<String> = o.m.rows.iter().map(|r| format!("{} {}", r.program, r.cycles)).collect();
        let reg: u64 = o.m.rows.iter().filter(|r| r.program == "registry").map(|r| r.cycles).sum();
        s += &format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            o.op,
            o.case,
            rows.join(" + "),
            reg,
            o.m.loader_cycles(),
            o.m.charged,
            o.data_bytes(),
            o.fee_at_min_base_fees()
        );
    }
    s
}
