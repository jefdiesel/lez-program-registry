//! Engine tests 1–13 (DESIGN §8.2; 12 and 13 come from the M1 review): the real registry guest
//! on `V03State`. Regression pins for the source facts in DESIGN §2.1. Run with `--release` (the risc0 executor is slow in debug).

use lee_core::{
    account::{AccountId, Actor},
    program::PROGRAM_LOADER_ACCOUNT_ID,
};
use lezreg_engine_test::*;
use lezreg_program::{Effect, Instruction, Message, Patch, actors_to_declare};
use lezreg_types::{self as t, pda::addr, walk};

const REG_SEED: u8 = 0x70;
const FWD_SEED: u8 = 0x71;

const FWD2_SEED: u8 = 0x72;

/// Registry, two forwarder programs, and existing accounts for alice (3), bob (4), mallory (5)
/// and carol (6). X-style header keys (1, 2, 8, 10..) start out nonexistent.
fn engine() -> Engine {
    let funded: Vec<AccountId> = (3..=6).map(|s| key(s).id).collect();
    Engine::new(key(REG_SEED).id, &[key(FWD_SEED).id, key(FWD2_SEED).id], &funded)
}

fn ids(list: &[(t::AccountId, t::Link)]) -> Vec<AccountId> {
    list.iter().map(|(e, _)| lid(e)).collect()
}

/// 1. A deployer registers, with author = X and with author ≠ X; then lookup, list, and the
/// name and author walks.
#[test]
fn t01_deployer_registers_and_is_discoverable() {
    let mut e = engine();
    let (x, y, alice) = (key(1), key(2), key(3));
    e.ok("X registers itself (author = X)", x.id, &register_deployer("token", &x.id, [7; 32], None), &[&x]);
    e.ok("Y registers with author alice (two signatures)", y.id, &register_deployer("token", &alice.id, [8; 32], None), &[&y, &alice]);

    // Lookup by address alone: (X, REG) is the deployer entry.
    let ex = e.entry(&x.id).expect("X's deployer entry");
    assert_eq!((ex.kind, ex.program, ex.author, ex.revision), (t::KIND_DEPLOYER, tid(&x.id), tid(&x.id), 1));
    assert!(!ex.born_with_deploy());
    let ey = e.entry(&y.id).expect("Y's deployer entry");
    assert_eq!((ey.author, ey.registrant), (tid(&alice.id), tid(&alice.id)));
    // Release #1 snapshot.
    let rel: t::Release = borsh::from_slice(&e.reg_state(&lid(&addr::release(&e.treg(), &tid(&y.id), 1)))).unwrap();
    assert_eq!((rel.revision, rel.image_id, rel.build_hash), (1, [8; 32], t::pda::build_hash(&build())));

    // List (newest first), name, author.
    let r = e.reader();
    let all = walk::global(&r, &e.treg(), 10).unwrap();
    assert_eq!(ids(&all), vec![y.id, x.id]);
    assert_eq!((all[0].1.seq, all[1].1.seq), (1, 0));
    assert_eq!(ids(&walk::by_name(&r, &e.treg(), "token", 10).unwrap()), vec![y.id, x.id]);
    assert!(walk::by_name(&r, &e.treg(), "other", 10).unwrap().is_empty());
    let by_alice = walk::by_author(&r, &e.treg(), &tid(&alice.id), 10).unwrap();
    assert_eq!(by_alice.iter().map(|(a, _)| lid(a)).collect::<Vec<_>>(), vec![y.id]);
    assert_eq!(walk::by_author(&r, &e.treg(), &tid(&x.id), 10).unwrap().len(), 1);
    let head = walk::head(&r, &addr::global_head(&e.treg())).unwrap();
    assert_eq!(head.count, 2);
    e.report("t01");
}

/// 2. Unauthorized deployer entry is refused (§3 case a); a second register is refused (d).
#[test]
fn t02_unauthorized_and_duplicate_deployer_entries_are_refused() {
    let mut e = engine();
    let (x, mallory) = (key(1), key(5));
    // X does not exist yet and does not sign: nothing admits (X, REG).
    e.refuse(
        "(a) Mallory registers a nonexistent X, signed only by her",
        "UnadmittedPublicActor",
        x.id,
        &register_deployer("token", &mallory.id, [6; 32], None),
        &[&mallory],
    );
    // X exists (say it holds a balance or a header): REG's own check refuses.
    e.fund(&[x.id]);
    e.refuse(
        "(a) Mallory registers an existing X, signed only by her",
        "must sign",
        x.id,
        &register_deployer("token", &mallory.id, [6; 32], None),
        &[&mallory],
    );
    assert!(e.entry(&x.id).is_none());
    e.ok("X registers", x.id, &register_deployer("token", &x.id, [7; 32], None), &[&x]);
    e.refuse("(d) a second register on X", "already exists", x.id, &register_deployer("token2", &x.id, [7; 32], None), &[&x]);
    // Mallory as author without her signature: the probe refuses.
    let z = key(6);
    e.refuse("author never signed", "co-signer did not sign", z.id, &register_deployer("token", &mallory.id, [6; 32], None), &[&z]);
    assert_eq!(e.entry(&x.id).unwrap().name, "token");
}

