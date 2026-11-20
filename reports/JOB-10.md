# JOB-10 report — CLI + FURPS truth + offline (LP-0023)

Branch `hy4-cli` in `/Users/jef/logos/lez-program-registry-hy4-cli`, from `547f183`. Four commits:
`733b611` (FURPS module writes), `4e9b105` (show + IDL status), `e673c3a` (verify/build offline),
`45c657d` (module appfix). No push, no chain, no proof, no nix, no Docker build. I stayed in my
worktree and did not touch `main` or another worker's worktree. The on-chain program and
`artifacts/` were not edited.

## 1. FURPS truth pass

`docs/FURPS.md` criterion 9, Summary and Open now reflect the write methods in
`module/rust-lib/src/writes.rs` (register, third-party register, update, transfer, freeze) and
their evidence: unit tests, `module/rust-lib/testdata/write-vectors.json` pinned by
`sdk/rust/tests/module_vectors.rs`, and `.github/workflows/module.yml:181-206` on a local rc3
chain. Kept **partly met**: the JS SDK still has no writes and the module has no author search.
No testnet or demo-row claim was added. The module is 0.3.0; the catalog still serves 0.1.0.

## 2. Criterion 11 — CLI degrade gracefully on IDL failure

- `lezreg show` now fetches the IDL status with the infallible `idl::fetch_idl` and prints
  `  idl status    …` next to the metadata, in both text and JSON (`idl_status` on the deployer
  entry, a located third-party entry and each listed community entry). It uses a new
  `StorageOpts::show_fetch_config` with no retries, no blob cache and no `--pin-peer` parsing, so
  an IDL problem cannot hide the on-chain entry.
- `lezreg idl get` was not changed: it still exits nonzero on an unavailable IDL.
- `sdk/rust/tests/idl_fetch_http.rs` uses a local `TcpListener` fake Storage node and covers
  no Storage node (dead port), unknown CID (404), not-JSON (ParseError) and sha256 mismatch,
  with no chain or Storage process. A CLI unit test checks all four statuses render as text and
  status JSON. FURPS criterion 11 and its Open item updated.

## 3. Criterion 12 — disclose third-party services, offline mode

- `verify` and `build` disclose Docker Hub (`risczero/risc0-guest-builder`), crates.io
  (`cargo fetch`) and, for `verify`, the entry's git forge in `--help`, on stderr at every run,
  and as `services` in JSON output (`offline` / `network_denied` alongside).
- New inputs: `--image-tar` (`docker load`, image ref taken from its output), `--vendor-tar`
  (unpacked into the build context with a cargo source replacement), `--archive-file` +
  `--no-forge` (source tar instead of a clone), and `--offline` (refuses other network steps).
  Missing inputs fail before any work with a message naming every missing switch; `--offline`
  with `--allow-network` is refused. The offline Dockerfile has no `cargo fetch` and no registry
  `FROM`, and compiles with `--network=none`.
- Tests: an SDK unit test on the offline Dockerfile, CLI unit tests for missing-input messages and
  flag parsing, and `cli/tests/verify_offline.rs` asserting `--help` disclosure and the clean
  failure. FURPS criterion 12, its Open item and a README service table updated.
- **Not run here:** no Docker build (this Mac has no usable Docker kernels and the owner's word is
  needed). The offline path is written and unit-tested, not exercised end to end.

## 4. Module `TODO(appfix)`

Removed the stale TODO and moved `verify_app` / `verify_package` onto the current SDK API:
`Contents::from_installed` for an installed module, `Contents::from_lgx` or an installed directory
for `verify_package`, then `Names::verify`; `root_matches()` and `RootMismatch` are checked before
publisher lookup. The root in the result is recomputed from files, never copied from a manifest.
`publisher_apps` uses `AppRecord::newest()` / `roots()`. Bumped the SDK pin to `547f183`.
`docs/sdk.md`, `docs/APPS.md` and FURPS criterion 9 updated. The test fixture now writes real
variant files and hashes, and asserts a swapped file flips the verdict to `root_mismatch`.

## Tests I ran

`~/logos/laptop-build.sh cargo test -p lezreg-sdk -p lezreg`: all green at each step. Final run:
27 CLI unit, 3 `verify_offline` integration, 58 SDK, 10 `reads`, 3 `js_vectors`, 2
`module_vectors`, 4 `idl_fetch_http`, 2 `registry_image`, 16 `app_fixtures`, plus smaller doc/vector
tests — **0 failed**. Module: `~/logos/laptop-build.sh cargo test` in `module/rust-lib` — **19
passed, 0 failed**, after temporarily staging a minimal local stand-in for
`logos-rust-sdk-src` (the real source is injected by `mkLogosModule`). That stand-in was deleted
before the commit; a clean builder build still needs the generated provider and the real SDK.

## Spec note / guesses

- The reviewer's "criterion 11/12" numbering is not in `LP-0023.md` or open PR #188 (read with
  `gh`). We mapped them to the two Reliability bullets: graceful IDL degradation and no mandatory
  third-party services. PR #188 is open, adds the "IDL format and tooling" criterion and the LEZ
  IDL format link, and does **not** change those two bullets.
- `risczero/risc0-guest-builder` comes from the recorded `BuildConfig` image tag/digest, not a
  hardcoded constant; the disclosure names it generically.
- Offline `--archive-file` assumes a `git archive` of the recorded commit/docker root with files
  at the tar root, matching `source_from_git`; a Storage CID archive is not fetched in this draft.
- `--image-tar` relies on `docker load` printing `Loaded image: <ref>`; that line was not observed
  here (no Docker run). The code fails clearly if the line is absent.
- Module tests used a stand-in for the builder-staged `logos-rust-sdk`; the stand-in's
  `LogosCaller::identity()` format (`module:<name>`) was inferred from the module's own test
  expectations. A real builder build on the Mini is still needed.
