//! LEZ Program Registry, `lez_program_registry` (DESIGN §2).
//!
//! This one file is the input to both:
//! 1. the released `spel generate-idl` v0.7.0 (syntactic; it never compiles this file), and
//! 2. the LEZ v0.4.0-rc1 guest build, through `spel04-shim` (`spel` in Cargo.toml).
//!
//! A registry entry is the registry's actor state at an account. A deployer entry for program
//! `X` lives at actor `(X, REG)`, and creating it needs `X`'s authorization: the credential the
//! loader requires for `CreateHeader`/`UpdateHeader`. Third-party entries live at
//! `T = pda["lezreg/v1/tp", program, publisher]`. Discovery uses linked lists in the registry's
//! own PDAs; every address a transaction touches is a pure function of its arguments.
//!
//! New `#[instruction]` and `#[effect]` fns may only be appended (`wire-tags.lock`).

use borsh::{BorshDeserialize, BorshSerialize};
use lezreg_types as t;
use lezreg_types::{BuildConfig, ContentRef, SourceRef};
use spel::lee_core::program::{ProgramEvent, SyncMessage};
use spel::prelude::*;

/// Same borsh layout as `program_loader_core::Message` (rc1), mirrored so the guest does not
/// link the loader's host-only crates. `tools/engine-test` checks the encoding.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum LoaderMessage {
    WriteSegment { bytecode: Vec<u8>, next_segment: Option<AccountId> },
    CreateHeader { first_segment: AccountId, immutable: bool },
    UpdateHeader { first_segment: AccountId, immutable: bool },
}

/// What travels down the index chain: global head → name head → author head
/// (→ program third-party head) → link.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct IndexCtx {
    pub entry: t::AccountId,
    pub kind: u8,
    pub program: t::AccountId,
    pub name: String,
    /// The registrant.
    pub author: t::AccountId,
    pub image_id: [u8; 32],
    pub idl_sha256: [u8; 32],
    pub born_with_deploy: bool,
    pub seq: u64,
    pub prev_global: Option<t::AccountId>,
    pub prev_name: Option<t::AccountId>,
    pub prev_author: Option<t::AccountId>,
    pub prev_program: Option<t::AccountId>,
}

/// A change to an entry, applied by the entry's own actor (or inline when the author's actor
/// is the entry). The sender is the author: `from.account_id`.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Patch {
    /// `Some(stored + 1)` for an `update`; content fields need it.
    pub revision: Option<u32>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub author_label: Option<String>,
    pub idl: Option<ContentRef>,
    pub source: Option<SourceRef>,
    pub build: Option<BuildConfig>,
    pub image_id: Option<[u8; 32]>,
    /// `transfer_author`.
    pub author: Option<t::AccountId>,
    /// `freeze` (irreversible).
    pub freeze: bool,
}

#[must_use]
fn tid(a: &AccountId) -> t::AccountId {
    t::AccountId(*a.value())
}

fn ok(r: Result<(), t::validate::Invalid>) {
    if let Err(e) = r {
        panic!("{e}");
    }
}

fn seed(s: &str) -> [u8; 32] {
    spel::seed_from_str(s)
}

fn global_head(reg: AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::HEAD)])
}
fn name_head(reg: AccountId, name: &str) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::NAME), seed(name)])
}
fn author_head(reg: AccountId, author: &t::AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::AUTHOR), author.0])
}
fn program_head(reg: AccountId, program: &t::AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::PROGRAM_HEAD), program.0])
}
fn link_of(reg: AccountId, entry: &t::AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::LINK), entry.0])
}
fn release_of(reg: AccountId, entry: &t::AccountId, revision: u32) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::RELEASE), entry.0, t::pda::revision_seed(revision)])
}
fn author_node_of(reg: AccountId, new_author: &t::AccountId, entry: &t::AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::AUTHOR_NODE), new_author.0, entry.0])
}
fn third_party_of(reg: AccountId, program: &t::AccountId, publisher: &t::AccountId) -> AccountWithMetadata {
    AccountWithMetadata::pda(reg, &[seed(t::seeds::THIRD_PARTY), program.0, publisher.0])
}

fn assert_at(input: &ReceiveInput, expected: &AccountWithMetadata, what: &str) {
    assert!(input.receiver.account_id == expected.account_id, "{what}: delivered to the wrong address");
}

