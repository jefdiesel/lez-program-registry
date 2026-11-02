//! Host unit tests of the generated dispatcher and the handlers (DESIGN §8.1), with
//! `ReceiveInput` built by hand. No zkVM.

use std::panic::{AssertUnwindSafe, catch_unwind};

use borsh::BorshDeserialize;
use lee_core::{
    account::{AccountId, Actor},
    program::{PROGRAM_LOADER_ACCOUNT_ID, PdaSeed, ReceiveInput, Response},
};
use lezreg_program::{
    Effect, INSTRUCTION_NAMES, IndexCtx, Instruction, LoaderMessage, Message, Patch, WIRE_TAGS, __lez_dispatch,
    actors_to_declare,
};
use lezreg_types::{self as t, BuildConfig, ContentRef, SourceRef, pda::addr};

const NOW: u64 = 1_800_000_000_000;

fn reg() -> AccountId {
    AccountId::new([0xEE; 32])
}
fn other_program() -> AccountId {
    AccountId::new([0xDD; 32])
}
fn x() -> AccountId {
    AccountId::new([0x11; 32])
}
fn alice() -> AccountId {
    AccountId::new([0x22; 32])
}
fn bob() -> AccountId {
    AccountId::new([0x33; 32])
}
fn tid(a: &AccountId) -> t::AccountId {
    t::AccountId(*a.value())
}
fn lid(a: &t::AccountId) -> AccountId {
    AccountId::new(a.0)
}

thread_local!(static SILENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });

/// Silences the expected panics inside `run`, and only those.
fn quiet() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !SILENT.with(std::cell::Cell::get) {
                default(info);
            }
        }));
    });
}

fn input(receiver: Actor, from: Option<Actor>, authorized: bool, pre: Vec<u8>, message: Vec<u8>) -> ReceiveInput {
    ReceiveInput { receiver, from, is_authorized: authorized, pre_state: pre.into(), message }
}

fn run(i: &ReceiveInput) -> Result<Response, String> {
    quiet();
    SILENT.with(|s| s.set(true));
    let r = catch_unwind(AssertUnwindSafe(|| __lez_dispatch(i)));
    SILENT.with(|s| s.set(false));
    r.map_err(|p| {
        p.downcast_ref::<&str>().map(ToString::to_string).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default()
    })
}

#[track_caller]
fn refused(i: &ReceiveInput, expected: &str) {
    match run(i) {
        Ok(_) => panic!("expected a refusal containing {expected:?}"),
        Err(e) => assert!(e.contains(expected), "refused for the wrong reason: {e:?} (expected {expected:?})"),
    }
}

fn content() -> ContentRef {
    ContentRef { cid: vec![1; 38], sha256: [2; 32], size: 100, peer_hint: None }
}
fn source() -> SourceRef {
    SourceRef { repo: "https://example.com/r.git".into(), commit: vec![3; 20], docker_root: ".".into(), archive: None }
}
fn build() -> BuildConfig {
    BuildConfig {
        schema: 1,
        builder: 1,
        image_digest: [4; 32],
        image_tag: "r0.1.91.1".into(),
        risc0_build: "3.0.5".into(),
        host_risc0_rust: "1.91.1".into(),
        manifest_path: "methods/guest/Cargo.toml".into(),
        binary: "guest".into(),
        features: vec![],
        lock_sha256: [5; 32],
        docker_env: vec![],
    }
}

fn register_deployer(author: &AccountId, name: &str, via: Option<[u8; 32]>) -> Instruction {
    Instruction::RegisterDeployer {
        name: name.into(),
        version: "1.0.0".into(),
        author_label: "Acme".into(),
        description: "a program".into(),
        tags: vec!["token".into()],
        idl: content(),
        source: source(),
        build: build(),
        image_id: [9; 32],
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: NOW,
        via_pda_seed: via,
    }
}

fn register_third_party(program: &AccountId) -> Instruction {
    Instruction::RegisterThirdParty {
        program: *program.value(),
        name: "community".into(),
        version: "0.1.0".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: content(),
        source: source(),
        build: build(),
        image_id: [9; 32],
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: NOW,
    }
}

fn update(entry: &AccountId, revision: u32) -> Instruction {
    Instruction::Update {
        entry_id: *entry.value(),
        revision,
        revision_seed: t::pda::revision_seed(revision),
        version: Some("2.0.0".into()),
        description: None,
        tags: None,
        author_label: None,
        idl: None,
        source: None,
        build: None,
        image_id: Some([8; 32]),
        now: NOW,
        via_pda_seed: None,
    }
}

fn msg(m: &Instruction) -> Vec<u8> {
    borsh::to_vec(m).unwrap()
}

fn emsg(e: &Effect) -> Vec<u8> {
    borsh::to_vec(&Message::Effect(e.clone())).unwrap()
}

fn stored_entry(author: &AccountId, flags: u8) -> t::Entry {
    t::Entry {
        schema: 1,
        kind: 0,
        flags,
        program: tid(&x()),
        author: tid(author),
        registrant: tid(author),
        name: "token".into(),
        version: "1.0.0".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: content(),
        source: source(),
        build: build(),
        image_id: [9; 32],
        registered_at: NOW,
        updated_at: NOW,
        revision: 1,
    }
}

fn ctx(entry: &AccountId, kind: u8) -> IndexCtx {
    IndexCtx {
        entry: tid(entry),
        kind,
        program: tid(&x()),
        name: "token".into(),
        author: tid(&alice()),
        image_id: [9; 32],
        idl_sha256: [2; 32],
        born_with_deploy: false,
        seq: 0,
        prev_global: None,
        prev_name: None,
        prev_author: None,
        prev_program: None,
    }
}

fn decode_effect(data: &[u8]) -> Effect {
    match Message::try_from_slice(data).unwrap() {
        Message::Effect(e) => e,
        other => panic!("not an effect: {other:?}"),
    }
}

