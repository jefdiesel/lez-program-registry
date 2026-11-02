//! LEZ Program Registry types (DESIGN §2.2, §2.3, §2.5).
//!
//! Records, limits, the SPEL `compute_pda` address rule and list walkers. This crate has no
//! LEZ dependency so SDKs, the module and third-party apps can link it cheaply; the on-chain
//! program uses the same records and limits and derives the same addresses through
//! `spel04-shim` (checked by tests in `program/` and `tools/idl-drift/`).

pub mod events;
pub mod pda;
pub mod records;
pub mod validate;
pub mod walk;

use borsh::{BorshDeserialize, BorshSerialize};

pub use records::{AuthorNode, BuildConfig, ContentRef, EnvVar, Entry, Head, Link, Release, SourceRef};

/// A 32-byte LEZ account id. Borsh-identical to `lee_core::account::AccountId`; named
/// `AccountId` so SPEL's IDL generator reports it as `account_id`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AccountId(pub [u8; 32]);

impl AccountId {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Base58, as LEZ wallets print account ids.
    #[must_use]
    pub fn to_base58(&self) -> String {
        bs58::encode(self.0).into_string()
    }

    /// Parses a base58 account id.
    pub fn from_base58(s: &str) -> Option<Self> {
        let v = bs58::decode(s).into_vec().ok()?;
        Some(Self(v.try_into().ok()?))
    }
}

impl From<[u8; 32]> for AccountId {
    fn from(v: [u8; 32]) -> Self {
        Self(v)
    }
}

impl core::fmt::Debug for AccountId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_base58())
    }
}

impl core::fmt::Display for AccountId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.to_base58())
    }
}

/// Every record's first byte.
pub const SCHEMA: u8 = 1;

pub const KIND_DEPLOYER: u8 = 0;
pub const KIND_THIRD_PARTY: u8 = 1;

pub const FLAG_FROZEN: u8 = 1;
pub const FLAG_BORN_WITH_DEPLOY: u8 = 2;

/// `BuildConfig.builder`: risc0-build Docker (`cargo risczero build`).
pub const BUILDER_RISC0_DOCKER: u8 = 1;

/// Every instruction is valid only in a block whose timestamp is in `[now, now + 10 min)`.
pub const TIME_WINDOW_MS: u64 = 600_000;

/// The program's name, as in the IDL and in event selectors.
pub const PROGRAM_NAME: &str = "lez_program_registry";

/// PDA seed domains (SPEL `const` seeds). Each is at most 32 bytes.
pub mod seeds {
    pub const THIRD_PARTY: &str = "lezreg/v1/tp";
    pub const RELEASE: &str = "lezreg/v1/rel";
    pub const LINK: &str = "lezreg/v1/link";
    pub const HEAD: &str = "lezreg/v1/head";
    pub const NAME: &str = "lezreg/v1/name";
    pub const AUTHOR: &str = "lezreg/v1/author";
    pub const PROGRAM_HEAD: &str = "lezreg/v1/tphead";
    pub const AUTHOR_NODE: &str = "lezreg/v1/anode";
}

/// Limits enforced in the guest (DESIGN §2.3).
pub mod limits {
    pub const NAME: usize = 32;
    pub const VERSION: usize = 32;
    pub const AUTHOR_LABEL: usize = 64;
    pub const DESCRIPTION: usize = 512;
    pub const TAGS: usize = 8;
    pub const TAG: usize = 24;
    pub const CID: usize = 64;
    pub const PEER_HINT: usize = 48;
    pub const REPO: usize = 160;
    pub const DOCKER_ROOT: usize = 96;
    pub const IMAGE_TAG: usize = 48;
    pub const RISC0_BUILD: usize = 16;
    pub const HOST_RISC0_RUST: usize = 16;
    pub const MANIFEST_PATH: usize = 96;
    pub const BINARY: usize = 48;
    pub const FEATURES: usize = 4;
    pub const FEATURE: usize = 32;
    pub const DOCKER_ENV: usize = 4;
    /// `key.len() + value.len()` of one `docker_env` entry.
    pub const DOCKER_ENV_PAIR: usize = 64;
}
