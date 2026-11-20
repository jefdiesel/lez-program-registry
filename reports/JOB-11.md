# JOB-11 report — network file in the Rust SDK and lezreg

Two worktrees, no push, no chain calls, no nix.

## Part 1: logos-sdk `network` module (`main`, commit `6533d78`)

`logos_sdk::network` in `crates/network`:
- parses/validates `names-network/1` and `/2`: 64 KiB cap, 64-hex lowercase chain id, https RPC
  (http only for `kind: local`), wire/lez lines this rc3 build speaks, mirrored v1 fields must equal
  `programs`, ≤6 bridges, ≤16 NFT programs, clean ≤40-char name, typed base58 program ids.
- v1 files map into the typed v2 view (`registry` → `programs.lns_registry`, bridges and NFT
  mirrors, missing built-ins come from the build).
- `resolve(explicit, config_dir, chain_id)` uses one whole file at a time: explicit path/URL/chain
  id, then `<config>/networks/<chain>.json`, then the published
  `https://nameswallet.app/networks/<chain>.json` (only when its revision is ≥ the built-in), then
  the embedded built-in profile. Fields are never merged.
- `check_program` enforces spec §6: `getChannelId` must match; live header image must match a
  built-in pin, or a program registry whose own image is pinned and whose entry image matches the
  live header; otherwise `Unchecked`, with `Refused` on chain mismatch. File-supplied image ids
  alone never count as proof.
- Tests use the real published `2da7f7df….json`, the embedded v2 profile, v1 mapping, invalid
  files, override precedence and fake chain/registry probes. 5 tests, 0 failed.
- `RISC0_SKIP_BUILD_KERNELS=1 cargo test --workspace`: 55 passed, 0 failed.

## Part 2: program registry rows (`hy4-net`, worktree from `547f183`)

- `sdk/rust/src/network_file.rs`: a small parse-only v2 reader (64 KiB cap, format/lez/chain/RPC
  checks, `programs.program_registry.{id,image_id}` and `programs.lns_registry.id`, v1 mirror
  fallback, `chain_matches`). Copied into the standalone module because the builder stages only
  `module/rust-lib/`.
- `cli/src/config.rs`: `Resolved` gained `chain_id`, `lns` and `storage_api`; `testnet` now resolves
  from the embedded v2 file instead of being an empty placeholder; `LEZREG_NETWORK_FILE` or a
  saved `network_file` overrides from a whole file; `LEZREG_LNS`/`--lns` override; `check_chain`
  compares `getChannelId` with the file's chain id, exposed as `lezreg network show --check-chain`.
  `network set` saves `--chain-id`, `--lns` and `--network-file`. Both "testnet has not launched"
  errors are gone.
- `module/rust-lib/src/lib.rs`: removed `TESTNET_SEQUENCER`/`TESTNET_LNS`/`default_lns` — an old
  0.3 URL no longer selects the 0.3 LNS program. Defaults now come from the embedded v2 profile;
  saved settings still override. `shown()` no longer reports a URL-derived default LNS.
- Tests: two config tests (embedded testnet profile, saved chain id/lns and flag override), three
  network_file tests, one module test that a 0.3 URL does not set LNS. Module: 22 passed, 0 failed
  with a temporary stand-in for the builder-staged `logos-rust-sdk` (removed before commit).
  `cargo test -p lezreg-sdk -p lezreg`: all green (24 CLI unit, 60 SDK, plus integration/vector
  tests). No live chain was used.

## Notes / limits

- The registry intentionally does **not** depend on `logos-sdk`: the module is a standalone crate
  and an absolute path would break Nix builds. The reader is duplicated in the two crates.
- `check_chain` is opt-in via `network show --check-chain`; it needs a reachable RPC and was not
  run here. Automatic checks on every start would need live-RPC tests we cannot run.
- Audit rows that are "Y if wire" (LEZ rc3 crate pins, loader header layout, clock constants, CI
  pins) were left unchanged; they need a decision after comparing official 0.4 with rc3.