/// 3. Composed-transaction attack: a program reached in a transaction X signed relays
/// `RegisterDeployer` to (X, REG) (b); a variant sends `WriteEntry` / `PatchEntry` (c).
#[test]
fn t03_composed_transaction_attacks_are_refused() {
    let mut e = engine();
    let (x, alice, mallory) = (key(1), key(3), key(5));
    let fwd = key(FWD_SEED).id;
    let ix = register_deployer("token", &x.id, [6; 32], None);
    let mut actors = actors_to_declare(x.id, e.reg, &ix);
    let root = Actor::new(mallory.id, fwd);
    let sends = vec![Send::to(e.actor(x.id), borsh::to_vec(&ix).unwrap(), vec![])];
    let err = e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors.clone(), &[&mallory, &x], false, T0).unwrap_err();
    assert!(err.contains("root message"), "(b) {err}");
    println!("refused (b) a program relays RegisterDeployer in a tx X signed (root message)");
    assert!(e.entry(&x.id).is_none());

    // (c) Effects straight from a foreign program.
    e.ok("X registers with author alice", x.id, &register_deployer("token", &alice.id, [7; 32], None), &[&x, &alice]);
    let patch = Message::Effect(Effect::PatchEntry { patch: Patch { author: Some(tid(&mallory.id)), ..Patch::default() }, now: T0 });
    let mut entry = e.entry(&x.id).unwrap();
    entry.kind = t::KIND_THIRD_PARTY;
    entry.author = tid(&mallory.id);
    entry.registrant = entry.author;
    let write = Message::Effect(Effect::WriteEntry { entry });
    // Targets that exist, so admission is not what refuses them.
    for (what, to, m) in [("PatchEntry", e.actor(x.id), patch), ("WriteEntry", e.actor(alice.id), write)] {
        actors.push(to);
        let sends = vec![Send::to(to, borsh::to_vec(&m).unwrap(), vec![])];
        let err = e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors.clone(), &[&mallory, &x, &alice], false, T0).unwrap_err();
        assert!(err.contains("only this program may send an effect"), "(c) {what}: {err}");
        println!("refused (c) a foreign program sends {what} (only this program may send an effect)");
    }
    assert_eq!(e.entry(&x.id).unwrap().author, tid(&alice.id));
}

/// 4. A third party registers; a duplicate publisher is refused; the third-party list walk works.
#[test]
fn t04_third_party_entries() {
    let mut e = engine();
    let (x, alice, bob) = (key(1), key(3), key(4));
    e.ok("X registers", x.id, &register_deployer("token", &x.id, [7; 32], None), &[&x]);
    e.ok("alice publishes a third-party entry for X", alice.id, &register_third_party(&x.id, "token-community"), &[&alice]);
    e.refuse("alice again for X", "already exists (write-once)", alice.id, &register_third_party(&x.id, "token-community"), &[&alice]);
    e.ok("bob publishes one too", bob.id, &register_third_party(&x.id, "token-alt"), &[&bob]);
    e.refuse("a third-party entry for one's own account", "publisher's own account", x.id, &register_third_party(&x.id, "self"), &[&x]);

    let tp_a = lid(&addr::third_party_entry(&e.treg(), &tid(&x.id), &tid(&alice.id)));
    let ta = e.entry(&tp_a).unwrap();
    assert_eq!((ta.kind, ta.program, ta.author), (t::KIND_THIRD_PARTY, tid(&x.id), tid(&alice.id)));
    // A third-party entry never occupies the deployer address.
    assert_eq!(e.entry(&x.id).unwrap().kind, t::KIND_DEPLOYER);
    let r = e.reader();
    let tps = walk::third_party(&r, &e.treg(), &tid(&x.id), 10).unwrap();
    let tp_b = lid(&addr::third_party_entry(&e.treg(), &tid(&x.id), &tid(&bob.id)));
    assert_eq!(ids(&tps), vec![tp_b, tp_a]);
    assert_eq!(ids(&walk::global(&r, &e.treg(), 10).unwrap()), vec![tp_b, tp_a, x.id]);
    assert_eq!(walk::by_author(&r, &e.treg(), &tid(&alice.id), 10).unwrap().len(), 1);
    e.report("t04");
}