fn check_first_revision(revision: u32, revision_seed: [u8; 32]) {
    assert!(revision == 1, "revision must be 1 at registration");
    check_revision_seed(revision, revision_seed);
}

fn check_revision_seed(revision: u32, revision_seed: [u8; 32]) {
    assert!(revision_seed == t::pda::revision_seed(revision), "revision_seed must be the revision as 32 little-endian bytes");
}

/// The fields a registration sets, validated.
struct Fields {
    name: String,
    version: String,
    author_label: String,
    description: String,
    tags: Vec<String>,
    idl: ContentRef,
    source: SourceRef,
    build: BuildConfig,
    image_id: [u8; 32],
}

impl Fields {
    fn into_entry(self, kind: u8, flags: u8, program: t::AccountId, author: t::AccountId, now: u64) -> t::Entry {
        ok(t::validate::entry_fields(
            &self.name,
            &self.version,
            &self.author_label,
            &self.description,
            &self.tags,
            &self.idl,
            &self.source,
            &self.build,
        ));
        t::Entry {
            schema: t::SCHEMA,
            kind,
            flags,
            program,
            author,
            registrant: author,
            name: self.name,
            version: self.version,
            author_label: self.author_label,
            description: self.description,
            tags: self.tags,
            idl: self.idl,
            source: self.source,
            build: self.build,
            image_id: self.image_id,
            registered_at: now,
            updated_at: now,
            revision: 1,
        }
    }
}

fn build_hash(build: &BuildConfig) -> [u8; 32] {
    spel::sha256(&borsh::to_vec(build).expect("borsh"))
}

fn release_record(entry_at: t::AccountId, e: &t::Entry, at: u64) -> t::Release {
    t::Release {
        schema: t::SCHEMA,
        entry: entry_at,
        revision: e.revision,
        version: e.version.clone(),
        idl: e.idl.clone(),
        source: e.source.clone(),
        build: e.build.clone(),
        image_id: e.image_id,
        build_hash: build_hash(&e.build),
        at,
    }
}

fn index_ctx(entry_at: t::AccountId, e: &t::Entry) -> IndexCtx {
    IndexCtx {
        entry: entry_at,
        kind: e.kind,
        program: e.program,
        name: e.name.clone(),
        author: e.registrant,
        image_id: e.image_id,
        idl_sha256: e.idl.sha256,
        born_with_deploy: e.born_with_deploy(),
        seq: 0,
        prev_global: None,
        prev_name: None,
        prev_author: None,
        prev_program: None,
    }
}

fn decode_entry(state: &[u8]) -> t::Entry {
    assert!(!state.is_empty(), "no registry entry at this account");
    borsh::from_slice(state).expect("a registry entry")
}

/// A deployer entry at the root `(X, REG)`. `create_header` (DeployAndRegister) is sent first.
#[allow(clippy::too_many_arguments)]
fn deployer_entry(
    program: &AccountWithMetadata,
    author: &AccountWithMetadata,
    release: &AccountWithMetadata,
    head: &AccountWithMetadata,
    fields: Fields,
    revision: u32,
    revision_seed: [u8; 32],
    now: u64,
    create_header: Option<SyncMessage>,
) -> Response {
    check_first_revision(revision, revision_seed);
    let flags = if create_header.is_some() { t::FLAG_BORN_WITH_DEPLOY } else { 0 };
    let x = tid(&program.account_id);
    let entry = fields.into_entry(t::KIND_DEPLOYER, flags, x, tid(&author.account_id), now);
    let rel = release_record(x, &entry, now);
    let ctx = index_ctx(x, &entry);
    let mut r = Response::set_state(borsh::to_vec(&entry).expect("borsh"));
    if let Some(m) = create_header {
        r = r.send(m);
    }
    r.send(release.effect(&Effect::WriteRelease { release: rel })).send(head.effect(&Effect::Index { ctx }))
}

