# Solution: LP-0023 — LEZ Program Registry (self-assessment)

**Submitted by:** jefdiesel (the submitter confirms this line and the Terms line at the end)

This is the FURPS self-assessment for the LP-0023 entry, in the shape of the λPrize solution
template (`solutions/LP-0000.md`). Every checkbox of the
[prize spec](https://github.com/logos-co/lambda-prize/blob/master/prizes/LP-0023.md) is quoted
below with a status and the evidence for it. Each claim was checked against the repository as of
this file's commit. Where this file and `docs/DESIGN.md` disagree, this file is the one that was
checked against the code. DESIGN describes the plan, and some of it is not built yet.

**Status words used below:**
- **met**: works and is tested here, and needs no testnet.
- **met on a local rc sequencer, pending testnet 0.4**: works end to end against a LEZ
  `v0.4.0-rc3` standalone sequencer (CI and `scripts/demo.sh`), but has not run on testnet 0.4,
  which has not launched.
- **partly met**: some of the criterion works; the rest is listed.
- **not yet**: not done.

## Summary

The registry is an on-chain LEZ 0.4 program (`program/`), the `lezreg` CLI (`cli/`), a Rust SDK
(`sdk/rust`), a read-only JavaScript SDK (`sdk/js`) and a Basecamp core module
(`module/`, `program_registry`). A program's **deployer entry** lives on the program's own account
`X`, and only the authorization the loader requires to deploy or upgrade `X` can write it. A client
therefore identifies a deployer entry from its address alone, and one `getAccount(X)` read returns
both the entry and the live program header. That read also shows whether the entry is fresh or
stale. Anyone can publish a **third-party entry** at `T(program, publisher)`, and it can never take
the deployer entry's place. Entries join on-chain linked lists (global, by name, by author, and by
program for third-party entries), so lookup, enumeration and search need only the sequencer. IDLs
live on Logos Storage; the chain holds their CID, sha256 and size. Entries record a full build
tuple, and `lezreg verify` rebuilds it in Docker and compares the image id with the claimed one
and with the live one.

Its Basecamp module can also prepare and sign registry writes (register, third-party register,
update, transfer, freeze) on the user's behalf through the lezreg wallet, with approval required
for untrusted callers; the built change is decoded into a confirm card.

Everything has run end to end on a local LEZ `v0.4.0-rc3` standalone sequencer: in CI on every
push to `main`, and through `scripts/demo.sh`. Module reads and writes are covered by
`.github/workflows/module.yml` on the same local rc sequencer. Nothing has run on testnet 0.4 yet,
because it has not launched. None of the adoption criteria are met yet.

## Repository

- **Repo:** <https://github.com/jefdiesel/lez-program-registry> (MIT OR Apache-2.0:
  `LICENSE-MIT`, `LICENSE-APACHE`)
- **Demo:** `scripts/demo.sh`. The transcript of a real run is in [`docs/demo.log`](demo.log)
  (where it ran and against what: see its header and §"Demo run" below).
- **Module catalog:** `https://raw.githubusercontent.com/jefdiesel/logos-modules/main/logos-repo.json`
  (a fork of `logos-modules-release-base`, released with `logos-modules-release-action`).
- **Deployed program id on testnet 0.4:** none yet. Testnet 0.4 has not launched.

## Success Criteria Checklist

Step ids such as `F3.c` are rows in the summary table of `docs/demo.log`.

### Functionality

**1.** "**On-chain registry**: entry metadata is stored on-chain as a LEZ program built with the
SPEL framework, with a published IDL. Source code and the entry's own IDL live off-chain on Logos
Storage; on-chain storage is limited to metadata and pointers, mindful of gas cost."

- **Status: partly met.**
  - The metadata is on chain, and only pointers are on chain.
  - The IDL is published, on Logos Storage.
  - Source is off-chain, but on a git forge, **not on Logos Storage** (see design choice 2).
  - "Built with SPEL": see the SPEL section; that one is TBD by the conductor.
