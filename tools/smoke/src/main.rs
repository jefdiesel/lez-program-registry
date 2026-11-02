//! M1 smoke client (the full CLI is M2). Against a LOCAL rc1 sequencer only (refuses any
//! non-loopback RPC), it:
//!
//! 1. deploys the registry guest (segments, then loader `CreateHeader` at a fresh header R) and
//!    writes REG's self-entry at (R, R) with `RegisterDeployer` (DESIGN §7.5, N5);
//! 2. writes a sample program's segments and runs one `DeployAndRegister` (X and author A sign);
//! 3. one third-party `RegisterThirdParty` for X (publisher Q signs);
//! 4. one `Update` by A (revision 2);
//! 5. the Mallory negative: `RegisterDeployer` at (X3, REG) for a header X3 she does not hold,
//!    signed only by her. Simulation refuses it ("must sign"); it is then forced on chain with a
//!    gas limit above what the honestly signed instruction needs, and must revert, charged to
//!    Mallory (at least the full limit × base fee);
//! 6. reads back: one `getAccount(X)` returns the deployer entry and the loader header; the
//!    global, name, author and third-party list walks.
//!
//! Registry transactions are built here, not by the wallet: actors from `actors_to_declare`,
//! **no admission evidence**, a fee declaration for payer P, and signatures from the wallet's
//! keys. Gas limit = simulated cycles × 1.5 (min 30,000), as DESIGN §2.7.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::Parser;
use common::{HashType, transaction::LeeTransaction};
use lee::{
    FeeDeclaration, PrivateKey, PublicTransaction, Simulation,
    privacy_preserving_transaction::circuit::ProgramCatalog,
    program::Program,
    public_transaction::{Message as TxMessage, WitnessSet},
    simulate_public_transaction,
};
use lee_core::{
    account::{AccountId, Actor, Nonce},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ProgramSegment},
};
use lezreg_program::{Instruction, actors_to_declare};
use lezreg_types::{self as t, pda::addr, walk};
use sequencer_service_rpc::SequencerClient;
use wallet::{
    AccountIdentity, WalletCore, WalletSequencerClient as _,
    config::{SequencerConnectionData, WalletConfigOverrides},
    program_facades::{CreditDelivery, native_token_transfer::NativeTokenTransfer, program_loader::ProgramLoader},
    utils::{fetch_config_path, fetch_persistent_storage_path, read_password},
};

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:3062")]
    rpc: String,
    /// The registry guest (`lez_program_registry.bin`).
    #[arg(long)]
    reg_elf: String,
    /// A sample program to deploy with DeployAndRegister.
    #[arg(long)]
    sample_elf: String,
    /// A funded wallet account that pays for everything.
    #[arg(long)]
    funder: AccountId,
    /// Reuse an already deployed registry instead of deploying one.
    #[arg(long)]
    reg: Option<AccountId>,
}

