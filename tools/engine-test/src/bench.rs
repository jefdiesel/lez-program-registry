//! LEZ's `cycle_bench` method (`tools/cycle_bench/src/main.rs` at v0.4.0-rc3) applied to the
//! registry guest, for docs/CYCLES.md.
//!
//! `cycle_bench` measures an operation as every transition it takes: it drives
//! `WholeExecution::execute` with its own `ExecutionEnvironment` (`Meter`), runs each guest
//! transition through `risc0_zkvm::default_executor()` and records `info.cycles()` (user cycles),
//! one row per transition. Its guests and fixtures are hard-coded (`guest()`, `cases()`), and its
//! `Meter` has no branch for the program loader, so it cannot run a third-party guest or a
//! `DeployAndRegister` as released. This module is the same method with three differences:
//! actor states come from a live engine state instead of fixtures, guests are looked up in
//! [`Engine::elfs`], and loader transitions run `program_loader_core::handle_message` with
//! LEZ's per-byte charge, as `run_program_loader` does at settlement
//! (`lee/state_machine/src/validated_state_diff/mod.rs`).
//!
//! [`measure`] then applies the same transaction to the real state machine and checks that the
//! rows add up to the cycles settlement charges (`ExecutionCharge::cycles`, the execution gas).

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
};

use lee::{PublicTransaction, V03State, program::Program};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    execution_state::{ExecutionEnvironment, ExecutionError, TransactionEntry, TransitionView, WholeExecution},
    from_frame,
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ReceiveInput, Transition},
};
use risc0_zkvm::{ExecutorEnv, ExitCode, default_executor};

use crate::{Applied, Engine};

/// `LOADER_CYCLES_PER_BYTE` in `lee/state_machine/src/validated_state_diff/mod.rs` (private
/// there): settlement charges the loader 1 gas per loader actor state byte it reads, its
/// receiver's pre-state included. [`measure`] checks the sum against the real charge.
pub const LOADER_CYCLES_PER_BYTE: u64 = 1;

/// One transition of an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// `registry`, `forwarder`, `loader`, `native` or a program id.
    pub program: String,
    /// The receiver's account.
    pub receiver: AccountId,
    /// Guest user cycles (`SessionInfo::cycles`); for the loader, the bytes it read ×
    /// [`LOADER_CYCLES_PER_BYTE`]; 0 for the native token.
    pub cycles: u64,
    /// Executor segments (guests only).
    pub segments: usize,
    /// Loader only: the bytes it read.
    pub loader_bytes: Option<u64>,
}

#[derive(Debug)]
pub struct BenchError(pub String);

impl From<ExecutionError> for BenchError {
    fn from(e: ExecutionError) -> Self {
        Self(e.to_string())
    }
}

/// The per-transition meter: `cycle_bench`'s `Meter`, reading a live state.
pub struct Meter<'a> {
    state: &'a V03State,
    elfs: &'a BTreeMap<AccountId, &'static [u8]>,
    names: &'a BTreeMap<AccountId, String>,
    signers: BTreeSet<AccountId>,
    pub rows: Vec<Row>,
    /// Every public actor state the execution asked for, `(account, program) -> state`.
    pub actor_reads: BTreeMap<(AccountId, AccountId), Vec<u8>>,
    /// Every loader actor state the loader read that the execution did not stage itself.
    pub loader_reads: BTreeMap<AccountId, Vec<u8>>,
}

fn loader_state(state: &V03State, id: AccountId) -> Option<&ActorState> {
    state.get_account_by_id_ref(id).map(|a| a.data.actor_state(PROGRAM_LOADER_ACCOUNT_ID))
}