// ---- wire format ----

#[test]
fn wire_tags_lock_matches_generated() {
    let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/wire-tags.lock")).unwrap();
    let locked: Vec<&str> = lock.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let generated: Vec<&str> = WIRE_TAGS.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(locked, generated, "append the new tags to wire-tags.lock");
}

#[test]
fn message_codec_tags() {
    let ix = register_deployer(&x(), "token", None);
    let bytes = msg(&ix);
    assert_eq!(bytes[0], 0);
    assert_eq!(Message::try_from_slice(&bytes).unwrap(), Message::Instruction(ix.clone()));
    assert_eq!(INSTRUCTION_NAMES[0], ix.name());
    let freeze = Instruction::Freeze { entry_id: [1; 32], now: 7, via_pda_seed: None };
    let b = msg(&freeze);
    assert_eq!(b[0], 4);
    assert_eq!(&b[1..33], &[1; 32]);
    assert_eq!(&b[33..41], &7u64.to_le_bytes());
    assert_eq!(b[41], 0);
    let e = Effect::IndexAuthorTransfer { entry: t::AccountId([1; 32]), new_author: t::AccountId([2; 32]) };
    let b = emsg(&e);
    assert_eq!(b[0], 0xC9);
    assert_eq!(decode_effect(&b), e);
    assert_eq!(borsh::to_vec(&Message::RequireSigner).unwrap(), vec![0xFF]);
    assert_eq!(Message::try_from_slice(&[0xFF]).unwrap(), Message::RequireSigner);
    for bad in [6u8, 0x10, 0xBF, 0xCA, 0xFE] {
        assert!(Message::try_from_slice(&[bad, 0, 0]).is_err(), "tag {bad:#x}");
    }
}

#[test]
fn unknown_tags_and_garbage_are_refused() {
    let root = Actor::new(x(), reg());
    refused(&input(root, None, true, vec![], vec![6]), "instruction must decode");
    refused(&input(root, None, true, vec![], vec![0xFE]), "only this program may send an effect");
    refused(&input(root, Some(Actor::new(alice(), reg())), true, vec![], vec![0xFE]), "effect must decode");
    // Trailing bytes after a valid instruction.
    let mut b = msg(&Instruction::Freeze { entry_id: [1; 32], now: 7, via_pda_seed: None });
    b.push(0);
    refused(&input(Actor::new(alice(), reg()), None, true, vec![], b), "instruction must decode");
}

#[test]
fn head_record_is_42_bytes() {
    let h = t::Head { schema: 1, count: 3, newest: Some(t::AccountId([1; 32])) };
    assert_eq!(borsh::to_vec(&h).unwrap().len(), 42);
}

// ---- register_deployer: auth matrix ----

#[test]
fn deployer_registers_with_author_x() {
    let ix = register_deployer(&x(), "token", None);
    let r = run(&input(Actor::new(x(), reg()), None, true, vec![], msg(&ix))).unwrap();
    let e: t::Entry = borsh::from_slice(r.post_state.as_deref().unwrap()).unwrap();
    assert_eq!((e.kind, e.flags, e.revision), (t::KIND_DEPLOYER, 0, 1));
    assert_eq!((e.program, e.author, e.registrant), (tid(&x()), tid(&x()), tid(&x())));
    assert_eq!((e.registered_at, e.updated_at), (NOW, NOW));
    assert_eq!(r.sync_messages.len(), 2, "release + index, no probe when author == X");
    let rel = &r.sync_messages[0];
    assert_eq!(rel.to, Actor::new(lid(&addr::release(&tid(&reg()), &tid(&x()), 1)), reg()));
    assert_eq!(rel.pda_seeds.len(), 1);
    match decode_effect(&rel.message) {
        Effect::WriteRelease { release } => {
            assert_eq!((release.revision, release.entry), (1, tid(&x())));
            assert_eq!(release.build_hash, t::pda::build_hash(&build()));
        }
        other => panic!("{other:?}"),
    }
    let idx = &r.sync_messages[1];
    assert_eq!(idx.to, Actor::new(lid(&addr::global_head(&tid(&reg()))), reg()));
    assert_eq!(*idx.pda_seeds.first().unwrap(), PdaSeed::new(t::pda::seed::global_head()));
    assert_eq!(r.validity.timestamps.start(), Some(NOW));
    assert_eq!(r.validity.timestamps.end(), Some(NOW + t::TIME_WINDOW_MS));
}

#[test]
fn deployer_with_separate_author_probes_the_author() {
    let ix = register_deployer(&alice(), "token", None);
    let r = run(&input(Actor::new(x(), reg()), None, true, vec![], msg(&ix))).unwrap();
    let e: t::Entry = borsh::from_slice(r.post_state.as_deref().unwrap()).unwrap();
    assert_eq!(e.author, tid(&alice()));
    let probe = r.sync_messages.last().unwrap();
    assert_eq!(probe.to, Actor::new(alice(), reg()));
    assert_eq!(probe.message, vec![0xFF]);
    assert!(probe.pda_seeds.is_empty());
}

#[test]
fn unauthorized_deployer_entry_is_refused() {
    let ix = msg(&register_deployer(&x(), "token", None));
    refused(&input(Actor::new(x(), reg()), None, false, vec![], ix), "must sign");
}

#[test]
fn second_registration_is_refused() {
    let ix = msg(&register_deployer(&x(), "token", None));
    let pre = borsh::to_vec(&stored_entry(&x(), 0)).unwrap();
    refused(&input(Actor::new(x(), reg()), None, true, pre, ix), "already exists");
}

