//! Parses an IDL produced by `spel generate-idl` back through SPEL v0.7.0's `SpelIdl`,
//! checks the expected shape, round-trips it, and checks an `x-lez04` extension is tolerated.
use spel_framework_core::idl::SpelIdl;

fn main() {
    let path = std::env::args().nth(1).expect("usage: idl-check <idl.json>");
    let text = std::fs::read_to_string(&path).unwrap();
    let idl: SpelIdl = serde_json::from_str(&text).expect("SpelIdl parse failed");
    println!("parsed: name={} version={} instruction_type={:?}", idl.name, idl.version, idl.instruction_type);
    for ix in &idl.instructions {
        let accs: Vec<String> = ix
            .accounts
            .iter()
            .map(|a| format!("{}{}{}{}{}", a.name, if a.signer { "[s]" } else { "" }, if a.writable { "[w]" } else { "" }, if a.init { "[init]" } else { "" }, a.pda.as_ref().map(|p| format!("[pda:{}]", p.seeds.len())).unwrap_or_default()))
            .collect();
        let args: Vec<&str> = ix.args.iter().map(|a| a.name.as_str()).collect();
        println!("  ix {:<9} accounts={:?} args={:?}", ix.name, accs, args);
    }
    println!("  accounts[]={:?}", idl.accounts.iter().map(|a| &a.name).collect::<Vec<_>>());
    println!("  types[]={:?}", idl.types.iter().map(|t| &t.name).collect::<Vec<_>>());

    let names: Vec<&str> = idl.instructions.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["register", "update", "freeze"], "instruction order");
    let reg = &idl.instructions[0];
    assert_eq!(reg.accounts.len(), 5);
    assert!(reg.accounts[0].signer && reg.accounts[1].signer);
    assert!(reg.accounts[2].init && reg.accounts[2].pda.as_ref().unwrap().seeds.len() == 3);
    assert_eq!(reg.args.len(), 7);
    assert!(idl.accounts.iter().any(|a| a.name == "Entry"));
    assert!(idl.accounts.iter().any(|a| a.name == "Release"));
    assert!(idl.types.iter().any(|t| t.name == "ContentRef"));

    // Round trip: SpelIdl -> JSON equals the generator's JSON.
    let orig: serde_json::Value = serde_json::from_str(&text).unwrap();
    let again = serde_json::to_value(&idl).unwrap();
    println!("roundtrip_equal={}", orig == again);

    // Unknown top-level key (the planned x-lez04 block) must not break parsing.
    let mut ext = orig.clone();
    ext["x-lez04"] = serde_json::json!({"spec": "lezreg-idl-ext/0.1", "wire": {"effect_tag_min": 192}});
    let ok = serde_json::from_value::<SpelIdl>(ext).is_ok();
    println!("x-lez04_tolerated={ok}");
    assert!(ok);
    println!("IDL-CHECK PASS");
}