/// 5. Update by a non-author is refused (e); a stale revision is refused; a transfer without
/// the new author's signature is refused; freeze works; the inline-patch path works; transfers
/// back are refused and the author walk stays right after A→B→C.
#[test]
fn t05_update_transfer_freeze() {
    let mut e = engine();
    let (x, y, alice, bob, carol) = (key(1), key(2), key(3), key(4), key(6));
    e.ok("X registers with author alice", x.id, &register_deployer("token", &alice.id, [7; 32], None), &[&x, &alice]);
    e.refuse("(e) bob updates alice's entry", "only the entry's author", bob.id, &update(&x.id, 2, "2.0.0"), &[&bob]);
    e.refuse("X (header key) updates an entry whose author is alice", "only the entry's author", x.id, &update(&x.id, 2, "2.0.0"), &[&x]);
    e.ok("alice updates (PatchEntry path)", alice.id, &update(&x.id, 2, "2.0.0"), &[&alice]);
    e.refuse("stale revision", "stale revision", alice.id, &update(&x.id, 2, "2.0.1"), &[&alice]);
    e.refuse("skipped revision", "stale revision", alice.id, &update(&x.id, 4, "2.0.1"), &[&alice]);
    let ex = e.entry(&x.id).unwrap();
    assert_eq!((ex.revision, ex.version.as_str(), ex.description.as_str()), (2, "2.0.0", "revision 2"));
    let r2: t::Release = borsh::from_slice(&e.reg_state(&lid(&addr::release(&e.treg(), &tid(&x.id), 2)))).unwrap();
    let r1: t::Release = borsh::from_slice(&e.reg_state(&lid(&addr::release(&e.treg(), &tid(&x.id), 1)))).unwrap();
    assert_eq!((r1.version.as_str(), r2.version.as_str()), ("1.0.0", "2.0.0"), "both revisions stay rebuildable");

    // Transfers: both sign.
    e.refuse("transfer without bob's signature", "co-signer did not sign", alice.id, &transfer(&x.id, &bob.id), &[&alice]);
    e.ok("alice → bob (both sign)", alice.id, &transfer(&x.id, &bob.id), &[&alice, &bob]);
    e.refuse("bob → alice (back to the registrant)", "back to a previous author", bob.id, &transfer(&x.id, &alice.id), &[&bob, &alice]);
    e.ok("bob → carol", bob.id, &transfer(&x.id, &carol.id), &[&bob, &carol]);
    e.refuse("carol → bob (bob's list already has it)", "a transfer back to a previous author is refused", carol.id, &transfer(&x.id, &bob.id), &[&carol, &bob]);
    e.refuse("alice can no longer update", "only the entry's author", alice.id, &update(&x.id, 3, "3.0.0"), &[&alice]);
    e.ok("carol updates", carol.id, &update(&x.id, 3, "3.0.0"), &[&carol]);
    {
        let r = e.reader();
        assert!(walk::by_author(&r, &e.treg(), &tid(&alice.id), 10).unwrap().is_empty());
        assert!(walk::by_author(&r, &e.treg(), &tid(&bob.id), 10).unwrap().is_empty());
        assert_eq!(walk::by_author(&r, &e.treg(), &tid(&carol.id), 10).unwrap()[0].0, tid(&x.id));
        assert_eq!(walk::author_list(&r, &e.treg(), &tid(&alice.id), 10).unwrap(), vec![tid(&x.id)], "history keeps the registration");
        assert_eq!(walk::author_list(&r, &e.treg(), &tid(&bob.id), 10).unwrap(), vec![tid(&x.id)]);
    }

    // Hand it back to the header key X itself: X's PatchEntry delivery checks X signed.
    e.refuse("carol → X without X", "new author must sign", carol.id, &transfer(&x.id, &x.id), &[&carol]);
    e.ok("carol → X (X signs)", carol.id, &transfer(&x.id, &x.id), &[&carol, &x]);
    // Now the author is X: the inline path (root (X, REG) is the entry).
    e.ok("X updates inline", x.id, &update(&x.id, 4, "4.0.0"), &[&x]);
    assert_eq!(e.entry(&x.id).unwrap().revision, 4);

    // Freeze.
    e.ok("Y registers itself", y.id, &register_deployer("other", &y.id, [9; 32], None), &[&y]);
    e.ok("X freezes inline", x.id, &freeze(&x.id), &[&x]);
    assert!(e.entry(&x.id).unwrap().is_frozen());
    e.refuse("update after freeze", "frozen", x.id, &update(&x.id, 5, "5.0.0"), &[&x]);
    e.refuse("freeze again", "frozen", x.id, &freeze(&x.id), &[&x]);
    e.report("t05");
}