#[test]
fn root_only_rule() {
    let ix = msg(&register_deployer(&x(), "token", None));
    // A program in a composed transaction X signed: authorized, but not the root message.
    let attacker = Actor::new(alice(), other_program());
    refused(&input(Actor::new(x(), reg()), Some(attacker), true, vec![], ix.clone()), "root message");
    // Another registry actor relaying an instruction (not an effect) is refused the same way.
    refused(&input(Actor::new(x(), reg()), Some(Actor::new(bob(), reg())), true, vec![], ix), "root message");
    // A root message may not claim the PDA path.
    let s = [7; 32];
    let ix = msg(&register_deployer(&x(), "token", Some(s)));
    refused(&input(Actor::new(x(), reg()), None, true, vec![], ix), "via_pda_seed is only for");
}

#[test]
fn pda_header_rule() {
    let owner = other_program();
    let s = [7; 32];
    let header = AccountId::for_public_pda(&owner, &PdaSeed::new(s));
    let sender = Actor::new(alice(), owner);
    let ok = msg(&register_deployer(&header, "token", Some(s)));
    let r = run(&input(Actor::new(header, reg()), Some(sender), true, vec![], ok.clone())).unwrap();
    assert!(r.post_state.is_some());
    // Wrong seed, wrong owner, or no grant.
    let wrong = msg(&register_deployer(&header, "token", Some([8; 32])));
    refused(&input(Actor::new(header, reg()), Some(sender), true, vec![], wrong), "does not derive");
    let intermediary = Actor::new(alice(), AccountId::new([0xCC; 32]));
    refused(&input(Actor::new(header, reg()), Some(intermediary), true, vec![], ok.clone()), "does not derive");
    refused(&input(Actor::new(header, reg()), Some(sender), false, vec![], ok), "must authorize");
}

#[test]
fn registration_limits() {
    let root = Actor::new(x(), reg());
    let mut ix = register_deployer(&x(), "token", None);
    if let Instruction::RegisterDeployer { revision, revision_seed, .. } = &mut ix {
        *revision = 2;
        *revision_seed = t::pda::revision_seed(2);
    }
    refused(&input(root, None, true, vec![], msg(&ix)), "revision must be 1");
    let mut ix = register_deployer(&x(), "token", None);
    if let Instruction::RegisterDeployer { revision_seed, .. } = &mut ix {
        *revision_seed = [0; 32];
    }
    refused(&input(root, None, true, vec![], msg(&ix)), "revision_seed");
    refused(&input(root, None, true, vec![], msg(&register_deployer(&x(), "toKen", None))), "name may only contain");
    refused(&input(root, None, true, vec![], msg(&register_deployer(&x(), "-token", None))), "name must start with");
    refused(&input(root, None, true, vec![], msg(&register_deployer(&x(), &"a".repeat(33), None))), "exceeds 32 bytes");
    let mut ix = register_deployer(&x(), "token", None);
    if let Instruction::RegisterDeployer { tags, .. } = &mut ix {
        *tags = vec!["UPPER".into()];
    }
    refused(&input(root, None, true, vec![], msg(&ix)), "tag may only contain");
}

#[test]
fn deploy_and_register_sends_create_header_first() {
    let ix = Instruction::DeployAndRegister {
        first_segment: [0x44; 32],
        immutable: true,
        name: "token".into(),
        version: "1.0.0".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: content(),
        source: source(),
        build: build(),
        image_id: [9; 32],
        author_id: *x().value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: NOW,
    };
    let r = run(&input(Actor::new(x(), reg()), None, true, vec![], msg(&ix))).unwrap();
    let e: t::Entry = borsh::from_slice(r.post_state.as_deref().unwrap()).unwrap();
    assert!(e.born_with_deploy());
    let first = &r.sync_messages[0];
    assert_eq!(first.to, Actor::new(x(), PROGRAM_LOADER_ACCOUNT_ID));
    assert!(first.pda_seeds.is_empty());
    assert_eq!(
        LoaderMessage::try_from_slice(&first.message).unwrap(),
        LoaderMessage::CreateHeader { first_segment: AccountId::new([0x44; 32]), immutable: true }
    );
    assert_eq!(first.message[0], 1, "CreateHeader is the loader's variant 1");
    // Root-only, with no PDA path at all.
    refused(&input(Actor::new(x(), reg()), Some(Actor::new(alice(), other_program())), true, vec![], msg(&ix)), "root message");
    let declared = actors_to_declare(x(), reg(), &ix);
    assert!(declared.contains(&Actor::new(x(), PROGRAM_LOADER_ACCOUNT_ID)));
}

// ---- effects and probes ----

fn one_of_each_effect() -> Vec<Effect> {
    let e = stored_entry(&alice(), 0);
    vec![
        Effect::WriteEntry { entry: e.clone() },
        Effect::PatchEntry { patch: Patch::default(), now: NOW },
        Effect::WriteRelease {
            release: t::Release {
                schema: 1,
                entry: tid(&x()),
                revision: 1,
                version: "1".into(),
                idl: content(),
                source: source(),
                build: build(),
                image_id: [0; 32],
                build_hash: [0; 32],
                at: NOW,
            },
        },
        Effect::Index { ctx: ctx(&x(), 0) },
        Effect::IndexName { ctx: ctx(&x(), 0) },
        Effect::IndexAuthor { ctx: ctx(&x(), 0) },
        Effect::IndexProgram { ctx: ctx(&x(), 1) },
        Effect::WriteLink { ctx: ctx(&x(), 0) },
        Effect::WriteAuthorNode { new_author: tid(&bob()), node: t::AuthorNode { schema: 1, entry: tid(&x()), prev: None } },
        Effect::IndexAuthorTransfer { entry: tid(&x()), new_author: tid(&bob()) },
    ]
}