/// Applies a patch to a stored entry; returns the new entry, the messages to send and events.
fn apply_patch(
    mut e: t::Entry,
    by: &AccountId,
    at: &AccountId,
    reg: AccountId,
    receiver_authorized: bool,
    p: Patch,
    now: u64,
) -> (t::Entry, Vec<SyncMessage>, Vec<ProgramEvent>) {
    assert!(e.author == tid(by), "only the entry's author may change it");
    assert!(!e.is_frozen(), "the entry is frozen");
    let entry_at = tid(at);
    let mut msgs = Vec::new();
    let mut events = Vec::new();
    e.updated_at = now;
    if let Some(revision) = p.revision {
        assert!(Some(revision) == e.revision.checked_add(1), "stale revision: an update must carry the stored revision + 1");
        if let Some(v) = p.version {
            ok(t::validate::version(&v));
            e.version = v;
        }
        if let Some(v) = p.description {
            ok(t::validate::description(&v));
            e.description = v;
        }
        if let Some(v) = p.tags {
            ok(t::validate::tags(&v));
            e.tags = v;
        }
        if let Some(v) = p.author_label {
            ok(t::validate::author_label(&v));
            e.author_label = v;
        }
        if let Some(v) = p.idl {
            ok(t::validate::content_ref(&v));
            e.idl = v;
        }
        if let Some(v) = p.source {
            ok(t::validate::source_ref(&v));
            e.source = v;
        }
        if let Some(v) = p.build {
            ok(t::validate::build_config(&v));
            e.build = v;
        }
        if let Some(v) = p.image_id {
            e.image_id = v;
        }
        e.revision = revision;
        let rel = release_record(entry_at, &e, now);
        events.push(spel::event(
            t::events::UPDATED,
            &t::events::Updated {
                entry: entry_at,
                revision,
                version: e.version.clone(),
                image_id: e.image_id,
                idl_sha256: e.idl.sha256,
                build_hash: rel.build_hash,
            },
        ));
        msgs.push(release_of(reg, &entry_at, revision).effect(&Effect::WriteRelease { release: rel }));
    } else {
        assert!(
            p.version.is_none()
                && p.description.is_none()
                && p.tags.is_none()
                && p.author_label.is_none()
                && p.idl.is_none()
                && p.source.is_none()
                && p.build.is_none()
                && p.image_id.is_none(),
            "content changes need a revision"
        );
    }
    if let Some(new) = p.author {
        assert!(new != e.author, "the entry already has this author");
        assert!(new != e.registrant, "a transfer back to a previous author is refused");
        if new == entry_at {
            // Handing a deployer entry back to X: no probe was sent, so this delivery checks it.
            assert!(receiver_authorized, "the new author must sign");
        }
        events.push(spel::event(t::events::AUTHOR_TRANSFERRED, &t::events::AuthorTransferred { entry: entry_at, from: e.author, to: new }));
        e.author = new;
    }
    if p.freeze {
        e.flags |= t::FLAG_FROZEN;
        events.push(spel::event(t::events::FROZEN, &t::events::Frozen { entry: entry_at }));
    }
    (e, msgs, events)
}

fn respond(e: &t::Entry, msgs: Vec<SyncMessage>, events: Vec<ProgramEvent>) -> Response {
    let mut r = Response::set_state(borsh::to_vec(e).expect("borsh"));
    for m in msgs {
        r = r.send(m);
    }
    for ev in events {
        r = r.event(ev);
    }
    r
}

/// Author-side change: inline when the author's actor is the entry, otherwise a `PatchEntry`.
fn change(author: &AccountWithMetadata, entry: &AccountWithMetadata, patch: Patch, now: u64) -> Response {
    if entry.account_id == author.account_id {
        let stored = decode_entry(author.pre_state());
        let (e, msgs, events) = apply_patch(
            stored,
            &author.account_id,
            &author.account_id,
            author.program_account_id,
            author.is_authorized,
            patch,
            now,
        );
        respond(&e, msgs, events)
    } else {
        Response::keep_state().send(entry.effect(&Effect::PatchEntry { patch, now }))
    }
}

fn next_after_author(reg: AccountId, ctx: IndexCtx) -> SyncMessage {
    if ctx.kind == t::KIND_THIRD_PARTY {
        program_head(reg, &ctx.program).effect(&Effect::IndexProgram { ctx })
    } else {
        link_of(reg, &ctx.entry).effect(&Effect::WriteLink { ctx })
    }
}

fn bump(state: &[u8], entry: t::AccountId) -> (t::Head, Option<t::AccountId>, u64) {
    let mut head = t::Head::decode_or_empty(state);
    let (prev, count) = (head.newest, head.count);
    head.count = count.checked_add(1).expect("count overflow");
    head.newest = Some(entry);
    (head, prev, count)
}

