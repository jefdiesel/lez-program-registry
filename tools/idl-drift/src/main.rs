//! `lezreg-idl-extend <generator-output.json>`: prints the extended IDL (DESIGN §6.3).
//! `lezreg-idl-extend --check <generator-output.json> <committed.idl.json>`: fails on any drift.

use std::process::ExitCode;

fn read(path: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [input] => match lezreg_idl::extend(&read(input)) {
            Ok(v) => {
                print!("{}", lezreg_idl::to_pretty(&v));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        [flag, input, committed] if flag == "--check" => {
            let want = lezreg_idl::extend(&read(input)).map(|v| lezreg_idl::to_pretty(&v));
            let have = std::fs::read_to_string(committed).unwrap_or_default();
            match want {
                Ok(w) if w == have => {
                    println!("{committed}: up to date");
                    ExitCode::SUCCESS
                }
                Ok(_) => {
                    eprintln!("{committed} is stale: run scripts/idl.sh");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("usage: lezreg-idl-extend <spel-output.json> | --check <spel-output.json> <committed.idl.json>");
            ExitCode::FAILURE
        }
    }
}
