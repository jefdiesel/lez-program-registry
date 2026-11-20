# B12 — JS SDK parity (LP-0023)

Worktree `/Users/jef/logos/lez-program-registry-hy4-js`, branch **`hy4-js`** from `547f183`, commit
`aed4dad` (+ report commit), no push. No other worktree or main touched; no chain/proof/ssh/nix.
`program/` and `artifacts/` were not changed, so no on-chain behaviour changed.

## Read

`AGENTS.md`, `docs/FURPS.md` (criterion 9 is ours: JS SDK had only name search and no writes),
`docs/DESIGN.md`, the LP-0023 spec via `gh` and open PR #188 (actor program / LEZ IDL rewrite),
`sdk/rust/src/{tx,read,idl}.rs`, `module/rust-lib/src/writes.rs`, `program/src/lib.rs`, and the
existing vectors.

## Built

- **Discovery**: `Registry.byAuthor(author, {limit, scan})` walks the author list and AuthorNode
  records like Rust `by_author`; `Registry.thirdParty(program, limit)` walks the per-program list
  like `third_party`. Cycle detection and truncate semantics match.
- **IDL statuses**: results now carry both the legacy `status`/`kind` and `statusKind` with Rust's
  names (`Ok`, `UnknownFormat`, `Unavailable`, `HashMismatch`, `ParseError`), plus `pendingIdl()`
  for `Pending`.
- **Writes** (`sdk/js/writes.js`): zero-dependency Borsh encoders for `register`,
  `register_third_party` and `update`, plus `buildChange()` producing the same accounts/root/
  signers/message/entry as the Rust `Built::change`. `transfer` and `freeze` remain out of scope.
- **Signing** (`sdk/js/signing.js`): copied from logos-sdk-js (not linked across repos). The core
  holds no keys; `signAndSend` refuses until an existing audited signer object is supplied
  explicitly. No transaction is built or sent by default.
- **Vectors**: `sdk/js/vectors.json` now embeds the eight Rust-generated module write cases;
  `sdk/rust/tests/js_vectors.rs` regenerates/verifies the combined file. Every JS encoder is
  checked byte for byte against the real program's `Instruction` bytes.
- `docs/sdk.md` gained a JS-parity section with examples and the test commands.

## Tests

- `node sdk/js/lezreg.test.mjs`: **ALL PASS** (existing decode/RPC/IDL vectors plus new write
  encoders, change accounts/signers/entry, status names and the signing guard).
- `~/logos/laptop-build.sh cargo test -p lezreg-sdk --test js_vectors --test module_vectors`:
  both pass (1/1 each). Vectors were regenerated with `LEZREG_WRITE_VECTORS=1` as the Rust test
  requires.

## Open questions

1. PR #188 changes the spec to a LEZ actor program and a LEZ IDL format; our JS IDL readers still
   recognise SPEL v1/v2, matching what the current Rust SDK emits. Align both if the PR lands.
2. `transfer`/`freeze` write encoders and two-machine signing are not in this job's JS parity set.
3. New search methods are verified with the existing mocked-sequencer style, not against a live chain.
