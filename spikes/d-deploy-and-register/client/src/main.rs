//! Spike (d) client: drives the toy REG program and the loader on a LOCAL rc1
//! sequencer. Refuses any non-loopback RPC so it can never touch testnet funds.
use std::collections::{BTreeSet, HashMap};

use anyhow::{Context as _, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use common::{HashType, transaction::LeeTransaction};
use lee::{
    PublicTransaction, Simulation,
    privacy_preserving_transaction::circuit::ProgramCatalog,
    program::Program,
    public_transaction::{Message as TxMessage, WitnessSet},
    simulate_public_transaction,
};
use lee_core::{
    account::{AccountId, Actor},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ProgramSegment},
};
use reg_core::{Instruction, LoaderMessage};
use sequencer_service_rpc::SequencerClient;
use wallet::{
    AccountIdentity, AccountMention, WalletCore, WalletSequencerClient as _,
    config::{SequencerConnectionData, WalletConfigOverrides},
    program_facades::{CreditDelivery, native_token_transfer::NativeTokenTransfer, program_loader::ProgramLoader},
    utils::{fetch_config_path, fetch_persistent_storage_path, read_password},
};

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "http://127.0.0.1:3062")]
    rpc: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
enum Variant {
    /// REG DeployAndRegister (one tx)
    Dar,
    /// REG Register (two-tx fallback, step 2)
    Register,
    /// REG DeployAndRegisterNoCheck (REG skips its own auth check; tests the loader's)
    DarNocheck,
    /// Plain loader CreateHeader rooted at (X, LOADER) (two-tx fallback, step 1)
    CreateHeader,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create N fresh public accounts in the wallet and print them.
    NewAccounts { n: usize },
    /// Native transfer (public -> public).
    Fund { #[arg(long)] from: AccountId, #[arg(long)] to: AccountId, #[arg(long)] amount: u128 },
    /// Chunk an ELF's user_elf (96 KiB) and write the segments tail-to-head. Prints fees.
    WriteSegments {
        #[arg(long)] elf: String,
        #[arg(long)] payer: AccountId,
        #[arg(long, num_args = 1..)] segments: Vec<AccountId>,
    },
    /// Send one REG/loader transaction for header X and report cycles, fee, revert.
    Tx {
        #[arg(long, value_enum)] variant: Variant,
        #[arg(long)] reg: Option<AccountId>,
        #[arg(long)] header: AccountId,
        #[arg(long)] first_segment: Option<AccountId>,
        #[arg(long)] immutable: bool,
        #[arg(long, default_value = "lezreg-spike-d")] record: String,
        #[arg(long)] payer: AccountId,
        /// Do NOT let X sign (X mentioned as PublicNoSign). Mallory = the payer.
        #[arg(long)] header_no_sign: bool,
        /// Send even if the local simulation refuses, with this execution gas limit.
        #[arg(long)] force_gas: Option<u64>,
    },
    /// Check reg_core::LoaderMessage encodes like program_loader_core::Message.
    CheckEncoding,
    /// Print how many 96 KiB segments an ELF's user_elf needs, and its image_id.
    Chunks { elf: String },
}

fn ensure_local(rpc: &str) -> Result<()> {
    if !(rpc.starts_with("http://127.0.0.1:") || rpc.starts_with("http://localhost:")) {
        bail!("refusing non-local RPC {rpc}");
    }
    Ok(())
}

async fn open_wallet(rpc: &str) -> Result<WalletCore<SequencerClient>> {
    ensure_local(rpc)?;
    let overrides = WalletConfigOverrides {
        sequencer: Some(SequencerConnectionData { sequencer_addr: rpc.parse()?, basic_auth: None }),
        seq_tx_poll_max_blocks: Some(30),
        ..Default::default()
    };
    let storage_path = fetch_persistent_storage_path()?;
    let password = read_password(&storage_path)?;
    let wallet = WalletCore::new_update_chain(fetch_config_path()?, storage_path, Some(overrides), &password)?;
    ensure_local(wallet.client_url().as_str())?;
    Ok(wallet)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::CheckEncoding => return check_encoding(),
        Cmd::Chunks { elf } => {
            let bytes = std::fs::read(&elf)?;
            let binary = risc0_binfmt::ProgramBinary::decode(&bytes).map_err(|e| anyhow!("{e}"))?;
            let n = binary.user_elf.len().div_ceil(program_loader_core::MAX_SEGMENT_DATA_LEN);
            let id = Program::new(bytes.clone().into()).map_err(|e| anyhow!("{e}"))?.id();
            println!("{n} {} {} {}", bytes.len(), binary.user_elf.len(), image_hex(&id));
        }
        Cmd::NewAccounts { n } => {
            let mut wallet = open_wallet(&cli.rpc).await?;
            for _ in 0..n {
                println!("{}", wallet.create_new_account_public(None).0);
            }
            wallet.store_persistent_data()?;
        }
        Cmd::Fund { from, to, amount } => {
            let wallet = open_wallet(&cli.rpc).await?;
            let mut config = wallet.config().clone();
            config.gas_limit = 1_000;
            let mut wallet = wallet;
            wallet.set_config(config);
            let (hash, _) = NativeTokenTransfer(&wallet)
                .transfer(AccountIdentity::Public(from), AccountIdentity::PublicNoSign(to), amount, CreditDelivery::Automatic)
                .await?;
            let block = finish(&wallet, hash).await?;
            println!("funded {to} with {amount} in block {block}; balance {}", wallet.get_account_balance(to).await?);
        }
        Cmd::WriteSegments { elf, payer, segments } => {
            let wallet = open_wallet(&cli.rpc).await?;
            write_segments(&wallet, &elf, payer, &segments).await?;
        }
        Cmd::Tx { variant, reg, header, first_segment, immutable, record, payer, header_no_sign, force_gas } => {
            let mut wallet = open_wallet(&cli.rpc).await?;
            tx(&mut wallet, variant, reg, header, first_segment, immutable, record, payer, header_no_sign, force_gas).await?;
        }
    }
    Ok(())
}