#[test]
fn every_effect_is_refused_from_outside_the_program() {
    let effects = one_of_each_effect();
    assert_eq!(effects.len(), lezreg_program::EFFECT_NAMES.len());
    for e in effects {
        let m = emsg(&e);
        let at = Actor::new(x(), reg());
        refused(&input(at, None, true, vec![], m.clone()), "only this program may send an effect");
        refused(&input(at, Some(Actor::new(alice(), other_program())), true, vec![], m.clone()), "only this program may send an effect");
        refused(&input(at, Some(Actor::new(reg(), other_program())), true, vec![], m), "only this program may send an effect");
    }
}

#[test]
fn an_unauthorized_registry_actor_emits_nothing() {
    // Effects come only out of instruction handlers, after the root guard and the signer
    // check: an unauthorized root refuses before sending any effect.
    for ix in [
        register_deployer(&x(), "token", None),
        update(&x(), 2),
        Instruction::Freeze { entry_id: *x().value(), now: NOW, via_pda_seed: None },
    ] {
        refused(&input(Actor::new(x(), reg()), None, false, borsh::to_vec(&stored_entry(&x(), 0)).unwrap(), msg(&ix)), "must sign");
    }
    refused(&input(Actor::new(alice(), reg()), None, false, vec![], msg(&register_third_party(&x()))), "must sign");
}

#[test]
fn require_signer_probe() {
    let at = Actor::new(alice(), reg());
    let ok = run(&input(at, Some(Actor::new(x(), reg())), true, vec![1, 2], vec![0xFF])).unwrap();
    assert!(ok.post_state.is_none() && ok.sync_messages.is_empty());
    refused(&input(at, Some(Actor::new(x(), reg())), false, vec![], vec![0xFF]), "co-signer did not sign");
    refused(&input(at, Some(Actor::new(x(), other_program())), true, vec![], vec![0xFF]), "only this program may probe");
    refused(&input(at, None, true, vec![], vec![0xFF]), "only this program may probe");
    refused(&input(at, Some(Actor::new(x(), reg())), true, vec![], vec![0xFF, 0]), "malformed signer probe");
}

#[test]
fn write_once_effects_refuse_existing_state() {
    // The write-once set comes from the macro (`INIT_EFFECTS`), plus `write_author_node`, which
    // checks `pre_state` itself to name the A→B→C→B refusal.
    assert_eq!(lezreg_program::INIT_EFFECTS, &["write_entry", "write_release", "write_link"]);
    let from = Some(Actor::new(x(), reg()));
    let mut checked = 0;
    for (e, name) in one_of_each_effect().into_iter().zip(lezreg_program::EFFECT_NAMES) {
        if lezreg_program::INIT_EFFECTS.contains(name) || *name == "write_author_node" {
            refused(&input(Actor::new(bob(), reg()), from, true, vec![1], emsg(&e)), "write-once");
            checked += 1;
        }
    }
    assert_eq!(checked, 4);
    // The author-node refusal is the named "transfer back" error.
    let node = Effect::WriteAuthorNode { new_author: tid(&bob()), node: t::AuthorNode { schema: 1, entry: tid(&x()), prev: None } };
    refused(&input(Actor::new(bob(), reg()), from, true, vec![1], emsg(&node)), "a transfer back to a previous author is refused");
}

// ---- index chain ----

#[test]
fn index_chain_order_contents_and_seeds() {
    let r = reg();
    let treg = tid(&r);
    let from = Some(Actor::new(x(), r));
    let head = Actor::new(lid(&addr::global_head(&treg)), r);
    let prev = t::AccountId([0x77; 32]);
    let pre = borsh::to_vec(&t::Head { schema: 1, count: 5, newest: Some(prev) }).unwrap();
    let out = run(&input(head, from, false, pre, emsg(&Effect::Index { ctx: ctx(&x(), 0) }))).unwrap();
    let h: t::Head = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!((h.count, h.newest), (6, Some(tid(&x()))));
    let next = &out.sync_messages[0];
    assert_eq!(next.to, Actor::new(lid(&addr::name_head(&treg, "token")), r));
    assert_eq!(*next.pda_seeds.first().unwrap(), PdaSeed::new(t::pda::seed::name_head("token")));
    let Effect::IndexName { ctx: c } = decode_effect(&next.message) else { panic!() };
    assert_eq!((c.seq, c.prev_global), (5, Some(prev)));

    // Wrong address.
    refused(&input(Actor::new(bob(), r), from, false, vec![], emsg(&Effect::Index { ctx: ctx(&x(), 0) })), "index: delivered to the wrong address");

    // Name head (empty) → author head.
    let nh = next.to;
    let out = run(&input(nh, from, false, vec![], next.message.clone())).unwrap();
    let next = &out.sync_messages[0];
    assert_eq!(next.to, Actor::new(lid(&addr::author_head(&treg, &tid(&alice()))), r));
    let Effect::IndexAuthor { ctx: c } = decode_effect(&next.message) else { panic!() };
    assert_eq!(c.prev_name, None);

    // Author head → link for a deployer entry.
    let out = run(&input(next.to, from, false, vec![], next.message.clone())).unwrap();
    let next = &out.sync_messages[0];
    assert_eq!(next.to, Actor::new(lid(&addr::link(&treg, &tid(&x()))), r));

    // Link: written once, emits Registered.
    let out = run(&input(next.to, from, false, vec![], next.message.clone())).unwrap();
    let l: t::Link = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!((l.seq, l.prev_global, l.author), (5, Some(prev), tid(&alice())));
    assert_eq!(out.events.len(), 1);
    assert_eq!(out.events[0].selector, t::events::selector(t::events::REGISTERED));
    let ev: t::events::Registered = borsh::from_slice(&out.events[0].data).unwrap();
    assert_eq!((ev.seq, ev.entry), (5, tid(&x())));
}

