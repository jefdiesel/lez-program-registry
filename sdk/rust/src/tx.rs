//! Writes (feature `tx`): build, simulate, sign, send and confirm registry transactions.
//!
//! Every transaction is built here, not by the wallet: the declared actors come from the
//! program's own `actors_to_declare` (the same source the guest and the IDL use), with **no
//! admission evidence** (the registry self-admits its PDAs; signers are admitted
//! automatically). Each one is first simulated against current chain state with the registry
//! bytecode read from the chain, so a refusal costs nothing; the gas limit is the simulated
//! cycles × 1.5 (at least 30,000), which also caps what any revert can cost (DESIGN §2.7).
//! After it applies, the change is read back from the chain.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
};

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
use lezreg_types::{self as t, BuildConfig, ContentRef, Entry, SourceRef, pda::addr, validate};
use sequencer_service_rpc::SequencerClient;
use wallet::{WalletCore, WalletSequencerClient as _, program_facades::program_loader::ProgramLoader, utils::fetch_persistent_storage_path};

use crate::error::{Error, Result};

/// The least gas limit a registry call is sent with (LNS pattern).
pub const MIN_GAS_LIMIT: u64 = 30_000;

/// Gas limit for a call simulated at `cycles`: 1.5×, rounded up, at least [`MIN_GAS_LIMIT`].
#[must_use]
pub const fn gas_limit_for(cycles: u64) -> u64 {
    let l = cycles.saturating_add(cycles.div_ceil(2));
    if l < MIN_GAS_LIMIT { MIN_GAS_LIMIT } else { l }
}

/// Gas limit for loader `WriteSegment`s and native transfers: the loader is host code and
/// meters no cycles (DESIGN §2.1); only bytes are billed.
const UNMETERED_GAS_LIMIT: u64 = 1_000;

#[must_use]
pub fn tid(a: &AccountId) -> t::AccountId {
    t::AccountId(*a.value())
}

#[must_use]
pub fn lid(a: &t::AccountId) -> AccountId {
    AccountId::new(a.0)
}

/// A program image id as the 32 bytes the registry stores (8 little-endian words).
#[must_use]
pub fn image_bytes(id: &[u32; 8]) -> [u8; 32] {
    let mut out = [0; 32];
    for (i, w) in id.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// The fields of a new entry.
#[derive(Debug, Clone, Default)]
pub struct EntryInput {
    pub name: String,
    pub version: String,
    pub author_label: String,
    pub description: String,
    pub tags: Vec<String>,
    pub idl: ContentRef,
    pub source: SourceRef,
    pub build: BuildConfig,
    /// The image id claimed. `None`: the live header's (or, for `deploy`, the ELF's).
    pub image_id: Option<[u8; 32]>,
}

impl EntryInput {
    fn validate(&self) -> Result<()> {
        validate::entry_fields(&self.name, &self.version, &self.author_label, &self.description, &self.tags, &self.idl, &self.source, &self.build)
            .map_err(|m| Error::Invalid(m.into()))
    }
}

/// Fields an `Update` changes; `None` keeps the stored value. `name`, `program` and `kind`
/// never change.
#[derive(Debug, Clone, Default)]
pub struct PatchInput {
    pub version: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub author_label: Option<String>,
    pub idl: Option<ContentRef>,
    pub source: Option<SourceRef>,
    pub build: Option<BuildConfig>,
    pub image_id: Option<[u8; 32]>,
}

impl PatchInput {
    fn validate(&self) -> Result<()> {
        let inv = |m: &str| Error::Invalid(m.into());
        if let Some(v) = &self.version {
            validate::version(v).map_err(inv)?;
        }
        if let Some(v) = &self.description {
            validate::description(v).map_err(inv)?;
        }
        if let Some(v) = &self.tags {
            validate::tags(v).map_err(inv)?;
        }
        if let Some(v) = &self.author_label {
            validate::author_label(v).map_err(inv)?;
        }
        if let Some(v) = &self.idl {
            validate::content_ref(v).map_err(inv)?;
        }
        if let Some(v) = &self.source {
            validate::source_ref(v).map_err(inv)?;
        }
        if let Some(v) = &self.build {
            validate::build_config(v).map_err(inv)?;
        }
        Ok(())
    }
}

/// How to send.
#[derive(Debug, Clone, Default)]
pub struct SendOpts {
    /// Who pays the fee (must sign). Default: the first signer.
    pub payer: Option<AccountId>,
    /// Simulate and report; send nothing.
    pub dry_run: bool,
    /// Send even if simulation refuses (or skip the client-side pre-checks), with this gas
    /// limit. For demonstrating on-chain refusals; the payer is charged the whole limit.
    pub force_gas: Option<u64>,
}

/// What a transaction did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub what: String,
    /// `None` for a dry run.
    pub hash: Option<String>,
    pub block: Option<u64>,
    /// Simulated cycles (`None` if simulation refused).
    pub cycles: Option<u64>,
    pub refused: Option<String>,
    pub gas_limit: u64,
    pub fee: u128,
    pub tx_bytes: usize,
    pub signers: Vec<AccountId>,
    pub payer: AccountId,
    /// The entry this transaction wrote or changed, if any.
    pub entry: Option<AccountId>,
}

