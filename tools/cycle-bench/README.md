# cycle-bench: the registry's numbers with LEZ's own `cycle_bench`

LEZ's `tools/cycle_bench` (v0.4.0-rc3) measures user cycles per guest transition, but only for
the built-in programs it lists in `guest()`, on fixtures written in `cases()`, and its `Meter`
has no branch for the program loader. As released it cannot run a third-party guest or a
deploy. This directory holds what it takes to run it on the registry, plus the chain
measurement. docs/CYCLES.md has the numbers.

- `cycle_bench-external-cases.patch` (against `logos-execution-zone` v0.4.0-rc3,
  `tools/cycle_bench`) adds three things:
  - `--elf <program account id hex>=<program.bin>` registers an external guest;
  - `--case <case.json>` adds an operation on it, read from a file: the root actor, the message,
    the declared actors with their pre-states and authorization, and the loader states the
    loader may read;
  - a loader branch in `Meter::handle_message` that runs `program_loader_core::handle_message`
    and charges `LOADER_CYCLES_PER_BYTE` (1) per loader-state byte read, as settlement's
    `run_program_loader` does. Loader rows are printed in their own table and kept out of the
    calibration fit.

  A case can carry `expected_rows`; the tool then says whether its user cycles match them.
  Built-in cases are unchanged.
- `chain-costs.sh` measures what each operation costs on a fresh rc3 standalone sequencer
  (balance deltas, receipts, dry-run reserves).

## Reproduce

```sh
# 1. Our harness (same method; also checks the rows against the real state machine's charge),
#    writing one case file per operation:
cargo run --release -p lezreg-engine-test --bin lezreg-cycles -- --json cycles.json --cases "$PWD/cases"
# 2. LEZ's cycle_bench on those cases:
git clone --depth 1 --branch v0.4.0-rc3 https://github.com/logos-blockchain/logos-execution-zone.git lez
cd lez && git apply -p1 ../tools/cycle-bench/cycle_bench-external-cases.patch
cargo run --release -p cycle_bench -- --exec-iters 2 $(cat ../cases/elfs.args) \
  $(for f in ../cases/[0-9]*.json; do echo --case "$f"; done)
```

The patch modifies LEZ code, which is licensed MIT or Apache-2.0 (the
`logos-execution-zone` workspace `Cargo.toml`), as this repository is. It is a proposal for
upstream; nothing here is vendored.