/// 6. Two registrations in one block, and two with the same name in one block: the lists stay
/// consistent.
#[test]
fn t06_same_block_registrations() {
    let mut e = engine();
    let ks: Vec<Key> = (10..14).map(key).collect();
    e.ok("a first entry", ks[0].id, &register_deployer("alpha", &ks[0].id, [1; 32], None), &[&ks[0]]);
    let block = e.block + 1;
    for (k, name) in [(&ks[1], "beta"), (&ks[2], "same"), (&ks[3], "same")] {
        let ix = register_deployer(name, &k.id, [2; 32], None);
        let actors = actors_to_declare(k.id, e.reg, &ix);
        let first = e.block < block;
        e.try_tx(e.actor(k.id), borsh::to_vec(&ix).unwrap(), actors, &[k], !first, T0).unwrap();
        assert_eq!(e.block, block, "same block");
    }
    let r = e.reader();
    let all = walk::global(&r, &e.treg(), 10).unwrap();
    assert_eq!(ids(&all), vec![ks[3].id, ks[2].id, ks[1].id, ks[0].id]);
    assert_eq!(all.iter().map(|(_, l)| l.seq).collect::<Vec<_>>(), vec![3, 2, 1, 0]);
    assert_eq!(ids(&walk::by_name(&r, &e.treg(), "same", 10).unwrap()), vec![ks[3].id, ks[2].id]);
    assert_eq!(walk::head(&r, &addr::name_head(&e.treg(), "same")).unwrap().count, 2);
    println!("ok      4 registrations, 3 in one block, 2 with the same name: lists consistent");
}

/// 7. Self-admission: no `PublicAccountEvidence` anywhere (the harness never sends any), and a
/// brand-new author account is admitted as a signer.
#[test]
fn t07_no_evidence_and_a_brand_new_author() {
    let mut e = engine();
    let (x, fresh) = (key(1), key(0x5A));
    assert!(e.state.get_account_by_id_ref(fresh.id).is_none(), "the author account has never existed");
    assert!(e.state.get_account_by_id_ref(x.id).is_none(), "nor has the header key");
    e.ok("X registers with a brand-new author, no evidence", x.id, &register_deployer("token", &fresh.id, [7; 32], None), &[&x, &fresh]);
    e.ok("the new author publishes a third-party entry", fresh.id, &register_third_party(&key(2).id, "fresh"), &[&fresh]);
    e.ok("the new author updates", fresh.id, &update(&x.id, 2, "2.0.0"), &[&fresh]);
    assert_eq!(e.entry(&x.id).unwrap().author, tid(&fresh.id));
}