fn ensure_local(rpc: &str) -> Result<()> {
    if !(rpc.starts_with("http://127.0.0.1:") || rpc.starts_with("http://localhost:")) {
        bail!("refusing non-local RPC {rpc}");
    }
    Ok(())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn tid(a: &AccountId) -> t::AccountId {
    t::AccountId(*a.value())
}

fn image_bytes(id: &[u32; 8]) -> [u8; 32] {
    let mut out = [0; 32];
    for (i, w) in id.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

struct Outcome {
    block: u64,
    reverted: bool,
    fee: u128,
    tx_bytes: usize,
    cycles: Option<u64>,
    /// The declared gas limit.
    limit: u64,
    /// Why simulation refused the transaction, when it did.
    sim_error: Option<String>,
}

struct Smoke {
    w: WalletCore<SequencerClient>,
    reg: AccountId,
    reg_program: Option<Program>,
    rows: Vec<String>,
}

impl Smoke {
    /// A fresh account: the wallet derives keys deterministically, so a copied wallet hands out
    /// accounts an earlier run may already have used on this chain. Skip those.
    async fn new_account(&mut self) -> AccountId {
        loop {
            let id = self.w.create_new_account_public(None).0;
            match self.w.client().get_account(id).await {
                Ok(None) => return id,
                Ok(Some(a)) if a == lee_core::account::Account::default() => return id,
                Ok(Some(_)) => {}
                Err(e) => panic!("getAccount {id}: {e}"),
            }
        }
    }

    async fn state(&self, actor: Actor) -> Result<Vec<u8>> {
        let view = self.w.get_account_view(actor).await?;
        Ok(view.map_or_else(Vec::new, |found| found.data.actor_state(actor.program_account_id).to_vec()))
    }

    async fn fund(&mut self, from: AccountId, to: AccountId, amount: u128) -> Result<()> {
        let mut config = self.w.config().clone();
        config.gas_limit = 1_000;
        self.w.set_config(config);
        let (hash, _) = NativeTokenTransfer(&self.w)
            .transfer(AccountIdentity::Public(from), AccountIdentity::PublicNoSign(to), amount, CreditDelivery::Automatic)
            .await?;
        let block = self.finish(hash).await?;
        println!("funded {to} with {amount} (block {block})");
        Ok(())
    }

    async fn finish(&self, hash: HashType) -> Result<u64> {
        let (_, block_id) = self.w.poll_transaction(hash).await.context("not included")?;
        if self.is_reverted(hash, block_id).await? {
            bail!("{hash} reverted in block {block_id}");
        }
        Ok(block_id)
    }

    async fn is_reverted(&self, hash: HashType, block_id: u64) -> Result<bool> {
        let block = self.w.get_block(block_id).await?.ok_or_else(|| anyhow!("block {block_id} missing"))?;
        Ok(block.body.transactions.iter().position(|t| t.hash() == hash).is_some_and(|p| block.body.is_reverted(p)))
    }

    /// Writes `elf`'s user ELF into fresh segment accounts (tail to head); returns the first.
    async fn write_segments(&mut self, elf: &[u8], payer: AccountId) -> Result<AccountId> {
        let binary = risc0_binfmt::ProgramBinary::decode(elf).map_err(|e| anyhow!("not a risc0 binary: {e}"))?;
        let chunks: Vec<Vec<u8>> = binary.user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).map(<[u8]>::to_vec).collect();
        let mut segs = Vec::new();
        for _ in &chunks {
            segs.push(self.new_account().await);
        }
        self.w.store_persistent_data()?;
        let mut config = self.w.config().clone();
        config.gas_limit = 1_000;
        self.w.set_config(config);
        let before = self.w.get_account_balance(payer).await?;
        for i in (0..chunks.len()).rev() {
            let hash = ProgramLoader(&self.w).write_segment(segs[i], chunks[i].clone(), segs.get(i + 1).copied(), Some(payer)).await?;
            self.finish(hash).await?;
        }
        let fee = before - self.w.get_account_balance(payer).await?;
        println!("wrote {} segments ({} bytes of user ELF), fee {fee}", chunks.len(), binary.user_elf.len());
        self.rows.push(format!("| WriteSegment ×{} | {} B user ELF | applied | {fee} |", chunks.len(), binary.user_elf.len()));
        Ok(segs[0])
    }

    /// Simulated cycles of a transaction, with the loader segment chain supplied when given.
    async fn simulate(&self, root: Actor, message: &[u8], declared: &[Actor], signers: &[AccountId], first_segment: Option<AccountId>) -> Result<u64> {
        let mut states = HashMap::new();
        for a in declared {
            states.insert(*a, self.state(*a).await?.into());
        }
        let mut next = first_segment;
        let mut n = 0;
        while let Some(seg) = next {
            let actor = Actor::new(seg, PROGRAM_LOADER_ACCOUNT_ID);
            let s = self.state(actor).await?;
            next = ProgramSegment::from_bytes(&s).and_then(|p| p.next_segment);
            states.insert(actor, s.into());
            n += 1;
            if n > 32 {
                break;
            }
        }
        let nonces = self.w.get_accounts_nonces(signers).await?;
        let tx = TxMessage::new(root, message.to_vec(), declared.to_vec(), signers.iter().copied().zip(nonces).collect(), None, Vec::new());
        let tx = PublicTransaction::new(tx.clone(), WitnessSet::for_message(&tx, &[]));
        let catalog: ProgramCatalog = self.reg_program.clone().map(|p| (self.reg, p)).into_iter().collect();
        let budget = self.w.client().get_fee_state().await?.max_gas_exec;
        let sim = Simulation { public_actor_states: states, admitted_accounts: None };
        let (cycles, result) = tokio::task::spawn_blocking(move || simulate_public_transaction(&tx, &sim, &catalog, budget)).await?;
        result.map_err(|e| anyhow!("{e}"))?;
        Ok(cycles)
    }

    /// Builds, signs (wallet keys), sends and settles one transaction. No admission evidence.
    #[allow(clippy::too_many_arguments)]
    async fn send(
        &mut self,
        what: &str,
        root: Actor,
        message: Vec<u8>,
        actors: Vec<Actor>,
        signers: &[AccountId],
        payer: AccountId,
        first_segment: Option<AccountId>,
        force_gas: Option<u64>,
    ) -> Result<Outcome> {
        let mut declared: Vec<Actor> = Vec::new();
        for a in std::iter::once(root).chain(actors) {
            if !declared.contains(&a) {
                declared.push(a);
            }
        }
        println!("\n== {what}\n   root {root:?}; {} actors declared; {} message bytes", declared.len(), message.len());
        let sim = self.simulate(root, &message, &declared, signers, first_segment).await;
        let limit = match (&sim, force_gas) {
            (Ok(c), None) => {
                println!("   simulated: OK, {c} cycles");
                c.saturating_add(c.div_ceil(2)).max(30_000)
            }
            (Ok(c), Some(g)) => {
                println!("   simulated: OK, {c} cycles; forcing gas limit {g}");
                g
            }
            (Err(e), Some(g)) => {
                println!("   simulated: REFUSED: {e}\n   sending anyway with gas limit {g}");
                g
            }
            (Err(e), None) => bail!("{what}: simulation refused: {e}"),
        };
        let all: BTreeSet<AccountId> = signers.iter().copied().chain(std::iter::once(payer)).collect();
        let all: Vec<AccountId> = all.into_iter().collect();
        let nonces: Vec<Nonce> = self.w.get_accounts_nonces(&all).await?;
        let nonces: BTreeMap<AccountId, Nonce> = all.iter().copied().zip(nonces).collect();
        let keys: Vec<PrivateKey> = all
            .iter()
            .map(|a| self.w.get_account_public_signing_key(*a).cloned().ok_or_else(|| anyhow!("no key for {a}")))
            .collect::<Result<_>>()?;
        let fee = FeeDeclaration::new(payer, limit, 0, wallet::max_fee_for(limit));
        let msg = TxMessage::new(root, message, declared, nonces, Some(fee), Vec::new());
        assert!(msg.admission_evidence.is_empty());
        let key_refs: Vec<&PrivateKey> = keys.iter().collect();
        let tx = PublicTransaction::new(msg.clone(), WitnessSet::for_message(&msg, &key_refs));
        let tx_bytes = borsh::to_vec(&tx)?.len();
        let before = self.w.get_account_balance(payer).await?;
        let hash = match self.w.client().send_transaction(LeeTransaction::Public(tx)).await {
            Ok(h) => h,
            Err(e) => bail!("{what}: rejected at submission: {e:?}"),
        };
        let (_, block) = self.w.poll_transaction(hash).await.context("not included")?;
        let reverted = self.is_reverted(hash, block).await?;
        let fee = before.saturating_sub(self.w.get_account_balance(payer).await?);
        println!("   tx {hash}: block {block}, {tx_bytes} bytes, gas limit {limit}, fee {fee}: {}", if reverted { "REVERTED (charged)" } else { "APPLIED" });
        let cycles = sim.as_ref().ok().copied();
        self.rows.push(format!(
            "| {what} | {} | {tx_bytes} | {} | {fee} |",
            cycles.map_or_else(|| "refused".into(), |c| c.to_string()),
            if reverted { "**REVERTED**" } else { "applied" }
        ));
        let sim_error = sim.as_ref().err().map(ToString::to_string);
        Ok(Outcome { block, reverted, fee, tx_bytes, cycles, limit, sim_error })
    }

    async fn ix(&mut self, what: &str, root: AccountId, ix: &Instruction, signers: &[AccountId], payer: AccountId, first_segment: Option<AccountId>, force_gas: Option<u64>) -> Result<Outcome> {
        let actors = actors_to_declare(root, self.reg, ix);
        self.send(what, Actor::new(root, self.reg), borsh::to_vec(ix)?, actors, signers, payer, first_segment, force_gas).await
    }

    async fn create_header(&mut self, header: AccountId, first: AccountId, payer: AccountId) -> Result<Outcome> {
        let to = Actor::new(header, PROGRAM_LOADER_ACCOUNT_ID);
        let m = borsh::to_vec(&program_loader_core::Message::CreateHeader { first_segment: first, immutable: false })?;
        self.send(&format!("CreateHeader {header}"), to, m, vec![], &[header], payer, Some(first), None).await
    }

    async fn header(&self, account: AccountId) -> Result<Option<ProgramHeader>> {
        Ok(ProgramHeader::from_bytes(&self.state(Actor::new(account, PROGRAM_LOADER_ACCOUNT_ID)).await?))
    }

    fn reader(&self) -> impl Fn(&t::AccountId) -> Vec<u8> + '_ {
        move |a: &t::AccountId| {
            let actor = Actor::new(AccountId::new(a.0), self.reg);
            tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(self.state(actor))).unwrap_or_default()
        }
    }
}