- **Evidence:**
  - `program/src/lib.rs` is the on-chain program, written in SPEL attribute syntax
    (`use spel::prelude::*`, `#[lez_program]`, `#[instruction]`, `#[account(...)]`).
    `program/Cargo.toml` maps `spel` to the in-repo `spel04-shim`.
  - The records are in `crates/types/src/records.rs`. `Entry` holds bounded strings plus
    `ContentRef {cid, sha256, size, peer_hint}` for the IDL, `SourceRef {repo, commit,
    docker_root, archive?}` and `BuildConfig`. No IDL or source bytes are on chain.
  - The published IDL is `idl/lez_program_registry.idl.json`. It is the output of the released
    `spel generate-idl` v0.7.0 (kept untouched in `idl/lez_program_registry.spel.json`), plus an
    `x-lez04` extension. `scripts/idl.sh --check` regenerates it, and the `idl` job of
    `.github/workflows/ci.yml` runs that check with spel pinned by commit.
  - Gas: the measured cycles, gas and fee of every instruction are in `docs/CYCLES.md` (LEZ rc3).
  - On chain, the IDL is uploaded to Storage by `chain/deploy.sh --idl-cid push`. Demo `F1.c`
    shows that `(REG, REG)` points at a Storage CID. Demo `F1.d` fetches it by that pointer,
    checks the hash, and finds it equal to the committed SPEL IDL.
  - **Caveat:** `chain/deploy.sh` without `--idl-cid push` records only a raw-codec CID of the
    IDL bytes and uploads nothing. The demo and `e2e.yml` both pass `push`.

**2.** "**Register**: any account may register a program it deployed, supplying at minimum a
**name**, **version**, **author**, **description**, an **IDL pointer**, a **code pointer**, and a
**registration timestamp**. Lookup of an entry must not depend on a centralized indexer."

- **Status: met on a local rc sequencer, pending testnet 0.4.**
- **Evidence:**
  - All the required fields are in `Entry`: `name`, `version`, a proven co-signing `author` plus
    an unproven `author_label`, `description`, `idl`, `source` plus `build`, and
    `registered_at`.
  - `registered_at` is in milliseconds and bound to the block time by a `[now, now + 10 min)`
    window. It is enforced by the program, and the engine test `timestamp_window_is_enforced`
    covers it.
  - There are three ways to register:
    - `lezreg deploy` (`DeployAndRegister`: deploys and registers in one transaction);
    - `lezreg register` (`RegisterDeployer` for an already deployed program);
    - `lezreg publish` (driven by `lezreg.toml`).
  - Demo `F2.a` and `F2.b`, plus `F3.f` for the RegisterDeployer path.
  - Lookup is one `getAccount` against the sequencer (`Registry::resolve`,
    `sdk/rust/src/read.rs`). There is no indexer in the repository.
  - Engine tests `t01`, `t08` and `t10` (`tools/engine-test/tests/engine.rs`, run by `ci.yml`).

**3.** "**Deployer vs third-party entries**: the registry may hold both an entry from the
program's own deployer and entries from third parties. The deployer's entry must be signed by the
account that deployed the program, and clients must be able to tell it apart from a third-party
entry without trusting an off-chain service. [...] The demo must show an unauthorised attempt to
create a deployer entry being rejected."

- **Status: met on a local rc sequencer, pending testnet 0.4.**
- **Evidence:**
  - The deployer entry is the registry's state at actor `(X, REG)`. Creating it requires
    `is_authorized` for `X`: the same credential the loader requires for `CreateHeader` and
    `UpdateHeader`. Every registry instruction must also be the transaction's root message, which
    closes the composed-transaction attack. See DESIGN §1 and §3; the code is `program/src/lib.rs`
    and `spel04-shim/`.
  - A third-party entry sits at a registry PDA `T(program, publisher)`.
  - A client tells the two apart by the address alone, with no off-chain service (demo `F3.b`).
  - **The unauthorised attempt is in the demo:**
    - `F3.c`: Mallory's `RegisterDeployer` for a program she did not deploy is refused in
      simulation ("must sign").
    - `F3.d`: forced on chain, it is reverted, and she pays her gas limit.
    - `F3.e`: the deployer slot is still empty afterwards.
    - `F3.g` and `F3.h`: a second deployer entry is refused, even when signed by the program's
      own key.
  - Engine tests `t02` (unauthorised and duplicate entries), `t03` (composed-transaction attacks),
    `t04`, `t09` and `t13` (PDA-owned headers).
  - **What "deployer" means here:** the 0.4 loader records no deployer. "The account that deployed
    the program" is taken to be whoever holds the authorization the loader requires to deploy or
    upgrade `X` (its header key, or the owning program for a PDA header), which is DESIGN §12 Q3,
    not yet confirmed by Logos.
  - **What the registry cannot check on chain:** it cannot read the loader's header, so it cannot
    check that a program exists at `X` or that the claimed image is the one running there.
    - A `RegisterDeployer` proves only that `X`'s key signed. Only a `DeployAndRegister` entry
      (`born_with_deploy`) proves co-creation on chain.
    - Missing programs (NO-PROGRAM) and stale images are flagged by the clients, from the same
      `getAccount` read (demo `X.c` to `X.e2`).
  - "Source verification should be used to confirm the claim": the entry's claimed `image_id` is
    compared with the live header on every read (fresh or stale, demo `X.c`/`X.d`), and
    `lezreg verify` rebuilds the source (criterion 7).