/// 8. `DeployAndRegister` against the real loader.
#[test]
fn t08_deploy_and_register() {
    let mut e = engine();
    let (x, x2, alice) = (key(1), key(2), key(3));
    let target = forwarder_elf();
    let image = image_id_of(target);
    let first = e.write_segments(target, 0xA0);
    let ix = deploy_and_register("forwarder", &alice.id, first, false, image_bytes(&image));
    let declared = actors_to_declare(x.id, e.reg, &ix);
    assert!(declared.contains(&Actor::new(x.id, PROGRAM_LOADER_ACCOUNT_ID)));
    assert!(!declared.iter().any(|a| a.account_id == first), "segments are not declared");
    let a = e.ok("DeployAndRegister X (X and alice sign)", x.id, &ix, &[&x, &alice]);
    let h = e.header(&x.id).expect("a header at X");
    assert_eq!(h.image_id, image, "the header's image_id is the guest's");
    assert_eq!(h.program_first_segment, first);
    let ent = e.entry(&x.id).unwrap();
    assert!(ent.born_with_deploy());
    assert_eq!((ent.image_id, ent.author), (image_bytes(&image), tid(&alice.id)));
    assert_eq!(a.events.len(), 1, "Registered");
    // The new program runs.
    e.try_tx(Actor::new(key(7).id, x.id), borsh::to_vec(&Vec::<Send>::new()).unwrap(), vec![], &[&key(7)], false, T0)
        .expect("the deployed program dispatches");
    println!("ok      the program deployed by DeployAndRegister dispatches");

    e.refuse("DeployAndRegister again on X", "already exists", x.id, &deploy_and_register("forwarder", &x.id, first, false, [0; 32]), &[&x]);
    e.create_header(&x2, first, false).expect("a plain CreateHeader at X2");
    e.refuse(
        "DeployAndRegister on X2, header already deployed",
        "header target already deployed",
        x2.id,
        &deploy_and_register("forwarder", &x2.id, first, false, [0; 32]),
        &[&x2],
    );
    assert!(e.entry(&x2.id).is_none(), "the whole transaction reverted");
    // An author who did not sign: the probe refuses and the CreateHeader reverts with it.
    let x4 = key(9);
    e.refuse("DeployAndRegister on X4 naming alice, alice not signing", "co-signer did not sign", x4.id, &deploy_and_register("forwarder", &alice.id, first, false, [0; 32]), &[&x4]);
    assert!(e.header(&x4.id).is_none(), "no header: the whole transaction reverted");
    // Mallory: DeployAndRegister for a key she does not hold.
    let (x3, mallory) = (key(8), key(5));
    e.fund(&[x3.id]);
    e.refuse("Mallory DeployAndRegister on X3", "must sign", x3.id, &deploy_and_register("evil", &mallory.id, first, false, [0; 32]), &[&mallory]);
    assert!(e.header(&x3.id).is_none());
    e.report("t08");
}

/// 9. PDA-owned header through a helper guest: the owner program creates the header and
/// registers it with `via_pda_seed` (author = the header itself: on the PDA path every signer
/// must be the root, security review F1); a wrong seed or another program is refused; the owner
/// program updates and freezes it the same way; a root message may not claim the PDA path.
#[test]
fn t09_pda_owned_header() {
    let mut e = engine();
    let alice = key(3);
    let p = key(FWD_SEED).id;
    let s = [0x5E; 32];
    let x = pda(&p, s);
    let first = e.write_segments(forwarder_elf(), 0xB0);
    let loader = Actor::new(x, PROGRAM_LOADER_ACCOUNT_ID);
    let create = program_loader_core::Message::CreateHeader { first_segment: first, immutable: false };
    let ix = register_deployer("pda-program", &x, [1; 32], Some(s));
    let mut actors = actors_to_declare(x, e.reg, &ix);
    actors.push(loader);
    let root = Actor::new(alice.id, p);

    let bad = register_deployer("pda-program", &x, [1; 32], Some([0x5F; 32]));
    let sends = vec![Send::to(loader, borsh::to_vec(&create).unwrap(), vec![s]), Send::to(e.actor(x), borsh::to_vec(&bad).unwrap(), vec![s])];
    let err = e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors.clone(), &[&alice], false, T0).unwrap_err();
    assert!(err.contains("does not derive"), "wrong seed: {err}");
    println!("refused wrong via_pda_seed (does not derive)");

    let sends = vec![Send::to(loader, borsh::to_vec(&create).unwrap(), vec![s]), Send::to(e.actor(x), borsh::to_vec(&ix).unwrap(), vec![s])];
    e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors.clone(), &[&alice], false, T0).expect("owner program creates and registers X");
    println!("ok      owner program P creates the header at X = pda(P, s) and registers it");
    assert!(e.header(&x).is_some());
    let ent = e.entry(&x).unwrap();
    assert_eq!((ent.program, ent.author, ent.flags), (tid(&x), tid(&x), 0));

    // Another program holding no grant for X relays the same instruction.
    let p2 = key(FWD2_SEED).id;
    let sends = vec![Send::to(e.actor(x), borsh::to_vec(&ix).unwrap(), vec![s])];
    let err = e
        .try_tx(Actor::new(alice.id, p2), borsh::to_vec(&sends).unwrap(), actors_to_declare(x, e.reg, &ix), &[&alice], false, T0)
        .unwrap_err();
    assert!(err.contains("must authorize"), "other program: {err}");
    println!("refused a program that does not own X");

    // A root-level update claiming the PDA path is refused.
    let mut up = update(&x, 2, "2.0.0");
    if let Instruction::Update { via_pda_seed, .. } = &mut up {
        *via_pda_seed = Some(s);
    }
    e.refuse("root-level update with via_pda_seed", "via_pda_seed is only for", x, &up, &[&alice]);

    // The owner program updates (inline: author == entry == X) and freezes through the PDA path.
    let sends = vec![Send::to(e.actor(x), borsh::to_vec(&up).unwrap(), vec![s])];
    e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors_to_declare(x, e.reg, &up), &[&alice], false, T0).expect("P updates X");
    assert_eq!(e.entry(&x).unwrap().revision, 2);
    println!("ok      owner program P updates X's entry (revision 2)");
    let fr = Instruction::Freeze { entry_id: *x.value(), now: T0, via_pda_seed: Some(s) };
    let sends = vec![Send::to(e.actor(x), borsh::to_vec(&fr).unwrap(), vec![s])];
    e.try_tx(root, borsh::to_vec(&sends).unwrap(), actors_to_declare(x, e.reg, &fr), &[&alice], false, T0).expect("P freezes X");
    assert!(e.entry(&x).unwrap().is_frozen());
    println!("ok      owner program P freezes X's entry");
}