#[test]
fn third_party_index_goes_through_the_program_head() {
    let r = reg();
    let treg = tid(&r);
    let from = Some(Actor::new(x(), r));
    let entry = AccountId::new([0x55; 32]);
    let ah = Actor::new(lid(&addr::author_head(&treg, &tid(&alice()))), r);
    let out = run(&input(ah, from, false, vec![], emsg(&Effect::IndexAuthor { ctx: ctx(&entry, 1) }))).unwrap();
    let next = &out.sync_messages[0];
    assert_eq!(next.to, Actor::new(lid(&addr::program_head(&treg, &tid(&x()))), r));
    let out = run(&input(next.to, from, false, vec![], next.message.clone())).unwrap();
    let next = &out.sync_messages[0];
    assert_eq!(next.to, Actor::new(lid(&addr::link(&treg, &tid(&entry))), r));
    // A deployer ctx at a program head is refused.
    refused(&input(Actor::new(lid(&addr::program_head(&treg, &tid(&x()))), r), from, false, vec![], emsg(&Effect::IndexProgram { ctx: ctx(&entry, 0) })), "deployer entries are not listed");
}

// ---- update, transfer, freeze ----

fn patch_at(entry: AccountId, by: AccountId, pre: &t::Entry, p: Patch, authorized: bool) -> Result<Response, String> {
    run(&input(Actor::new(entry, reg()), Some(Actor::new(by, reg())), authorized, borsh::to_vec(pre).unwrap(), emsg(&Effect::PatchEntry { patch: p, now: NOW + 1 })))
}

#[test]
fn patch_rules() {
    let e = stored_entry(&alice(), 0);
    let upd = Patch { revision: Some(2), version: Some("2.0.0".into()), image_id: Some([8; 32]), ..Patch::default() };
    // Non-author.
    let err = patch_at(x(), bob(), &e, upd.clone(), false).unwrap_err();
    assert!(err.contains("only the entry's author"), "{err}");
    // Stale and skipped revisions.
    for bad in [1, 3] {
        let p = Patch { revision: Some(bad), ..upd.clone() };
        assert!(patch_at(x(), alice(), &e, p, false).unwrap_err().contains("stale revision"));
    }
    // Content without a revision.
    let p = Patch { version: Some("x".into()), ..Patch::default() };
    assert!(patch_at(x(), alice(), &e, p, false).unwrap_err().contains("need a revision"));
    // Frozen.
    let frozen = stored_entry(&alice(), t::FLAG_FROZEN);
    assert!(patch_at(x(), alice(), &frozen, upd.clone(), false).unwrap_err().contains("frozen"));
    // No entry.
    let r = run(&input(Actor::new(x(), reg()), Some(Actor::new(alice(), reg())), false, vec![], emsg(&Effect::PatchEntry { patch: upd.clone(), now: NOW }))).unwrap_err();
    assert!(r.contains("no registry entry"), "{r}");
    // Good update.
    let out = patch_at(x(), alice(), &e, upd, false).unwrap();
    let n: t::Entry = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!((n.revision, n.version.as_str(), n.image_id, n.updated_at), (2, "2.0.0", [8; 32], NOW + 1));
    assert_eq!(n.registered_at, NOW);
    assert_eq!(out.sync_messages[0].to, Actor::new(lid(&addr::release(&tid(&reg()), &tid(&x()), 2)), reg()));
    assert_eq!(out.events[0].selector, t::events::selector(t::events::UPDATED));
}

#[test]
fn transfer_rules() {
    let e = stored_entry(&alice(), 0);
    let to = |a: &AccountId| Patch { author: Some(tid(a)), ..Patch::default() };
    let out = patch_at(x(), alice(), &e, to(&bob()), false).unwrap();
    let n: t::Entry = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!((n.author, n.registrant), (tid(&bob()), tid(&alice())));
    // B → A (back to the registrant) is refused.
    assert!(patch_at(x(), bob(), &n, to(&alice()), false).unwrap_err().contains("back to a previous author"));
    // To the current author.
    assert!(patch_at(x(), bob(), &n, to(&bob()), false).unwrap_err().contains("already has this author"));
    // Handing a deployer entry back to X itself: X's delivery must be authorized.
    assert!(patch_at(x(), bob(), &n, to(&x()), false).unwrap_err().contains("new author must sign"));
    let back = patch_at(x(), bob(), &n, to(&x()), true).unwrap();
    let n2: t::Entry = borsh::from_slice(back.post_state.as_deref().unwrap()).unwrap();
    assert_eq!(n2.author, tid(&x()));
}

#[test]
fn update_inline_and_by_effect() {
    // Inline: the author's actor is the entry (deployer entry with author X).
    let pre = borsh::to_vec(&stored_entry(&x(), 0)).unwrap();
    let out = run(&input(Actor::new(x(), reg()), None, true, pre, msg(&update(&x(), 2)))).unwrap();
    let n: t::Entry = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!(n.revision, 2);
    assert_eq!(out.sync_messages.len(), 1, "only the release");
    // By effect: author alice, entry X. The root keeps its own state.
    let out = run(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&update(&x(), 2)))).unwrap();
    assert!(out.post_state.is_none());
    assert_eq!(out.sync_messages.len(), 1);
    assert_eq!(out.sync_messages[0].to, Actor::new(x(), reg()));
    assert!(out.sync_messages[0].pda_seeds.is_empty());
    assert!(matches!(decode_effect(&out.sync_messages[0].message), Effect::PatchEntry { .. }));
    // Revision 1 and a bad seed are refused at the root.
    refused(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&update(&x(), 1))), "at least 2");
    let mut u = update(&x(), 2);
    if let Instruction::Update { revision_seed, .. } = &mut u {
        *revision_seed = [1; 32];
    }
    refused(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&u)), "revision_seed");
    // Inline by an X that is not the author.
    let pre = borsh::to_vec(&stored_entry(&alice(), 0)).unwrap();
    refused(&input(Actor::new(x(), reg()), None, true, pre, msg(&update(&x(), 2))), "only the entry's author");
}

