//! Host checks of what the shim generated from the SPEL-syntax source.
use spel::lee_core::account::AccountId;
use toy_registry::{actors_to_declare, ContentRef, Instruction};

fn hex(a: &AccountId) -> String {
    a.value().iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn pdas_match_spel_rule_and_spel_cli() {
    // Same inputs as the `spel --dry-run` probe (program id a5.., X = 22.., author = 33..).
    let reg = AccountId::new([0xa5; 32]);
    let x = AccountId::new([0x22; 32]);
    let ix = Instruction::Register {
        author_id: [0x33; 32],
        name: "alice".into(),
        version: "1.0.0".into(),
        idl: ContentRef { cid: vec![1, 2], sha256: [0; 32], size: 5 },
        image_id: [0; 32],
        revision: 1,
        now: 1000,
    };
    let actors = actors_to_declare(x, reg, &ix);
    let ids: Vec<String> = actors.iter().map(|a| hex(&a.account_id)).collect();
    println!("{ids:#?}");
    assert!(actors.iter().all(|a| a.program_account_id == reg));
    assert_eq!(ids[0], "22".repeat(32));
    assert_eq!(ids[1], "33".repeat(32));
    // release = pda[const, account(program), arg(revision: u32 LE)] (python reference)
    assert_eq!(ids[2], "5edadec27bc6e4308cbd4bc129212162a499a5c171fb74771e7a3472f7efdfb3");
    // head and name_head equal what the released spel CLI derived from the IDL
    // (7oS5b5cW…KyPr and DYxSzNmx…Zpok in base58).
    assert_eq!(ids[3], "650b321ff315c5b2b4e551e577c204f84e009429d29a04d57aa197aef58c47cd");
    assert_eq!(ids[4], "ba7bf18cb0d2a5fc9ab9e50b26080ea5f06ce6d4a393b0c2aa6ce76cb6805a03");
}

#[test]
fn wire_is_borsh_enum_in_fn_order() {
    let ix = Instruction::Freeze { entry_id: [0x22; 32], now: 1000 };
    let bytes = borsh::to_vec(&ix).unwrap();
    assert_eq!(bytes[0], 2, "freeze is the 3rd #[instruction] fn");
    assert_eq!(&bytes[1..33], &[0x22; 32]);
    assert_eq!(&bytes[33..], &1000u64.to_le_bytes());
}