#[lez_program(instruction = "Instruction")]
mod lez_program_registry {
    use super::*;

    /// Deployer entry for program `X`, at the root actor `(X, self)`. `X` must authorize it: its
    /// signature, or (via `via_pda_seed`) the owner program of a PDA header. `author` co-signs
    /// and may equal `X`.
    #[instruction]
    pub fn register_deployer(
        #[account(init, signer)] program: AccountWithMetadata,
        #[account(signer)] author: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/rel"), account("program"), arg("revision_seed")])] release: AccountWithMetadata,
        #[account(mut, pda = literal("lezreg/v1/head"))] head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/author"), account("author")])] author_head: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/link"), account("program")])] link: AccountWithMetadata,
        name: String,
        version: String,
        author_label: String,
        description: String,
        tags: Vec<String>,
        idl: ContentRef,
        source: SourceRef,
        build: BuildConfig,
        image_id: [u8; 32],
        author_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    ) -> Response {
        // name_head, author_head and link are reached through the index chain.
        let _ = (name_head, author_head, link, author_id, via_pda_seed);
        let fields = Fields { name, version, author_label, description, tags, idl, source, build, image_id };
        deployer_entry(&program, &author, &release, &head, fields, revision, revision_seed, now, None)
    }

    /// Third-party entry for `program`, at `T = pda["lezreg/v1/tp", program, publisher]`. The
    /// publisher is the author; at most one entry per (program, publisher).
    #[instruction]
    pub fn register_third_party(
        #[account(signer)] publisher: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/tp"), arg("program"), account("publisher")])] entry: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/rel"), account("entry"), arg("revision_seed")])] release: AccountWithMetadata,
        #[account(mut, pda = literal("lezreg/v1/head"))] head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/author"), account("publisher")])] author_head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/tphead"), arg("program")])] program_head: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/link"), account("entry")])] link: AccountWithMetadata,
        program: [u8; 32],
        name: String,
        version: String,
        author_label: String,
        description: String,
        tags: Vec<String>,
        idl: ContentRef,
        source: SourceRef,
        build: BuildConfig,
        image_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        now: u64,
    ) -> Response {
        let _ = (name_head, author_head, program_head, link);
        assert!(program != *publisher.account_id.value(), "a third-party entry cannot be for the publisher's own account");
        check_first_revision(revision, revision_seed);
        let fields = Fields { name, version, author_label, description, tags, idl, source, build, image_id };
        let at = tid(&entry.account_id);
        let e = fields.into_entry(t::KIND_THIRD_PARTY, 0, t::AccountId(program), tid(&publisher.account_id), now);
        let rel = release_record(at, &e, now);
        let ctx = index_ctx(at, &e);
        Response::keep_state()
            .send(entry.effect(&Effect::WriteEntry { entry: e }))
            .send(release.effect(&Effect::WriteRelease { release: rel }))
            .send(head.effect(&Effect::Index { ctx }))
    }

    /// Update by the entry's author, rooted at the author's own actor. Every update writes a
    /// write-once `Release` snapshot; `revision` must be the stored revision + 1.
    #[instruction]
    pub fn update(
        #[account(mut, signer)] author: AccountWithMetadata,
        #[account(mut)] entry: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/rel"), account("entry"), arg("revision_seed")])] release: AccountWithMetadata,
        entry_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        version: Option<String>,
        description: Option<String>,
        tags: Option<Vec<String>>,
        author_label: Option<String>,
        idl: Option<ContentRef>,
        source: Option<SourceRef>,
        build: Option<BuildConfig>,
        image_id: Option<[u8; 32]>,
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    ) -> Response {
        let _ = (release, entry_id, via_pda_seed);
        check_revision_seed(revision, revision_seed);
        assert!(revision >= 2, "an update's revision is at least 2");
        let patch = Patch {
            revision: Some(revision),
            version,
            description,
            tags,
            author_label,
            idl,
            source,
            build,
            image_id,
            author: None,
            freeze: false,
        };
        change(&author, &entry, patch, now)
    }