#[test]
fn transfer_probes() {
    let tr = |new: &AccountId| Instruction::TransferAuthor { entry_id: *x().value(), new_author_id: *new.value(), now: NOW, via_pda_seed: None };
    // Alice → Bob on entry X: PatchEntry to X, index to Bob's head, probe to Bob.
    let out = run(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&tr(&bob())))).unwrap();
    let to: Vec<Actor> = out.sync_messages.iter().map(|m| m.to).collect();
    assert_eq!(to, vec![
        Actor::new(x(), reg()),
        Actor::new(lid(&addr::author_head(&tid(&reg()), &tid(&bob()))), reg()),
        Actor::new(bob(), reg()),
    ]);
    assert_eq!(out.sync_messages[2].message, vec![0xFF]);
    // Alice → X (back to the header account): no probe, X's PatchEntry delivery checks it.
    let out = run(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&tr(&x())))).unwrap();
    assert_eq!(out.sync_messages.len(), 2);
    assert!(out.sync_messages.iter().all(|m| m.message != vec![0xFF]));
}

#[test]
fn freeze_inline() {
    let pre = borsh::to_vec(&stored_entry(&x(), 0)).unwrap();
    let f = Instruction::Freeze { entry_id: *x().value(), now: NOW, via_pda_seed: None };
    let out = run(&input(Actor::new(x(), reg()), None, true, pre, msg(&f))).unwrap();
    let n: t::Entry = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert!(n.is_frozen());
    assert_eq!(out.events[0].selector, t::events::selector(t::events::FROZEN));
}

// ---- third party ----

#[test]
fn third_party_rules() {
    let out = run(&input(Actor::new(alice(), reg()), None, true, vec![], msg(&register_third_party(&x())))).unwrap();
    assert!(out.post_state.is_none(), "the publisher's own actor is never written");
    let tp = lid(&addr::third_party_entry(&tid(&reg()), &tid(&x()), &tid(&alice())));
    assert_eq!(out.sync_messages[0].to, Actor::new(tp, reg()));
    let Effect::WriteEntry { entry } = decode_effect(&out.sync_messages[0].message) else { panic!() };
    assert_eq!((entry.kind, entry.author, entry.registrant), (t::KIND_THIRD_PARTY, tid(&alice()), tid(&alice())));
    // WriteEntry checks the address against the sender.
    let ok = run(&input(Actor::new(tp, reg()), Some(Actor::new(alice(), reg())), false, vec![], out.sync_messages[0].message.clone()));
    assert!(ok.is_ok());
    refused(&input(Actor::new(tp, reg()), Some(Actor::new(bob(), reg())), false, vec![], out.sync_messages[0].message.clone()), "malformed third-party entry");
    // Not for one's own account.
    refused(&input(Actor::new(x(), reg()), None, true, vec![], msg(&register_third_party(&x()))), "publisher's own account");
}

// ---- addresses: shim (risc0 sha) == lezreg-types (sha2) ----

#[test]
fn declared_actors_match_types_addresses() {
    let treg = tid(&reg());
    let a = |id: t::AccountId| Actor::new(lid(&id), reg());
    let d = actors_to_declare(x(), reg(), &register_deployer(&alice(), "token", None));
    assert_eq!(d, vec![
        Actor::new(x(), reg()),
        Actor::new(alice(), reg()),
        a(addr::release(&treg, &tid(&x()), 1)),
        a(addr::global_head(&treg)),
        a(addr::name_head(&treg, "token")),
        a(addr::author_head(&treg, &tid(&alice()))),
        a(addr::link(&treg, &tid(&x()))),
    ]);
    // author == X is declared once.
    assert_eq!(actors_to_declare(x(), reg(), &register_deployer(&x(), "token", None)).len(), 6);
    let tp = addr::third_party_entry(&treg, &tid(&x()), &tid(&alice()));
    let d = actors_to_declare(alice(), reg(), &register_third_party(&x()));
    assert_eq!(d, vec![
        Actor::new(alice(), reg()),
        a(tp),
        a(addr::release(&treg, &tp, 1)),
        a(addr::global_head(&treg)),
        a(addr::name_head(&treg, "community")),
        a(addr::author_head(&treg, &tid(&alice()))),
        a(addr::program_head(&treg, &tid(&x()))),
        a(addr::link(&treg, &tp)),
    ]);
    let d = actors_to_declare(alice(), reg(), &Instruction::TransferAuthor { entry_id: *x().value(), new_author_id: *bob().value(), now: NOW, via_pda_seed: None });
    assert_eq!(d, vec![
        Actor::new(alice(), reg()),
        Actor::new(x(), reg()),
        Actor::new(bob(), reg()),
        a(addr::author_head(&treg, &tid(&bob()))),
        a(addr::author_node(&treg, &tid(&bob()), &tid(&x()))),
    ]);
    let d = actors_to_declare(alice(), reg(), &update(&x(), 3));
    assert_eq!(d[2], a(addr::release(&treg, &tid(&x()), 3)));
}

// ---- review fixes (M1 review) ----

fn entry_for(program: &AccountId, author: &AccountId) -> t::Entry {
    t::Entry { program: tid(program), ..stored_entry(author, 0) }
}

fn with_via(mut ix: Instruction, via: Option<[u8; 32]>) -> Instruction {
    match &mut ix {
        Instruction::RegisterDeployer { via_pda_seed, .. }
        | Instruction::Update { via_pda_seed, .. }
        | Instruction::TransferAuthor { via_pda_seed, .. }
        | Instruction::Freeze { via_pda_seed, .. } => *via_pda_seed = via,
        _ => panic!("{} has no via_pda_seed", ix.name()),
    }
    ix
}