fn check_encoding() -> Result<()> {
    let a = AccountId::new([7; 32]);
    let ours = borsh::to_vec(&LoaderMessage::CreateHeader { first_segment: a, immutable: true })?;
    let theirs = borsh::to_vec(&program_loader_core::Message::CreateHeader { first_segment: a, immutable: true })?;
    assert_eq!(ours, theirs, "LoaderMessage::CreateHeader encoding differs");
    let ours = borsh::to_vec(&LoaderMessage::WriteSegment { bytecode: vec![1, 2], next_segment: Some(a) })?;
    let theirs = borsh::to_vec(&program_loader_core::Message::WriteSegment { bytecode: vec![1, 2], next_segment: Some(a) })?;
    assert_eq!(ours, theirs, "LoaderMessage::WriteSegment encoding differs");
    println!("encoding OK: CreateHeader = {}", hex::encode(&ours[..1]));
    Ok(())
}

async fn write_segments(wallet: &WalletCore<SequencerClient>, elf: &str, payer: AccountId, segments: &[AccountId]) -> Result<()> {
    let bytes = std::fs::read(elf).with_context(|| format!("read {elf}"))?;
    let binary = risc0_binfmt::ProgramBinary::decode(&bytes).map_err(|e| anyhow!("not a risc0 binary: {e}"))?;
    let chunks: Vec<&[u8]> = binary.user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).collect();
    println!("elf {elf}: {} bytes total, user_elf {} bytes, {} chunks", bytes.len(), binary.user_elf.len(), chunks.len());
    if chunks.len() != segments.len() {
        bail!("need exactly {} segment accounts, got {}", chunks.len(), segments.len());
    }
    let expected = Program::new(bytes.clone().into()).map_err(|e| anyhow!("{e}"))?.id();
    println!("expected image_id {}", image_hex(&expected));
    let loader = ProgramLoader(wallet);
    let mut total_fee = 0u128;
    for i in (0..chunks.len()).rev() {
        let next = segments.get(i + 1).copied();
        let before = wallet.get_account_balance(payer).await?;
        let hash = loader.write_segment(segments[i], chunks[i].to_vec(), next, Some(payer)).await?;
        let block = finish(wallet, hash).await?;
        let after = wallet.get_account_balance(payer).await?;
        let fee = before - after;
        total_fee += fee;
        println!("segment {i} -> {} ({} bytes) block {block} fee {fee}", segments[i], chunks[i].len());
    }
    println!("segments written; total fee {total_fee}");
    Ok(())
}