**4.** "**Update**: only the original registering author may update an existing entry (version,
IDL pointer, description). Authorship transfer is optional; if implemented, it must require
signatures from both parties."

- **Status: met on a local rc sequencer, pending testnet 0.4.**
- **Evidence:**
  - Demo `F4.a`: the author updates.
  - Demo `F4.b` and `F4.c`: a non-author is refused in simulation, and reverted when forced on
    chain.
  - Demo `F4.d` to `F4.g`: transfer, then the old author is refused and the new one can update.
  - Every update writes a write-once `Release` snapshot.
  - Engine test `t05` refuses a transfer without the new author's signature ("co-signer did not
    sign") and accepts it with both signatures. `t12` covers the same for third-party entries.
  - "Original registering author" is read as "the current author". After a transfer, the new
    author updates and the registrant is kept on the entry (`registrant`).
  - The CLI signs a transfer with both keys from one wallet. Signing in two places (building the
    transaction on one machine and co-signing on another) is not built (DESIGN §11 M2).

**5.** "**IDL publishing**: the IDL is stored on **Logos Storage**."

- **Status: met on a local rc sequencer with local Logos Storage v0.5.3 nodes, pending testnet
  0.4.**
- **Evidence:**
  - Upload: `lezreg idl push`, and `--idl-cid push` on register, update and deploy.
  - Fetch: `lezreg idl get`, with the fetch ladder of cache, then node, then retries, and a
    sha256 check before parsing (`sdk/rust/src/idl.rs`).
  - Pinning: `lezreg pin` and `lezreg pin-all`.
  - `e2e.yml`, step "IDL across nodes, pinning, degradation".
  - Demo `F5.a` to `F5.c`: the IDL is fetched through a second node, pinned, and fetched by a
    fresh node after the uploader stopped.
  - **Not tested:** a fetch across the public Storage network through relays only. All the runs
    used local nodes. DESIGN §11 M3 lists the relay-only fetch as not done.

**6.** "**Discovery**: look up an entry by program ID, enumerate the full set of registered
entries, and search by name and author, using only the submitted software."

- **Status: met on a local rc sequencer, pending testnet 0.4.**
- **Evidence:**
  - The commands are `lezreg show`, `lezreg list` (paged with `--after`), `lezreg search name`
    and `lezreg search author`. The SDK equivalents are `Registry::resolve`, `list`,
    `search_name` and `by_author`.
  - Demo `F6.a` to `F6.d`: a full enumeration by cursor finds exactly 4 of 4 entries.
  - Author search returns proven authors only. The label and tag searches cover free text.
  - Enumeration and search walk on-chain linked lists over `getAccount`, with no indexer.
  - Searches by label, tag and substring read at most `--scan` entries (default 1000) and say so
    when they stop early.

**7.** "**Source verification**: an entry must additionally record a pointer to source [...], a
commit, and a build configuration sufficient to reproduce the deployed bytecode. A user may choose
to rebuild the recorded source locally [...]; if the resulting byte code matches, the user can mark
that program as 'locally verified' for themselves."

- **Status: met in CI (Linux, Docker).**
  - The evaluator demo runs the rebuild in `.github/workflows/demo.yml` (`scripts/demo.sh
    --verify`; steps F7.b and F7.c, verdict Match).
  - In the macOS run recorded in `docs/demo.log` the rebuild was skipped, because there is no
    Docker on that machine.