    /// Hands the entry to `new_author`. Both sign. An entry joins each author's list once, so a
    /// transfer back to a previous author is refused.
    #[instruction]
    pub fn transfer_author(
        #[account(mut, signer)] author: AccountWithMetadata,
        #[account(mut)] entry: AccountWithMetadata,
        #[account(signer)] new_author: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/author"), account("new_author")])] new_author_head: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/anode"), account("new_author"), account("entry")])] author_node: AccountWithMetadata,
        entry_id: [u8; 32],
        new_author_id: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    ) -> Response {
        let _ = (author_node, entry_id, new_author_id, via_pda_seed);
        let to = tid(&new_author.account_id);
        let patch = Patch { author: Some(to), ..Patch::default() };
        change(&author, &entry, patch, now)
            .send(new_author_head.effect(&Effect::IndexAuthorTransfer { entry: tid(&entry.account_id), new_author: to }))
    }

    /// Irreversibly freezes the entry.
    #[instruction]
    pub fn freeze(
        #[account(mut, signer)] author: AccountWithMetadata,
        #[account(mut)] entry: AccountWithMetadata,
        entry_id: [u8; 32],
        now: u64,
        via_pda_seed: Option<[u8; 32]>,
    ) -> Response {
        let _ = (entry_id, via_pda_seed);
        change(&author, &entry, Patch { freeze: true, ..Patch::default() }, now)
    }

    /// Deploys program `X` (loader `CreateHeader`, sent first) and writes its deployer entry in
    /// one transaction; the entry carries `born_with_deploy`. Key-held headers only: there is no
    /// `via_pda_seed`.
    #[instruction]
    pub fn deploy_and_register(
        #[account(init, signer)] program: AccountWithMetadata,
        #[account(mut, owner = loader("program"))] program_header: AccountWithMetadata,
        #[account(signer)] author: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/rel"), account("program"), arg("revision_seed")])] release: AccountWithMetadata,
        #[account(mut, pda = literal("lezreg/v1/head"))] head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/author"), account("author")])] author_head: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/link"), account("program")])] link: AccountWithMetadata,
        first_segment: [u8; 32],
        immutable: bool,
        name: String,
        version: String,
        author_label: String,
        description: String,
        tags: Vec<String>,
        idl: ContentRef,
        source: SourceRef,
        build: BuildConfig,
        image_id: [u8; 32],
        author_id: [u8; 32],
        revision: u32,
        revision_seed: [u8; 32],
        now: u64,
    ) -> Response {
        let _ = (name_head, author_head, link, author_id);
        let create = program_header.send(&LoaderMessage::CreateHeader { first_segment: AccountId::new(first_segment), immutable });
        let fields = Fields { name, version, author_label, description, tags, idl, source, build, image_id };
        deployer_entry(&program, &author, &release, &head, fields, revision, revision_seed, now, Some(create))
    }

    // ---- internal effects (ignored by SPEL's generator: no #[instruction]) ----

    /// Writes a third-party entry at `T(program, sender)`.
    #[effect(init)]
    pub fn write_entry(input: &ReceiveInput, entry: t::Entry) -> Response {
        let from = input.from.expect("an effect has a sender");
        let publisher = tid(&from.account_id);
        assert!(
            entry.kind == t::KIND_THIRD_PARTY && entry.author == publisher && entry.registrant == publisher,
            "write_entry: malformed third-party entry"
        );
        assert_at(input, &third_party_of(input.receiver.program_account_id, &entry.program, &publisher), "write_entry");
        Response::set_state(borsh::to_vec(&entry).expect("borsh"))
    }

    /// Applies an author's change at the entry's own actor.
    #[effect]
    pub fn patch_entry(input: &ReceiveInput, patch: Patch, now: u64) -> Response {
        let from = input.from.expect("an effect has a sender");
        let stored = decode_entry(&input.pre_state);
        let (e, msgs, events) = apply_patch(
            stored,
            &from.account_id,
            &input.receiver.account_id,
            input.receiver.program_account_id,
            input.is_authorized,
            patch,
            now,
        );
        respond(&e, msgs, events)
    }

    /// Writes a release snapshot, once.
    #[effect(init)]
    pub fn write_release(input: &ReceiveInput, release: t::Release) -> Response {
        assert_at(input, &release_of(input.receiver.program_account_id, &release.entry, release.revision), "write_release");
        Response::set_state(borsh::to_vec(&release).expect("borsh"))
    }