fn deploy_and_register(author: &AccountId) -> Instruction {
    Instruction::DeployAndRegister {
        first_segment: [0x44; 32],
        immutable: false,
        name: "token".into(),
        version: "1.0.0".into(),
        author_label: String::new(),
        description: String::new(),
        tags: vec![],
        idl: content(),
        source: source(),
        build: build(),
        image_id: [9; 32],
        author_id: *author.value(),
        revision: 1,
        revision_seed: t::pda::revision_seed(1),
        now: NOW,
    }
}

/// §8.1 auth matrix: every instruction × `from ∈ {None, Some(own), Some(other)}` ×
/// `is_authorized`, and for the four with `via_pda_seed`, the PDA path too.
#[test]
fn auth_matrix_every_instruction() {
    let stored = borsh::to_vec(&stored_entry(&x(), 0)).unwrap();
    let cases: Vec<(AccountId, Vec<u8>, Instruction)> = vec![
        (x(), vec![], register_deployer(&x(), "token", None)),
        (alice(), vec![], register_third_party(&x())),
        (x(), stored.clone(), update(&x(), 2)),
        (x(), stored.clone(), Instruction::TransferAuthor { entry_id: *x().value(), new_author_id: *bob().value(), now: NOW, via_pda_seed: None }),
        (x(), stored.clone(), Instruction::Freeze { entry_id: *x().value(), now: NOW, via_pda_seed: None }),
        (x(), vec![], deploy_and_register(&x())),
    ];
    assert_eq!(cases.len(), INSTRUCTION_NAMES.len());
    let own = Actor::new(bob(), reg());
    let other = Actor::new(alice(), other_program());
    let s = [7; 32];
    for (root, pre, ix) in cases {
        let at = Actor::new(root, reg());
        let n = ix.name();
        let m = msg(&ix);
        run(&input(at, None, true, pre.clone(), m.clone())).unwrap_or_else(|e| panic!("{n}: a signed root message: {e}"));
        refused(&input(at, None, false, pre.clone(), m.clone()), "must sign");
        for from in [own, other] {
            for auth in [true, false] {
                refused(&input(at, Some(from), auth, pre.clone(), m.clone()), "root message");
            }
        }
        let has_via = matches!(ix, Instruction::RegisterDeployer { .. } | Instruction::Update { .. } | Instruction::TransferAuthor { .. } | Instruction::Freeze { .. });
        if !has_via {
            continue;
        }
        // The PDA path on an account that is not a PDA of the sender's program.
        let v = msg(&with_via(ix.clone(), Some(s)));
        refused(&input(at, None, true, pre.clone(), v.clone()), "via_pda_seed is only for");
        for from in [own, other] {
            refused(&input(at, Some(from), true, pre.clone(), v.clone()), "does not derive");
            refused(&input(at, Some(from), false, pre.clone(), v.clone()), "must authorize");
        }
    }
}

/// Security review F1/F2: on the PDA-header path a co-signer's `is_authorized` only proves it
/// signed *a* transaction that reached the owner program, so every signer must be the root.
#[test]
fn pda_path_signers_must_be_the_root() {
    let owner = other_program();
    let s = [7; 32];
    let h = AccountId::for_public_pda(&owner, &PdaSeed::new(s));
    let at = Actor::new(h, reg());
    let sender = Some(Actor::new(alice(), owner));
    let rule = "on the PDA-header path every signer must be the root";
    // Registration naming any other author (alice signed the tx for the owner program).
    let ix = msg(&register_deployer(&alice(), "token", Some(s)));
    refused(&input(at, sender, true, vec![], ix), rule);
    // With author == the header: accepted, and no probe.
    let r = run(&input(at, sender, true, vec![], msg(&register_deployer(&h, "token", Some(s))))).unwrap();
    assert!(r.sync_messages.iter().all(|m| m.message != vec![0xFF]));
    // Update and freeze of the header's entry (inline) work through the owner program.
    let pre = borsh::to_vec(&entry_for(&h, &h)).unwrap();
    let up = with_via(update(&h, 2), Some(s));
    let out = run(&input(at, sender, true, pre.clone(), msg(&up))).unwrap();
    let n: t::Entry = borsh::from_slice(out.post_state.as_deref().unwrap()).unwrap();
    assert_eq!(n.revision, 2);
    let fr = Instruction::Freeze { entry_id: *h.value(), now: NOW, via_pda_seed: Some(s) };
    assert!(run(&input(at, sender, true, pre.clone(), msg(&fr))).is_ok());
    // A transfer away from the header on the PDA path names a non-root signer: refused (A1b).
    let tr = |to: &AccountId| Instruction::TransferAuthor { entry_id: *h.value(), new_author_id: *to.value(), now: NOW, via_pda_seed: Some(s) };
    refused(&input(at, sender, true, pre.clone(), msg(&tr(&alice()))), rule);
    refused(&input(at, sender, true, pre, msg(&tr(&bob()))), rule);
    // The same transfer as a root message, signed by both, is unaffected.
    let pre = borsh::to_vec(&stored_entry(&x(), 0)).unwrap();
    let tr = Instruction::TransferAuthor { entry_id: *x().value(), new_author_id: *bob().value(), now: NOW, via_pda_seed: None };
    assert!(run(&input(Actor::new(x(), reg()), None, true, pre, msg(&tr))).is_ok());
}