fn image_hex(id: &[u32; 8]) -> String {
    id.iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

async fn actor_state_of(wallet: &WalletCore<SequencerClient>, actor: Actor) -> Result<Vec<u8>> {
    let view = wallet.get_account_view(actor).await?;
    Ok(view.map_or_else(Vec::new, |found| found.data.actor_state(actor.program_account_id).to_vec()))
}

#[allow(clippy::too_many_arguments)]
async fn tx(
    wallet: &mut WalletCore<SequencerClient>,
    variant: Variant,
    reg: Option<AccountId>,
    header: AccountId,
    first_segment: Option<AccountId>,
    immutable: bool,
    record: String,
    payer: AccountId,
    header_no_sign: bool,
    force_gas: Option<u64>,
) -> Result<()> {
    let ident = if header_no_sign { AccountIdentity::PublicNoSign(header) } else { AccountIdentity::Public(header) };
    let need_first = || first_segment.ok_or_else(|| anyhow!("--first-segment required"));
    let record = record.into_bytes();
    let (mentions, message, programs): (Vec<AccountMention>, Vec<u8>, Vec<(AccountId, Program)>) = match variant {
        Variant::CreateHeader => (
            vec![ident.clone().select_program_actor_state(PROGRAM_LOADER_ACCOUNT_ID)],
            borsh::to_vec(&LoaderMessage::CreateHeader { first_segment: need_first()?, immutable })?,
            vec![],
        ),
        _ => {
            let reg = reg.ok_or_else(|| anyhow!("--reg required"))?;
            let reg_program = ProgramLoader(&*wallet).program(reg).await?;
            if reg_program.is_none() && force_gas.is_none() {
                bail!("{reg} is not a deployed program (pass --force-gas to send anyway)");
            }
            let ix = match variant {
                Variant::Dar => Instruction::DeployAndRegister { first_segment: need_first()?, immutable, record },
                Variant::DarNocheck => Instruction::DeployAndRegisterNoCheck { first_segment: need_first()?, immutable, record },
                Variant::Register => Instruction::Register { record },
                Variant::CreateHeader => unreachable!(),
            };
            let mut mentions = vec![ident.clone().select_program_actor_state(reg)];
            if variant != Variant::Register {
                mentions.push(ident.clone().select_program_actor_state(PROGRAM_LOADER_ACCOUNT_ID));
            }
            (mentions, borsh::to_vec(&ix)?, reg_program.map(|p| (reg, p)).into_iter().collect())
        }
    };
    println!("variant {variant:?} header {header} signs={} payer {payer}", !header_no_sign);
    println!("message {} bytes; declared actors: {}", message.len(), mentions.len());

    // Local simulation, with the segment chain's loader states supplied so the loader can
    // compute image_id exactly as settlement does.
    let sim = simulate(wallet, &mentions, &message, programs, first_segment).await;
    let limit = match (&sim, force_gas) {
        (Ok(cycles), None) => {
            println!("simulated: OK, {cycles} cycles");
            cycles.saturating_add(cycles.div_ceil(2)).max(30_000)
        }
        (Ok(cycles), Some(g)) => {
            println!("simulated: OK, {cycles} cycles (forcing gas {g})");
            g
        }
        (Err(e), Some(g)) => {
            println!("simulated: REFUSED: {e}; sending anyway with gas {g}");
            g
        }
        (Err(e), None) => bail!("simulated: REFUSED: {e}"),
    };

    let quote = wallet.client().get_fee_state().await?;
    println!("fee state: base_fee_exec {} base_fee_stor {} max_gas_exec {} max_gas_stor {}", quote.base_fee_exec, quote.base_fee_stor, quote.max_gas_exec, quote.max_gas_stor);
    let before = wallet.get_account_balance(payer).await?;
    let saved = wallet.config().clone();
    let mut config = saved.clone();
    config.gas_limit = limit;
    wallet.set_config(config);
    let sent = wallet.send_pub_tx_paid_by(mentions, 0, message, Some(payer)).await;
    wallet.set_config(saved);
    let hash = match sent {
        Ok(h) => h,
        Err(e) => {
            println!("RESULT: sequencer REJECTED at submission (no fee): {e:?}");
            return Ok(());
        }
    };
    println!("submitted {hash} gas_limit {limit}");
    let (tx, block_id) = wallet.poll_transaction(hash).await.context("not included")?;
    let tx_bytes = match &tx {
        LeeTransaction::Public(p) => borsh::to_vec(p).map(|v| v.len()).unwrap_or(0),
        _ => 0,
    };
    let reverted = is_reverted(wallet, hash, block_id).await?;
    let after = wallet.get_account_balance(payer).await?;
    println!("included block {block_id}; tx {tx_bytes} bytes; reverted={reverted}; fee charged {}", before.saturating_sub(after));
    println!("RESULT: {}", if reverted { "REVERTED (charged)" } else { "APPLIED" });

    // Read back.
    let hdr = actor_state_of(wallet, Actor::new(header, PROGRAM_LOADER_ACCOUNT_ID)).await?;
    match ProgramHeader::from_bytes(&hdr) {
        Some(h) => println!("(X, LOADER): header image_id {} first_segment {} immutable {}", image_hex(&h.image_id), h.program_first_segment, h.immutable),
        None => println!("(X, LOADER): {} bytes, no header", hdr.len()),
    }
    if let Some(reg) = reg {
        let entry = actor_state_of(wallet, Actor::new(header, reg)).await?;
        if entry.is_empty() {
            println!("(X, REG): empty");
        } else {
            let e: reg_core::Entry = borsh::from_slice(&entry)?;
            println!("(X, REG): schema {} born_with_deploy {} record {:?}", e.schema, e.born_with_deploy, String::from_utf8_lossy(&e.record));
        }
    }
    Ok(())
}

async fn simulate(
    wallet: &WalletCore<SequencerClient>,
    mentions: &[AccountMention],
    message: &[u8],
    programs: Vec<(AccountId, Program)>,
    first_segment: Option<AccountId>,
) -> Result<u64> {
    let actors: Vec<Actor> = {
        let mut v: Vec<Actor> = Vec::new();
        for a in mentions.iter().map(AccountMention::actor) {
            if !v.contains(&a) {
                v.push(a);
            }
        }
        v
    };
    let mut states = HashMap::new();
    for actor in &actors {
        states.insert(*actor, actor_state_of(wallet, *actor).await?.into());
    }
    let mut next = first_segment;
    let mut n = 0;
    while let Some(seg) = next {
        let actor = Actor::new(seg, PROGRAM_LOADER_ACCOUNT_ID);
        let s = actor_state_of(wallet, actor).await?;
        next = ProgramSegment::from_bytes(&s).and_then(|p| p.next_segment);
        states.insert(actor, s.into());
        n += 1;
        if n > 32 {
            break;
        }
    }
    let signers: BTreeSet<AccountId> = mentions
        .iter()
        .filter(|m| m.authorizes && m.identity.public_account_id().is_some())
        .map(|m| m.identity.account_id())
        .filter(|a| wallet.get_account_public_signing_key(*a).is_some())
        .collect();
    let signers: Vec<AccountId> = signers.into_iter().collect();
    let nonces = wallet.get_accounts_nonces(&signers).await?;
    let root = actors[0];
    let txm = TxMessage::new(root, message.to_vec(), actors, signers.into_iter().zip(nonces).collect(), None, Vec::new());
    let witness = WitnessSet::for_message(&txm, &[]);
    let tx = PublicTransaction::new(txm, witness);
    let simulation = Simulation { public_actor_states: states, admitted_accounts: None };
    let catalog: ProgramCatalog = programs.into_iter().collect();
    let budget = wallet.client().get_fee_state().await?.max_gas_exec;
    let (cycles, result) = tokio::task::spawn_blocking(move || simulate_public_transaction(&tx, &simulation, &catalog, budget)).await?;
    result.map_err(|e| anyhow!("{e}"))?;
    Ok(cycles)
}

async fn is_reverted(wallet: &WalletCore<SequencerClient>, hash: HashType, block_id: u64) -> Result<bool> {
    let block = wallet.get_block(block_id).await?.ok_or_else(|| anyhow!("block {block_id} missing"))?;
    Ok(block
        .body
        .transactions
        .iter()
        .position(|t| t.hash() == hash)
        .is_some_and(|p| block.body.is_reverted(p)))
}

async fn finish(wallet: &WalletCore<SequencerClient>, hash: HashType) -> Result<u64> {
    let (_, block_id) = wallet.poll_transaction(hash).await.context("not included")?;
    if is_reverted(wallet, hash, block_id).await? {
        bail!("{hash} reverted in block {block_id}");
    }
    Ok(block_id)
}