- **Evidence:**
  - What the entry records: `SourceRef {repo, commit, docker_root, archive?}` and `BuildConfig`
    (builder image tag and digest, risc0-build version, host rust, manifest path, binary,
    features, `Cargo.lock` sha256, docker env). Demo `F7.a`.
  - What `lezreg verify <program> [--mark]` does (`cli/src/verify_cmd.rs`):
    - clones the recorded repo at the recorded commit;
    - rebuilds in Docker, with the network off for the compile;
    - compares the result with the claimed and the live image id (verdict `Match`, `MatchStale`,
      `Mismatch` or `NotReproducible`);
    - records a local mark with `--mark`. `lezreg verified` lists the marks, and the module's
      `program_label.verified` reports them.
  - **CI evidence:**
    - `e2e.yml`, step "lezreg verify REG ... says Match", green on `main` (LEZ rc3; see criterion 18 for the runs).
    - `verify.yml`: `lezreg build` reproduces `artifacts/`.
    - `repro.yml`: a stock Docker build on two runners gives byte-identical output.
  - **Limits:**
    - `verify` needs Docker with buildx. On Apple silicon the amd64 builder runs emulated (about
      8 GB of Docker memory).
    - It needs the git forge to still serve the commit. It cannot rebuild from a Storage archive:
      an archive-only entry is refused with "archive sources come with the source pinner".

**8.** "**Testnet 0.4**: the registry operates end-to-end against the Logos LEZ zone on Logos
testnet 0.4."

- **Status: not yet.** Testnet 0.4 has not launched. `lezreg`'s `testnet` network is a placeholder
  that refuses to run (demo `X.i`). `scripts/demo.sh --testnet --rpc … --registry …` is the
  read-only walk to run on launch day; until then it stops at once with a message. That walk has
  been exercised once, against a local rc3 chain standing in for the testnet
  (`docs/demo-testnet-mode.log`: 9 PASS, verify skipped for lack of Docker). The repo is
  pinned to LEZ `v0.4.0-rc3` (`bafd07be`), read by `scripts/demo.sh` from `e2e.yml` and
  `Cargo.toml`.

### Usability

**9.** "Provide a module/SDK usable by other Logos modules and third-party applications to
register entries, resolve a program ID to its metadata and IDL, and run searches."

- **Status: partly met.**
  - Rust SDK: all three (register, resolve, search).
  - JS SDK: resolve, list and search by name only (no search by author, no writes).
  - Logos Core module: resolve, list, search by name, **and prepare/sign writes** (register,
    third-party register, update, transfer, freeze) through `submit`; another Logos module can
    register, but its change waits for approval unless the caller is the registry browser or the
    host. Still no search by author, and the JS SDK has no writes.
- **Evidence:**
  - Rust SDK (`sdk/rust`):
    - reads: `Registry::resolve`, `list`, `search_name`, `by_author`, `third_party`;
    - IDL: `idl::fetch_idl`;
    - writes: `tx::Writer`, which every `lezreg` write goes through.
    - The worked example `sdk/rust/examples/resolve_program.rs` runs in `e2e.yml` and in demo
      `U1.a`.
  - JS SDK (`sdk/js/lezreg.js`, no dependencies): `resolve`, `programLabel`, `list`, `byName`,
    `fetchIdl`. It is checked against the Rust SDK with vectors (`node sdk/js/lezreg.test.mjs` in
    `ci.yml`). The demo runs it as `U1.b` in `demo.yml`. It was skipped in the macOS run in
    `docs/demo.log`, because that machine has no node.
  - Module `program_registry` (`module/`): `resolve_program`, `program_label`, `get_idl`,
    `list_entries`, `search_name`, `third_party_entries`, `verify_app`, `verify_package`,
    `publisher_apps`, `config_get`/`config_set`.
    - It also exposes `prepare_register`, `prepare_register_third_party`, `prepare_update`,
      `prepare_transfer`, `prepare_freeze`, `submit`/`submit_status`, `signer_status`,
      `pending_requests`, `approve_request`/`deny_request` and the `change_settled` event. The
      builders live in `module/rust-lib/src/writes.rs` and are pinned byte-for-byte to the
      program by `module/rust-lib/testdata/write-vectors.json` (`sdk/rust/tests/module_vectors.rs:205`,
      unit tests at `module/rust-lib/src/lib.rs:1109,1174,1206,1219,1270,1288`).
      `.github/workflows/module.yml:181-206` loads the module in logosctl on a local rc3 chain
      and registers/updates/freeses an entry through it, with an on-chain assertion — met on a
      local rc sequencer, pending testnet 0.4. No `docs/demo.log` row loads the module
      (`docs/demo.log:426-427` still says it has no write methods).
    - Its search is by name only: there is no search by author in the module.
    - The repo's module is 0.3.0 (`module/metadata.json:4`), but **the catalog still serves 0.1.0**
      (released 2026-11-04, before the app methods), so the catalog build has none of the write
      methods.
    - A CI job does load it: `.github/workflows/module.yml:166-206` (logosctl install/load and
      reads, register/update/freeze, and the caller gate). It also ran by hand on the Mini
      (DESIGN §11 M6).

