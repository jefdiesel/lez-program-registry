//! Engine-test harness: the real registry guest ELF on LEZ v0.4.0-rc1's real state machine
//! (`V03State`, `ValidatedStateDiff`), real signatures, real loader. No chain, no network, no
//! fees (fee-exempt transactions), and **no admission evidence ever**: every transaction is
//! built with an empty `admission_evidence` (DESIGN §8.2 test 7).

use std::collections::BTreeMap;

use borsh::BorshSerialize;
use lee::{
    PrivateKey, PublicKey, PublicTransaction, V03State, ValidatedStateDiff,
    program::{DEFAULT_PUBLIC_CYCLE_BUDGET, Program},
    public_transaction::{Message as TxMessage, WitnessSet},
};
use lee_core::{
    account::{AccountId, Actor, Nonce},
    program::{PROGRAM_LOADER_ACCOUNT_ID, PdaSeed, ProgramHeader},
};
use lezreg_program::{Instruction, actors_to_declare};
use lezreg_types::{self as t, BuildConfig, ContentRef, SourceRef};
use program_loader_core::Message as LoaderMessage;

/// Block timestamp used by default (ms).
pub const T0: u64 = 1_800_000_000_000;

pub struct Key {
    pub key: PrivateKey,
    pub id: AccountId,
}

#[must_use]
pub fn key(seed: u8) -> Key {
    let key = PrivateKey::try_new([seed; 32]).expect("a valid key");
    let id = AccountId::from(&PublicKey::new_from_private_key(&key));
    Key { key, id }
}

#[must_use]
pub fn tid(a: &AccountId) -> t::AccountId {
    t::AccountId(*a.value())
}

#[must_use]
pub fn lid(a: &t::AccountId) -> AccountId {
    AccountId::new(a.0)
}

/// The forwarder test guest's root message (`helper-methods/guest`).
#[derive(BorshSerialize)]
pub struct Send {
    pub to_account: [u8; 32],
    pub to_program: [u8; 32],
    pub data: Vec<u8>,
    pub pda_seeds: Vec<[u8; 32]>,
}

impl Send {
    #[must_use]
    pub fn to(actor: Actor, data: Vec<u8>, pda_seeds: Vec<[u8; 32]>) -> Self {
        Self { to_account: *actor.account_id.value(), to_program: *actor.program_account_id.value(), data, pda_seeds }
    }
}

/// One applied transaction's metering.
#[derive(Debug, Clone)]
pub struct Applied {
    pub cycles: u64,
    pub tx_bytes: usize,
    pub events: Vec<lee_core::program::TransactionEvent>,
}

pub struct Engine {
    pub state: V03State,
    pub reg: AccountId,
    pub block: u64,
    /// `(what, cycles, tx bytes)` of every applied transaction, for the report.
    pub meter: Vec<(String, u64, usize)>,
}

#[must_use]
pub fn registry_elf() -> &'static [u8] {
    lezreg_methods::LEZ_PROGRAM_REGISTRY_ELF
}

#[must_use]
pub fn forwarder_elf() -> &'static [u8] {
    lezreg_helper_methods::LEZREG_FORWARDER_ELF
}

#[must_use]
pub fn image_id_of(elf: &[u8]) -> [u32; 8] {
    Program::new(elf.to_vec().into()).expect("a guest").id()
}