/// 10. Self-registration of REG at (REG, REG): `CreateHeader`, then `RegisterDeployer`;
/// `born_with_deploy` unset. A `DeployAndRegister` rooted at an undeployed (R, R) cannot run.
#[test]
fn t10_registry_registers_itself() {
    let reg_key = key(REG_SEED);
    let mut e = Engine::empty(reg_key.id);
    let first = e.write_segments(registry_elf(), 0xC0);
    // N5: before REG's code exists, (REG, REG) cannot dispatch.
    let early = deploy_and_register("lez_program_registry", &reg_key.id, first, false, [0; 32]);
    let err = e.try_ix(reg_key.id, &early, &[&reg_key], T0).unwrap_err();
    assert!(err.contains("UnknownProgram"), "an undeployed root program cannot run: {err}");
    println!("refused DeployAndRegister rooted at undeployed (REG, REG)");
    let a = e.create_header(&reg_key, first, false).expect("CreateHeader for REG");
    println!("ok      CreateHeader REG ({} cycles, {} tx bytes)", a.cycles, a.tx_bytes);
    let image = image_id_of(registry_elf());
    assert_eq!(e.header(&reg_key.id).unwrap().image_id, image);
    e.ok("RegisterDeployer at (REG, REG)", reg_key.id, &register_deployer("lez_program_registry", &reg_key.id, image_bytes(&image), None), &[&reg_key]);
    let ent = e.entry(&reg_key.id).unwrap();
    assert!(!ent.born_with_deploy());
    assert_eq!((ent.program, ent.image_id), (tid(&reg_key.id), image_bytes(&image)));
    // And the deployed registry serves others.
    let x = key(1);
    e.ok("X registers on the loader-deployed registry", x.id, &register_deployer("token", &x.id, [7; 32], None), &[&x]);
    assert_eq!(walk::global(&e.reader(), &e.treg(), 10).unwrap().len(), 2);
}

/// 11. An over-declared but untouched actor is legal.
#[test]
fn t11_over_declared_actor_is_legal() {
    let mut e = engine();
    let x = key(1);
    let ix = register_deployer("token", &x.id, [7; 32], None);
    let mut actors = actors_to_declare(x.id, e.reg, &ix);
    actors.push(e.actor(AccountId::new([0x99; 32])));
    actors.push(Actor::new(AccountId::new([0x98; 32]), key(FWD_SEED).id));
    actors.push(Actor::new(key(2).id, PROGRAM_LOADER_ACCOUNT_ID));
    e.try_tx(e.actor(x.id), borsh::to_vec(&ix).unwrap(), actors, &[&x], false, T0).expect("over-declared actors are legal");
    println!("ok      3 extra, untouched actors declared");
    assert!(e.entry(&x.id).is_some());
}

/// The program's loader mirror encodes like the real loader's message.
#[test]
fn loader_message_encoding() {
    let a = AccountId::new([7; 32]);
    let ours = borsh::to_vec(&lezreg_program::LoaderMessage::CreateHeader { first_segment: a, immutable: true }).unwrap();
    let theirs = borsh::to_vec(&program_loader_core::Message::CreateHeader { first_segment: a, immutable: true }).unwrap();
    assert_eq!(ours, theirs);
    let ours = borsh::to_vec(&lezreg_program::LoaderMessage::UpdateHeader { first_segment: a, immutable: false }).unwrap();
    let theirs = borsh::to_vec(&program_loader_core::Message::UpdateHeader { first_segment: a, immutable: false }).unwrap();
    assert_eq!(ours, theirs);
}