**10.** "Provide a CLI covering register, update, lookup, search, and source verification."

- **Status: met.** `lezreg` has `register`, `deploy`, `publish`, `adopt`, `update`, `transfer`,
  `freeze`, `show`, `list`, `search name|author|label|tag`, `idl get|push`, `pin`, `pin-all`,
  `verify`, `verified`, `build`, `stats` and `doctor`. Demo row `U2`, and `F7.b` for verify (which
  needs Docker).

### Reliability

**11.** "Clients degrade gracefully when an IDL identifier is unresolvable or the stored IDL fails
to parse: the entry's on-chain metadata still displays, with the IDL surfaced as unavailable
rather than the view failing."

- **Status: met in the SDKs and the module. The CLI only partly does this.**
- **Evidence:**
  - `fetch_idl` never fails. It returns a status instead: `IdlStatus::{Ok, UnknownFormat,
    Unavailable(..), HashMismatch, ParseError}`. Its unit tests are in `sdk/rust/src/idl.rs`.
  - The worked example prints the metadata, then `IDL unavailable (<reason>)`. Demo rows:
    - `R1.a`: no Storage node;
    - `R1.b`: a CID nobody holds;
    - `R1.c`: bytes that are not JSON, so they fail to parse;
    - `R1.c2`: valid JSON in no format the client knows, shown as "available, format not
      recognised" next to the metadata;
    - `R1.d`: the bytes do not match the on-chain sha256.
  - The module's `get_idl` answers `success: true` with `status: "unavailable: …"`. The JS
    `fetchIdl` returns a status.
  - **The CLI gap:**
    - `lezreg show` displays the metadata but never fetches the IDL. It cannot fail because of
      the IDL, and it does not show whether the IDL is available either.
    - `lezreg idl get` reports "IDL unavailable" and exits nonzero (`R1.b3`).
    - No single CLI view shows the metadata together with the IDL status.
  - `e2e.yml` covers only the "no Storage node" case. The other three cases are covered by
    `scripts/demo.sh`, not by CI. DESIGN §5.4 says e2e covers all of them; it does not.

**12.** "**No mandatory centralised external services.** Beyond the user's configured sequencer,
Logos LEZ node, and Logos Storage, the registry and its tooling must not depend on any third-party
web service to function. If one is used, it is disclosed, the user can disable it, and the registry
remains fully functional with it disabled. Any analytics must be strictly opt-in."

- **Status: partly met.** Register, update, lookup, enumeration, search and IDL resolution use
  only the sequencer and the Storage node. `lezreg verify` and `lezreg build` cannot run without
  third-party services, and they have no offline switches yet.
- **Evidence:**
  - There is no analytics code.
  - Reads refuse HTTP redirects (`max_redirects(0)`) and ignore `HTTP(S)_PROXY` unless
    `LEZREG_USE_PROXY=1` (`sdk/rust/src/rpc.rs`, `sdk/rust/src/storage.rs`).
  - The README records a network-isolation check (2026-11-02, macOS `sandbox-exec`): reads and
    writes worked with outbound traffic limited to loopback.
  - **The gap:**
    - `verify` and `build` need Docker Hub (the builder image), crates.io (`cargo fetch`) and the
      git forge (the clone). The user can disable them only by not verifying.
    - DESIGN §7.7's off switches (`--image-tar`, `--vendor-tar`, `--no-forge`, `--offline`) are
      **not implemented**.
    - The README does not list these services in one place.
    - The GitHub Action and `publish --from-release` use GitHub. Both are optional: the CLI does
      the same work locally.