#[must_use]
pub fn image_bytes(id: &[u32; 8]) -> [u8; 32] {
    let mut out = [0; 32];
    for (i, w) in id.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

impl Engine {
    /// A state with the registry at `reg` (seeded as a genesis-style header + segments), the
    /// forwarder test program at each of `forwarders`, and `funded` existing public accounts.
    #[must_use]
    pub fn new(reg: AccountId, forwarders: &[AccountId], funded: &[AccountId]) -> Self {
        let mut programs = vec![(reg, Program::new(registry_elf().to_vec().into()).expect("the registry guest loads"))];
        for f in forwarders {
            programs.push((*f, Program::new(forwarder_elf().to_vec().into()).expect("the forwarder loads")));
        }
        let state = V03State::new().with_named_programs(programs).with_public_account_balances(funded.iter().map(|a| (*a, 1_000_000)));
        Self { state, reg, block: 0, meter: Vec::new() }
    }

    /// An empty state with nothing deployed; the registry is deployed later through the loader.
    #[must_use]
    pub fn empty(reg: AccountId) -> Self {
        Self { state: V03State::new(), reg, block: 0, meter: Vec::new() }
    }

    /// Makes accounts exist (with a balance), as if funded earlier.
    pub fn fund(&mut self, accounts: &[AccountId]) {
        let state = std::mem::take(&mut self.state);
        self.state = state.with_public_account_balances(accounts.iter().map(|a| (*a, 1_000_000)));
    }

    #[must_use]
    pub fn actor(&self, account: AccountId) -> Actor {
        Actor::new(account, self.reg)
    }

    fn nonce(&self, id: AccountId) -> Nonce {
        self.state.get_account_by_id_ref(id).map_or(Nonce(0), |a| a.nonce)
    }

    /// Builds, signs and applies one public transaction in a new block (or `same_block`).
    pub fn try_tx(
        &mut self,
        root: Actor,
        message: Vec<u8>,
        actors: Vec<Actor>,
        signers: &[&Key],
        same_block: bool,
        now: u64,
    ) -> Result<Applied, String> {
        let mut declared: Vec<Actor> = Vec::new();
        for a in std::iter::once(root).chain(actors) {
            if !declared.contains(&a) {
                declared.push(a);
            }
        }
        let nonces: BTreeMap<AccountId, Nonce> = signers.iter().map(|k| (k.id, self.nonce(k.id))).collect();
        // No admission evidence, ever.
        let tx = TxMessage::new(root, message, declared, nonces, None, Vec::new());
        let keys: Vec<&PrivateKey> = signers.iter().map(|k| &k.key).collect();
        let witness = WitnessSet::for_message(&tx, &keys);
        let tx = PublicTransaction::new(tx, witness);
        assert!(tx.message.admission_evidence.is_empty());
        let tx_bytes = borsh::to_vec(&tx).expect("borsh").len();
        if !same_block || self.block == 0 {
            self.block += 1;
        }
        let (diff, charge) = ValidatedStateDiff::from_public_transaction_with_cycle_budget(
            &tx,
            &self.state,
            self.block,
            now,
            DEFAULT_PUBLIC_CYCLE_BUDGET,
        )
        .map_err(|e| format!("{e:?} / {e}"))?;
        let events = self.state.apply_state_diff(diff).map_err(|e| format!("{e:?}"))?;
        Ok(Applied { cycles: charge.cycles, tx_bytes, events })
    }

    /// A registry instruction rooted at `root_account`, declaring exactly `actors_to_declare`.
    pub fn try_ix(&mut self, root_account: AccountId, ix: &Instruction, signers: &[&Key], now: u64) -> Result<Applied, String> {
        let actors = actors_to_declare(root_account, self.reg, ix);
        self.try_tx(self.actor(root_account), borsh::to_vec(ix).expect("borsh"), actors, signers, false, now)
    }

    #[track_caller]
    pub fn ok(&mut self, what: &str, root_account: AccountId, ix: &Instruction, signers: &[&Key]) -> Applied {
        let a = self.try_ix(root_account, ix, signers, T0).unwrap_or_else(|e| panic!("{what}: {e}"));
        println!("ok      {what}  ({} cycles, {} tx bytes)", a.cycles, a.tx_bytes);
        self.meter.push((what.to_string(), a.cycles, a.tx_bytes));
        a
    }

    #[track_caller]
    pub fn refuse(&mut self, what: &str, expected: &str, root_account: AccountId, ix: &Instruction, signers: &[&Key]) {
        let before = self.state.clone();
        let e = match self.try_ix(root_account, ix, signers, T0) {
            Ok(_) => panic!("{what}: was accepted, expected a refusal containing {expected:?}"),
            Err(e) => e,
        };
        assert!(e.contains(expected), "{what}: refused for the wrong reason:\n  {e}\n  expected {expected:?}");
        assert!(self.state == before, "{what}: a refused transaction changed state");
        println!("refused {what}  ({expected})");
    }

    /// The registry's actor state at `account` (empty if none).
    #[must_use]
    pub fn reg_state(&self, account: &AccountId) -> Vec<u8> {
        self.state.get_account_by_id(*account).data.actor_state(self.reg).to_vec()
    }

    /// A reader for `lezreg_types::walk`.
    pub fn reader(&self) -> impl Fn(&t::AccountId) -> Vec<u8> + '_ {
        move |a: &t::AccountId| self.reg_state(&lid(a))
    }

    #[must_use]
    pub fn entry(&self, account: &AccountId) -> Option<t::Entry> {
        let s = self.reg_state(account);
        (!s.is_empty()).then(|| borsh::from_slice(&s).expect("an entry"))
    }

    #[must_use]
    pub fn header(&self, account: &AccountId) -> Option<ProgramHeader> {
        let s = self.state.get_account_by_id(*account).data.actor_state(PROGRAM_LOADER_ACCOUNT_ID).to_vec();
        ProgramHeader::from_bytes(&s)
    }

    #[must_use]
    pub fn treg(&self) -> t::AccountId {
        tid(&self.reg)
    }

    /// Writes `elf`'s segments through the real loader, tail to head, one key per segment.
    /// Returns the first segment's account.
    pub fn write_segments(&mut self, elf: &[u8], first_key_seed: u8) -> AccountId {
        let user_elf = risc0_binfmt::ProgramBinary::decode(elf).expect("a risc0 binary").user_elf.to_vec();
        let chunks: Vec<&[u8]> = user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).collect();
        let keys: Vec<Key> = (0..chunks.len()).map(|i| key(first_key_seed + u8::try_from(i).expect("few segments"))).collect();
        for i in (0..chunks.len()).rev() {
            let to = Actor::new(keys[i].id, PROGRAM_LOADER_ACCOUNT_ID);
            let m = LoaderMessage::WriteSegment { bytecode: chunks[i].to_vec(), next_segment: keys.get(i + 1).map(|k| k.id) };
            self.try_tx(to, borsh::to_vec(&m).expect("borsh"), vec![to], &[&keys[i]], false, T0)
                .unwrap_or_else(|e| panic!("WriteSegment {i}: {e}"));
        }
        keys[0].id
    }

    /// Plain loader `CreateHeader` at `header` (signed by it).
    pub fn create_header(&mut self, header: &Key, first_segment: AccountId, immutable: bool) -> Result<Applied, String> {
        let to = Actor::new(header.id, PROGRAM_LOADER_ACCOUNT_ID);
        let m = LoaderMessage::CreateHeader { first_segment, immutable };
        self.try_tx(to, borsh::to_vec(&m).expect("borsh"), vec![to], &[header], false, T0)
    }

    /// Prints the metering of every applied transaction.
    pub fn report(&self, title: &str) {
        println!("\n{title}: cycles and tx bytes of applied transactions");
        for (what, cycles, bytes) in &self.meter {
            println!("  {cycles:>9}  {bytes:>6} B  {what}");
        }
    }
}