    /// Global head: assigns `seq`, links `prev_global`.
    #[effect]
    pub fn index(input: &ReceiveInput, mut ctx: IndexCtx) -> Response {
        let reg = input.receiver.program_account_id;
        assert_at(input, &global_head(reg), "index");
        let (head, prev, count) = bump(&input.pre_state, ctx.entry);
        ctx.seq = count;
        ctx.prev_global = prev;
        let next = name_head(reg, &ctx.name);
        Response::set_state(borsh::to_vec(&head).expect("borsh")).send(next.effect(&Effect::IndexName { ctx }))
    }

    /// Name head: links `prev_name`.
    #[effect]
    pub fn index_name(input: &ReceiveInput, mut ctx: IndexCtx) -> Response {
        let reg = input.receiver.program_account_id;
        assert_at(input, &name_head(reg, &ctx.name), "index_name");
        let (head, prev, _) = bump(&input.pre_state, ctx.entry);
        ctx.prev_name = prev;
        let next = author_head(reg, &ctx.author);
        Response::set_state(borsh::to_vec(&head).expect("borsh")).send(next.effect(&Effect::IndexAuthor { ctx }))
    }

    /// Author head: links `prev_author`.
    #[effect]
    pub fn index_author(input: &ReceiveInput, mut ctx: IndexCtx) -> Response {
        let reg = input.receiver.program_account_id;
        assert_at(input, &author_head(reg, &ctx.author), "index_author");
        let (head, prev, _) = bump(&input.pre_state, ctx.entry);
        ctx.prev_author = prev;
        Response::set_state(borsh::to_vec(&head).expect("borsh")).send(next_after_author(reg, ctx))
    }

    /// A program's third-party head: links `prev_program`.
    #[effect]
    pub fn index_program(input: &ReceiveInput, mut ctx: IndexCtx) -> Response {
        let reg = input.receiver.program_account_id;
        assert!(ctx.kind == t::KIND_THIRD_PARTY, "index_program: deployer entries are not listed per program");
        assert_at(input, &program_head(reg, &ctx.program), "index_program");
        let (head, prev, _) = bump(&input.pre_state, ctx.entry);
        ctx.prev_program = prev;
        let next = link_of(reg, &ctx.entry);
        Response::set_state(borsh::to_vec(&head).expect("borsh")).send(next.effect(&Effect::WriteLink { ctx }))
    }

    /// The entry's link, once; emits `Registered`.
    #[effect(init)]
    pub fn write_link(input: &ReceiveInput, ctx: IndexCtx) -> Response {
        assert_at(input, &link_of(input.receiver.program_account_id, &ctx.entry), "write_link");
        let link = t::Link {
            schema: t::SCHEMA,
            seq: ctx.seq,
            kind: ctx.kind,
            program: ctx.program,
            name: ctx.name.clone(),
            author: ctx.author,
            prev_global: ctx.prev_global,
            prev_name: ctx.prev_name,
            prev_author: ctx.prev_author,
            prev_program: ctx.prev_program,
        };
        let ev = t::events::Registered {
            entry: ctx.entry,
            program: ctx.program,
            kind: ctx.kind,
            author: ctx.author,
            name: ctx.name,
            seq: ctx.seq,
            image_id: ctx.image_id,
            idl_sha256: ctx.idl_sha256,
            born_with_deploy: ctx.born_with_deploy,
        };
        Response::set_state(borsh::to_vec(&link).expect("borsh")).event(spel::event(t::events::REGISTERED, &ev))
    }

    /// An author node, once: an entry joins each author's list at most once.
    #[effect(init)]
    pub fn write_author_node(input: &ReceiveInput, new_author: t::AccountId, node: t::AuthorNode) -> Response {
        assert_at(input, &author_node_of(input.receiver.program_account_id, &new_author, &node.entry), "write_author_node");
        Response::set_state(borsh::to_vec(&node).expect("borsh"))
    }

    /// New author's head after a transfer.
    #[effect]
    pub fn index_author_transfer(input: &ReceiveInput, entry: t::AccountId, new_author: t::AccountId) -> Response {
        let reg = input.receiver.program_account_id;
        assert_at(input, &author_head(reg, &new_author), "index_author_transfer");
        let (head, prev, _) = bump(&input.pre_state, entry);
        let node = t::AuthorNode { schema: t::SCHEMA, entry, prev };
        let next = author_node_of(reg, &new_author, &entry);
        Response::set_state(borsh::to_vec(&head).expect("borsh")).send(next.effect(&Effect::WriteAuthorNode { new_author, node }))
    }
}
