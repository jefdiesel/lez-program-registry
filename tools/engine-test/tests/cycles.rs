//! docs/CYCLES.md, checked: every operation is measured with LEZ's cycle_bench method on the
//! committed registry binary and must match the documented numbers exactly (the cycles of a
//! given binary and input do not depend on the machine). `bench::measure` also checks, for
//! every operation, that its per-transition cycles plus the loader's per-byte charge equal what
//! the real state machine charges. A change to the program or to LEZ that moves a number fails
//! here until CYCLES.md (and this table) are updated.

use lee_core::program::ProgramSegment;
use lezreg_engine_test::cycles::{self, Op};

/// `(operation, case, execution gas, data bytes)` for every case whose bytes come from
/// `artifacts/` alone. Same numbers as docs/CYCLES.md.
const EXPECTED: &[(&str, &str, u64, u64)] = &[
    ("RegisterDeployer", "the registry's own entry at launch (REG signs), typical fields", 173849, 1358),
    ("RegisterDeployer", "min fields, author = X (one signature)", 164509, 1043),
    ("RegisterDeployer", "typical fields, author ≠ X (two signatures)", 187438, 1591),
    ("RegisterDeployer", "worst-case fields, author ≠ X", 258128, 3317),
    ("RegisterDeployer", "typical fields, taken name, author = X", 180712, 1351),
    ("RegisterThirdParty", "min fields", 219985, 1171),
    ("RegisterThirdParty", "typical fields", 241215, 1481),
    ("RegisterThirdParty", "worst-case fields", 344010, 3204),
    ("Update", "one field (version), author ≠ X (PatchEntry)", 92827, 621),
    ("Update", "typical (version, description, IDL), author ≠ X", 97958, 776),
    ("Update", "every field at its limit, author ≠ X", 233322, 2857),
    ("Update", "one field (version), author = X (inline)", 66979, 553),
    ("Update", "third-party entry, one field (version)", 95259, 621),
    ("TransferAuthor", "alice → carol, both sign", 103470, 904),
    ("Freeze", "author ≠ X (PatchEntry)", 47618, 504),
    ("Freeze", "author = X (inline)", 26962, 440),
    ("WriteSegment", "lez_program_registry, segment 6/6 (31328 bytes, the tail)", 0, 31732),
    ("WriteSegment", "lez_program_registry, segment 5/6 (98304 bytes, links the next)", 31334, 98740),
    ("WriteSegment", "lez_program_registry, segment 4/6 (98304 bytes, links the next)", 98342, 98740),
    ("WriteSegment", "lez_program_registry, segment 3/6 (98304 bytes, links the next)", 98342, 98740),
    ("WriteSegment", "lez_program_registry, segment 2/6 (98304 bytes, links the next)", 98342, 98740),
    ("WriteSegment", "lez_program_registry, segment 1/6 (98304 bytes, links the next)", 98342, 98740),
    ("DeployAndRegister", "lez_program_registry: 555272 B binary, 522848 B user ELF", 710820, 1687),
    ("CreateHeader", "lez_program_registry (522848 B user ELF), plain loader", 523044, 432),
    ("UpdateHeader", "lez_program_registry (522848 B user ELF), plain loader", 523110, 432),
];

/// Cases that run a guest built on this machine (the forwarder and the ballast programs):
/// only their registry part and the loader's byte count are checked.
fn local(o: &Op) -> bool {
    o.case.contains("forwarder") || o.case.contains("ballast") || o.case.contains("via_pda_seed")
}

/// Bytes the loader reads to walk `user_elf`'s segment chain: every segment's loader state.
fn chain_bytes(user_elf_len: usize) -> u64 {
    let n = user_elf_len.div_ceil(program_loader_core::MAX_SEGMENT_DATA_LEN);
    (0..n)
        .map(|i| {
            let len = (user_elf_len - i * program_loader_core::MAX_SEGMENT_DATA_LEN).min(program_loader_core::MAX_SEGMENT_DATA_LEN);
            let next = (i + 1 < n).then(|| lee_core::account::AccountId::new([0; 32]));
            ProgramSegment { bytecode: vec![0; len], next_segment: next }.to_loader_actor_state().len() as u64
        })
        .sum()
}

#[test]
fn cycles_match_docs_cycles_md() {
    let ops = cycles::run();
    print!("{}", cycles::markdown(&ops));

    // The documented numbers.
    let mut seen = 0;
    for (op, case, gas, data) in EXPECTED {
        let o = ops.iter().find(|o| o.op == *op && o.case == *case).unwrap_or_else(|| panic!("no case {op} / {case}"));
        assert_eq!((o.m.charged, o.data_bytes()), (*gas, *data), "{op} / {case}: (execution gas, data bytes) moved; update docs/CYCLES.md");
        seen += 1;
    }
    assert_eq!(seen, ops.iter().filter(|o| !local(o)).count(), "every artifact-only case is documented");

    // The loader charges exactly the bytes it reads (LOADER_CYCLES_PER_BYTE = 1).
    for o in &ops {
        match o.op.as_str() {
            "CreateHeader" | "DeployAndRegister" => {
                let (_, user) = o.program_bytes.expect("deploy cases carry sizes");
                assert_eq!(o.m.loader_cycles(), chain_bytes(user), "{}: the loader reads the whole chain", o.case);
            }
            "UpdateHeader" => {
                let (_, user) = o.program_bytes.expect("deploy cases carry sizes");
                assert_eq!(o.m.loader_cycles(), chain_bytes(user) + 66, "{}: the chain and the 66-byte header", o.case);
            }
            "WriteSegment" if o.case.contains("the tail") => assert_eq!(o.m.loader_cycles(), 0, "{}", o.case),
            "WriteSegment" => assert!(o.m.loader_cycles() > 0, "{}: reads the next segment", o.case),
            _ => assert_eq!(o.m.loader_cycles(), 0, "{} {}: no loader", o.op, o.case),
        }
    }

    // DeployAndRegister's registry part does not depend on the program's size.
    let dar: Vec<u64> = ops.iter().filter(|o| o.op == "DeployAndRegister").map(|o| o.m.guest_cycles()).collect();
    assert_eq!(dar.len(), 4);
    assert!(dar.iter().all(|c| *c == dar[0]), "registry cycles of DeployAndRegister vary with size: {dar:?}");
}