/// The timestamp window is enforced by the engine: `now` must be at most 10 minutes before the
/// block time and not after it.
#[test]
fn timestamp_window_is_enforced() {
    let mut e = engine();
    let x = key(1);
    let ix = register_deployer("token", &x.id, [7; 32], None);
    let before = e.state.clone();
    for (what, block_time) in [("block 10 min after now", T0 + t::TIME_WINDOW_MS), ("block before now", T0 - 1)] {
        let err = e.try_ix(x.id, &ix, &[&x], block_time).unwrap_err();
        assert!(err.contains("OutOfValidityWindow"), "{what}: {err}");
        assert!(e.state == before, "{what}: state changed");
    }
    e.try_ix(x.id, &ix, &[&x], T0 + t::TIME_WINDOW_MS - 1).expect("last millisecond of the window");
}

/// 12. Third-party entry lifecycle on the real engine: update, transfer and freeze through
/// `PatchEntry` to the REG-PDA entry `T`, then the walks.
#[test]
fn t12_third_party_lifecycle() {
    let mut e = engine();
    let (x, alice, bob, carol) = (key(1), key(3), key(4), key(6));
    e.ok("X registers", x.id, &register_deployer("token", &x.id, [7; 32], None), &[&x]);
    e.ok("alice publishes a third-party entry for X", alice.id, &register_third_party(&x.id, "token-community"), &[&alice]);
    let tp = lid(&addr::third_party_entry(&e.treg(), &tid(&x.id), &tid(&alice.id)));

    e.refuse("bob updates alice's third-party entry", "only the entry's author", bob.id, &update(&tp, 2, "0.2.0"), &[&bob]);
    e.refuse("X updates alice's third-party entry for X", "only the entry's author", x.id, &update(&tp, 2, "0.2.0"), &[&x]);
    e.ok("alice updates it (revision 2)", alice.id, &update(&tp, 2, "0.2.0"), &[&alice]);
    let te = e.entry(&tp).unwrap();
    assert_eq!((te.revision, te.version.as_str(), te.kind), (2, "0.2.0", t::KIND_THIRD_PARTY));
    let r2: t::Release = borsh::from_slice(&e.reg_state(&lid(&addr::release(&e.treg(), &tid(&tp), 2)))).unwrap();
    assert_eq!((r2.entry, r2.version.as_str()), (tid(&tp), "0.2.0"));

    // Handing it to its own address is refused (nobody can sign for a PDA): never orphaned.
    e.refuse("alice → T itself", "new author must sign", alice.id, &transfer(&tp, &tp), &[&alice]);
    e.refuse("bob transfers alice's entry", "only the entry's author", bob.id, &transfer(&tp, &carol.id), &[&bob, &carol]);
    e.ok("alice → carol (both sign)", alice.id, &transfer(&tp, &carol.id), &[&alice, &carol]);
    e.refuse("alice updates after the transfer", "only the entry's author", alice.id, &update(&tp, 3, "0.3.0"), &[&alice]);
    e.refuse("bob freezes carol's entry", "only the entry's author", bob.id, &freeze(&tp), &[&bob]);
    e.ok("carol updates (revision 3)", carol.id, &update(&tp, 3, "0.3.0"), &[&carol]);
    e.ok("carol freezes it", carol.id, &freeze(&tp), &[&carol]);
    assert!(e.entry(&tp).unwrap().is_frozen());
    e.refuse("update after freeze", "frozen", carol.id, &update(&tp, 4, "0.4.0"), &[&carol]);
    e.refuse("transfer after freeze", "frozen", carol.id, &transfer(&tp, &bob.id), &[&carol, &bob]);

    // A freeze aimed at a registry record that is not an entry.
    e.refuse("freeze aimed at the global head", "a registry entry", carol.id, &freeze(&lid(&addr::global_head(&e.treg()))), &[&carol]);

    let r = e.reader();
    let tps = walk::third_party(&r, &e.treg(), &tid(&x.id), 10).unwrap();
    assert_eq!(ids(&tps), vec![tp]);
    assert_eq!(tps[0].1.author, tid(&alice.id), "the link keeps the registrant");
    assert!(walk::by_author(&r, &e.treg(), &tid(&alice.id), 10).unwrap().is_empty());
    let held = walk::by_author(&r, &e.treg(), &tid(&carol.id), 10).unwrap();
    assert_eq!((held.len(), held[0].0, held[0].1.revision), (1, tid(&tp), 3));
    assert_eq!(walk::author_list(&r, &e.treg(), &tid(&alice.id), 10).unwrap(), vec![tid(&tp)], "history keeps the registration");
    let ent = e.entry(&tp).unwrap();
    assert_eq!((ent.author, ent.registrant, ent.program), (tid(&carol.id), tid(&alice.id), tid(&x.id)));
    e.report("t12");
}