impl Receipt {
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "what": self.what,
            "tx": self.hash,
            "block": self.block,
            "simulated_cycles": self.cycles,
            "simulation_refused": self.refused,
            "gas_limit": self.gas_limit,
            "fee": self.fee.to_string(),
            "tx_bytes": self.tx_bytes,
            "signers": self.signers.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "payer": self.payer.to_string(),
            "entry": self.entry.map(|e| e.to_string()),
            "dry_run": self.hash.is_none(),
        })
    }
}

/// Signs and sends registry transactions through a LEZ wallet.
pub struct Writer {
    w: WalletCore<SequencerClient>,
    reg: AccountId,
    program: Option<Program>,
    log: Box<dyn Fn(&str) + Send + Sync>,
}

impl Writer {
    /// A writer for the registry `reg` through wallet `w`. Progress goes to stderr.
    pub fn new(w: WalletCore<SequencerClient>, reg: AccountId) -> Self {
        Self { w, reg, program: None, log: Box::new(|m| eprintln!("{m}")) }
    }

    /// Sends progress lines somewhere else.
    pub fn set_log(&mut self, log: impl Fn(&str) + Send + Sync + 'static) {
        self.log = Box::new(log);
    }

    #[must_use]
    pub const fn wallet(&self) -> &WalletCore<SequencerClient> {
        &self.w
    }

    pub fn wallet_mut(&mut self) -> &mut WalletCore<SequencerClient> {
        &mut self.w
    }

    #[must_use]
    pub const fn registry(&self) -> AccountId {
        self.reg
    }

    fn say(&self, m: &str) {
        (self.log)(m);
    }

    /// Whether this wallet holds `a`'s signing key.
    #[must_use]
    pub fn holds(&self, a: AccountId) -> bool {
        self.w.get_account_public_signing_key(a).is_some()
    }

    /// `program`'s actor state on `account` (empty if none).
    pub async fn state(&self, account: AccountId, program: AccountId) -> Result<Vec<u8>> {
        let actor = Actor::new(account, program);
        let view = self.w.get_account_view(actor).await.map_err(|e| Error::Transport { url: self.w.client_url().to_string(), message: e.to_string() })?;
        Ok(view.map_or_else(Vec::new, |found| found.data.actor_state(program).to_vec()))
    }

    pub async fn header(&self, account: AccountId) -> Result<Option<ProgramHeader>> {
        Ok(ProgramHeader::from_bytes(&self.state(account, PROGRAM_LOADER_ACCOUNT_ID).await?))
    }

    async fn entry(&self, at: AccountId) -> Result<Option<Entry>> {
        let s = self.state(at, self.reg).await?;
        if s.is_empty() {
            return Ok(None);
        }
        borsh::from_slice(&s).map(Some).map_err(|_| Error::BadRecord { at: tid(&at), what: "entry" })
    }

