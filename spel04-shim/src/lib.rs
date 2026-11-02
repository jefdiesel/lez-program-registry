//! `spel04-shim`: SPEL attribute syntax, compiled for LEZ v0.4 actors.
//!
//! A program crate depends on this as `spel = { package = "spel04-shim", .. }` and writes
//! `use spel::prelude::*;`. The attribute names (`lez_program`, `instruction`, `account`,
//! `account_type`) and the parameter type `AccountWithMetadata` are the tokens the released
//! SPEL v0.7.0 `generate-idl` matches, so the same source feeds the generator unchanged.
//!
//! Conventions this backend adds for 0.4 (documented in the IDL's `x-lez04` block):
//! - the first account parameter is the **root actor** `(receiver, self)`;
//! - `pda = [..]` accounts derive under `self` by SPEL's `compute_pda` rule; integer `arg`
//!   seeds are a compile error (F3);
//! - any other account `foo` needs an arg `foo_id: [u8; 32]` (F5);
//! - `owner = loader("foo")` names the program loader's actor at `foo`'s account;
//! - non-root `signer` accounts get a `RequireSigner` probe (tag `0xFF`), unless the signer's id
//!   equals the root's or another by-id account's, whose own delivery must check `is_authorized`;
//! - `#[effect]` fns are internal messages, wire tags `0xC0 + i`, accepted only from this
//!   program's own actors; `#[effect(init)]` also requires an empty `pre_state`;
//! - every instruction is root-only (`from == None`), except with an arg
//!   `via_pda_seed: Option<[u8; 32]>` = `Some(s)` where the sender's program `P` satisfies
//!   `for_public_pda(P, s) == receiver` and the receiver is authorized; on that path every
//!   `signer` account must be the root (a co-signer's `is_authorized` there only proves it
//!   signed *some* transaction that reached `P`, not that it consented to this call);
//! - an arg `now: u64` bounds the transaction to blocks with timestamp in
//!   `[now, now + TIME_WINDOW_MS)`;
//! - `wire-tags.lock` beside the program's Cargo.toml pins tag order: new instructions and
//!   effects may only be appended.

pub use borsh;
pub use lee_core;
pub use spel04_shim_macros::{account_type, lez_program};

use borsh::BorshSerialize;
use lee_core::account::{AccountId, ActorState};
use lee_core::program::{PdaSeed, ProgramEvent, ReceiveInput, Response, SyncMessage};

/// First byte of an internal effect on the wire (DESIGN §2.4).
pub const EFFECT_TAG_MIN: u8 = 0xC0;
/// Reserved effect tag: "assert the receiving actor is authorized".
pub const REQUIRE_SIGNER_TAG: u8 = 0xFF;
/// Width of the timestamp window an instruction with a `now` arg is bound to.
pub const TIME_WINDOW_MS: u64 = 600_000;

pub mod prelude {
    pub use crate::{AccountWithMetadata, account_type, lez_program};
    pub use lee_core::account::{AccountId, Actor};
    pub use lee_core::program::{ReceiveInput, Response};
}

/// SPEL's name for an account parameter; under 0.4 it is an *actor handle*.
///
/// Only the root actor carries `pre_state` and `is_authorized`: a 0.4 guest cannot see any
/// other actor's state.
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

    /// An actor of `program` at `account_id`.
    #[must_use]
    pub fn at(account_id: AccountId, program: AccountId) -> Self {
        Self { account_id, program_account_id: program, is_root: false, is_authorized: false, root_state: None, pda_seed: None }
    }

    /// A PDA actor of `program`, derived from seed components by SPEL's rule. Messages sent to
    /// it carry the seed, so the program self-admits it.
    #[must_use]
    pub fn pda(program: AccountId, parts: &[[u8; 32]]) -> Self {
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

    #[must_use]
    pub fn actor(&self) -> lee_core::account::Actor {
        lee_core::account::Actor::new(self.account_id, self.program_account_id)
    }

    /// The root actor's state. Panics on a non-root handle (0.4 has no cross-actor reads).
    #[must_use]
    pub fn pre_state(&self) -> &[u8] {
        self.root_state.as_deref().expect("only the root actor's state is visible in LEZ 0.4")
    }

    /// A sync message to this actor; carries the PDA seed so the program self-admits it.
    pub fn send<M: BorshSerialize>(&self, message: &M) -> SyncMessage {
        let m = SyncMessage::new(self.actor(), message);
        match self.pda_seed {
            Some(s) => m.with_pda_seeds([s]),
            None => m,
        }
    }

    /// An internal effect: variant `i` of the program's generated `Effect` enum goes on the
    /// wire as tag `0xC0 + i`.
    pub fn effect<E: BorshSerialize>(&self, effect: &E) -> SyncMessage {
        self.send(&Shifted(effect))
    }
}