// ---- sample instruction builders ----

#[must_use]
pub fn content(tag: u8) -> ContentRef {
    ContentRef { cid: vec![0x01, 0x55, 0x12, 0x20, tag], sha256: [tag; 32], size: 1234, peer_hint: None }
}

#[must_use]
pub fn source() -> SourceRef {
    SourceRef {
        repo: "https://github.com/example/program.git".into(),
        commit: vec![0xAB; 20],
        docker_root: ".".into(),
        archive: None,
    }
}

#[must_use]
pub fn build() -> BuildConfig {
    BuildConfig {
        schema: 1,
        builder: 1,
        image_digest: [0xFA; 32],
        image_tag: "r0.1.91.1".into(),
        risc0_build: "3.0.5".into(),
        host_risc0_rust: "1.91.1".into(),
        manifest_path: "methods/guest/Cargo.toml".into(),
        binary: "guest".into(),
        features: vec![],
        lock_sha256: [0x10; 32],
        docker_env: vec![],
    }
}

#[must_use]
pub fn register_deployer(name: &str, author: &AccountId, image_id: [u8; 32], via_pda_seed: Option<[u8; 32]>) -> Instruction {
    Instruction::RegisterDeployer {
        name: name.into(),
        version: "1.0.0".into(),
        author_label: "Example Labs".into(),
        description: format!("{name}: a sample program"),
        tags: vec!["sample".into()],
        idl: content(1),
        source: source(),
        build: build(),
        image_id,
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
        via_pda_seed,
    }
}

#[must_use]
pub fn register_third_party(program: &AccountId, name: &str) -> Instruction {
    Instruction::RegisterThirdParty {
        program: *program.value(),
        name: name.into(),
        version: "0.1.0".into(),
        author_label: "community".into(),
        description: String::new(),
        tags: vec![],
        idl: content(2),
        source: source(),
        build: build(),
        image_id: [0; 32],
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
    }
}

#[must_use]
pub fn deploy_and_register(name: &str, author: &AccountId, first_segment: AccountId, immutable: bool, image_id: [u8; 32]) -> Instruction {
    Instruction::DeployAndRegister {
        first_segment: *first_segment.value(),
        immutable,
        name: name.into(),
        version: "1.0.0".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: content(3),
        source: source(),
        build: build(),
        image_id,
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: T0,
    }
}

#[must_use]
pub fn update(entry: &AccountId, revision: u32, version: &str) -> Instruction {
    Instruction::Update {
        entry_id: *entry.value(),
        revision,
        revision_seed: t::pda::revision_seed(revision),
        version: Some(version.into()),
        description: Some(format!("revision {revision}")),
        tags: None,
        author_label: None,
        idl: Some(content(u8::try_from(revision).unwrap_or(0xFF))),
        source: None,
        build: None,
        image_id: None,
        now: T0,
        via_pda_seed: None,
    }
}

#[must_use]
pub fn transfer(entry: &AccountId, new_author: &AccountId) -> Instruction {
    Instruction::TransferAuthor { entry_id: *entry.value(), new_author_id: *new_author.value(), now: T0, via_pda_seed: None }
}

#[must_use]
pub fn freeze(entry: &AccountId) -> Instruction {
    Instruction::Freeze { entry_id: *entry.value(), now: T0, via_pda_seed: None }
}

/// `AccountId::for_public_pda`.
#[must_use]
pub fn pda(program: &AccountId, seed: [u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &PdaSeed::new(seed))
}