    /// The chain's time (clock program), or this machine's clock minus a minute.
    pub async fn now(&self) -> u64 {
        let local = || {
            u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())
                .unwrap_or(u64::MAX)
                .saturating_sub(60_000)
        };
        self.state(clock_core::CLOCK_01_PROGRAM_ACCOUNT_ID, clock_core::clock_account_id())
            .await
            .ok()
            .and_then(|s| borsh::from_slice::<clock_core::ClockAccountData>(&s).ok())
            .map_or_else(local, |d| d.timestamp)
    }

    /// The registry's bytecode, read from the chain (checked against its header's image id)
    /// and cached next to the wallet. `LEZREG_PROGRAM_ELF` overrides it with a local file.
    async fn registry_program(&mut self) -> Result<Program> {
        if let Some(p) = &self.program {
            return Ok(p.clone());
        }
        let p = if let Some(path) = std::env::var_os("LEZREG_PROGRAM_ELF") {
            let bytes = std::fs::read(&path).map_err(|e| Error::Config(format!("cannot read {}: {e}", PathBuf::from(path).display())))?;
            Program::new(bytes.into()).map_err(|e| Error::Config(format!("LEZREG_PROGRAM_ELF does not load: {e}")))?
        } else {
            let header = self.header(self.reg).await?.ok_or(Error::NotDeployed(tid(&self.reg)))?;
            let cache = fetch_persistent_storage_path().ok().map(|p| p.with_file_name(format!("lezreg-registry-{}.bin", self.reg)));
            let cached = cache
                .as_ref()
                .and_then(|c| std::fs::read(c).ok())
                .and_then(|b| Program::new(b.into()).ok())
                .filter(|p| p.id() == header.image_id);
            match cached {
                Some(p) => p,
                None => {
                    self.say("reading the registry's bytecode from the chain (once; it is cached next to the wallet)");
                    let p = ProgramLoader(&self.w)
                        .program(self.reg)
                        .await
                        .map_err(|e| Error::Wallet(format!("reading the registry bytecode: {e}")))?
                        .ok_or(Error::NotDeployed(tid(&self.reg)))?;
                    if let Some(c) = &cache {
                        std::fs::write(c, p.elf()).ok();
                    }
                    p
                }
            }
        };
        self.program = Some(p.clone());
        Ok(p)
    }

    /// A fresh public account in the wallet, unused on chain (a copied wallet derives the same
    /// accounts again, so skip used ones).
    pub async fn new_account(&mut self) -> Result<AccountId> {
        for _ in 0..1000 {
            let id = self.w.create_new_account_public(None).0;
            match self.w.client().get_account(id).await {
                Ok(None) => return Ok(id),
                Ok(Some(a)) if a == lee_core::account::Account::default() => return Ok(id),
                Ok(Some(_)) => {}
                Err(e) => return Err(Error::Transport { url: self.w.client_url().to_string(), message: e.to_string() }),
            }
        }
        Err(Error::Wallet("no unused account in the first 1000 derived".into()))
    }

    /// Saves the wallet's storage (new accounts).
    pub fn save_wallet(&self) -> Result<()> {
        self.w.store_persistent_data().map_err(|e| Error::Wallet(e.to_string()))
    }

    async fn balance(&self, a: AccountId) -> Result<u128> {
        self.w.get_account_balance(a).await.map_err(|e| Error::Wallet(e.to_string()))
    }

    async fn nonces(&self, accounts: &[AccountId]) -> Result<Vec<Nonce>> {
        self.w.get_accounts_nonces(accounts).await.map_err(|e| Error::Wallet(e.to_string()))
    }

    async fn is_reverted(&self, hash: HashType, block_id: u64) -> Result<bool> {
        let block = self.w.get_block(block_id).await.map_err(|e| Error::Wallet(e.to_string()))?.ok_or_else(|| Error::NotIncluded(format!("block {block_id} missing")))?;
        Ok(block.body.transactions.iter().position(|t| t.hash() == hash).is_some_and(|p| block.body.is_reverted(p)))
    }

    /// Simulated cycles; the loader segment chain from `first_segment` is supplied when given.
    async fn simulate(&mut self, root: Actor, message: &[u8], declared: &[Actor], signers: &[AccountId], first_segment: Option<AccountId>) -> Result<u64> {
        let mut states = HashMap::new();
        for a in declared {
            states.insert(*a, self.state(a.account_id, a.program_account_id).await?.into());
        }
        let mut next = first_segment;
        let mut n = 0;
        while let Some(seg) = next {
            let actor = Actor::new(seg, PROGRAM_LOADER_ACCOUNT_ID);
            let s = self.state(seg, PROGRAM_LOADER_ACCOUNT_ID).await?;
            next = ProgramSegment::from_bytes(&s).and_then(|p| p.next_segment);
            states.insert(actor, s.into());
            n += 1;
            if n > 32 {
                break;
            }
        }
        let nonces = self.nonces(signers).await?;
        let tx = TxMessage::new(root, message.to_vec(), declared.to_vec(), signers.iter().copied().zip(nonces).collect(), None, Vec::new());
        let tx = PublicTransaction::new(tx.clone(), WitnessSet::for_message(&tx, &[]));
        // The registry's bytecode is only needed when the registry runs (not for a plain loader
        // deploy, which may come before any registry exists).
        let catalog: ProgramCatalog = if root.program_account_id == self.reg {
            std::iter::once((self.reg, self.registry_program().await?)).collect()
        } else {
            ProgramCatalog::default()
        };
        let budget = self.w.client().get_fee_state().await.map_err(|e| Error::Wallet(e.to_string()))?.max_gas_exec;
        let sim = Simulation { public_actor_states: states, admitted_accounts: None };
        let (cycles, result) = tokio::task::spawn_blocking(move || simulate_public_transaction(&tx, &sim, &catalog, budget))
            .await
            .map_err(|e| Error::Refused(e.to_string()))?;
        result.map_err(|e| Error::Refused(e.to_string()))?;
        Ok(cycles)
    }

    /// Builds, simulates, signs, sends and settles one transaction. `signers` must sign;
    /// those whose key this wallet lacks are left out, so the program's own refusal shows in
    /// simulation (and, forced, on chain).
    #[allow(clippy::too_many_arguments)]
    async fn send(
        &mut self,
        what: &str,
        root: Actor,
        message: Vec<u8>,
        actors: Vec<Actor>,
        signers: &[AccountId],
        first_segment: Option<AccountId>,
        opts: &SendOpts,
        entry: Option<AccountId>,
    ) -> Result<Receipt> {
        let mut declared: Vec<Actor> = Vec::new();
        for a in std::iter::once(root).chain(actors) {
            if !declared.contains(&a) {
                declared.push(a);
            }
        }
        let mut held: Vec<AccountId> = Vec::new();
        let mut missing: Vec<AccountId> = Vec::new();
        for s in signers {
            if held.contains(s) || missing.contains(s) {
                continue;
            }
            if self.holds(*s) { held.push(*s) } else { missing.push(*s) }
        }
        for m in &missing {
            self.say(&format!("   warning: this wallet holds no key for {m}, which must sign; the registry will refuse"));
        }
        self.say(&format!("== {what}\n   root ({}, {}); {} actors declared; {} message bytes; signing: {}", root.account_id, if root.program_account_id == self.reg { "REG".to_owned() } else { root.program_account_id.to_string() }, declared.len(), message.len(), held.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));
        let sim = self.simulate(root, &message, &declared, &held, first_segment).await;
        let limit = match (&sim, opts.force_gas) {
            (Ok(c), None) => {
                self.say(&format!("   simulated: OK, {c} cycles -> gas limit {}", gas_limit_for(*c)));
                gas_limit_for(*c)
            }
            (Ok(c), Some(g)) => {
                self.say(&format!("   simulated: OK, {c} cycles; forcing gas limit {g}"));
                g
            }
            (Err(e), Some(g)) => {
                self.say(&format!("   simulated: {e}\n   --force-send: sending anyway with gas limit {g} (the payer is charged for a revert)"));
                g
            }
            (Err(e), None) => return Err(e.clone()),
        };
        if let Some(m) = missing.first()
            && opts.force_gas.is_none()
        {
            return Err(Error::NoKey(tid(m)));
        }
        let payer = match opts.payer {
            Some(p) => p,
            None => *held.first().ok_or_else(|| Error::Config("no signer this wallet holds a key for; pass --payer".into()))?,
        };
        if !self.holds(payer) {
            return Err(Error::NoKey(tid(&payer)));
        }
        let quote = self.w.client().get_fee_state().await.map_err(|e| Error::Wallet(e.to_string()))?;
        let reserve = u128::from(limit) * u128::from(quote.next_base_fee_exec_ceiling) + (u128::try_from(message.len()).unwrap_or(0) + 2_000) * u128::from(quote.next_base_fee_stor_ceiling);
        let before = self.balance(payer).await?;
        let cycles = sim.as_ref().ok().copied();
        let refused = sim.as_ref().err().map(ToString::to_string);
        if opts.dry_run {
            self.say(&format!("   dry run: nothing sent (fee reserve ~{reserve} from {payer}, balance {before})"));
            return Ok(Receipt { what: what.into(), hash: None, block: None, cycles, refused, gas_limit: limit, fee: 0, tx_bytes: 0, signers: held, payer, entry });
        }
        if before < reserve {
            return Err(Error::Funds(format!("{payer} holds {before}; about {reserve} is held for the fee while this runs")));
        }
        let all: Vec<AccountId> = held.iter().copied().chain(std::iter::once(payer)).collect::<BTreeSet<_>>().into_iter().collect();
        let nonces: BTreeMap<AccountId, Nonce> = all.iter().copied().zip(self.nonces(&all).await?).collect();
        let keys: Vec<PrivateKey> = all.iter().map(|a| self.w.get_account_public_signing_key(*a).cloned().ok_or(Error::NoKey(tid(a)))).collect::<Result<_>>()?;
        let fee = FeeDeclaration::new(payer, limit, 0, wallet::max_fee_for(limit));
        let msg = TxMessage::new(root, message, declared, nonces, Some(fee), Vec::new());
        debug_assert!(msg.admission_evidence.is_empty());
        let key_refs: Vec<&PrivateKey> = keys.iter().collect();
        let tx = PublicTransaction::new(msg.clone(), WitnessSet::for_message(&msg, &key_refs));
        let tx_bytes = borsh::to_vec(&tx).map(|v| v.len()).unwrap_or(0);
        let hash = self.w.client().send_transaction(LeeTransaction::Public(tx)).await.map_err(|e| Error::Rejected(format!("{e:?}")))?;
        self.say(&format!("   submitted {hash}"));
        let (_, block) = self.w.poll_transaction(hash).await.map_err(|e| Error::NotIncluded(e.to_string()))?;
        let reverted = self.is_reverted(hash, block).await?;
        let fee = before.saturating_sub(self.balance(payer).await?);
        if reverted {
            self.say(&format!("   block {block}: REVERTED; {tx_bytes} bytes, gas limit {limit}, fee {fee} charged to {payer}"));
            return Err(Error::Reverted { hash: hash.to_string(), block, fee });
        }
        self.say(&format!("   block {block}: APPLIED; {tx_bytes} bytes, gas limit {limit}, fee {fee}"));
        Ok(Receipt { what: what.into(), hash: Some(hash.to_string()), block: Some(block), cycles, refused, gas_limit: limit, fee, tx_bytes, signers: held, payer, entry })
    }

    #[allow(clippy::too_many_arguments)]
    async fn ix(&mut self, what: &str, root: AccountId, ix: &Instruction, signers: &[AccountId], first_segment: Option<AccountId>, opts: &SendOpts, entry: Option<AccountId>) -> Result<Receipt> {
        let actors = actors_to_declare(root, self.reg, ix);
        let msg = borsh::to_vec(ix).map_err(|e| Error::Invalid(e.to_string()))?;
        self.send(what, Actor::new(root, self.reg), msg, actors, signers, first_segment, opts, entry).await
    }

    async fn claimed_image(&self, program: AccountId, given: Option<[u8; 32]>) -> Result<[u8; 32]> {
        if let Some(i) = given {
            return Ok(i);
        }
        let h = self.header(program).await?.ok_or_else(|| {
            Error::Invalid(format!("no program is deployed at {program}, so there is no image id to claim; pass one explicitly"))
        })?;
        Ok(image_bytes(&h.image_id))
    }

    /// Checks the change applied by reading the entry back.
    async fn read_back(&self, at: AccountId, check: impl Fn(&Entry) -> bool, what: &str) -> Result<Entry> {
        let e = self.entry(at).await?.ok_or_else(|| Error::ReadBack(format!("no entry at {at}")))?;
        if !check(&e) {
            return Err(Error::ReadBack(format!("{what} not visible at {at}")));
        }
        self.say(&format!("   read back {at}: {} v{} revision {}, author {} ({what}: confirmed)", e.name, e.version, e.revision, e.author));
        Ok(e)
    }

    /// `RegisterDeployer` at `(program, REG)`: `program` (the header key) and `author` sign.
    pub async fn register_deployer(&mut self, program: AccountId, author: AccountId, input: &EntryInput, opts: &SendOpts) -> Result<Receipt> {
        input.validate()?;
        if opts.force_gas.is_none() && self.entry(program).await?.is_some() {
            return Err(Error::Invalid(format!("{program} already has a deployer entry; use update")));
        }
        let image_id = self.claimed_image(program, input.image_id).await?;
        let ix = Instruction::RegisterDeployer {
            name: input.name.clone(),
            version: input.version.clone(),
            author_label: input.author_label.clone(),
            description: input.description.clone(),
            tags: input.tags.clone(),
            idl: input.idl.clone(),
            source: input.source.clone(),
            build: input.build.clone(),
            image_id,
            author_id: *author.value(),
            revision: 1,
            revision_seed: t::pda::revision_seed(1),
            now: self.now().await,
            via_pda_seed: None,
        };
        let r = self.ix("RegisterDeployer", program, &ix, &[program, author], None, opts, Some(program)).await?;
        if r.hash.is_some() {
            let name = input.name.clone();
            self.read_back(program, |e| e.kind == t::KIND_DEPLOYER && e.revision == 1 && e.name == name && e.author == tid(&author), "deployer entry").await?;
        }
        Ok(r)
    }

    /// `RegisterThirdParty`: a community entry for `program` at `T(program, publisher)`.
    pub async fn register_third_party(&mut self, program: AccountId, publisher: AccountId, input: &EntryInput, opts: &SendOpts) -> Result<Receipt> {
        input.validate()?;
        if program == publisher {
            return Err(Error::Invalid("a third-party entry cannot be for the publisher's own account".into()));
        }
        let at = lid(&addr::third_party_entry(&tid(&self.reg), &tid(&program), &tid(&publisher)));
        if opts.force_gas.is_none() && self.entry(at).await?.is_some() {
            return Err(Error::Invalid(format!("{publisher} already has a third-party entry for {program} at {at}; use update")));
        }
        let image_id = self.claimed_image(program, input.image_id).await?;
        let ix = Instruction::RegisterThirdParty {
            program: *program.value(),
            name: input.name.clone(),
            version: input.version.clone(),
            author_label: input.author_label.clone(),
            description: input.description.clone(),
            tags: input.tags.clone(),
            idl: input.idl.clone(),
            source: input.source.clone(),
            build: input.build.clone(),
            image_id,
            revision: 1,
            revision_seed: t::pda::revision_seed(1),
            now: self.now().await,
        };
        let r = self.ix("RegisterThirdParty", publisher, &ix, &[publisher], None, opts, Some(at)).await?;
        if r.hash.is_some() {
            self.read_back(at, |e| e.kind == t::KIND_THIRD_PARTY && e.program == tid(&program) && e.author == tid(&publisher), "third-party entry").await?;
        }
        Ok(r)
    }

    /// Writes `elf`'s segments (paid by `payer`) and returns the first segment.
    pub async fn write_segments(&mut self, elf: &[u8], payer: AccountId) -> Result<(AccountId, usize)> {
        let binary = risc0_binfmt::ProgramBinary::decode(elf).map_err(|e| Error::Invalid(format!("not a risc0 program binary: {e}")))?;
        let chunks: Vec<Vec<u8>> = binary.user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).map(<[u8]>::to_vec).collect();
        if chunks.len() > 20 {
            return Err(Error::Invalid(format!("the program needs {} segments; the loader takes at most 20", chunks.len())));
        }
        let mut segs = Vec::new();
        for _ in &chunks {
            segs.push(self.new_account().await?);
        }
        self.save_wallet()?;
        let saved = self.w.config().clone();
        let mut config = saved.clone();
        config.gas_limit = UNMETERED_GAS_LIMIT;
        self.w.set_config(config);
        let result = async {
            for i in (0..chunks.len()).rev() {
                self.say(&format!("   writing segment {}/{} ({} bytes) at {}", chunks.len() - i, chunks.len(), chunks[i].len(), segs[i]));
                let hash = ProgramLoader(&self.w)
                    .write_segment(segs[i], chunks[i].clone(), segs.get(i + 1).copied(), Some(payer))
                    .await
                    .map_err(|e| Error::Rejected(e.to_string()))?;
                let (_, block) = self.w.poll_transaction(hash).await.map_err(|e| Error::NotIncluded(e.to_string()))?;
                if self.is_reverted(hash, block).await? {
                    return Err(Error::Reverted { hash: hash.to_string(), block, fee: 0 });
                }
            }
            Ok(())
        }
        .await;
        self.w.set_config(saved);
        result?;
        Ok((segs[0], binary.user_elf.len()))
    }

    /// Deploys `elf` at the header account `program` (its key must be in the wallet). With
    /// `register = Some((author, input))` this is one `DeployAndRegister` (the entry carries
    /// `born_with_deploy`); otherwise a plain loader `CreateHeader`.
    pub async fn deploy(&mut self, elf: &[u8], program: AccountId, immutable: bool, register: Option<(AccountId, &EntryInput)>, opts: &SendOpts) -> Result<Receipt> {
        let image = Program::new(elf.to_vec().into()).map_err(|e| Error::Invalid(format!("the ELF does not load as a program: {e}")))?.id();
        if let Some((_, input)) = &register {
            input.validate()?;
            if let Some(claim) = input.image_id
                && claim != image_bytes(&image)
            {
                return Err(Error::Invalid("the claimed image id differs from the ELF's".into()));
            }
        }
        if self.header(program).await?.is_some() {
            return Err(Error::Invalid(format!("a program is already deployed at {program}")));
        }
        if !self.holds(program) {
            return Err(Error::NoKey(tid(&program)));
        }
        let payer = opts.payer.unwrap_or(program);
        if opts.dry_run {
            return Err(Error::Config("deploy has no dry run: writing segments costs fees".into()));
        }
        self.say(&format!("== deploying {} bytes (image {}) at {program}", elf.len(), hex::encode(image_bytes(&image))));
        let (first, user_len) = self.write_segments(elf, payer).await?;
        self.say(&format!("   wrote the segment chain ({user_len} bytes of user ELF), first segment {first}"));
        let opts = SendOpts { payer: Some(payer), ..opts.clone() };
        let r = match register {
            Some((author, input)) => {
                let ix = Instruction::DeployAndRegister {
                    first_segment: *first.value(),
                    immutable,
                    name: input.name.clone(),
                    version: input.version.clone(),
                    author_label: input.author_label.clone(),
                    description: input.description.clone(),
                    tags: input.tags.clone(),
                    idl: input.idl.clone(),
                    source: input.source.clone(),
                    build: input.build.clone(),
                    image_id: image_bytes(&image),
                    author_id: *author.value(),
                    revision: 1,
                    revision_seed: t::pda::revision_seed(1),
                    now: self.now().await,
                };
                let r = self.ix("DeployAndRegister", program, &ix, &[program, author], Some(first), &opts, Some(program)).await?;
                self.read_back(program, |e| e.born_with_deploy() && e.kind == t::KIND_DEPLOYER && e.author == tid(&author), "deploy-and-register").await?;
                r
            }
            None => {
                let to = Actor::new(program, PROGRAM_LOADER_ACCOUNT_ID);
                let m = borsh::to_vec(&program_loader_core::Message::CreateHeader { first_segment: first, immutable }).map_err(|e| Error::Invalid(e.to_string()))?;
                self.send("CreateHeader", to, m, vec![], &[program], Some(first), &opts, None).await?
            }
        };
        let h = self.header(program).await?.ok_or_else(|| Error::ReadBack(format!("no header at {program}")))?;
        if h.image_id != image {
            return Err(Error::ReadBack("the deployed header's image id differs from the ELF's".into()));
        }
        self.say(&format!("   read back header at {program}: image {} ({}): confirmed", hex::encode(image_bytes(&h.image_id)), if h.immutable { "immutable" } else { "upgradeable" }));
        Ok(r)
    }

    /// Reads the entry at `at` and checks `signer` may change it.
    async fn changeable(&self, at: AccountId, signer: Option<AccountId>, opts: &SendOpts) -> Result<(Entry, AccountId)> {
        let e = self.entry(at).await?.ok_or(Error::NoEntry(tid(&at)))?;
        let signer = signer.unwrap_or_else(|| lid(&e.author));
        if opts.force_gas.is_none() {
            if e.author != tid(&signer) {
                return Err(Error::NotAuthor { entry: tid(&at), author: e.author, signer: tid(&signer) });
            }
            if e.is_frozen() {
                return Err(Error::Frozen(tid(&at)));
            }
        }
        Ok((e, signer))
    }

    /// `Update` of the entry at `at` by its author (default: the stored author).
    pub async fn update(&mut self, at: AccountId, author: Option<AccountId>, patch: &PatchInput, opts: &SendOpts) -> Result<Receipt> {
        patch.validate()?;
        let (e, author) = self.changeable(at, author, opts).await?;
        let revision = e.revision.checked_add(1).ok_or_else(|| Error::Invalid("revision overflow".into()))?;
        let ix = Instruction::Update {
            entry_id: *at.value(),
            revision,
            revision_seed: t::pda::revision_seed(revision),
            version: patch.version.clone(),
            description: patch.description.clone(),
            tags: patch.tags.clone(),
            author_label: patch.author_label.clone(),
            idl: patch.idl.clone(),
            source: patch.source.clone(),
            build: patch.build.clone(),
            image_id: patch.image_id,
            now: self.now().await,
            via_pda_seed: None,
        };
        let r = self.ix(&format!("Update (revision {revision})"), author, &ix, &[author], None, opts, Some(at)).await?;
        if r.hash.is_some() {
            let p = patch.clone();
            self.read_back(
                at,
                |n| n.revision == revision && p.version.as_ref().is_none_or(|v| *v == n.version) && p.image_id.is_none_or(|i| i == n.image_id),
                &format!("revision {revision}"),
            )
            .await?;
        }
        Ok(r)
    }

    /// `TransferAuthor`: the current author and `new_author` both sign, in one transaction.
    pub async fn transfer(&mut self, at: AccountId, author: Option<AccountId>, new_author: AccountId, opts: &SendOpts) -> Result<Receipt> {
        let (e, author) = self.changeable(at, author, opts).await?;
        if opts.force_gas.is_none() {
            if e.author == tid(&new_author) {
                return Err(Error::Invalid(format!("{new_author} is already the author")));
            }
            if e.registrant == tid(&new_author) {
                return Err(Error::Invalid("a transfer back to a previous author is refused (an entry joins each author's list once)".into()));
            }
            if !self.holds(new_author) {
                return Err(Error::Config(format!(
                    "a transfer needs both signatures in one transaction, and this wallet holds no key for the new author {new_author}"
                )));
            }
        }
        let ix = Instruction::TransferAuthor { entry_id: *at.value(), new_author_id: *new_author.value(), now: self.now().await, via_pda_seed: None };
        let r = self.ix("TransferAuthor", author, &ix, &[author, new_author], None, opts, Some(at)).await?;
        if r.hash.is_some() {
            self.read_back(at, |n| n.author == tid(&new_author), "new author").await?;
        }
        Ok(r)
    }

    /// `Freeze`: irreversible.
    pub async fn freeze(&mut self, at: AccountId, author: Option<AccountId>, opts: &SendOpts) -> Result<Receipt> {
        let (_, author) = self.changeable(at, author, opts).await?;
        let ix = Instruction::Freeze { entry_id: *at.value(), now: self.now().await, via_pda_seed: None };
        let r = self.ix("Freeze", author, &ix, &[author], None, opts, Some(at)).await?;
        if r.hash.is_some() {
            self.read_back(at, Entry::is_frozen, "frozen").await?;
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gas_limit_is_one_and_a_half_times_with_a_floor() {
        assert_eq!(gas_limit_for(0), MIN_GAS_LIMIT);
        assert_eq!(gas_limit_for(10_000), MIN_GAS_LIMIT);
        assert_eq!(gas_limit_for(168_000), 252_000);
        assert_eq!(gas_limit_for(3), MIN_GAS_LIMIT);
        assert_eq!(gas_limit_for(100_001), 150_002);
    }

    #[test]
    fn image_bytes_are_le_words() {
        let mut id = [0u32; 8];
        id[0] = 0x0403_0201;
        id[7] = 0xFFEE_DDCC;
        let b = image_bytes(&id);
        assert_eq!(&b[..4], &[1, 2, 3, 4]);
        assert_eq!(&b[28..], &[0xCC, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn invalid_input_is_caught_before_anything_is_built() {
        let bad = EntryInput { name: "Bad Name".into(), version: "1".into(), ..EntryInput::default() };
        assert!(matches!(bad.validate(), Err(Error::Invalid(m)) if m.contains("name")));
        let patch = PatchInput { tags: Some(vec!["UPPER".into()]), ..PatchInput::default() };
        assert!(matches!(patch.validate(), Err(Error::Invalid(m)) if m.contains("tag")));
        assert!(PatchInput::default().validate().is_ok());
    }

    #[test]
    fn declared_actors_need_no_chain_state() {
        // Every address a transaction touches is a pure function of its arguments (DESIGN §2.4).
        let reg = AccountId::new([0xEE; 32]);
        let x = AccountId::new([1; 32]);
        let a = AccountId::new([2; 32]);
        let ix = Instruction::Freeze { entry_id: *x.value(), now: 0, via_pda_seed: None };
        let first = actors_to_declare(a, reg, &ix);
        assert_eq!(first, actors_to_declare(a, reg, &ix));
        assert!(first.contains(&Actor::new(x, reg)));
    }
}