/// Serializes an `Effect` with its variant tag shifted to `0xC0 + i`.
pub struct Shifted<'a, E>(pub &'a E);

impl<E: BorshSerialize> BorshSerialize for Shifted<'_, E> {
    fn serialize<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        let mut v = borsh::to_vec(self.0)?;
        assert!(v[0] < REQUIRE_SIGNER_TAG - EFFECT_TAG_MIN, "too many effect variants");
        v[0] += EFFECT_TAG_MIN;
        w.write_all(&v)
    }
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

#[doc(hidden)]
pub fn __require_signer_handler(input: &ReceiveInput) -> Response {
    assert!(input.from_own_program(), "only this program may probe a signer");
    assert!(input.message.len() == 1, "malformed signer probe");
    assert!(input.is_authorized, "co-signer did not sign");
    Response::keep_state()
}

/// Root-only rule with the PDA-header exception (DESIGN §2.4 rule 1).
#[doc(hidden)]
pub fn __root_guard(input: &ReceiveInput, via_pda_seed: Option<[u8; 32]>) {
    let Some(from) = input.from else {
        assert!(via_pda_seed.is_none(), "via_pda_seed is only for a message sent by the header's owner program");
        return;
    };
    let seed = via_pda_seed.expect("instructions must be the transaction's root message");
    assert!(input.is_authorized, "a PDA header's owner program must authorize it");
    assert!(
        AccountId::for_public_pda(&from.program_account_id, &PdaSeed::new(seed)) == input.receiver.account_id,
        "via_pda_seed does not derive the receiver under the sender's program"
    );
}

/// Valid only in a block whose timestamp is in `[now, now + TIME_WINDOW_MS)`.
#[doc(hidden)]
pub fn __bounded(response: Response, now: u64) -> Response {
    let end = now.checked_add(TIME_WINDOW_MS).expect("now is too large");
    response.try_timestamp_window(now..end).expect("a window of positive length")
}

/// SPEL `ToSeed` rule (`spel-framework-core/src/pda.rs`) for the seed types the shim accepts
/// as `arg` seeds, plus account ids (raw bytes) for `account` seeds.
pub trait ToSeed {
    fn to_seed(&self) -> [u8; 32];
}
impl ToSeed for [u8; 32] {
    fn to_seed(&self) -> [u8; 32] {
        *self
    }
}
impl ToSeed for String {
    fn to_seed(&self) -> [u8; 32] {
        seed_from_str(self)
    }
}
impl ToSeed for &str {
    fn to_seed(&self) -> [u8; 32] {
        seed_from_str(self)
    }
}
impl ToSeed for AccountId {
    fn to_seed(&self) -> [u8; 32] {
        *self.value()
    }
}

/// A string seed component: UTF-8, zero-padded, at most 32 bytes.
#[must_use]
pub fn seed_from_str(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    assert!(b.len() <= 32, "seed string exceeds 32 bytes");
    let mut out = [0; 32];
    out[..b.len()].copy_from_slice(b);
    out
}

/// sha256 through the zkVM's accelerator (software on the host).
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    use risc0_zkvm::sha::{Impl, Sha256 as _};
    let d = Impl::hash_bytes(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(d.as_bytes());
    out
}

/// SPEL `compute_pda` seed combination: one part is used as is, several are sha256(p1‖p2‖…).
#[must_use]
pub fn combine_seeds(parts: &[[u8; 32]]) -> PdaSeed {
    assert!(!parts.is_empty(), "PDA requires at least one seed");
    if parts.len() == 1 {
        return PdaSeed::new(parts[0]);
    }
    let mut buf = Vec::with_capacity(32 * parts.len());
    for p in parts {
        buf.extend_from_slice(p);
    }
    PdaSeed::new(sha256(&buf))
}

/// The address SPEL's rule gives for these components under `program`.
#[must_use]
pub fn pda_address(program: &AccountId, parts: &[[u8; 32]]) -> AccountId {
    AccountId::for_public_pda(program, &combine_seeds(parts))
}

/// A program event: selector `sha256(name)[..8]`, borsh data.
pub fn event<E: BorshSerialize>(name: &str, data: &E) -> ProgramEvent {
    ProgramEvent {
        selector: sha256(name.as_bytes())[..8].try_into().expect("8 bytes"),
        data: borsh::to_vec(data).expect("borsh serialization is infallible"),
    }
}
