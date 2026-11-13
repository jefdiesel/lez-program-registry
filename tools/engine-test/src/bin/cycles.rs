//! `lezreg-cycles [--json FILE] [--cases DIR]`: measures every registry operation (docs/CYCLES.md)
//! with LEZ's cycle_bench method on the committed registry binary and prints the table.
//! `--json` writes every measurement; `--cases` writes one case file per operation for LEZ's
//! own `cycle_bench` with `tools/cycle-bench/cycle_bench-external-cases.patch` applied.

use lezreg_engine_test::cycles;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let ops = cycles::run();
    print!("{}", cycles::markdown(&ops));
    if let Some(path) = flag("--json") {
        let all: Vec<serde_json::Value> = ops
            .iter()
            .map(|o| {
                serde_json::json!({
                    "op": o.op,
                    "case": o.case,
                    "transitions": o.m.rows.iter().map(|r| serde_json::json!({
                        "program": r.program,
                        "receiver": r.receiver.to_string(),
                        "user_cycles": r.cycles,
                        "segments": r.segments,
                        "loader_bytes_read": r.loader_bytes,
                    })).collect::<Vec<_>>(),
                    "guest_cycles": o.m.guest_cycles(),
                    "loader_gas": o.m.loader_cycles(),
                    "execution_gas": o.m.charged,
                    "data_bytes": o.data_bytes(),
                    "fee_at_base_fees_8_8": o.fee_at_min_base_fees().to_string(),
                    "program_bytes": o.program_bytes.map(|(bin, user)| serde_json::json!({ "bin": bin, "user_elf": user })),
                })
            })
            .collect();
        std::fs::write(&path, serde_json::to_string_pretty(&all).expect("json")).expect("write --json");
        eprintln!("wrote {path}");
    }
    if let Some(dir) = flag("--cases") {
        std::fs::create_dir_all(&dir).expect("--cases dir");
        for (i, o) in ops.iter().enumerate() {
            let path = format!("{dir}/{i:02}.json");
            std::fs::write(&path, serde_json::to_string_pretty(&o.m.case).expect("json")).expect("write case");
        }
        // The guests the cases reach, for `cycle_bench --elf <id>=<path>`.
        let mut elfs = String::new();
        for (id, name, elf) in [
            (lezreg_engine_test::key(cycles::REG_SEED).id, "registry", lezreg_engine_test::artifact_elf()),
            (lezreg_engine_test::key(cycles::FWD_SEED).id, "forwarder", lezreg_engine_test::forwarder_elf()),
        ] {
            let path = format!("{dir}/{name}.bin");
            std::fs::write(&path, elf).expect("write elf");
            let hex: String = id.value().iter().map(|b| format!("{b:02x}")).collect();
            elfs += &format!("--elf {hex}={path}\n");
        }
        std::fs::write(format!("{dir}/elfs.args"), elfs).expect("write elfs.args");
        eprintln!("wrote {} cases to {dir}", ops.len());
    }
}