/// Security review F1: a program the user signed for cannot make them an entry's author.
/// (A1) Program M registers its own PDA X' naming alice as author, in a transaction alice
/// signed for M. (A1b) M transfers an entry it holds to alice the same way. (A2) Program Q,
/// handed X''s grant by P, names X' as author of Q's own PDA. All refused, state unchanged.
#[test]
fn t13_pda_path_cannot_attribute_authorship() {
    let mut e = engine();
    let (alice, mallory) = (key(3), key(5));
    let m = key(FWD_SEED).id;
    let rule = "on the PDA-header path every signer must be the root";
    let alice_in_m = Actor::new(alice.id, m);

    // A1
    let s = [0x66; 32];
    let xp = pda(&m, s);
    let ix = register_deployer("wallet-drainer", &alice.id, [0xEE; 32], Some(s));
    let sends = vec![Send::to(e.actor(xp), borsh::to_vec(&ix).unwrap(), vec![s])];
    let before = e.state.clone();
    let err = e.try_tx(alice_in_m, borsh::to_vec(&sends).unwrap(), actors_to_declare(xp, e.reg, &ix), &[&alice], false, T0).unwrap_err();
    assert!(err.contains(rule), "A1: {err}");
    assert!(e.state == before && e.entry(&xp).is_none());
    assert!(walk::by_author(&e.reader(), &e.treg(), &tid(&alice.id), 10).unwrap().is_empty());
    println!("refused A1: M registers its PDA with author = alice in a tx alice signed for M");

    // A1b
    let s2 = [0x67; 32];
    let xq = pda(&m, s2);
    let own = register_deployer("spam", &xq, [0xEF; 32], Some(s2));
    let sends = vec![Send::to(e.actor(xq), borsh::to_vec(&own).unwrap(), vec![s2])];
    e.try_tx(Actor::new(mallory.id, m), borsh::to_vec(&sends).unwrap(), actors_to_declare(xq, e.reg, &own), &[&mallory], false, T0)
        .expect("M registers its own PDA with author = the PDA");
    let tr = Instruction::TransferAuthor { entry_id: *xq.value(), new_author_id: *alice.id.value(), now: T0, via_pda_seed: Some(s2) };
    let sends = vec![Send::to(e.actor(xq), borsh::to_vec(&tr).unwrap(), vec![s2])];
    let before = e.state.clone();
    let err = e.try_tx(alice_in_m, borsh::to_vec(&sends).unwrap(), actors_to_declare(xq, e.reg, &tr), &[&alice], false, T0).unwrap_err();
    assert!(err.contains(rule), "A1b: {err}");
    assert!(e.state == before);
    assert_eq!(e.entry(&xq).unwrap().author, tid(&xq));
    assert!(e.reg_state(&lid(&addr::author_node(&e.treg(), &tid(&alice.id), &tid(&xq)))).is_empty(), "alice's author node stays free");
    println!("refused A1b: M transfers its entry to alice in a tx alice signed for M");

    // A2
    let q = key(FWD2_SEED).id;
    let (sp, sq) = ([0x11; 32], [0x22; 32]);
    let xp2 = pda(&m, sp);
    let yq = pda(&q, sq);
    let ix = register_deployer("attributed", &xp2, [1; 32], Some(sq));
    let inner = vec![Send::to(e.actor(yq), borsh::to_vec(&ix).unwrap(), vec![sq])];
    let outer = vec![Send::to(Actor::new(xp2, q), borsh::to_vec(&inner).unwrap(), vec![sp])];
    let mut actors = actors_to_declare(yq, e.reg, &ix);
    actors.push(Actor::new(xp2, q));
    actors.push(e.actor(xp2));
    let before = e.state.clone();
    let err = e.try_tx(Actor::new(mallory.id, m), borsh::to_vec(&outer).unwrap(), actors, &[&mallory], false, T0).unwrap_err();
    assert!(err.contains(rule), "A2: {err}");
    assert!(e.state == before && e.entry(&yq).is_none());
    println!("refused A2: Q names P's PDA (an inherited grant) as author of Q's PDA");
}