/// Security review F3: `PatchEntry` checks the record is an entry living at this address.
#[test]
fn patch_checks_the_entry_address() {
    let upd = Patch { revision: Some(2), version: Some("2.0.0".into()), ..Patch::default() };
    // A deployer entry for Y stored at X.
    let y = AccountId::new([0x44; 32]);
    let err = patch_at(x(), alice(), &entry_for(&y, &alice()), upd.clone(), false).unwrap_err();
    assert!(err.contains("not a registry entry at this address"), "{err}");
    // A third-party entry at the wrong PDA, and at the right one.
    let mut tp = stored_entry(&alice(), 0);
    tp.kind = t::KIND_THIRD_PARTY;
    let tp_at = lid(&addr::third_party_entry(&tid(&reg()), &tid(&x()), &tid(&alice())));
    assert!(patch_at(x(), alice(), &tp, upd.clone(), false).unwrap_err().contains("not a registry entry"));
    let out = patch_at(tp_at, alice(), &tp, upd.clone(), false).unwrap();
    assert_eq!(out.sync_messages[0].to, Actor::new(lid(&addr::release(&tid(&reg()), &tid(&tp_at), 2)), reg()));
    // After a transfer the registrant still keys the address.
    tp.author = tid(&bob());
    assert!(patch_at(tp_at, bob(), &tp, upd.clone(), false).is_ok());
    // Unknown kind or schema.
    let mut bad = stored_entry(&alice(), 0);
    bad.kind = 7;
    assert!(patch_at(x(), alice(), &bad, upd.clone(), false).unwrap_err().contains("not a registry entry"));
    let mut bad = stored_entry(&alice(), 0);
    bad.schema = 2;
    assert!(patch_at(x(), alice(), &bad, upd, false).unwrap_err().contains("not a registry entry"));
}

/// §8.1 / §2.3: borsh sizes of typical and worst-case records (the §2.3 size table).
#[test]
fn record_sizes_match_the_design_table() {
    let typical_content = ContentRef { cid: vec![1; 38], sha256: [2; 32], size: 1000, peer_hint: None };
    let max_content = ContentRef { cid: vec![1; 64], sha256: [2; 32], size: 1000, peer_hint: Some(vec![3; 48]) };
    let typical_source = SourceRef { repo: "https://github.com/example-org/example-program.git".into(), commit: vec![3; 20], docker_root: ".".into(), archive: Some(typical_content.clone()) };
    let max_source = SourceRef { repo: "r".repeat(160), commit: vec![3; 32], docker_root: "d".repeat(96), archive: Some(max_content.clone()) };
    let max_build = BuildConfig {
        image_tag: "i".repeat(48),
        risc0_build: "r".repeat(16),
        host_risc0_rust: "h".repeat(16),
        manifest_path: "m".repeat(96),
        binary: "b".repeat(48),
        features: vec!["f".repeat(32); 4],
        docker_env: vec![t::EnvVar { key: "K".repeat(16), value: "v".repeat(48) }; 4],
        ..build()
    };
    let typical = t::Entry {
        name: "token-program".into(),
        version: "1.2.3".into(),
        author_label: "Example Labs".into(),
        description: "A fungible token program with mint, burn and transfer, deployed on LEZ.".into(),
        tags: vec!["token".into(), "defi".into()],
        idl: typical_content.clone(),
        source: typical_source.clone(),
        ..stored_entry(&x(), 0)
    };
    let worst = t::Entry {
        name: "a".repeat(32),
        version: "v".repeat(32),
        author_label: "é".repeat(32),
        description: "d".repeat(512),
        tags: vec!["t".repeat(24); 8],
        idl: max_content.clone(),
        source: max_source.clone(),
        build: max_build.clone(),
        ..stored_entry(&x(), 0)
    };
    let rel = |e: &t::Entry| t::Release {
        schema: 1,
        entry: e.program,
        revision: 1,
        version: e.version.clone(),
        idl: e.idl.clone(),
        source: e.source.clone(),
        build: e.build.clone(),
        image_id: e.image_id,
        build_hash: [0; 32],
        at: NOW,
    };
    let link = t::Link {
        schema: 1,
        seq: 1,
        kind: 1,
        program: tid(&x()),
        name: "token-program".into(),
        author: tid(&alice()),
        prev_global: Some(tid(&x())),
        prev_name: Some(tid(&x())),
        prev_author: Some(tid(&x())),
        prev_program: Some(tid(&x())),
    };
    let len = |v: Vec<u8>| v.len();
    let sizes = [
        ("Entry typical", len(borsh::to_vec(&typical).unwrap())),
        ("Entry worst", len(borsh::to_vec(&worst).unwrap())),
        ("Release typical", len(borsh::to_vec(&rel(&typical)).unwrap())),
        ("Release worst", len(borsh::to_vec(&rel(&worst)).unwrap())),
        ("Link typical (13-char name, all prevs)", len(borsh::to_vec(&link).unwrap())),
        ("Head", len(borsh::to_vec(&t::Head { schema: 1, count: 1, newest: Some(tid(&x())) }).unwrap())),
        ("AuthorNode", len(borsh::to_vec(&t::AuthorNode { schema: 1, entry: tid(&x()), prev: Some(tid(&x())) }).unwrap())),
    ];
    for (n, b) in sizes {
        println!("{n}: {b} B");
    }
    // Pinned to the measured values in the DESIGN §2.3 table.
    assert_eq!(sizes.map(|(_, b)| b), [EXPECTED_SIZES[0], EXPECTED_SIZES[1], EXPECTED_SIZES[2], EXPECTED_SIZES[3], EXPECTED_SIZES[4], 42, 66]);
    // And the worst case fits the validator's limits (it is a valid registration).
    let ix = Instruction::RegisterDeployer {
        name: worst.name.clone(), version: worst.version.clone(), author_label: worst.author_label.clone(),
        description: worst.description.clone(), tags: worst.tags.clone(), idl: worst.idl.clone(), source: worst.source.clone(),
        build: worst.build.clone(), image_id: [9; 32], author_id: *x().value(), revision: 1, revision_seed: t::pda::revision_seed(1),
        now: NOW, via_pda_seed: None,
    };
    let r = run(&input(Actor::new(x(), reg()), None, true, vec![], msg(&ix))).unwrap();
    assert_eq!(r.post_state.unwrap().len(), EXPECTED_SIZES[1]);
}

/// Entry typical, Entry worst, Release typical, Release worst, Link: DESIGN §2.3.
const EXPECTED_SIZES: [usize; 5] = [674, 2400, 503, 1510, 223];
