//! `spel04-shim` (spike): SPEL attribute syntax, compiled for LEZ v0.4 actors.
//!
//! A program crate depends on this as `spel = { package = "spel04-shim", .. }` and writes
//! `use spel::prelude::*;`. The attribute names (`lez_program`, `instruction`, `account`,
//! `account_type`) and the parameter type `AccountWithMetadata` are the ones SPEL's
//! syntactic IDL generator matches, so the same source feeds `spel generate-idl`.

pub use borsh;
pub use lee_core;
pub use spel04_shim_macros::{account_type, lez_program};

use borsh::BorshSerialize;
use lee_core::account::{AccountId, ActorState};
use lee_core::program::{PdaSeed, ReceiveInput, SyncMessage};

/// First byte of an internal effect on the wire (DESIGN §2.4).
pub const EFFECT_TAG_MIN: u8 = 0xC0;
/// Reserved effect tag: "assert the receiving actor is authorized" (the `RequireSigner` probe).
pub const REQUIRE_SIGNER_TAG: u8 = 0xFF;

pub mod prelude {
    pub use crate::{account_type, lez_program, AccountWithMetadata};
    pub use lee_core::account::{AccountId, Actor};
    pub use lee_core::program::{ReceiveInput, Response};
}

/// SPEL's name for an account parameter; under 0.4 it is an *actor handle*.
///
/// Only the root actor (the first account parameter) carries `pre_state` and
/// `is_authorized`; in 0.4 a guest cannot see any other actor's state.
#[derive(Clone, Debug)]
pub struct AccountWithMetadata {
    pub account_id: AccountId,
    pub program_account_id: AccountId,
    pub is_root: bool,
    pub is_authorized: bool,
    root_state: Option<ActorState>,
    pda_seed: Option<PdaSeed>,
}

impl AccountWithMetadata {
    #[doc(hidden)]
    pub fn __root(input: &ReceiveInput) -> Self {
        Self {
            account_id: input.receiver.account_id,
            program_account_id: input.receiver.program_account_id,
            is_root: true,
            is_authorized: input.is_authorized,
            root_state: Some(input.pre_state.clone()),
            pda_seed: None,
        }
    }

    #[doc(hidden)]
    pub fn __at(account_id: AccountId, program: AccountId) -> Self {
        Self { account_id, program_account_id: program, is_root: false, is_authorized: false, root_state: None, pda_seed: None }
    }

    #[doc(hidden)]
    pub fn __pda(program: AccountId, parts: &[[u8; 32]]) -> Self {
        let seed = combine_seeds(parts);
        Self {
            account_id: AccountId::for_public_pda(&program, &seed),
            program_account_id: program,
            is_root: false,
            is_authorized: false,
            root_state: None,
            pda_seed: Some(seed),
        }
    }

    pub fn actor(&self) -> lee_core::account::Actor {
        lee_core::account::Actor::new(self.account_id, self.program_account_id)
    }

    /// The root actor's state. Panics on a non-root handle (0.4 has no cross-actor reads).
    pub fn pre_state(&self) -> &[u8] {
        self.root_state.as_deref().expect("only the root actor's state is visible in LEZ 0.4")
    }

    /// A sync message to this actor; carries the PDA seed so the program self-admits it.
    pub fn send<M: BorshSerialize>(&self, message: &M) -> SyncMessage {
        let m = SyncMessage::new(self.actor(), message);
        match self.pda_seed { Some(s) => m.with_pda_seeds([s]), None => m }
    }

    /// An internal effect (`Effect` enum variant `i` goes on the wire as tag `0xC0 + i`).
    pub fn effect<E: BorshSerialize>(&self, effect: &E) -> SyncMessage {
        self.send(&Shifted(effect))
    }
}

struct Shifted<'a, E>(&'a E);
impl<E: BorshSerialize> BorshSerialize for Shifted<'_, E> {
    fn serialize<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        let mut v = borsh::to_vec(self.0)?;
        assert!(v[0] < REQUIRE_SIGNER_TAG - EFFECT_TAG_MIN, "too many effect variants");
        v[0] += EFFECT_TAG_MIN;
        w.write_all(&v)
    }
}

/// Undo the effect tag shift (for the program's `#[effect_handler]`).
pub fn unshift(data: &[u8]) -> Vec<u8> {
    let mut v = data.to_vec();
    v[0] -= EFFECT_TAG_MIN;
    v
}

struct RequireSigner;
impl BorshSerialize for RequireSigner {
    fn serialize<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        w.write_all(&[REQUIRE_SIGNER_TAG])
    }
}

#[doc(hidden)]
pub fn __require_signer(of: &AccountWithMetadata) -> SyncMessage {
    SyncMessage::new(of.actor(), &RequireSigner)
}

/// SPEL v0.7.0 `ToSeed` rule (`spel-framework-core/src/pda.rs`), plus `AccountId` (raw bytes).
pub trait ToSeed {
    fn to_seed(&self) -> [u8; 32];
}
impl ToSeed for [u8; 32] {
    fn to_seed(&self) -> [u8; 32] { *self }
}
impl ToSeed for u64 {
    fn to_seed(&self) -> [u8; 32] { let mut s = [0; 32]; s[..8].copy_from_slice(&self.to_le_bytes()); s }
}
impl ToSeed for u32 {
    fn to_seed(&self) -> [u8; 32] { let mut s = [0; 32]; s[..4].copy_from_slice(&self.to_le_bytes()); s }
}
impl ToSeed for String {
    fn to_seed(&self) -> [u8; 32] { seed_from_str(self) }
}
impl ToSeed for &str {
    fn to_seed(&self) -> [u8; 32] { seed_from_str(self) }
}
impl ToSeed for AccountId {
    fn to_seed(&self) -> [u8; 32] { *self.value() }
}

pub fn seed_from_str(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() <= 32, "seed string '{s}' exceeds 32 bytes");
    let mut out = [0; 32];
    out[..b.len()].copy_from_slice(b);
    out
}

/// SPEL `compute_pda` seed combination: one part is used as is, several are sha256(p1‖p2‖…).
pub fn combine_seeds(parts: &[[u8; 32]]) -> PdaSeed {
    assert!(!parts.is_empty(), "PDA requires at least one seed");
    if parts.len() == 1 {
        return PdaSeed::new(parts[0]);
    }
    use risc0_zkvm::sha::{Impl, Sha256 as _};
    let mut buf = Vec::with_capacity(32 * parts.len());
    for p in parts {
        buf.extend_from_slice(p);
    }
    let d = Impl::hash_bytes(&buf);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    PdaSeed::new(out)
}

#[doc(hidden)]
pub fn __require_signer_handler(input: &ReceiveInput) -> lee_core::program::Response {
    assert!(input.from_own_program(), "only this program may probe a signer");
    assert!(input.is_authorized, "co-signer did not sign");
    lee_core::program::Response::keep_state()
}