fn content(tag: u8, size: u32) -> t::ContentRef {
    // A CIDv1 shape (0x01, varint 0xCD01, 0x12 0x20, 32 B); M1 uploads nothing to Storage.
    let mut cid = vec![0x01, 0xCD, 0x01, 0x12, 0x20];
    cid.extend_from_slice(&[tag; 32]);
    t::ContentRef { cid, sha256: [tag; 32], size, peer_hint: None }
}

fn build_config() -> t::BuildConfig {
    t::BuildConfig {
        schema: 1,
        builder: t::BUILDER_RISC0_DOCKER,
        image_digest: [0; 32],
        image_tag: "r0.1.91.1".into(),
        risc0_build: "3.0.5".into(),
        host_risc0_rust: "1.91.1".into(),
        manifest_path: "methods/guest/Cargo.toml".into(),
        binary: "lez_program_registry".into(),
        features: vec![],
        lock_sha256: [0; 32],
        docker_env: vec![],
    }
}

fn source() -> t::SourceRef {
    t::SourceRef { repo: "https://github.com/jefdiesel/lez-program-registry".into(), commit: vec![0; 20], docker_root: ".".into(), archive: None }
}

fn now_ms() -> u64 {
    u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis()).expect("ms")
}

/// Gas limit Mallory's transaction is forced with: above any honest `RegisterDeployer`
/// (165–183k cycles, limit ≤ ~275k), so only the authorization check can make it revert.
const FORCED_GAS: u64 = 400_000;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure_local(&cli.rpc)?;
    let overrides = WalletConfigOverrides {
        sequencer: Some(SequencerConnectionData { sequencer_addr: cli.rpc.parse()?, basic_auth: None }),
        seq_tx_poll_max_blocks: Some(30),
        ..Default::default()
    };
    let storage_path = fetch_persistent_storage_path()?;
    let password = read_password(&storage_path)?;
    let w = WalletCore::new_update_chain(fetch_config_path()?, storage_path, Some(overrides), &password)?;
    ensure_local(w.client_url().as_str())?;
    let fee_state = w.client().get_fee_state().await?;
    println!("sequencer {}: base_fee_exec {} base_fee_stor {}", cli.rpc, fee_state.base_fee_exec, fee_state.base_fee_stor);

    let reg_elf = std::fs::read(&cli.reg_elf).with_context(|| cli.reg_elf.clone())?;
    let sample_elf = std::fs::read(&cli.sample_elf).with_context(|| cli.sample_elf.clone())?;
    let reg_image = Program::new(reg_elf.clone().into()).map_err(|e| anyhow!("{e}"))?.id();
    let sample_image = Program::new(sample_elf.clone().into()).map_err(|e| anyhow!("{e}"))?.id();
    println!("registry image {}\nsample image   {}", hex(&image_bytes(&reg_image)), hex(&image_bytes(&sample_image)));

    let mut s = Smoke { w, reg: AccountId::new([0; 32]), reg_program: None, rows: Vec::new() };
    // Accounts: payer P, Mallory M, registry header R, sample header X, author A, publisher Q,
    // Mallory's target X3.
    let mut acc = Vec::new();
    for _ in 0..7 {
        acc.push(s.new_account().await);
    }
    s.w.store_persistent_data()?;
    let [p, m, r, x, a, q, x3] = acc[..] else { unreachable!() };
    println!("P={p}\nM={m}\nR={r}\nX={x}\nA={a}\nQ={q}\nX3={x3}");
    s.fund(cli.funder, p, 300_000_000).await?;
    s.fund(cli.funder, m, 20_000_000).await?;

    // 1. Registry.
    if let Some(reg) = cli.reg {
        s.reg = reg;
    } else {
        let first = s.write_segments(&reg_elf, p).await?;
        s.create_header(r, first, p).await?;
        s.reg = r;
    }
    let h = s.header(s.reg).await?.ok_or_else(|| anyhow!("no registry header"))?;
    println!("registry {} header image {} (expected {})", s.reg, hex(&image_bytes(&h.image_id)), hex(&image_bytes(&reg_image)));
    if cli.reg.is_none() {
        assert_eq!(h.image_id, reg_image, "the deployed registry is this guest");
    }
    s.reg_program = Some(Program::new(reg_elf.clone().into()).map_err(|e| anyhow!("{e}"))?);
    let reg = s.reg;
    let now = now_ms().saturating_sub(5_000);
    if cli.reg.is_none() {
        let self_entry = Instruction::RegisterDeployer {
            name: "lez_program_registry".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            author_label: "LEZ Program Registry".into(),
            description: "The registry, registered in itself (smoke run)".into(),
            tags: vec!["registry".into()],
            idl: content(0x11, 37130),
            source: source(),
            build: build_config(),
            image_id: image_bytes(&reg_image),
            author_id: *reg.value(),
            revision: 1,
            revision_seed: t::pda::revision_seed(1),
            now,
            via_pda_seed: None,
        };
        s.ix("RegisterDeployer (REG, REG) self-entry", reg, &self_entry, &[reg], p, None, None).await?;
    }

    // 2. DeployAndRegister.
    let first = s.write_segments(&sample_elf, p).await?;
    let dar = Instruction::DeployAndRegister {
        first_segment: *first.value(),
        immutable: false,
        name: "smoke-forwarder".into(),
        version: "1.0.0".into(),
        author_label: "smoke".into(),
        description: "sample program deployed and registered in one transaction".into(),
        tags: vec!["sample".into()],
        idl: content(0x22, 900),
        source: source(),
        build: build_config(),
        image_id: image_bytes(&sample_image),
        author_id: *a.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: now_ms().saturating_sub(5_000),
    };
    s.ix("DeployAndRegister X (X + A sign, P pays)", x, &dar, &[x, a], p, Some(first), None).await?;

    // 3. Third party.
    let tp = Instruction::RegisterThirdParty {
        program: *x.value(),
        name: "smoke-forwarder".into(),
        version: "1.0.0".into(),
        author_label: "a community packager".into(),
        description: String::new(),
        tags: vec![],
        idl: content(0x33, 950),
        source: source(),
        build: build_config(),
        image_id: image_bytes(&sample_image),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: now_ms().saturating_sub(5_000),
    };
    s.ix("RegisterThirdParty for X (Q signs, P pays)", q, &tp, &[q], p, None, None).await?;

    // 4. Update by the author.
    let up = Instruction::Update {
        entry_id: *x.value(),
        revision: 2,
        revision_seed: t::pda::revision_seed(2),
        version: Some("1.1.0".into()),
        description: Some("updated by its author".into()),
        tags: None,
        author_label: None,
        idl: Some(content(0x44, 1000)),
        source: None,
        build: None,
        image_id: None,
        now: now_ms().saturating_sub(5_000),
        via_pda_seed: None,
    };
    s.ix("Update X by author A (revision 2)", a, &up, &[a], p, None, None).await?;

    // 5. Mallory.
    s.create_header(x3, first, p).await?;
    let evil = Instruction::RegisterDeployer {
        name: "smoke-forwarder".into(),
        version: "6.6.6".into(),
        author_label: "totally the deployer".into(),
        description: String::new(),
        tags: vec![],
        idl: content(0x66, 1),
        source: source(),
        build: build_config(),
        image_id: image_bytes(&sample_image),
        author_id: *m.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: now_ms().saturating_sub(5_000),
        via_pda_seed: None,
    };
    // Control: the same instruction, signed by X3 too, simulates fine and needs far less gas
    // than Mallory's forced limit, so running out of gas cannot be why her transaction reverts.
    let declared: Vec<Actor> = std::iter::once(Actor::new(x3, reg)).chain(actors_to_declare(x3, reg, &evil)).collect();
    let honest = s.simulate(Actor::new(x3, reg), &borsh::to_vec(&evil)?, &declared, &[x3, m], None).await?;
    let honest_limit = honest.saturating_add(honest.div_ceil(2));
    println!("\n== control: the same RegisterDeployer signed by X3 and M simulates OK, {honest} cycles (limit {honest_limit})");
    assert!(honest_limit < FORCED_GAS, "the forced limit must exceed an honest registration's");
    let refused = s.ix("Mallory: RegisterDeployer (X3, REG), signed only by M (simulate)", x3, &evil, &[m], m, None, None).await;
    match refused {
        Err(e) => {
            println!("   refused before sending: {e}");
            assert!(e.to_string().contains("must sign"), "refused for the wrong reason: {e}");
        }
        Ok(_) => bail!("Mallory's registration was accepted"),
    }
    let forced = s.ix("Mallory: RegisterDeployer (X3, REG), forced", x3, &evil, &[m], m, None, Some(FORCED_GAS)).await?;
    assert!(forced.sim_error.as_deref().is_some_and(|e| e.contains("must sign")), "{:?}", forced.sim_error);
    assert!(forced.reverted, "Mallory's forced registration must revert");
    assert!(s.state(Actor::new(x3, reg)).await?.is_empty(), "no entry at (X3, REG)");
    let floor = u128::from(forced.limit) * u128::from(fee_state.base_fee_exec);
    assert!(forced.fee >= floor && forced.fee <= wallet::max_fee_for(forced.limit), "charged {} for limit {}", forced.fee, forced.limit);
    println!(
        "   Mallory paid {} (>= the full gas limit {} x base fee) for her own failed attempt; (X3, REG) is still empty",
        forced.fee, forced.limit
    );
    let _ = (forced.block, forced.tx_bytes, forced.cycles);

    // 6. Read back.
    println!("\n== read back");
    let account = s.w.client().get_account(x).await?.ok_or_else(|| anyhow!("no account X"))?;
    let entry_bytes = account.data.actor_state(reg).to_vec();
    let header = ProgramHeader::from_bytes(account.data.actor_state(PROGRAM_LOADER_ACCOUNT_ID)).ok_or_else(|| anyhow!("no header at X"))?;
    let entry: t::Entry = borsh::from_slice(&entry_bytes)?;
    println!(
        "one getAccount(X): entry kind {} name {} v{} rev {} author {} born_with_deploy {}; header image {} (entry claims {}) -> {}",
        entry.kind,
        entry.name,
        entry.version,
        entry.revision,
        entry.author,
        entry.born_with_deploy(),
        hex(&image_bytes(&header.image_id)),
        hex(&entry.image_id),
        if entry.image_id == image_bytes(&header.image_id) { "fresh" } else { "stale" }
    );
    assert!(entry.kind == t::KIND_DEPLOYER && entry.born_with_deploy() && entry.revision == 2 && entry.author == tid(&a));
    let treg = tid(&reg);
    let rd = s.reader();
    let all = walk::global(&rd, &treg, 50).map_err(|e| anyhow!("{e:?}"))?;
    println!("global list (newest first): {}", all.iter().map(|(e, l)| format!("#{} {} {}", l.seq, l.name, e)).collect::<Vec<_>>().join(", "));
    let named = walk::by_name(&rd, &treg, "smoke-forwarder", 50).map_err(|e| anyhow!("{e:?}"))?;
    println!("name 'smoke-forwarder': {} entries", named.len());
    let by_a = walk::by_author(&rd, &treg, &tid(&a), 50).map_err(|e| anyhow!("{e:?}"))?;
    println!("author A holds: {:?}", by_a.iter().map(|(e, _)| *e).collect::<Vec<_>>());
    let tps = walk::third_party(&rd, &treg, &tid(&x), 50).map_err(|e| anyhow!("{e:?}"))?;
    let tp_at = addr::third_party_entry(&treg, &tid(&x), &tid(&q));
    println!("third-party entries for X: {:?} (expected {tp_at})", tps.iter().map(|(e, _)| *e).collect::<Vec<_>>());
    assert_eq!(tps.len(), 1);
    assert_eq!(tps[0].0, tp_at);
    let rel2: t::Release = borsh::from_slice(&s.state(Actor::new(AccountId::new(addr::release(&treg, &tid(&x), 2).0), reg)).await?)?;
    println!("Release #2 of X: version {} build_hash {}", rel2.version, hex(&rel2.build_hash));

    println!("\n| Transaction | Simulated cycles | Tx bytes | Outcome | Fee |\n|---|---|---|---|---|");
    for row in &s.rows {
        println!("{row}");
    }
    println!("\nREG={reg} X={x} A={a} Q={q} M={m} X3={x3}\nSMOKE PASS");
    Ok(())
}
