//! The integration path most consumers take (docs/sdk.md): a program id in, its registry
//! metadata, freshness and IDL out, with an unavailable IDL shown as such rather than failing.
//!
//! ```sh
//! cargo run -p lezreg-sdk --no-default-features --example resolve_program -- \
//!     http://127.0.0.1:3062 <registry id> <program id> [storage api]
//! ```

use lezreg_sdk::{
    Freshness, HttpRpc, Registry,
    idl::{FetchConfig, IdlStatus, fetch_idl},
    types::AccountId,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(rpc), Some(registry), Some(program)) = (args.next(), args.next(), args.next()) else {
        eprintln!("usage: resolve_program <rpc url> <registry id> <program id> [storage api]");
        std::process::exit(2);
    };
    let registry = AccountId::from_base58(&registry).ok_or("not a registry id")?;
    let program = AccountId::from_base58(&program).ok_or("not a program id")?;

    // 1. One getAccount: the deployer entry, the live program header, and whether they agree.
    let reg = Registry::new(HttpRpc::new(&rpc), registry);
    let resolved = reg.resolve(&program)?;
    let Some(entry) = &resolved.deployer else {
        println!("{program}: no deployer entry (third-party entries: {})", reg.third_party(&program, 20)?.len());
        return Ok(());
    };
    println!("{} {} by {}", entry.name, entry.version, entry.author.to_base58());
    println!("  {}", entry.description);
    match resolved.freshness() {
        Some(Freshness::Fresh) => println!("  fresh: the program runs the image the entry claims"),
        Some(Freshness::Stale { live }) => println!(
            "  stale: the program now runs image {}, not the one the entry claims; don't trust the IDL blindly",
            hex::encode(live)
        ),
        Some(Freshness::NoProgram) => println!("  no program is deployed at this account"),
        None => {}
    }

    // 2. The IDL from Logos Storage, checked against the sha256 the chain records. Every failure
    //    is a status, never an error: the entry above stays shown.
    let cfg = FetchConfig { storage_api: args.next().or(FetchConfig::default().storage_api), ..FetchConfig::default() };
    //    An IDL can be in any format (SPEL v1, SPEL v2, or one this SDK doesn't know); the
    //    status says which, and `calls()` lists its messages or instructions either way.
    let status = fetch_idl(&entry.idl, &cfg);
    match (&status, status.info()) {
        (IdlStatus::Ok(_), Some(info)) => {
            let calls = status.calls();
            let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
            println!(
                "  IDL: {} {} ({} {}): {}",
                calls.len(),
                // SPEL v1 lists instructions, v2 messages.
                if info.format == lezreg_sdk::IdlFormat::SpelV1 { "instructions" } else { "messages" },
                info.format,
                info.format_version.as_deref().unwrap_or("(no version)"),
                names.join(", ")
            );
        }
        (IdlStatus::UnknownFormat(_), _) => {
            println!("  IDL: available, format not recognised (not SPEL v1 or v2); shown as raw JSON only")
        }
        (other, _) => println!("  IDL unavailable ({other}); the entry's metadata still stands"),
    }
    Ok(())
}