### Performance

**13.** "Document cycle count usage for each on-chain operation (register, update)."

- **Status: met.**
- **Evidence:**
  - [`docs/CYCLES.md`](CYCLES.md) has the measured cycles, gas and fee of every registry operation
    on LEZ rc3. It covers `RegisterDeployer`, `RegisterThirdParty`, `Update`, `TransferAuthor`,
    `Freeze`, `DeployAndRegister` at four program sizes, and the loader's `WriteSegment`,
    `CreateHeader` and `UpdateHeader`.
    - The numbers are taken with LEZ `cycle_bench`'s method (`tools/engine-test/src/bench.rs`).
    - They are reproduced with LEZ's own `cycle_bench`, using the patch in `tools/cycle-bench/`.
  - CI pins them: `tools/engine-test/tests/cycles.rs` asserts the documented numbers exactly, and
    runs in the "Engine tests" step of `ci.yml`.
  - The recorded demo run agrees with CYCLES.md's ranges, measured independently by lezreg's
    pre-send simulation (`docs/demo.log`):
    - `RegisterDeployer`: 173,982 and 174,451 cycles;
    - `RegisterThirdParty`: 240,222 and 239,816 cycles;
    - `Update`: 90,190–94,023 cycles;
    - `TransferAuthor`: 99,821 cycles;
    - `Freeze`: 45,700 cycles.
  - Not measured: proving cycles and proving time. Public transactions are not proven.

### Supportability

**14.** "Deployed and tested against Logos testnet 0.4 (the canonical Logos LEZ zone)."

- **Status: not yet.** Testnet 0.4 has not launched.

**15.** "Platform support (recommended, not a separate build gate): [...] Linux and macOS, and
from testnet 0.4 also Windows and Android."

- **Status: partly met (Linux and macOS).**
  - Linux: `ci.yml`, `e2e.yml`, `verify.yml` and `repro.yml` all run on `ubuntu-24.04`.
  - macOS (Apple silicon): `lezreg` is built and the demo was run on a Mac mini (`docs/demo.log`).
    The catalog has darwin-arm64, linux-amd64 and linux-arm64 variants of the module.
  - No CI job runs on macOS.
  - Windows and Android are not built.

**16.** "The Logos Core Module is built with `logos-module-builder` (`mkLogosModule`)."

- **Status: met.**
  - `module/flake.nix` calls `logos-module-builder.lib.mkLogosModule`, with the builder pinned by
    rev (`de169fd`).
  - It is published in the catalog `jefdiesel/logos-modules`, as `program_registry` 0.1.0.

**17.** "Provide an IDL for the registry program itself, generated via the SPEL framework: the
registry must be registered in itself."

- **Status: met on a local rc sequencer, pending testnet 0.4.** The SPEL wording carries the same
  caveat as criterion 1 (see the SPEL section).
- **Evidence:**
  - The IDL is in `idl/`, generated as described in criterion 1.
  - Self-registration: `chain/deploy.sh` sends `CreateHeader`, then `RegisterDeployer` at
    `(REG, REG)`.
  - Engine test `t10`, `e2e.yml` step "Deploy the registry …", and demo `F1.a` to `F1.c`.

**18.** "End-to-end integration tests run against a real sequencer (standalone mode) and are
included in CI; CI is green on the default branch."

