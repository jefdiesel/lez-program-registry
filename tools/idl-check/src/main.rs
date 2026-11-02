//! Drift test 2 (DESIGN §6.3): parses the committed registry IDL with the released SPEL v0.7.0
//! `SpelIdl` type, checks its shape, and round-trips it. Separate workspace: SPEL v0.7.0 pins
//! LEZ 0.2.5 and must never meet the 0.4 crates.
//!
//! Usage: idl-check <idl.json>   (exit 0 = pass)
use spel_framework_core::idl::{IdlSeed, SpelIdl};

fn main() {
    let path = std::env::args().nth(1).expect("usage: idl-check <idl.json>");
    let text = std::fs::read_to_string(&path).unwrap();
    let idl: SpelIdl = serde_json::from_str(&text).expect("SpelIdl parse failed");
    println!("parsed: name={} version={} spec={:?} instruction_type={:?}", idl.name, idl.version, idl.spec, idl.instruction_type);
    for (i, ix) in idl.instructions.iter().enumerate() {
        let accs: Vec<String> = ix
            .accounts
            .iter()
            .map(|a| {
                let flags = format!("{}{}{}", if a.signer { "s" } else { "" }, if a.writable { "w" } else { "" }, if a.init { "i" } else { "" });
                match &a.pda {
                    Some(p) => format!("{}[{flags}|pda:{}]", a.name, p.seeds.len()),
                    None => format!("{}[{flags}]", a.name),
                }
            })
            .collect();
        println!("  ix {i} {:<21} disc={:?} accounts={accs:?} args={}", ix.name, ix.discriminator, ix.args.len());
    }
    println!("  accounts[]={:?}", idl.accounts.iter().map(|a| &a.name).collect::<Vec<_>>());
    println!("  types[]={:?}", idl.types.iter().map(|t| &t.name).collect::<Vec<_>>());

    assert_eq!(idl.name, "lez_program_registry");
    assert_eq!(idl.instruction_type.as_deref(), Some("Instruction"));
    let names: Vec<&str> = idl.instructions.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["register_deployer", "register_third_party", "update", "transfer_author", "freeze", "deploy_and_register"]);
    for (i, ix) in idl.instructions.iter().enumerate() {
        assert_eq!(ix.discriminator.as_deref(), Some(&[i as u8][..]), "{}: discriminator is the wire tag", ix.name);
        assert!(ix.accounts[0].signer, "{}: the root signs", ix.name);
    }
    let reg = &idl.instructions[0];
    assert_eq!(reg.accounts.len(), 7);
    let rel = reg.accounts.iter().find(|a| a.name == "release").unwrap();
    assert!(rel.init);
    let seeds = &rel.pda.as_ref().unwrap().seeds;
    assert!(matches!(&seeds[0], IdlSeed::Const { value } if value == "lezreg/v1/rel"));
    assert!(matches!(&seeds[2], IdlSeed::Arg { path } if path == "revision_seed"));
    for a in ["Entry", "Release", "Link", "Head", "AuthorNode"] {
        assert!(idl.accounts.iter().any(|x| x.name == a), "accounts[] has {a}");
    }
    for t in ["ContentRef", "SourceRef", "BuildConfig", "EnvVar"] {
        assert!(idl.types.iter().any(|x| x.name == t), "types[] has {t}");
    }

    // Round trip: SpelIdl -> JSON equals the file minus the x-lez04 extension SpelIdl ignores.
    let mut orig: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(orig.get("x-lez04").is_some(), "the committed IDL carries x-lez04");
    orig.as_object_mut().unwrap().remove("x-lez04");
    let again = serde_json::to_value(&idl).unwrap();
    assert_eq!(orig, again, "SpelIdl round trip");
    println!("roundtrip_equal=true x-lez04_tolerated=true");
    println!("IDL-CHECK PASS");
}
