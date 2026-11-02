//! Toy registry in SPEL attribute syntax (DESIGN.md §6.2), spike (a).
//!
//! This one file is the input to BOTH:
//!   1. the released `spel generate-idl` v0.7.0 (syntactic; never compiles it), and
//!   2. `cargo build` of a LEZ v0.4.0-rc1 guest, through `spel04-shim`.
//!
//! The only thing that picks the backend is the `spel` dependency alias in
//! Cargo.toml (`spel = { package = "spel04-shim", .. }`). The source has no cfg.

use borsh::{BorshDeserialize, BorshSerialize};
use spel::prelude::*;

/// Off-chain content pointer (not annotated: must land in the IDL's `types[]`).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContentRef {
    pub cid: Vec<u8>,
    pub sha256: [u8; 32],
    pub size: u32,
}

/// Registry record, stored as the actor state of `(X, REG)`.
#[account_type]
pub struct Entry {
    pub schema: u8,
    pub kind: u8,
    pub flags: u8,
    pub program: AccountId,
    pub author: AccountId,
    pub name: String,
    pub version: String,
    pub idl: ContentRef,
    pub image_id: [u8; 32],
    pub registered_at: u64,
    pub updated_at: u64,
    pub revision: u64,
}

/// Write-once snapshot of one revision.
#[account_type]
pub struct Release {
    pub schema: u8,
    pub entry: AccountId,
    pub revision: u64,
    pub version: String,
    pub image_id: [u8; 32],
    pub at: u64,
}

/// Internal messages between the registry's own actors (wire tags 0xC0..).
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub enum Effect {
    WriteRelease(Release),
    Index { entry: AccountId, name: String },
}

#[lez_program(instruction = "Instruction")]
mod toy_registry {
    use super::*;

    /// Deployer entry: root actor is `(program, self)`; `program` is the header account.
    #[instruction]
    pub fn register(
        #[account(mut, signer)] program: AccountWithMetadata,
        #[account(signer)] author: AccountWithMetadata,
        #[account(init, pda = [literal("toyreg/v1/rel"), account("program"), arg("revision")])]
        release: AccountWithMetadata,
        #[account(mut, pda = literal("toyreg/v1/head"))] head: AccountWithMetadata,
        #[account(mut, pda = [literal("toyreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        author_id: [u8; 32],
        name: String,
        version: String,
        idl: ContentRef,
        image_id: [u8; 32],
        revision: u64,
        now: u64,
    ) -> Response {
        assert!(program.pre_state().is_empty(), "entry already exists");
        assert!(revision == 1, "revision starts at 1");
        assert!(!name.is_empty() && name.len() <= 32, "bad name");
        let entry = Entry {
            schema: 1,
            kind: 0,
            flags: 0,
            program: program.account_id,
            author: author.account_id,
            name: name.clone(),
            version: version.clone(),
            idl,
            image_id,
            registered_at: now,
            updated_at: now,
            revision,
        };
        let release_rec = Release { schema: 1, entry: program.account_id, revision, version, image_id, at: now };
        Response::set_state(borsh::to_vec(&entry).unwrap())
            .send(release.effect(&Effect::WriteRelease(release_rec)))
            .send(head.effect(&Effect::Index { entry: program.account_id, name: name.clone() }))
            .send(name_head.effect(&Effect::Index { entry: program.account_id, name }))
    }

    /// Author-side update: root actor is `(author, self)`; the entry is patched by an effect.
    #[instruction]
    pub fn update(
        #[account(mut, signer)] author: AccountWithMetadata,
        #[account(mut)] entry: AccountWithMetadata,
        #[account(init, pda = [literal("toyreg/v1/rel"), account("entry"), arg("revision")])]
        release: AccountWithMetadata,
        entry_id: [u8; 32],
        revision: u64,
        version: Option<String>,
        image_id: Option<[u8; 32]>,
        now: u64,
    ) -> Response {
        let _ = (&entry, image_id);
        let release_rec = Release {
            schema: 1,
            entry: AccountId::new(entry_id),
            revision,
            version: version.unwrap_or_default(),
            image_id: [0; 32],
            at: now,
        };
        Response::keep_state().send(release.effect(&Effect::WriteRelease(release_rec)))
    }

    /// Irreversible freeze.
    #[instruction]
    pub fn freeze(
        #[account(signer)] author: AccountWithMetadata,
        #[account(mut)] entry: AccountWithMetadata,
        entry_id: [u8; 32],
        now: u64,
    ) -> Response {
        let _ = (author, entry, entry_id, now);
        Response::keep_state()
    }

    /// seedcheck only: const and string-arg seeds through the spel CLI resolver.
    #[instruction]
    pub fn probe(
        #[account(signer)] author: AccountWithMetadata,
        #[account(mut, pda = literal("toyreg/v1/head"))] head: AccountWithMetadata,
        #[account(mut, pda = [literal("toyreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        name: String,
    ) -> Response {
        let _ = (author, head, name_head, name);
        Response::keep_state()
    }

    /// Internal effects; ignored by SPEL's generator (no #[instruction]).
    #[effect_handler]
    pub fn on_effect(input: &ReceiveInput, bytes: &[u8]) -> Response {
        match borsh::from_slice::<Effect>(bytes).expect("effect must decode") {
            Effect::WriteRelease(rec) => {
                assert!(input.pre_state.is_empty(), "release is write-once");
                Response::set_state(borsh::to_vec(&rec).unwrap())
            }
            Effect::Index { .. } => Response::keep_state(),
        }
    }
}