- **Status: met.**
  - `.github/workflows/e2e.yml` builds `sequencer_service --features standalone` at the pinned
    LEZ tag and starts a fresh chain and Logos Storage v0.5.3 nodes. It then deploys from
    `artifacts/`, runs the SDK example, the IDL steps, the adoption tools and `verify` (expecting
    Match), and the GitHub Action.
  - All four workflows (`ci`, `repro`, `verify`, `e2e`) are green on `main` at `74a0010` (LEZ
    rc3 plus the IDL-format work), 2026-11-13: runs 43450763487, 43450763482, 43450763645 and
    43450763571.
  - `.github/workflows/demo.yml` runs the evaluator demo the way an evaluator would: a clean
    ubuntu runner, a fresh checkout, only the README's prerequisites installed, then
    `scripts/demo.sh --verify`.
    - It runs on dispatch, and on pushes to `main` that touch the demo, the CLI, the SDKs, the
      program or `artifacts/`.
    - It fails on any FAIL, and on any SKIP other than F8 (testnet).
    - It restores e2e's sequencer build from cache instead of rebuilding it.
  - On `5e3dac5` (docs/CYCLES.md, now `main`), `ci` is green (43473019998). `e2e`, `verify` and
    `repro` were still running when this file was written (43473020169, 43473020152,
    43473020395).
  - The Rust e2e test `sdk/rust/tests/e2e.rs` skips in CI unless `LEZREG_E2E_RPC` is set. The
    e2e coverage in CI is the workflow, not that test.

**19.** "README documents setup, the deployed program ID, and step-by-step usage via CLI and
Logos Core Module."

- **Status: partly met.**
  - Setup and CLI usage are documented step by step in `README.md`.
  - The deployed program id is missing: testnet 0.4 has not launched.
  - Module usage in the README is one paragraph. The step-by-step module walk-through is in
    `docs/sdk.md` §3.

**20.** "SDK documentation covers resolving a program ID to metadata and IDL as a worked example."

- **Status: met.** `docs/sdk.md` covers it for Rust, JS and the module. The Rust example
  (`sdk/rust/examples/resolve_program.rs`) runs in `e2e.yml` ("SDK worked example") and in demo
  `U1.a`.

### Adoption

**21–27.** The adoption criteria:
- 5 independent third-party Logos UI modules;
- 50 registered programs from at least 20 independent authors;
- 20 verified source claims;
- at least 2 months with at least 10 registrations a month;
- 20 Discord testimonials;
- 20 Twitter/X testimonials;
- macOS coverage in the adoption evidence.

- **Status: not yet, all of them.** They need testnet 0.4, which has not launched.
- The tooling built for them:
  - `lezreg publish`, `adopt` and `init --detect`;
  - the GitHub Action (`action/`, with `verify` and `prepare` modes);
  - `lezreg stats --by-month --by-author --exclude-keys`, which builds the per-month evidence
    table from chain data.
- No third-party integration exists yet. Names Wallet uses the module on its approval sheet, but
  it is our own app and does not count toward the five.

## Additional design choices left to the submitter

**What lives on-chain?**
- **Our position:** a deployer or third-party `Entry` that holds:
  - metadata (name, version, label, description, tags);
  - the proven `author` and the original `registrant`;
  - pointers (IDL: CID, sha256 and size; source: repo, commit and an optional archive CID);
  - the whole build tuple;
  - the claimed `image_id`;
  - timestamps and a revision.
- Each revision also leaves a write-once `Release` with the version, the pointers and the build
  tuple, so that every past claim can still be rebuilt.
- Small `Link` and `Head` records make up the lists for enumeration and search.
- No IDL or source bytes are on chain.
- **Why:**
  - The build tuple is on chain so that an evaluator can rebuild from chain data plus the source
    alone, without trusting a metadata service.
  - The lists are on chain because the spec forbids depending on a centralised indexer, and the
    sequencer has no accounts-by-program query.
- **Gas cost, measured on LEZ rc3 (`docs/CYCLES.md` has every operation, including deploys):**
  - a deployer registration is about 174k cycles and 1.4–1.6 KB of transaction;
  - a third-party registration is about 240k cycles;
  - an update is about 90–94k cycles.
  - Records do not grow with history, because the lists use fixed-size heads and write-once
    links.

**How is verified source hosted?**
- **As built:** a git repository URL plus a commit, on the author's forge. Optionally the entry
  can also record an archive pointer (`--archive-cid`, a CID plus sha256 on Logos Storage).
- **What is missing:**
  - No tool creates or uploads that archive.
  - `lezreg verify` cannot rebuild from it. It clones the repo, and checks the archive sha256 only
    when one is recorded.
- **So we have, for now, the weakness the spec names:** if the repository is deleted or
  force-pushed, re-verification becomes impossible.