impl ExecutionEnvironment for Meter<'_> {
    type Error = BenchError;

    fn handle_message(&mut self, input: &ReceiveInput, view: &TransitionView<'_>) -> Result<Transition, BenchError> {
        let program = input.receiver.program_account_id;
        if program == PROGRAM_LOADER_ACCOUNT_ID {
            const ABSENT: &ActorState = &ActorState::empty();
            let state = self.state;
            let read = Cell::new(input.pre_state.len() as u64);
            let reads: RefCell<Vec<(AccountId, Vec<u8>)>> = RefCell::new(Vec::new());
            let out = catch_unwind(AssertUnwindSafe(|| {
                program_loader_core::handle_message(input, |id| {
                    let s = match view.staged_state(Actor::new(id, PROGRAM_LOADER_ACCOUNT_ID)) {
                        Some(s) => s,
                        None => {
                            let s = loader_state(state, id).unwrap_or(ABSENT);
                            reads.borrow_mut().push((id, s.to_vec()));
                            s
                        }
                    };
                    read.set(read.get() + s.len() as u64);
                    s
                })
            }))
            .map_err(|_| BenchError("the loader refused (panicked)".into()))?;
            self.loader_reads.extend(reads.into_inner());
            self.rows.push(Row {
                program: "loader".into(),
                receiver: input.receiver.account_id,
                cycles: read.get() * LOADER_CYCLES_PER_BYTE,
                segments: 0,
                loader_bytes: Some(read.get()),
            });
            return Ok(out.0);
        }
        if program == NATIVE_TOKEN_PROGRAM_ID {
            self.rows.push(Row { program: "native".into(), receiver: input.receiver.account_id, cycles: 0, segments: 0, loader_bytes: None });
            return native_token::handle_message(input).map_err(|e| BenchError(format!("native transfer: {e:?}")));
        }
        let elf = self.elfs.get(&program).ok_or_else(|| BenchError(format!("no guest is registered at {program}")))?;
        // cycle_bench's `sample`: a fresh ExecutorEnv with the framed ReceiveInput, then
        // `default_executor().execute(env, elf)` and `info.cycles()`.
        let mut b = ExecutorEnv::builder();
        Program::write_receive_input(input, &mut b).map_err(|e| BenchError(e.to_string()))?;
        let env = b.build().map_err(|e| BenchError(e.to_string()))?;
        let info = default_executor().execute(env, elf).map_err(|e| BenchError(e.to_string()))?;
        if info.exit_code != ExitCode::Halted(0) {
            return Err(BenchError(format!("{program} exited with {:?}", info.exit_code)));
        }
        self.rows.push(Row {
            program: self.names.get(&program).cloned().unwrap_or_else(|| program.to_string()),
            receiver: input.receiver.account_id,
            cycles: info.cycles(),
            segments: info.segments.len(),
            loader_bytes: None,
        });
        let payload = from_frame(&info.journal.bytes).ok_or_else(|| BenchError("malformed journal frame".into()))?;
        borsh::from_slice(payload).map_err(|e| BenchError(e.to_string()))
    }

    fn public_actor_state(&mut self, actor: Actor) -> Result<ActorState, BenchError> {
        let s = self
            .state
            .get_account_by_id_ref(actor.account_id)
            .map_or_else(ActorState::empty, |a| a.data.actor_state(actor.program_account_id).clone());
        self.actor_reads.insert((actor.account_id, actor.program_account_id), s.to_vec());
        Ok(s)
    }

    /// As settlement (`PublicBackend::admits`): an existing account, or a signer.
    fn admits(&mut self, account_id: AccountId) -> Result<bool, BenchError> {
        Ok(self.state.get_account_by_id_ref(account_id).is_some() || self.signers.contains(&account_id))
    }
}

/// One measured operation.
#[derive(Debug, Clone)]
pub struct Measured {
    pub label: String,
    /// One row per transition, in execution order.
    pub rows: Vec<Row>,
    /// What settlement charged (`ExecutionCharge::cycles`): the execution gas.
    pub charged: u64,
    /// Borsh size of the `PublicTransaction`; a charged transaction's data bytes (its storage
    /// gas) are this + 1, the `LeeTransaction::Public` tag.
    pub tx_bytes: usize,
    /// The case, for LEZ's `cycle_bench` with `patches/cycle_bench-external-cases.patch`.
    pub case: serde_json::Value,
}

impl Measured {
    #[must_use]
    pub fn guest_cycles(&self) -> u64 {
        self.rows.iter().filter(|r| r.loader_bytes.is_none()).map(|r| r.cycles).sum()
    }

    #[must_use]
    pub fn loader_cycles(&self) -> u64 {
        self.rows.iter().filter_map(|r| r.loader_bytes).sum::<u64>() * LOADER_CYCLES_PER_BYTE
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Meters `tx` transition by transition against the engine's current state, then applies it
/// to the real state machine and checks the rows sum to the charge.
pub fn measure(e: &mut Engine, label: &str, tx: &PublicTransaction, names: &BTreeMap<AccountId, String>) -> Result<(Measured, Applied), String> {
    let m = &tx.message;
    let mut meter = Meter {
        state: &e.state,
        elfs: &e.elfs,
        names,
        signers: m.nonces.keys().copied().collect(),
        rows: Vec::new(),
        actor_reads: BTreeMap::new(),
        loader_reads: BTreeMap::new(),
    };
    WholeExecution::new(m.public_context.clone(), TransactionEntry::Sync(m.execution.root.clone()), Vec::new(), None, &[])
        .map_err(|err| format!("{label}: {err}"))?
        .execute(&mut meter)
        .map_err(|err| format!("{label}: {}", err.0))?;
    let rows = meter.rows;
    let case = serde_json::json!({
        "label": label,
        "root": { "account": hex(m.execution.root.to.account_id.value()), "program": hex(m.execution.root.to.program_account_id.value()) },
        "message": hex(&m.execution.root.message),
        "actors": m.public_context.actors.iter().map(|a| serde_json::json!({
            "account": hex(a.account_id.value()),
            "program": hex(a.program_account_id.value()),
            "authorized": m.public_context.authorized_accounts.contains(&a.account_id),
            "state": hex(meter.actor_reads.get(&(a.account_id, a.program_account_id)).map_or(&[][..], Vec::as_slice)),
        })).collect::<Vec<_>>(),
        "loader_states": meter.loader_reads.iter().map(|(id, s)| serde_json::json!({ "account": hex(id.value()), "state": hex(s) })).collect::<Vec<_>>(),
        "expected_rows": rows.iter().map(|r| serde_json::json!({ "program": r.program, "cycles": r.cycles })).collect::<Vec<_>>(),
    });
    let applied = e.apply_tx(tx, false, crate::T0).map_err(|err| format!("{label}: {err}"))?;
    let measured = Measured { label: label.into(), rows, charged: applied.cycles, tx_bytes: applied.tx_bytes, case };
    if measured.guest_cycles() + measured.loader_cycles() != measured.charged {
        return Err(format!(
            "{label}: per-transition cycles {} + loader {} != charged {}",
            measured.guest_cycles(),
            measured.loader_cycles(),
            measured.charged
        ));
    }
    Ok((measured, applied))
}
