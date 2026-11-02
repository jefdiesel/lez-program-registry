//! On-chain records (DESIGN §2.3). Borsh; every record starts with `schema: u8 = 1`.
//!
//! The `#[account_type]` structs are what the released `spel generate-idl` lists under the
//! IDL's `accounts[]`; the plain structs they reference land in `types[]`. The generator reaches
//! this file because the program crate names this crate as a `path` dependency.

use borsh::{BorshDeserialize, BorshSerialize};
use spel04_shim_macros::account_type;

use crate::AccountId;

/// A registry entry. Deployer entries live at actor `(X, REG)`, third-party entries at
/// `(T, REG)` with `T = pda["lezreg/v1/tp", program, publisher]`.
#[account_type]
pub struct Entry {
    pub schema: u8,
    /// 0 = Deployer, 1 = ThirdParty.
    pub kind: u8,
    /// bit0 frozen, bit1 born_with_deploy.
    pub flags: u8,
    /// The program's header account `X`.
    pub program: AccountId,
    /// Proven co-signer; the only account allowed to change the entry.
    pub author: AccountId,
    /// The original author, kept after a transfer.
    pub registrant: AccountId,
    /// `[a-z0-9][a-z0-9._-]*`, 1..=32 bytes, immutable (also a PDA seed).
    pub name: String,
    /// Printable ASCII, 1..=32 bytes.
    pub version: String,
    /// UTF-8, 0..=64 bytes, UNPROVEN display name.
    pub author_label: String,
    /// UTF-8, 0..=512 bytes.
    pub description: String,
    /// At most 8 tags of `[a-z0-9-]`, 1..=24 bytes each.
    pub tags: Vec<String>,
    pub idl: ContentRef,
    pub source: SourceRef,
    pub build: BuildConfig,
    /// Claimed image id; equals `ProgramHeader.image_id` while the entry is fresh.
    pub image_id: [u8; 32],
    /// Milliseconds, bound to the block time by a `[now, now + 10 min)` window.
    pub registered_at: u64,
    pub updated_at: u64,
    /// 1 at registration, +1 per `Update`.
    pub revision: u32,
}

/// Write-once snapshot of one revision of an entry, at `pda["lezreg/v1/rel", entry, revision_seed]`.
#[account_type]
pub struct Release {
    pub schema: u8,
    pub entry: AccountId,
    pub revision: u32,
    pub version: String,
    pub idl: ContentRef,
    pub source: SourceRef,
    pub build: BuildConfig,
    pub image_id: [u8; 32],
    /// `sha256(borsh(BuildConfig))`.
    pub build_hash: [u8; 32],
    pub at: u64,
}

/// One per entry, written once with it, at `pda["lezreg/v1/link", entry]`: the entry's place in
/// every list it joined at registration.
#[account_type]
pub struct Link {
    pub schema: u8,
    /// Position in the global list (0 = first entry ever).
    pub seq: u64,
    pub kind: u8,
    pub program: AccountId,
    pub name: String,
    /// The registrant: tells an author-list walker where `prev` lives.
    pub author: AccountId,
    pub prev_global: Option<AccountId>,
    pub prev_name: Option<AccountId>,
    pub prev_author: Option<AccountId>,
    /// Third-party entries only: previous third-party entry for the same program.
    pub prev_program: Option<AccountId>,
}

/// Head of a linked list (global, per name, per author, per program's third-party entries).
#[account_type]
pub struct Head {
    pub schema: u8,
    pub count: u64,
    pub newest: Option<AccountId>,
}

/// An entry joining a new author's list after a transfer, at
/// `pda["lezreg/v1/anode", new_author, entry]`. Written once: an entry joins each list once.
#[account_type]
pub struct AuthorNode {
    pub schema: u8,
    pub entry: AccountId,
    pub prev: Option<AccountId>,
}

/// Off-chain content pointer (IDL, source archive).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ContentRef {
    /// Binary CIDv1 of the Storage manifest, at most 64 bytes.
    pub cid: Vec<u8>,
    /// sha256 of the content bytes.
    pub sha256: [u8; 32],
    pub size: u32,
    /// libp2p peer id of a pinning node, at most 48 bytes.
    pub peer_hint: Option<Vec<u8>>,
}

/// Where the source lives.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct SourceRef {
    /// `https://` or git URL, at most 160 bytes; may be empty if `archive` is set.
    pub repo: String,
    /// 20 bytes (sha1) or 32 bytes (sha256 object format); empty only without a repo.
    pub commit: Vec<u8>,
    /// Build context inside the repo, at most 96 bytes.
    pub docker_root: String,
    /// Deterministic `git archive` of `docker_root` at `commit`, on Storage.
    pub archive: Option<ContentRef>,
}

/// Everything that determines the guest bytes (DESIGN §4.1).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct BuildConfig {
    pub schema: u8,
    /// 1 = risc0-build Docker (`cargo risczero build`).
    pub builder: u8,
    /// sha256 digest of `risczero/risc0-guest-builder` (authoritative).
    pub image_digest: [u8; 32],
    /// e.g. `r0.1.91.1` (informational).
    pub image_tag: String,
    /// e.g. `3.0.5`.
    pub risc0_build: String,
    /// rzup Rust version on the host (changes RUSTFLAGS).
    pub host_risc0_rust: String,
    /// Guest Cargo.toml, relative to `docker_root`.
    pub manifest_path: String,
    /// Bin target name.
    pub binary: String,
    pub features: Vec<String>,
    /// sha256 of the Cargo.lock the guest build uses.
    pub lock_sha256: [u8; 32],
    /// Extra `ENV` lines from `DockerOptions::env`; usually empty.
    pub docker_env: Vec<EnvVar>,
}

/// One `docker_env` entry. Borsh-identical to the `(String, String)` tuple in DESIGN §2.3; a
/// named struct because SPEL v0.7.0's generator maps tuples to `unknown`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

impl Entry {
    #[must_use]
    pub const fn is_frozen(&self) -> bool {
        self.flags & crate::FLAG_FROZEN != 0
    }

    #[must_use]
    pub const fn born_with_deploy(&self) -> bool {
        self.flags & crate::FLAG_BORN_WITH_DEPLOY != 0
    }
}

impl Head {
    /// An empty actor state reads as an empty list.
    #[must_use]
    pub fn decode_or_empty(state: &[u8]) -> Self {
        if state.is_empty() {
            Self { schema: crate::SCHEMA, count: 0, newest: None }
        } else {
            borsh::from_slice(state).expect("a Head record")
        }
    }
}