- **The intended position (DESIGN §4.4):**
  - both a forge link and a reproducible `git archive` on Logos Storage, with the archive
    authoritative;
  - **not built yet**, and source archives larger than about 128 KiB also need a directly
    reachable Storage node, which we do not run.

**Is there a trusted-signer overlay?**
- **Our position: none.**
- Only the user's own local marks (`lezreg verify --mark`) count as "verified". Nothing ships
  pre-trusted, and nothing ranks one third-party verifier above another.
- **Why:** the deployer's on-chain claim plus the user's own rebuild needs no central trust root.
- A user-configurable attestation list (DESIGN §4.5: an `Attest` instruction and a `trust.toml`)
  is planned for after launch and is not built.

## SPEL (TBD by the conductor)

The facts, without a conclusion:
- The prize asks for a program "built with the SPEL framework" and an IDL "generated via the SPEL
  framework".
- SPEL has no release for LEZ 0.4. Its released v0.7.0 targets LEZ 0.2.x.
- `program/src/lib.rs` is written in SPEL attribute syntax. It is compiled for LEZ 0.4 by
  `spel04-shim/` (in this repo). The shim implements SPEL's macros against 0.4's actor model and
  adds conventions SPEL does not define: `<name>_id` account-id arguments, `#[effect]` functions,
  and the instruction and effect tag layout (DESIGN §6.2, §6.5).
- The IDL is produced by the released, unmodified `spel generate-idl` v0.7.0 over that source.
  An added `x-lez04` block carries what the SPEL 0.1.0 IDL format cannot express.
- Transactions are encoded by `lezreg` and the SDKs (borsh), not by the stock `spel` CLI.
- Upstream SPEL PR #301 ("Refactor/lez v0.4.x", draft, opened 2026-11-10) is under evaluation on
  another branch. Whether to move to it, and how to describe this criterion, is **TBD by the
  conductor**.

## Open

- **Testnet 0.4:**
  - deploy with `chain/deploy.sh --idl-cid push`;
  - publish the registry program id in the README;
  - run `scripts/demo.sh --testnet`;
  - add a scheduled read-only testnet job (DESIGN §8.3 `testnet.yml`, which does not exist).
- **LEZ version:** the repo is on `v0.4.0-rc3`. Whatever testnet 0.4 launches with may need
  another re-pin.
- **Adoption evidence:** all seven adoption bullets.
- **SPEL 0.4:** see above (TBD).
- **Source hosting:**
  - make the reproducible source archive and upload it to Storage;
  - make `verify` able to rebuild from it.
- **No mandatory services:**
  - offline inputs for `verify` (a builder image tar, vendored crates, no forge);
  - one README list of the services `verify` uses.
- **CLI degradation:** a CLI view that shows the metadata plus the IDL status. Today `show` never
  fetches the IDL, and `idl get` fails when the IDL is unavailable.
- **Module:**
  - a demo/FURPS row for module writes: unit tests and `.github/workflows/module.yml` show
    register/update/freeze on a local rc chain, but `docs/demo.log:426-427` still says the module
    has no write methods;
  - release 0.3.0 (with the write methods) to the catalog, which serves 0.1.0;
  - load the module in the real Basecamp (so far only logosctl on CI/Mini) and add the lez_core
    signer once a usable release exists.
- **Storage:** an IDL fetch across the public Storage network through relays only.
- **Platforms:** Windows and Android builds; macOS in CI.
- **Tooling** listed in DESIGN but not built: `lezreg idl gen` and `idl check`, the Action's
  `publish` mode, and two-machine signing for transfers.

## Demo run

`scripts/demo.sh` is the evaluator demo. It needs one command from a clean clone (README, "Run the
demo").
- **On Linux, in CI** (`demo.yml`): every step passes except F8 (testnet). That includes the Docker
  rebuild and the JS SDK step. The transcript is the run's `demo-transcript` artifact.
- **On macOS**: the run recorded in `docs/demo.log` is described in that file's header and
  summary table:
- where it ran;
- which sequencer binary it used;
- which steps were skipped, and why.

## Terms & Conditions

By submitting this solution, I confirm that I have read and agree to the
[Terms & Conditions](https://github.com/logos-co/lambda-prize/blob/master/TERMS.md).
