//! Events (DESIGN §2.5), for an optional self-hosted indexer. No client depends on them.
//! Selector: `sha256("lez_program_registry::<Name>")[..8]`; data is borsh.

use borsh::{BorshDeserialize, BorshSerialize};

use crate::AccountId;

pub const REGISTERED: &str = "lez_program_registry::Registered";
pub const UPDATED: &str = "lez_program_registry::Updated";
pub const AUTHOR_TRANSFERRED: &str = "lez_program_registry::AuthorTransferred";
pub const FROZEN: &str = "lez_program_registry::Frozen";

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    pub entry: AccountId,
    pub program: AccountId,
    pub kind: u8,
    pub author: AccountId,
    pub name: String,
    pub seq: u64,
    pub image_id: [u8; 32],
    pub idl_sha256: [u8; 32],
    pub born_with_deploy: bool,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Updated {
    pub entry: AccountId,
    pub revision: u32,
    pub version: String,
    pub image_id: [u8; 32],
    pub idl_sha256: [u8; 32],
    pub build_hash: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthorTransferred {
    pub entry: AccountId,
    pub from: AccountId,
    pub to: AccountId,
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Frozen {
    pub entry: AccountId,
}

/// `sha256(name)[..8]`.
#[must_use]
pub fn selector(name: &str) -> [u8; 8] {
    crate::pda::sha256(name.as_bytes())[..8].try_into().expect("8 bytes")
}
