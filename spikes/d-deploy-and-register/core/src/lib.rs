//! Spike (d): a toy registry program "REG".
//!
//! A deployer entry for program `X` is REG's actor state at `(X, REG)`. The only
//! credential is `is_authorized` at that actor, i.e. X's own signature, which is
//! the same credential the loader demands for `CreateHeader` on `(X, LOADER)`.
//!
//! `DeployAndRegister` sends the loader's `CreateHeader` for X as a sync message
//! from the root `(X, REG)`, then writes the entry, all in one transaction.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ReceiveInput, Response},
};

/// Same borsh layout as `program_loader_core::Message` (rc1). Mirrored here so the
/// guest does not link the loader's host-only crates (risc0-binfmt etc).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum LoaderMessage {
    WriteSegment { bytecode: Vec<u8>, next_segment: Option<AccountId> },
    CreateHeader { first_segment: AccountId, immutable: bool },
    UpdateHeader { first_segment: AccountId, immutable: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// One-tx deploy + register. Root actor: (X, REG).
    DeployAndRegister { first_segment: AccountId, immutable: bool, record: Vec<u8> },
    /// Two-tx fallback, step 2: register an X whose header already exists. Root: (X, REG).
    Register { record: Vec<u8> },
    /// Test-only: like `DeployAndRegister` but REG skips its own `is_authorized`
    /// check, so the loader's own check is what stands between Mallory and X.
    DeployAndRegisterNoCheck { first_segment: AccountId, immutable: bool, record: Vec<u8> },
}

/// What REG stores at (X, REG).
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Entry {
    pub schema: u8,
    pub born_with_deploy: u8,
    pub record: Vec<u8>,
}

pub const MAX_RECORD: usize = 256;

#[must_use]
pub fn handle(input: &ReceiveInput, instruction: Instruction) -> Response {
    assert!(input.from.is_none(), "REG: root message only");
    let x = input.receiver.account_id;
    match instruction {
        Instruction::DeployAndRegister { first_segment, immutable, record } => {
            assert!(input.is_authorized, "REG: X must sign (is_authorized false at (X, REG))");
            entry(input, 1, record).send_sync(
                Actor::new(x, PROGRAM_LOADER_ACCOUNT_ID),
                &LoaderMessage::CreateHeader { first_segment, immutable },
            )
        }
        Instruction::Register { record } => {
            assert!(input.is_authorized, "REG: X must sign (is_authorized false at (X, REG))");
            entry(input, 0, record)
        }
        Instruction::DeployAndRegisterNoCheck { first_segment, immutable, record } => entry(input, 1, record)
            .send_sync(
                Actor::new(x, PROGRAM_LOADER_ACCOUNT_ID),
                &LoaderMessage::CreateHeader { first_segment, immutable },
            ),
    }
}

fn entry(input: &ReceiveInput, born_with_deploy: u8, record: Vec<u8>) -> Response {
    assert!(input.pre_state.is_empty(), "REG: entry already exists at (X, REG)");
    assert!(record.len() <= MAX_RECORD, "REG: record too long");
    Response::set_state(borsh::to_vec(&Entry { schema: 1, born_with_deploy, record }).expect("borsh"))
}
