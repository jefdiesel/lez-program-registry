# LEZ Program Registry: final design

Status: final design, 2026-11-02. Target: λPrize LP-0023, LEZ testnet 0.4. The registry is built and tested against LEZ `v0.4.0-rc1` (`1806e873`) on a local standalone sequencer, and deployed to testnet 0.4 on the day it launches.

**Revised 2026-11-02 after M0 spikes (a) and (d).** Findings F1–F5 (`spikes/spel-idl/RESULT.md`) and N5 plus the measured fees (`spikes/d-deploy-and-register/RESULT.md`) are folded in below. M0 (b) and (c) are replaced by GitHub Actions reproducible builds, and (e) is deferred (§11).

**Names.**
- Product: **LEZ Program Registry**.
- On-chain program: `lez_program_registry`.
- CLI: **`lezreg`**. We avoid `lpr` because it is the CUPS/BSD print command on every macOS and Linux machine.
- Logos core module: `program_registry`.
- Reference UI module: `registry_browser`.
- JS package: `@lezreg/sdk`.
- Project config: `lezreg.toml`.

**Path conventions.**
- `lez:` = `~/logos/upstream/lez-0.4-rc1`.
- `LNS/` = `~/logos/logos-names-0.4` (branch `lez-0.4`).
- `SPEL/` = SPEL `main` `c6f73ce` (clone at `scratchpad/src/spel-main`).
- `risc0-build/` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/risc0-build-3.0.5`.

Every LEZ mechanic is cited by file. Anything we have not checked in source or on a running chain is marked **(unverified)** or **(estimate)**.

**How this document was made.** It merges three candidate designs:
- **"trust"**: the base. It won with all three judges.
- **"adoption"**: we took its tooling and `DeployAndRegister`.
- **"lean"**: we took the single-read resolve and the "easiest way to redeploy on 0.4" positioning.

Appendix C lists every fatal flaw the judges raised and how it was fixed.

---

## 1. Summary

A registry entry is the **actor state of the registry program at an account**. That one fact, plus how 0.4 authorization works, gives most of the trust model.

- **Deployer entry for program `X`.**
  - It lives at actor `(X, REG)`, where `X` is the program's header account.
  - Creating it requires `input.is_authorized` for `X`. That is exactly the credential the loader requires for `CreateHeader`/`UpdateHeader` (`lez:lez/programs/program_loader/core/src/lib.rs:73-101`). So only the party that can deploy or upgrade the program can write its deployer entry.
  - A client recognises a deployer entry by its **address alone**.
  - **One `getAccount(X)` call** returns the loader header and the deployer entry together, so resolving a program costs one RPC.
- **Third-party entries.**
  - Each sits at a registry PDA keyed by `(program, publisher)`, at most one per pair.
  - A third-party entry can never occupy the deployer address.
- **Root-only instructions.**
  - In 0.4 a root signature authorizes the signer in *every* program a transaction reaches (`execution_state.rs:1028`, `1431-1470`; test `execution_state/tests.rs:706`).
  - So every registry instruction also requires `input.from.is_none()`: it must be the transaction's own root message.
  - This closes the attack where a malicious program in a composed transaction rewrites your entry.
  - One narrow exception: a header that is a PDA of program `P`. There the guest recomputes `for_public_pda(P, seed) == X`, and **every signer must be `X` itself** (M1 review F1): on that path a co-signer's `is_authorized` only proves it signed *some* transaction that reached `P`, not that it agreed to this registry call. So a PDA-owned entry's author is the header, and it can't be transferred.
- **Proven authors.**
  - `author` is an account that co-signs at registration, in a transaction whose root message is that registry instruction. It may be `X` itself, so a single-key path exists. (On the PDA-header path the author must be `X`, see above.)
  - Search by author, and the "20 distinct author accounts" count, are therefore facts on chain.
  - The free-text `author_label` is shown as unproven.
  - Only the current `author` can update. Transfer needs both signatures.
- **Race-free discovery with no indexer.**
  - The program keeps intrusive linked lists in its own PDAs: global, per name, per author, and third-party entries per program.
  - **Every address a transaction touches is a pure function of its arguments.** There are no client-predicted counters or pages, so concurrent registrations never race and nobody can grief anyone into a charged revert.
  - Enumeration and search use only `getAccountView`/`getAccount`.
- **No client-side admission evidence.**
  - The registry admits its own PDAs by sending effects `.with_pda_seeds(..)` (`execution_state.rs:1190-1210`, verified).
  - Signers are admitted automatically (`validated_state_diff/mod.rs:579-591`, verified).
  - A client only *declares* actors. Any generic 0.4 wallet that can declare actors can sign registry transactions.
- **SPEL-compatible addresses.** PDA seeds follow SPEL's own `compute_pda` rule (`SPEL/spel-framework-core/src/pda.rs`) and use only const, account, string and `[u8;32]` components (no integer seeds, F3), so the released `spel` CLI's resolver and any future SPEL 0.4 client derive the same addresses. Transaction encoding is ours: the released `spel` CLI encodes risc0-serde words, not borsh (F2), and can't take struct args (F4), so `lezreg` and the SDKs are the clients.
- **Verification is keyed by `image_id`.**
  - The entry stores the claimed `image_id` and a complete build tuple: builder image **digest**, risc0-build version, host rzup Rust version, `Cargo.lock` sha256, docker root, manifest, binary, features, commit, and the source-archive CID + sha256.
  - Every revision is snapshotted in a write-once `Release`, so any past claim stays rebuildable.
  - Verification rebuilds the recorded tuple with the exact `risc0-build` Dockerfile, the image pinned by digest, and `--network=none` on the compile step.
  - It compares the result against both the claimed and the live `image_id`: verdicts `Match`, `MatchStale`, `Mismatch`, `NeedsNetwork` or `NotReproducible`.
  - Local "verified" marks are keyed by `(X, image_id, build_hash, source_sha256)`, so they lapse on their own.
- **`DeployAndRegister`.** The registry sends `CreateHeader` to the loader in the same transaction that writes the deployer entry. The entry then carries `born_with_deploy`, and the "lost header key" failure mode disappears. Proven on the rc1 sequencer in spike (d) with a 427 KB guest.
- **IDL.**
  - It is SPEL spec-0.1.0 JSON, produced by the **released, unmodified `spel generate-idl`** over the **real program source**.
  - That source is written in SPEL attribute syntax and compiled for 0.4 by an in-repo `spel04-shim`.
  - Every declared actor appears in the IDL's `accounts[]` with SPEL `pda.seeds`.
  - An `x-lez04` block carries what spec 0.1.0 cannot express; `lezreg-idl-extend` also fills `discriminator`/`spec`/`metadata`, which the v0.7.0 generator leaves null (F1b).
  - The IDL lives on Logos Storage. The chain holds its CID, sha256 and size.
- **Adoption tooling.**
  - `lezreg publish`: one command to build, deploy-and-register or update, and self-verify. It is marketed as *the easiest way to redeploy on 0.4*.
  - `lezreg adopt` for programs that are already deployed.
  - `lezreg idl gen --from enum|spel` and `lezreg idl check` for the many 0.4 programs that have no IDL.
  - A GitHub Action with `verify` / `prepare` / `publish` modes plus `lezreg publish --from-release`. The deployer key never enters CI.
  - `lezreg stats --by-month --exclude-keys --group-by-image`.
  - Module calls `program_label` and `decode_instruction`, plus a copy-paste `RegistryLabel.qml`.
- **No server we run is ever on a client's path.**
  - Clients talk to the user's sequencer and Storage node.
  - The docs site, explorer and snapshot are static files with zero marginal cost (§7.7).
  - No analytics.

---

## 2. On-chain program

### 2.1 Mechanics this design relies on (verified in source)

| Mechanic | Source |
|---|---|
| An actor is `(account_id, program_account_id)`. Only the program's own handler writes that actor's state (`Response::set_state`) | `lee/state_machine/core/src/account.rs:194`; `program/mod.rs:428-512` |
| `ReceiveInput{receiver, from, is_authorized, pre_state, message}`. `from` is `None` for the transaction's own root message | `program/mod.rs:373-393` |
| `from_own_program()` only checks `from.program_account_id == receiver.program_account_id` (any actor of REG) | `program/mod.rs:390-393` |
| `is_authorized = credential ‖ grant`:<br>• `credential` is the account-level signer flag (`authorized_accounts`, i.e. the signers);<br>• `grant` covers the sender's `pda_seeds` with `for_public_pda(issuer, seed) == account`, plus inherited grants | `execution_state.rs:1020-1030`, `1431-1470`; test `public_transaction/transaction.rs:237` |
| A root-signed account is authorized in every delivery from any origin | `execution_state/tests.rs:706` |
| An async delivery has `from = Some(original sender)`. Only the sync root has `from = None` | `execution_state.rs:1086-1094`, `1109-1117` |
| Sync messages run depth-first in send order | `execution_state.rs:1400-1401` |
| **Admission.** A public target is admitted if it is already admitted, **or the issuer's `pda_seeds` derive it (`seeded`)**, or it exists, or it is in the evidence set | `execution_state.rs:1190-1210`; `validated_state_diff/public_backend.rs:104-107` |
| **Signers are always in the evidence set** (`evidence(signers, admission_evidence)`). A fresh signer account, such as a new header key or a new author, needs no `Key` evidence | `lee/state_machine/src/validated_state_diff/mod.rs:229-234`, `579-591` |
| Every actor reached must be **declared** in `public_context.actors`, otherwise `UndeclaredActor`. Self-admission removes evidence, not declaration. Declaring an actor that is never touched is legal (there is no unused-actor check) | `execution_state.rs:1440-1444`; `validated_state_diff/mod.rs:261-264` |
| The root actor only has to be declared; it need not sign. `check_signers` requires `nonces == signers == authorized_accounts` and nothing about the root | `validated_state_diff/mod.rs:261-264`; `transaction_message.rs:74-86` |
| `write_once` accepts **identical** data, so creation paths must assert `pre_state.is_empty()` | `program/mod.rs:754-761` |
| Loader at `[0xFE;32]` (`JAQ5MVHb…mcvZ`):<br>• `CreateHeader`/`UpdateHeader` assert `is_authorized`;<br>• `image_id` is recomputed from the segment chain (≤ 20 segments, V1COMPAT);<br>• `immutable` is enforced;<br>• there is no read or reply message | `program_loader/core/src/lib.rs:26-40`, `73-101`, `141-168`; `program/mod.rs:29` |
| `ProgramHeader{image_id, program_first_segment, immutable}`, borsh, 65 B. The program account id is chosen by the deployer, and one image can sit at many headers | `program/mod.rs:511-538`; kill-check (two headers, one `image_id 6f7b8745…`) |
| **The loader runs as host Rust, not a guest ELF, and is not cycle-metered** ("there is no session to charge"). `CreateHeader`'s `compute_image_id` over all segments costs no execution gas; only the transaction bytes count. **Confirmed on chain** (spike d): a standalone `CreateHeader` over 427 KB of segments simulated at 0 cycles | `lee/state_machine/src/validated_state_diff/public_backend.rs:68-77` |
| Seed-grant family bindings start from **private witnesses only**, so declaring public REG PDAs that REG then seed-grants does not trip `PublicFamilyMemberDeclared` | `execution_state.rs:990-1005`, `1033-1044` |
| A guest can sync-send `CreateHeader` to `(header, LOADER)`. The new program is dispatchable later in the same transaction. Only `[deployer root, header, deployed]` are declared; segment accounts are not. **Confirmed on the rc1 sequencer** (spike d, T1: only `(X, REG)` and `(X, LOADER)` declared) | `lee/state_machine/src/state/tests/deploy.rs:511-551` |
| `for_public_pda(program, seed) = sha256("/LEE/v0.2/AccountId/PDA/"‖pad ‖ program ‖ seed)` | `program/mod.rs:150-168` |
| Events: `ProgramEvent{selector:[u8;8], data}`, conventionally `sha256("<program>::<Event>")[..8]` | `program/mod.rs:702-711` |
| Time: `Response::timestamp_window`, intersected across the transaction and enforced by the sequencer | `program/mod.rs:489-503` |
| Sequencer RPC has `getAccount(id)` (the whole account with all actor states) and `getAccountView(Actor)` (one actor). There is no accounts-by-program and no `getEvents` | `lez/sequencer/service/rpc/src/lib.rs:94`, `135`; `lez/sequencer/actors/executor/src/actor.rs:432-470` |
| The indexer RPC (needs a Bedrock node) has `getEvents` and `getTransactionsByAccount` | `lez/indexer/service/rpc/src/lib.rs:98-120` |
| A failed public transaction is a **charged revert**, and it is charged the **whole declared gas limit**, not the cycles used (spike d: every negative with a 100k limit paid 800,000 + bytes × 8). A transaction whose root program is not deployed is never included and costs nothing (spike d, N5) | `lez/chain_state/src/apply.rs:426-464`, `555` |

### 2.2 Accounts and actors

`REG` is the registry's program account id, which is its header account.

**PDA rule (SPEL's).** Each seed component is 32 bytes:
- strings are UTF-8, zero-padded and **at most 32 bytes**;
- account ids and `[u8;32]` are used raw;
- `u32`/`u64` are little-endian and zero-padded.

**No integer `arg` seeds (F3).** SPEL's two PDA rules disagree on integers: `ToSeed` (on-chain side, above) is little-endian left-aligned, while the released v0.7.0 CLI resolver (`spel-cli/src/pda.rs`) is big-endian right-aligned for u64/u128 and rejects u32. Const, account, string and `[u8;32]` seeds match exactly (spike a). So every `pda = [...]` here uses only those, and the shim makes an integer `arg` seed a compile error.

One component is used as the `PdaSeed` directly. Several are combined as `sha256(c1‖c2‖…)` (`SPEL/spel-framework-core/src/pda.rs` `ToSeed`, `seed_from_str`, `compute_pda`). The address is `AccountId::for_public_pda(REG, seed)`.

Domain strings are versioned and at most 32 bytes. Because `name` is a seed component, `Entry.name` is capped at 32 bytes.

| Actor | Address (SPEL seed notation) | Holds | Written by |
|---|---|---|---|
| Deployer entry | `(X, REG)`, where X is the header account | `Entry` (kind 0) | root `RegisterDeployer`/`DeployAndRegister`; `PatchEntry` |
| Third-party entry | `T = pda[const "lezreg/v1/tp", arg program, account publisher]` | `Entry` (kind 1) | `Effect::WriteEntry`, `PatchEntry` |
| Release | `pda[const "lezreg/v1/rel", account entry, arg revision_seed:[u8;32]]`; the guest asserts `revision_seed == revision.to_le_bytes()` zero-padded to 32 B (the same address `ToSeed(u32)` would give) | `Release`, a full snapshot of one revision | `Effect::WriteRelease` |
| Link | `pda[const "lezreg/v1/link", account entry]` | `Link`: list pointers and `seq` | `Effect::WriteLink` |
| Global head | `pda[const "lezreg/v1/head"]` | `Head{count, newest}` | `Effect::Index` |
| Name head | `pda[const "lezreg/v1/name", arg name]` | `Head` | `Effect::IndexName` |
| Author head | `pda[const "lezreg/v1/author", account author]` | `Head` | `Effect::IndexAuthor` |
| Program third-party head | `pda[const "lezreg/v1/tphead", arg program]` | `Head` | `Effect::IndexProgram` |
| Author node (after a transfer) | `pda[const "lezreg/v1/anode", account new_author, account entry]` | `AuthorNode{prev}` | `Effect::WriteAuthorNode` |
| Signer actor | `(A, REG)` for a signer `A` | nothing new. It is the root of author-side instructions and the target of the `RequireSigner` probe (`keep_state`) | own handler |
| *Post-launch:* attestation | `pda[const "lezreg/v1/att", account entry, account attester]` | `Attestation` | `Effect::WriteAttestation` |
| *Post-launch:* attestation head | `pda[const "lezreg/v1/atthead", account entry]` | `Head` | `Effect::IndexAttestation` |

`E` (the "entry" account) is `X` for a deployer entry and `T` for a third-party entry.

There is no settings account, no admin, no fee and no moderation. The registry's only privileged key is its own upgrade key (§10).

The registry registers itself through a deployer entry at `(REG, REG)`, created at launch by **`CreateHeader`, then `RegisterDeployer`** (two transactions). `DeployAndRegister` can't do it: its root `(REG, REG)` needs REG's code already deployed, and spike (d) N5 showed such a transaction is dropped as `Unknown program` (never included, no fee). The self-entry therefore has no `born_with_deploy`.

**Signer actor collisions are harmless.** `(A, REG)` can also be `A`'s own deployer entry if `A` is itself a program header. Root handlers at a signer actor never write their own state, except when that actor *is* the entry being changed (§2.4, "inline patch").

### 2.3 Records

Records are borsh, and every record starts with `schema: u8 = 1`. Limits are enforced in the guest. Size notation: `String`/`Vec` cost 4 + n bytes, and `Option` costs 1 + inner.

```rust
#[account_type] pub struct Entry {
    schema: u8,                 // 1
    kind: u8,                   // 1   0 = Deployer, 1 = ThirdParty
    flags: u8,                  // 1   bit0 frozen, bit1 born_with_deploy
    program: AccountId,         // 32  X
    author: AccountId,          // 32  proven co-signer; the only account allowed to update
    registrant: AccountId,      // 32  original author (kept after a transfer, for audit)
    name: String,               // 4+1..=32  [a-z0-9][a-z0-9._-]*  (immutable; also a PDA seed)
    version: String,            // 4+1..=32  printable ASCII, semver recommended
    author_label: String,       // 4+0..=64  UTF-8, UNPROVEN display name
    description: String,        // 4+0..=512 UTF-8
    tags: Vec<String>,          // 4+≤8×(4+1..=24) [a-z0-9-]
    idl: ContentRef,
    source: SourceRef,
    build: BuildConfig,
    image_id: [u8; 32],         // 32  claimed; equals ProgramHeader.image_id when fresh
    registered_at: u64,         // 8   ms, bound by a timestamp window (§2.6)
    updated_at: u64,            // 8
    revision: u32,              // 4   1 at registration
}
pub struct ContentRef {        // plain borsh struct (no #[account_type]): the IDL lists it under types[]
    cid: Vec<u8>,               // 4+≤64  binary CIDv1 of the Storage manifest (typ. 38 B: 0x01, varint 0xCD01, 0x12 0x20, 32 B)
    sha256: [u8; 32],           // content hash (independent of filename/mimetype)
    size: u32,
    peer_hint: Option<Vec<u8>>, // 1+(4+≤48) libp2p peer id of a pinning node (optional)
}
pub struct SourceRef {
    repo: String,               // 4+≤160  https:// or git URL (may be empty if archive is set)
    commit: Vec<u8>,            // 4+20 (sha1) or 4+32 (sha256 object format)
    docker_root: String,        // 4+≤96   build context path inside the repo
    archive: Option<ContentRef>,// deterministic `git archive` of docker_root at commit, on Storage
}
pub struct BuildConfig {
    schema: u8,
    builder: u8,                // 1 = risc0-build Docker (`cargo risczero build`)
    image_digest: [u8; 32],     // sha256 digest of risczero/risc0-guest-builder (authoritative)
    image_tag: String,          // 4+≤48 e.g. "r0.1.91.1" (informational)
    risc0_build: String,        // 4+≤16 e.g. "3.0.5"  (writes the Dockerfile)
    host_risc0_rust: String,    // 4+≤16 rzup Rust version (changes RUSTFLAGS, §4.1)
    manifest_path: String,      // 4+≤96 guest Cargo.toml relative to docker_root
    binary: String,             // 4+≤48 bin target name
    features: Vec<String>,      // 4+≤4×(4+≤32)
    lock_sha256: [u8; 32],      // sha256 of the Cargo.lock the guest build uses
    docker_env: Vec<EnvVar>,    // 4+≤4×(8+key+value), key 1.., key+value ≤ 64; EnvVar{key: String, value: String}: extra ENV lines risc0-build emits from DockerOptions::env (docker.rs ~165); usually empty
}
#[account_type] pub struct Release {   // write-once, one per revision
    schema: u8, entry: AccountId, revision: u32, version: String,
    idl: ContentRef, source: SourceRef, build: BuildConfig, image_id: [u8; 32],
    build_hash: [u8; 32],       // sha256(borsh(BuildConfig))
    at: u64,
}
#[account_type] pub struct Link {      // one per entry, created with it
    schema: u8, seq: u64, kind: u8, program: AccountId, name: String,
    author: AccountId,          // registrant: tells an author-list walker where prev lives
    prev_global: Option<AccountId>, prev_name: Option<AccountId>,
    prev_author: Option<AccountId>, prev_program: Option<AccountId>,   // prev_program: third-party only
}
#[account_type] pub struct Head { schema: u8, count: u64, newest: Option<AccountId> }   // 42 B
#[account_type] pub struct AuthorNode { schema: u8, entry: AccountId, prev: Option<AccountId> }
```

**Sizes (measured, pinned by `record_sizes_match_the_design_table` in `program/tests/handlers.rs`).** "Typical" is a 13-character name, a 1-sentence description, 2 tags, a 38-byte CID and a source archive; "worst case" is every field at its limit.

| Record | Typical | Worst case |
|---|---|---|
| `Entry` | 674 B | 2,400 B |
| `Release` (full snapshot) | 503 B | 1,510 B |
| `Link` | 223 B (all four `prev` set) | |
| `Head` | 42 B | |
| `AuthorNode` | 66 B | |

Actor state has no fixed cap in 0.4 (`lee/state_machine/core/src/account/actor_state.rs`, test `state_encoding_matches_bytes_beyond_the_former_limit`). Our caps exist to bound cycles and storage gas.

**On chain on purpose:** the whole build tuple (~250 B). A claim then stays rebuildable from chain data plus git while the repo exists, and from chain data plus Storage after the repo is gone. The prize names the deleted-GitHub-repo weakness. `Release` repeats the tuple so that every past revision stays rebuildable, including a header rolled back to an older image. That costs ~650 B written once per revision. We accept the cost because "verify any revision" is the property evaluators sample.

**Off chain on purpose:** the IDL, the source, and long docs, logos and links. These go in the IDL's `metadata` block on Storage.

### 2.4 Messages and instructions

**Wire format.** A root message is a bare `Instruction`:
- one `u8` variant index;
- then the borsh args.

This matches SPEL `main`'s borsh encoding (`ba8d7f1`, `serialize_to_borsh`, from the LEZ 0.2.5 port), which is **not released**. The released `spel` v0.7.0 CLI encodes risc0-serde u32 words (a u32 variant, then one u32 per u8) and can't take struct args (F2, F4), so a stock SPEL client does **not** produce valid registry transactions; `lezreg` and the SDKs carry their own IDL-driven borsh encoder. Internal effects share the same message type, under tags **`0xC0` and up**. The shim generates `Message`'s `BorshDeserialize` (no derive): a tag below the instruction count is an `Instruction`, a tag of `0xC0` or above is an `Effect`, and anything else panics. No `Message::Instruction(..)` wrapper byte exists, so a client generated from the IDL can't silently drop one. New `#[instruction]` fns are only **appended** at the end of the module, which keeps existing tags stable; the shim checks this against a committed tag list.

```rust
// GENERATED by spel04-shim from the #[instruction] fns: variants in fn order, fields in arg order (F1).
// There is no hand-written enum; wire tag order = IDL instruction order by construction.
pub enum Instruction {
    RegisterDeployer   { /* flattened Entry fields */, author_id: [u8;32], revision: u32, revision_seed: [u8;32], now: u64, via_pda_seed: Option<[u8;32]> }, // 0
    RegisterThirdParty { program: [u8; 32], /* flattened fields */, revision: u32, revision_seed: [u8;32], now: u64 },                                   // 1
    Update             { entry_id: [u8;32], revision: u32, revision_seed: [u8;32], /* Option<_> per patchable field */, now: u64, via_pda_seed: Option<[u8;32]> }, // 2
    TransferAuthor     { entry_id: [u8;32], new_author_id: [u8;32], now: u64, via_pda_seed: Option<[u8;32]> },                  // 3
    Freeze             { entry_id: [u8;32], now: u64, via_pda_seed: Option<[u8;32]> },                                          // 4
    DeployAndRegister  { first_segment: [u8;32], immutable: bool, /* RegisterDeployer fields, without via_pda_seed */ },     // 5
    // appended post-launch: Attest { entry, image_id, build_hash, source_sha256, verdict, tool, now }                          // 6
}
pub enum Effect {   // tags 0xC0.. (RequireSigner = 0xFF, not a variant), accepted only if input.from_own_program()
    WriteEntry{entry}, PatchEntry{patch, now /* the author is from.account_id */}, WriteRelease{release},
    Index{ctx}, IndexName{ctx}, IndexAuthor{ctx}, IndexProgram{ctx}, WriteLink{ctx},
    WriteAuthorNode{new_author, node}, IndexAuthorTransfer{entry, new_author},   // 0xC8, 0xC9 (wire-tags.lock)
    /* post-launch: */ WriteAttestation, IndexAttestation,
}
// ctx = IndexCtx{entry, kind, program, name, author (the registrant), image_id, idl_sha256,
//                born_with_deploy, seq, prev_global, prev_name, prev_author, prev_program}:
// the last three content fields only feed the Registered event.
```

**Arguments are flattened** (no `EntryInput` struct), and ids used as seeds are typed `[u8; 32]` (SPEL's `ToSeed` has impls for `[u8;32]`, `u32`, `u64` and `String`, not for `AccountId`). This has two benefits:
- SPEL's generator emits every argument type;
- SPEL `pda` seeds can reference `arg name` directly.

`idl`, `source` and `build` stay struct args (`ContentRef`/`SourceRef`/`BuildConfig`). The IDL describes them correctly in `types[]`, but the released `spel` CLI refuses struct args (F4); we accept that rather than flatten them, since `lezreg` is the real client.

**`<name>_id` pairing (F5).** A 0.4 guest sees only its receiver, so a non-root, non-PDA account (a co-signing `author`, `new_author`, or the target `entry`) can't be resolved from the transaction. The shim requires, at compile time, an arg `foo_id: [u8; 32]` for every such account `foo`. The IDL then lists `author` under `accounts` **and** `author_id` under `args` (Rust forbids two params with one name). `x-lez04` documents the pairing (§5.1).

Patchable fields: `version`, `description`, `tags`, `author_label`, `idl`, `source`, `build`, `image_id`. `name`, `program` and `kind` are immutable.

**`revision` is an explicit argument everywhere**, together with `revision_seed`. The `Release` PDA is keyed by `revision_seed` (a `[u8;32]`, F3), so it has to be an argument for the client to derive the address; the guest asserts `revision_seed == revision.to_le_bytes()` zero-padded. Register asserts `revision == 1`; Update asserts `revision == stored + 1`. This doubles as optimistic concurrency: the only possible race is an author racing themselves, which fails cleanly.

**The registry is public-only.** It has no way to tell from inside the guest whether the root actor executes privately; a privately executed registration would leave public `Link`/`Release`/list records for an `Entry` that is not public. Readers treat a `Link` whose entry account shows no registry state as "private or unverifiable" (M1 review F5, not testable with the public-only harness).

**Rules shared by every `Instruction`.** The shim generates them into the dispatcher, so no handler can forget them:

1. **Root only:** `assert!(input.from.is_none())`.
   - *PDA-header exception:* the entry is at `X = pda(P, s)` and the instruction carries `via_pda_seed = Some(s)`. Then the guest asserts all of:
     - `from == Some(a)`;
     - `input.is_authorized`;
     - `AccountId::for_public_pda(&a.program_account_id, &PdaSeed::new(s)) == X`.
   - The recomputation matters. Inherited authorizations could otherwise let an intermediary program act on `X`; only the real owner program `P` satisfies it.
   - **On this path every `signer` account must be the root `X`** (shim-generated, before the handler runs). A co-signer's `is_authorized` is "signed this transaction", whoever delivers; on the PDA path the transaction's root is `P`'s message, not the registry instruction, so a user who signed a transaction for a dapp `P` would otherwise be made the author of `P`'s entries (M1 review F1, engine test 13). Consequences: a PDA-owned entry is registered with `author = X`, is updated and frozen through `P`, and can't be transferred (a two-step accept could lift this later).
2. Validate every length and charset (a panic means a charged revert at the full gas limit, §2.7; the CLI simulates first).
3. Never trust a client-supplied address. The guest derives every PDA itself from `input.receiver.program_account_id` and the arguments.
4. Bound time: `timestamp_window([now, now + 10 min))` (pattern `LNS/core/src/lib.rs:501-509`).

| Instruction | Root actor | Auth checks | Effects, in send order |
|---|---|---|---|
| `RegisterDeployer` | `(X, REG)` | `pre_state.is_empty()`; `is_authorized` (X signed) **or** the PDA-header rule; `revision == 1`. If `author != X`: `RequireSigner` → `(author, REG)` | `set_state(Entry)` on itself; `WriteRelease(1)`; `Index` → global head → `IndexName` → `IndexAuthor` → `WriteLink` |
| `DeployAndRegister` | `(X, REG)` | as `RegisterDeployer`, but **key-held headers only** (`via_pda_seed` refused): whether a PDA grant would carry into the loader delivery is unverified (Q13). Then sync `program_loader::CreateHeader{first_segment, immutable}` → `(X, LOADER)`, **sent first**. The loader's own `is_authorized` check passes because X signed. It panics if the header already exists, which reverts the whole transaction. Not usable for REG's own self-entry `(REG, REG)` (§2.2, N5) | `CreateHeader`; then as `RegisterDeployer`, with `flags.born_with_deploy = 1` |
| `RegisterThirdParty` | `(publisher, REG)` | `is_authorized`; `program != publisher`. The guest **forces** `author = registrant = publisher` and ignores any author in the input. The root never writes its own actor | `WriteEntry` → `T` (`pre_state.is_empty()`); `WriteRelease(1)`; `Index` → … → `IndexProgram` → `WriteLink` |
| `Update` | `(author, REG)` (the signer's actor) | `is_authorized`. **Inline patch** if `author == entry` (a deployer entry whose author is X): patch `set_state` directly. Otherwise `PatchEntry{patch, now}` → entry; the handler takes the author from `from.account_id` and asserts the record is an entry that lives at this address (deployer: `program == receiver`; third party: `receiver == T(program, registrant)`), `stored.author == from`, not frozen, and `revision == stored + 1` | patch; `WriteRelease(revision)`; event |
| `TransferAuthor` | `(author, REG)` | as `Update`, **plus** `RequireSigner` → `(new_author, REG)`: two signatures in one transaction. If `new_author == entry` (handing a deployer entry back to X), no probe is sent. Instead the `PatchEntry` handler asserts `input.is_authorized` whenever `patch.author == Some(receiver.account_id)`, a condition it reads from the message, so no actor is delivered to twice. Also asserts `new_author != stored.registrant`, and `WriteAuthorNode` asserts `pre_state.is_empty()`: **an entry joins each author's list at most once, and a transfer back to a previous author is refused** with a named error. Without this, A→B→A would make A's walk follow the stale `Link.prev_author` and skip entries | patch `author`; `IndexAuthorTransfer{entry, new_author}` → new author's head → `WriteAuthorNode{new_author, node}`. `WriteAuthorNode` checks `pre_state` itself so the refusal reads "a transfer back to a previous author is refused" |
| `Freeze` | `(author, REG)` | as `Update` | set `flags.frozen`; irreversible |
| *(post-launch)* `Attest` | `(attester, REG)` | `is_authorized` | `WriteAttestation` (only the same attester may overwrite); `IndexAttestation` on the first write |

**Why `Update` roots at the author's actor.** Every author-side instruction is "signer's own actor, sync effects to own PDAs", the LNS 0.4 pattern (`LNS/core/src/lib.rs:731-760`).
- The root is then always a signer.
- No transaction ever delivers twice to the same actor, so we never depend on what `pre_state` a second delivery sees (§12 Q12).
- The inline-patch branch covers the one case where the author's actor *is* the entry.
- (Rooting `Update` at `(X, REG)` with only `A` signing would also be legal, since the root need not sign. We don't need it.)

**Effect handlers.** Every one starts with `assert!(input.from_own_program())` (`program/mod.rs:390-393`).

**Invariant that makes `from.account_id` trustworthy.** REG emits an effect only after the root guard and the `is_authorized` check have passed at the emitting actor. Internal effects are sent only to:
- addresses REG derives itself;
- `(X, REG)` / `(new_author, REG)` for `RequireSigner`, which re-checks `is_authorized` there.

So an effect handler can rely on `from.account_id` (for example `PatchEntry`'s author). `from_own_program` checks only the program id, so the invariant rests on handlers emitting effects only after their guards. Unit tests send every effect variant from outsider actors (refused) and check that an unauthorized root emits no effect.

`RequireSigner` is `assert!(input.is_authorized); keep_state()`. The probes are appended **after** the handler's own messages; that is harmless because a failed probe reverts the whole transaction and a charged revert costs the full gas limit either way (M1 review, conformance 11). A probe is skipped when the signer's id equals the root's or another by-id account's (`author == program`; `new_author == entry`), because that account's own delivery checks `is_authorized` and no actor is delivered to twice. An `author` equal to a registry PDA it also indexes (e.g. the global head) gets two deliveries; the probe then fails, so the transaction reverts (review F4). A co-signer is in `authorized_accounts`, so its credential is true in every delivery. It is admitted without evidence because signers are in the evidence set.

**The index chain.** Each forward carries `.with_pda_seeds([seed_of_target])`, so the target is self-admitted. Sync messages run depth-first (`execution_state.rs:1400`).

1. `Index{E, kind, name, author, program}` arrives at the global head. It reads `{count, newest}` and writes `{count+1, E}`.
2. The global head forwards `IndexName{E, seq: count, prev_global: newest, …}` to the name head.
3. The name head updates itself and forwards `IndexAuthor{…, prev_name}`.
4. For third-party entries, the author head forwards `IndexProgram` to the program's third-party head.
5. The last head sends `WriteLink{seq, prevs…}` to `E`'s link PDA.

Two registrations in one block are ordered by the sequencer, and each reads the heads as the previous one left them. Nothing the client declares depends on chain state, so nothing can collide.

**Why there are no counters or pages.** A counter-indexed `idx:N` PDA or a `count/32` page would make the client predict an address, and an actor must be declared even when self-admitted. Two transactions in flight would then race. The loser reverts and pays, and a spammer could force that on purpose. Pages beside the lists, which one judge suggested for parallel enumeration, bring the same problem back, so we leave them out. §2.5 covers enumeration speed.

### 2.5 Discovery with only the sequencer

| Query | Reads | How |
|---|---|---|
| **Resolve program `X`** (wallet path) | **1** `getAccount(X)` | The account carries `actor_states[REG]` (deployer entry) and `actor_states[LOADER]` (header) together (`actor.rs:432-446`). Fallback: 2 × `getAccountView` |
| Third-party entries for X | 1 + k | `tphead(X)`, then walk `prev_program` over the `Link`s. A known publisher `P` can be read directly at `T(X, P)` |
| **Enumerate all** | 1 + N sequential `Link` reads; entries fetched concurrently | Head → newest → `Link.prev_global` → … `Link`s are write-once, so the SDK caches them forever. A later sync walks only from `head.newest` down to the highest cached `seq`. Snapshot option below |
| **Search by name**, exact | 1 + k | Name head (`name` is a normalised slug), then walk `prev_name` |
| Search by name, substring or fuzzy | local | Over the SDK's synced entry cache, refreshed when `revision` changes |
| **Search by author account** | 1 + k | Author head, then walk. If `Link(E).author == A`, prev is `Link(E).prev_author`; otherwise it is `AuthorNode(A, E).prev` (E reached A by transfer). The client drops entries whose current `author != A` |
| Search by `author_label` | local | Substring over the cache; labelled "unproven" |

**Enumeration speed, stated honestly.** The first full sync is N sequential reads: at 20–100 ms each, 500 entries take 10–50 s. After that, sync is incremental. Two faster paths, neither required:
- an optional static snapshot (`lezreg snapshot`, §7.7), which the SDK checks against `getAccountView` before trusting it;
- JSON-RPC batching, if the public sequencer allows it (Q5).

**Events** are emitted for an optional self-hosted indexer. No client depends on them. Selector: `sha256("lez_program_registry::<Name>")[..8]`; data is borsh.
- `Registered{entry, program, kind, author, name, seq, image_id, idl_sha256, born_with_deploy}`
- `Updated{entry, revision, version, image_id, idl_sha256, build_hash}`
- `AuthorTransferred{entry, from, to}`
- `Frozen{entry}`
- *(post-launch)* `Attested{entry, attester, verdict, image_id}`

The LEZ indexer's `getEvents` serves them (`lez/indexer/service/rpc/src/lib.rs:98-120`; ≤1000 blocks or 8 MiB per query, `event_filter.rs:9,13`).

### 2.6 Timestamps

`registered_at = now`, supplied by the client. The handler sets `timestamp_window([now, now+600_000))`, so the sequencer rejects the transaction unless its block timestamp falls in that range. The stored value is therefore at most 10 minutes earlier than the including block.

The client takes `now` from the chain clock (`lez/programs/clock/core/src/lib.rs:6-46`), as `LNS/cli/src/main.rs:862-871` does. `seq` gives a total order. Evaluators can also read the block timestamp of the registering transaction.

We rejected a sync `clock::AssertTimestamp`: it costs one more delivery for no gain.

### 2.7 Gas and size estimates (replaced by measured numbers in M5, §8.4)

**Preliminary measurements (spike d, rc1 sequencer, base fees 8/8).** The REG there is a toy: 2 declared actors instead of 7–9, no index chain, no `Release`. Only the deltas and the loader's zero carry over to the real registry.

| Transaction | Cycles (simulated) | Tx bytes | Fee charged |
|---|---|---|---|
| `DeployAndRegister` (toy; X signs, P pays) | 9,738 | 693 | 83,456 |
| `CreateHeader` alone (427 KB, 5 segments) | **0** | 607 | 4,864 |
| `Register` alone (toy, fallback step 2) | 7,613 | 587 | 65,608 |
| `WriteSegment` | 0 | ~98 KB | ~791,328 per 96 KiB segment; 3,440,448 for a 427 KB guest |

- `DeployAndRegister` minus `Register` is **2,125 cycles** (one `send_sync` plus a longer message), with nothing that scales with program size. Fee ≈ `cycles × 8 + bytes × 8` held exactly.
- The bulk of a deploy's cost is the `WriteSegment` bytes, not the registration.
- The two-transaction path (`CreateHeader` + `Register`) cost 70,472 against 83,456 for one transaction, but needs X to sign twice and loses `born_with_deploy`.

The estimates below stand until M5 measures the real guest. **Measured in M1** (rc1 local sequencer smoke run and engine tests, real guest): `RegisterDeployer` (REG self-entry, author = X) 168k cycles / 1,445 B; `DeployAndRegister` (author ≠ X) 183k / 1,770 B; `RegisterThirdParty` 227–234k / 1,509 B, above the 110–200k estimate (candidate saving for M5: the per-head `assert_at` address re-derivations); `Update` by `PatchEntry` 88–93k / 899 B; `TransferAuthor` ~97k; `Freeze` ~43k; worst-case-size `RegisterDeployer` ~238k / 2,876 B.

Execution gas = cycles, summed over every delivery. Storage gas = borsh transaction bytes (`lez/chain_state/src/apply.rs:466`, `classify.rs:170`). Built-in handlers measure 8–26k cycles, and a 4 KiB state write ~36–44k (`docs/benchmarks/cycle_bench.md`, pre-actor numbers, indicative only).

| Operation | Deliveries | Est. cycles | Tx bytes | Actors declared |
|---|---|---|---|---|
| `RegisterDeployer` (author = X) | 6: root, release, global, name, author, link | 100–180k | ~1.6 KB typ, ≤ 3.2 KB | 7 (+ payer if different) |
| `RegisterDeployer` (author ≠ X) | 7 (+ `RequireSigner`) | +10k | +~100 B (2nd signature) | 8 |
| `DeployAndRegister` | 7–8 + loader | registry part as above; the loader delivery is host Rust and unmetered (§2.1), so `compute_image_id` adds no execution gas | as above + ~40 B | 8–9 |
| `RegisterThirdParty` | 7 (+ `IndexProgram`) | 110–200k | ~1.7 KB | 8 |
| `Update` | 2–3 | 40–90k | 0.4–1.6 KB | 2–3 |
| `TransferAuthor` | 4–6 | 50–100k | ~0.4 KB | 5–6 |

Fee ≈ `cycles × base_fee_exec + bytes × base_fee_stor`, with both at 8 today (`lez/programs/fee/core/src/market.rs`). A registration is therefore on the order of 1–2M native units, well below `MAX_GAS_EXEC` 10M / `MAX_GAS_STOR` 1M per block. The gas limit is the simulated value × 1.5, with a 30,000 minimum (`LNS/cli/src/main.rs:2041-2050`). **A charged revert costs the whole declared gas limit** (spike d), so this rule also caps what any revert can cost: the client always simulates first and never sends a fixed large limit. An attacker pays for their own failed attempts; the victim pays nothing.

**`DeployAndRegister` cost:** the loader is not metered (`public_backend.rs:68-77`, confirmed on chain), so a `DeployAndRegister` costs about what a `RegisterDeployer` costs, whatever the program size (spike d: 83,456 vs 65,608 for the toy). The segments are written in earlier `WriteSegment` transactions (≤ 96 KiB each, well under `MAX_GAS_STOR` 1M per block). The two-transaction path (`CreateHeader`, then `RegisterDeployer`, no `born_with_deploy`) is kept for PDA-owned headers, where `DeployAndRegister` refuses `via_pda_seed` (§2.4, Q13), and for REG's own self-entry `(REG, REG)` (§2.2, N5). Its squat window is closed: between the two transactions nobody but X can write `(X, REG)` (spike d, N1).

---

## 3. Proving a deployer entry with no off-chain trust

**Claim.** An `Entry` at actor `(X, REG)` with `kind = Deployer` was created in a transaction in which the account controlling `X`'s loader header authorized it.

**Proof sketch.**
1. Only REG's code writes REG actor state. The state at `(X, REG)` changes only through `set_state` in a REG delivery to `(X, REG)`.
2. A creating write at `(X, REG)` happens only in the `RegisterDeployer`/`DeployAndRegister` root handlers. Both declare their root `#[account(init, signer)]`, which the shim compiles to an assert that the pre-state is empty ("`program` already exists"; `spel04-shim/macros/src/lib.rs`), and assert `input.is_authorized` for `X`. The M2 demo shows both the client-side refusal and, with `--force-send`, the on-chain revert of a second deployer entry.
3. `is_authorized` for public `X` is true iff:
   - `X` signed, or
   - a sender held a PDA grant for `X` (`execution_state.rs:1446-1467`).

   The root-only rule removes every grant path except the explicit PDA-header path, which recomputes the PDA.
4. The loader accepts `CreateHeader`/`UpdateHeader` at `X` under exactly the same condition (`program_loader/core/src/lib.rs:73-101`). There is no deployer field (kill-check). So "the writer of `(X, REG)`" is "the party able to create or upgrade the program at `X`", which is the only sense of "deployer" that 0.4 records.
5. Later writes to `(X, REG)` (`PatchEntry`, or an inline patch) are authorized by the recorded `author`. That is the spec's rule: "only the original registering author may update".

**Client procedure (sequencer only).**
1. `getAccount(X)`. A non-empty `actor_states[REG]` decoding to `Entry{kind: 0, program: X}` is the deployer entry. The address is the proof.
2. Decode `actor_states[[0xFE;32]]` as a `ProgramHeader`. If it is missing, show "no program at this address (yet)": the entry is shown but flagged. A key holder may register before deploying; that is harmless, since it is the same key. **This client check is the only test that X is a program.** `RegisterDeployer` proves only that X signed: a 0.4 actor sees only its own state and the loader has no read message, so REG can't check the header on chain (spike d; Q8). The header comes back in the same `getAccount` read, so the check costs nothing.
3. `entry.image_id == header.image_id` means **fresh**; otherwise **stale** (the header was repointed after the last `Update`). `header.immutable` shows **frozen code** or **upgradeable**.
4. `flags.born_with_deploy` shows "registered at deploy". The registry's own code sent `CreateHeader` in the transaction that created the entry, so the entry has existed as long as the program has. It is the **only on-chain proof** that entry and header were made together. Entries from the two-transaction path, including REG's own `(REG, REG)`, don't carry it.
5. Third-party entries come from the program's third-party list. Each address is checked by recomputing `T(X, publisher)`, so no third-party record can pass as the deployer entry. UIs label them "community entry by `<publisher>`" and rank them below the deployer entry.

**Demo of the required rejection.** Each case reverts at execution, with the fee (the attacker's whole declared gas limit, §2.7) charged to the attacker. Spike (d) showed cases (a) and (d) on the rc1 sequencer (N1, N2, N2b, N4), and that the loader independently refuses a `CreateHeader` for an unauthorized X even with REG's own check removed (N3). `lezreg` first shows the simulation refusal; `--force-send` then shows the on-chain revert (`is_reverted`, as LNS `finish()` reads it).

| Case | Attempt | Result |
|---|---|---|
| (a) | Mallory sends `RegisterDeployer` to `(X, REG)`, signed only by her own key | panics: "deployer entry requires the program header account's signature" |
| (b) | Mallory gets X's owner to sign a transaction for her program `M`, and `M` sends `RegisterDeployer` to `(X, REG)` | rejected because `from = Some(M)` (root-only) |
| (c) | `M` sends `Effect::WriteEntry` or `PatchEntry` directly | fails `from_own_program` |
| (d) | A second `RegisterDeployer` on an existing entry | refused: `pre_state` not empty |
| (e) | `Update` by a non-author | refused: `stored.author != by` |

---

## 4. Source verification

### 4.1 Build configuration: what determines the bytes

`cargo risczero build` (`risc0-build/src/docker.rs:60-200`) does the following:
- It writes a Dockerfile: `FROM risczero/risc0-guest-builder:<tag>`, `WORKDIR /src`, `COPY . .`. The context is the docker root, ignoring `**/.git`, `**/target`, `**/node_modules`, `**/tmp` and `**/Dockerfile`.
- It sets `CARGO_MANIFEST_PATH`, `CARGO_ENCODED_RUSTFLAGS` (`encode_rust_flags`), `CARGO_TARGET_DIR=target`, `RISC0_FEATURE_bigint2` and the C compiler flags.
- It runs `cargo +risc0 fetch --locked`, **then** `cargo +risc0 build --release --locked [--features …]`.

`encode_rust_flags` depends on the **host's** rzup Rust version (`> 1.81` selects `passes=lower-atomic`; `risc0-build/src/lib.rs:443-462`). It also takes the guest's `[package.metadata.risc0] rustc_flags` from the source.

The image tag is mutable, and only the digest pins the in-container toolchain. LEZ's own CI uses `r0.1.91.1@sha256:fafb377a…48f0` (rc1 `artifacts` CI job log).

Hence `BuildConfig` holds:
- `image_digest`, `risc0_build`, `host_risc0_rust`, `manifest_path`, `binary`, `features`, `docker_env`;
- `lock_sha256`, a cheap early check that the fetched tree is the intended one, and evidence of which dependency set was claimed.

`SourceRef` adds `commit`, `docker_root` and the optional `archive`. `build_hash = sha256(borsh(BuildConfig))`.

`lezreg.toml` (author side, committed; `lezreg init --detect` writes it):

```toml
[program]
name = "my-token"                      # Entry.name (≤ 32, slug)
version = "1.2.0"                      # default: Cargo version
author_label = "Acme Labs"             # default: Cargo authors[0] / git user.name
description = "Fungible token program"
tags = ["token", "defi"]               # default: Cargo keywords
[build]
image = "risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a44e1cfca415577c48d2f7012bda99ed36f2fae27f9a663b9fe6048f0"
risc0_build = "3.0.5"                  # detected from Cargo.lock
docker_root = "."
manifest_path = "methods/guest/Cargo.toml"   # detected from [package.metadata.risc0] methods
binary = "my_token_guest"
features = []
[idl]
path = "idl/my_token.json"             # or: generate = "spel" | "enum" (§6.4)
[source]
mirror = true                          # upload git-archive tarball to Storage
```

`lezreg init --detect` works as follows:
- It reads `risc0-build` from `Cargo.lock` and maps it to a builder tag and digest through a table shipped in the CLI. The table starts at `3.0.5 → r0.1.91.1@sha256:fafb377a…`.
- It reads `rzup` for `host_risc0_rust`.
- It takes the remote and commit from git, and refuses a dirty tree unless `--allow-dirty` is given.
- It records any `DockerOptions` env the guest's build script sets (`risc0-build` adds them to the Dockerfile as `ENV` lines, `docker.rs` ~165) in `BuildConfig.docker_env`. This matters for `lezreg adopt`: a program the author built earlier with custom Docker env only rebuilds to the same image if that env is replayed. If the env cannot be detected statically, `adopt` asks for it, or says the program needs a redeploy through `lezreg build`.

### 4.2 `lezreg build` and `lezreg verify`

**`lezreg build`** (author side). It runs the canonical build from a clean `git archive HEAD:docker_root` and writes:
- `target/lezreg/<binary>.bin` and the `image_id`;
- `build.json`;
- `source.tar` and its sha256.

**`lezreg verify <program> [--publisher P] [--revision N] [--deep] [--mark]`** (anyone):

1. **Resolve.** Read the entry (deployer by default) and the live header (§3). Select the tuple to rebuild:
   - by default, the entry's current revision;
   - with `--revision N`, `Release(E, N)`;
   - with `--live`, the newest `Release` whose `image_id == header.image_id`, if there is one. This covers a header rolled back to an older image.

   We always rebuild a **recorded** tuple and never pair one revision's source with another image. A stale entry with no matching release cannot verify the live code; the author must `Update` first. The tool says so.
2. **Get the source.**
   - Fetch the archive from Storage if `archive` is set, and check its sha256 against the chain.
   - Otherwise `git clone`, check out `commit`, and run `git archive commit:docker_root`.
   - If both are available, their sha256 must agree. This detects force-pushes and moved tags.
   - Check `sha256(Cargo.lock) == lock_sha256`.
3. **Build.** Emit the Dockerfile `risc0-build` 3.0.5 would emit, with exactly two changes:
   - `FROM …@sha256:<image_digest>`. The tool refuses if the local image digest differs (`docker image inspect`).
   - The build step becomes `RUN --network=none cargo +risc0 build …`. Avoid a `# syntax=docker/dockerfile:1` line if possible: it pulls a frontend image from Docker Hub, which is one more network dependency on the verify path. Whether the engine's built-in BuildKit frontend accepts `RUN --network=none` (Docker ≥ 23 assumed, **unverified**) is checked in the `repro.yml` GitHub Actions job (§8.3), which replaces M0 spike (b): there is no local Docker build (decision 2026-11-02). `lezreg doctor` checks the engine version.

   The fetch step keeps network access, so dependencies come only from `Cargo.lock` checksums. Every `build.rs` and proc-macro then runs offline, which addresses the Accretion attack. Paths (`/src`, `/root/.cargo`) are unchanged, so the bytes should be identical; `repro.yml` proves that against `cargo risczero build` on every push. The build runs with `docker buildx build --output`.
4. **Compare.**
   - `compute_image_id(ProgramBinary::new(user_elf, V1COMPAT_ELF))`, the loader's own formula (`program_loader/core/src/lib.rs:147-168`; tool `scratchpad/imgid`), is compared three ways: rebuilt vs claimed (`entry` or `Release`) vs `header.image_id`.
   - `--deep` also walks the segment chain (`get_program_via`, `program/mod.rs:764-787`) and byte-compares the user ELF, as the kill-check did (`read_program.py`: "IDENTICAL").
5. **Verdict.**

   | Verdict | Meaning |
   |---|---|
   | `Match` | rebuilt = claimed = live header |
   | `MatchStale` | rebuilt = claimed, but the header was repointed since. The claim is true; the running code is a different image |
   | `Mismatch` | rebuilt ≠ claimed |
   | `NeedsNetwork` | the offline build failed and the opt-in `--allow-network` rebuild matched. Recorded as `network_denied = false` and shown as weaker |
   | `NotReproducible` | the build failed |

   The report also lists the crates with `build.rs` or proc-macros (from `cargo metadata`): "these ran during the build (offline)". `--json` emits all of the above.
6. **Mark.** On `Match` (or `MatchStale` with `--mark-stale`), ask before writing a local mark (§4.3).

**Apple silicon note.** The builder image is amd64, so it runs emulated and needs ≥ 8 GB of Docker memory (`LNS/README` ~l.202). `lezreg doctor` checks this. That is why the primary *author* verification path is the GitHub Action (§7.6), which runs natively on x86_64. Users can still rebuild locally on any machine with Docker.

### 4.3 Local "verified" marks and staleness

**Storage.** `$XDG_DATA_HOME/lez-registry/verified.json`, or `~/Library/Application Support/lez-registry/` on macOS and `%APPDATA%\lez-registry\` on Windows. The module uses the same file, so a CLI verification shows up in Basecamp.

**Key and contents.** Marks are keyed by `(chain_id, X, image_id, build_hash, source_sha256)` and hold:
- verdict, revision, time and tool version;
- the rebuilt ELF's sha256;
- the Docker digest used;
- `network_denied`.

Marks are never published, unless the user attests (post-launch).

**Status, recomputed on every view at no extra RPC** (the `getAccount` read is already done):

| Condition | Shown |
|---|---|
| Live `header.image_id` = mark's `image_id`, and the entry's `build_hash`/source still equal the mark | **Locally verified** (date) |
| Header repointed | **Stale: code changed since you verified** (the mark no longer applies) |
| Entry `Update` changed source or build but not the image | **Verified image, source pointer changed: re-verify** |
| `header.immutable == false` | add **upgradeable**: the current code is verified, future code is not |
| `entry.image_id ≠ header.image_id` | **Entry stale**, in addition to the above |
| No header at `X` | **No program at this address** |

### 4.4 Hosting verified source (a design choice the prize asks about)

**Both, and the archive is authoritative.**
- **Repo URL + commit:** discoverability, and review in a familiar forge.
- **The `git archive` of `docker_root` at `commit`, on Storage**, with its CID and sha256 on chain. Re-verification then survives the repo being deleted, and an evaluator can rebuild from chain plus Storage alone. `lezreg publish` builds the tarball reproducibly (fixed mtime, sorted entries, `gzip -n`), so anyone can regenerate the same sha256 from git while the repo exists.
- **The catch:** tarballs usually exceed the ~128 KiB relay allowance **(unverified)**, so a directly reachable node has to serve them. `peer_hint` records one, and `lezreg pin` lets anyone add more (§5.3). None of our nodes is directly reachable, and we will not open a port forward at home (decision 2026-11-02, security; M0 spike (e) deferred).
- **Launch position:** the archive is recorded on chain and pinned, but it is **best-effort**. Repo + commit is the working path at launch; the archive takes over as soon as a reachable provider exists (a fixed-cost VPS pinner, decided near launch, §5.3). The FURPS self-assessment says so plainly.
- **Dependency crates** come from crates.io, pinned by checksum. `--vendor` (off by default, because of size) also mirrors a `cargo vendor` tarball.

### 4.5 Trusted-signer overlay (a design choice the prize asks about)

**Position: user-configurable, empty by default, and no central trust root.**

- **v1 (launch):** only the user's own local marks count as "verified". The deployer's claim plus *your* rebuild is the trust path.
- **Post-launch (M9):** we append the `Attest` instruction. Attestations are on-chain records `{attester, image_id, build_hash, source_sha256, verdict, tool}`, listed per entry.
  - The client ranks attestations from accounts in the user's `trust.toml` (`lezreg trust list|add|remove`, the same file in the module) above others.
  - The file is plain, inspectable, and in the shape of `logos-repo.json`'s `trustedSigners`.
  - Nothing ships pre-trusted. An attestation from an untrusted account is shown as plain "attested by `<account>`", with no badge.

---

## 5. IDL

### 5.1 Format

The IDL is SPEL spec "0.1.0" JSON (`SPEL/spel-framework-core/src/idl.rs`, `docs/reference/idl.md`), generated as in §6:

`{version, name, instructions[{name, accounts[{name, writable, signer, init, pda{seeds}}], args, discriminator, …}], accounts, types, errors, spec, metadata, instruction_type}`

The released v0.7.0 generator emits `discriminator: null`, `spec: null` and `metadata: null` (F1b). `lezreg-idl-extend` fills them (§6.3); they are not generator output.

- **Instruction order** equals the `Instruction` enum order, which is the wire tag. v0.7.0 emits instructions in **fn declaration order** and never reads the enum (`order_by_enum_variants`, `d58984a`, is only in `v0.7.0-rc.3` and `main`). The shim generates the enum from the same fns in the same order (F1), so the two agree by construction; drift test 3 guards it.
- **Each instruction's `accounts[]` lists exactly the actors the client must declare**, written in SPEL's own vocabulary:
  - `signer: true` means the account signs;
  - `pda.seeds` (`const` / `account` / `arg`) derive under SPEL's rule, which is ours;
  - `init: true` means the actor may be created. No evidence is needed, because the program self-admits; `x-lez04` says so.

  A SPEL-literate client can therefore derive every actor from the IDL alone. Spike (a) checked this against the released `spel` CLI's resolver for const, account and string seeds; integer seeds are banned for that reason (F3, §2.2). Deriving addresses is all a stock SPEL client can do: it can't encode valid registry transactions (F2, F4, §2.4).

**The `x-lez04` extension.** SPEL's IDL structs do not `deny_unknown_fields` (checked, `idl.rs:16-39`), so `SpelIdl` still parses the file. The extension carries what 0.4 needs and spec 0.1.0 cannot say:

```json
"x-lez04": {
  "spec": "lezreg-idl-ext/0.1",
  "lez": "v0.4.0-rc1",
  "wire": {"root": "bare Instruction (u8 variant + borsh args; not the spel v0.7.0 CLI's risc0-serde words)", "effect_tag_min": 192, "require_signer_tag": 255},
  "account_id_args": {"rule": "<name>_id: [u8;32] supplies the id of each non-root, non-PDA account <name>",
                      "pairs": {"author": "author_id", "new_author": "new_author_id", "entry": "entry_id"}},
  "instructions": {
    "register_deployer": {
      "root": {"account": "program", "program": "self"},
      "signers": ["program", "author"],
      "signers_note": "author may equal program: declare and sign once",
      "evidence": "none (program self-admits via pda_seeds; signers auto-admitted)",
      "time": {"arg": "now", "window_ms": 600000}
    },
    "update": { "root": {"account": "author", "program": "self"}, "inline_if": "author == entry" }
  },
  "internal": ["WriteEntry", "PatchEntry", "..."],
  "events": [{"name": "Registered", "selector": "<8 bytes hex>", "type": "RegisteredEvent"}],
  "state": {"(X,self)": "Entry", "lezreg/v1/link": "Link", "lezreg/v1/rel": "Release", "...": "..."},
  "pda_rule": "spel-compute_pda",
  "program": {"account_id": "<base58 on testnet>", "image_id": "<hex>"}
}
```

`metadata` (written by `lezreg-idl-extend`, F1b) holds `{name, version}`, plus optional `{homepage, docs, license, audit_cids[]}`. Clients warn if `metadata.name/version` differ from the entry's.

The same extension is the template `lezreg idl gen` writes for third-party programs (§6.4).

### 5.2 Hashing and integrity

- Canonical bytes are RFC 8785 JCS: sorted keys, no whitespace.
- The upload name is always `<name>-<version>.idl.json`, mimetype `application/json`. The manifest CID depends on bytes, filename and mimetype, and identical triples give identical CIDs (Storage v0.5.3, research §storage).
- On chain, `ContentRef` holds the binary manifest CID, the content sha256, the size and an optional `peer_hint`.
- The client checks that `size ≤ 120 KiB`, below the relay allowance, and that the sha256 matches, and only then parses JSON (with depth and size limits) as `SpelIdl` and checks `x-lez04`.
- If the hash doesn't match, the bytes are discarded unparsed. Every transport is untrusted: Storage, mirrors, caches, and bring-your-own bytes.

### 5.3 Upload and fetch, with retries and fallbacks

**Upload (`lezreg publish` / `lezreg idl push`).**
- Upload goes to the user's local Storage REST endpoint (`POST /api/storage/v1/data`), or to `storage_module` `uploadUrl` inside Basecamp. The client code comes from `~/logos/logos-mirror/crates/mirror-sdk/src/storage.rs` (`upload_with`, `reannounce`, `wait_reachable`).
- Before announcing, wait until `debug/info.addrs` is non-empty, then re-announce. This is the announce-after-address gotcha.
- Over 120 KiB is refused.
- The command prints the CID, the sha256 and the node's peer id, which becomes `peer_hint`.

**Fetch ladder** (SDK `fetch_content(ref)`, shared by the CLI, the module and the JS SDK where it applies). Every step has a hard timeout, the whole ladder is bounded at ~90 s, and the UI never waits on it (§5.4).

1. Local cache `$XDG_CACHE_HOME/lez-registry/blobs/<sha256>`.
2. Bring-your-own bytes, for example a wallet that ships IDLs or a project directory, accepted if the hash matches.
3. The user's Storage node:
   - `GET /data/{cid}/network/manifest?advertise=false` (15 s);
   - then `GET /data/{cid}/network/stream?advertise=false` (30 s stall timeout);
   - 3 attempts, with backoff 2 s / 8 s / 20 s.

   `advertise=false` keeps readers from becoming dead providers.
4. On a stall: `GET /connect/{peer}?addrs=<relay or direct multiaddr>` for `peer_hint`, then for each peer in the user-editable `pin_peers` list, then retry. In the kill-check this cleared stalls that a plain retry did not. The default list contains our pinning node; **the SDK works with the list empty**.
5. Optional, **off by default**, disclosed in the README: user-configured HTTP mirrors serving `<sha256>`, content-checked.
6. If every step fails: `IdlStatus::Unavailable{reason}`.

**Persistence.**
- We run ≥ 2 Storage nodes with `--block-ttl` of 10 years. It is never `0`, which expires blocks immediately in v0.5.3.
- **Reachability is not solved yet.** A probe of the M2 node on 2026-11-02 showed `nat.reachability: "NotReachable"`, `clientMode: true`, and only `/p2p-circuit` relay addresses. Today it can serve nothing above the relay allowance, so it cannot serve source archives (§4.4). **Decision (2026-11-02): no home port forward** (security), and M0 spike (e) is deferred. The launch path needs no direct reachability: IDLs are capped at 120 KiB and travel over the relay, and source is reachable through the on-chain git repo + commit pointer. Whether to add a fixed-cost VPS pinner (a flat monthly price, no per-request billing) for archives is **decided near launch**; it is not a D-day gate. These nodes are fixed-cost and not on any client's critical path.
- **`lezreg pin-all` ships before D-day (M3), not post-launch.** It is a self-hostable pinner that walks the registry every few minutes and fetches every IDL and archive with `advertise=true`. Most outside authors will not keep a Storage node online, and a counted entry needs a *resolvable* IDL when an evaluator looks. Our pinners run it, and the README asks integrators to run it too. Its cost is fixed by the walk interval, not by readers.
- `lezreg pin <program|--all-verified>` fetches with `advertise=true`, turning any user's node into an extra provider. More providers help with discovery bug `logos-storage-nim#1556`.
- **Authors without a node.** `lezreg storage up` downloads the pinned Storage v0.5.3 release binary, starts it with a long `--block-ttl` and a fixed API port, and waits for non-empty `debug/info.addrs`. Inside Basecamp, `storage_module`'s node is used instead. `lezreg publish` `connect`s that node to the `pin_peers` and keeps it up for two pinner walk intervals (default 30 min, `--hold`), so a pinner can fetch the new CIDs before the author's node goes away. **(unverified)** whether a remote pinner's copy can be confirmed from the author's side; until then the CLI prints a reminder to run `lezreg idl <program>` from another machine. `lezreg doctor` reports which case applies.

### 5.4 Graceful degradation

`resolve(program)` returns `{entry, header, verification, idl: IdlStatus}` and never fails because of the IDL.

`IdlStatus` is one of:
- `Ok(SpelIdl)`
- `Pending` (still fetching; an event follows)
- `Unavailable{NoStorageNode|NotFound|Timeout|TooLarge}`
- `HashMismatch{expected, got}`
- `ParseError{msg}`

What each client shows:
- **CLI:** the metadata block, then `IDL: unavailable (timeout after 3 tries; try 'lezreg idl <id> --connect')`.
- **Module:** returns `success: true` with `idl_status` set.
- **Wallet, via `decode_instruction` without an IDL:** returns `{decoded: false, reason}`, and the wallet falls back to raw hex labelled with the entry name.
- **Sequencer unreachable:** the SDK serves its last-synced cache, labelled "cached at block N".

Unit tests cover every state. E2E covers a bogus CID, a hash mismatch, an unparseable IDL, and a stopped Storage node.

---

## 6. SPEL strategy

### 6.1 Facts (re-checked 2026-11-02)

- SPEL `main` (`c6f73ce`) pins LEZ `v0.2.5-rc2` in every crate. There is no 0.3 or 0.4 branch, and spel#291 (0.3 support) is open.
- Its runtime macro generates a 0.2-style account-list dispatcher that cannot run on 0.4 actors.
- Its **IDL generator is syntactic** (`syn` over the source, no compilation; `idl_gen.rs:133-360`):
  - it finds `#[lez_program]` modules and `#[instruction]` fns;
  - **it treats parameters whose type ident is `AccountWithMetadata` as accounts** (`idl_gen.rs:989`), with `#[account(signer|init|mut, pda = [literal(..)|account(..)|arg(..)])]`, and everything else as args;
  - it collects `#[account_type]` structs and the types they reference.
- On `main`/`v0.7.0-rc.3`, `#[lez_program(instruction = "Instruction")]` orders the instructions by that enum (`order_by_enum_variants`, `d58984a`). **The released `v0.7.0` (`4107dcb`) does not** (spike a, F1): the attribute only sets the `instruction_type` string, instructions come out in fn declaration order, and the enum is never read or checked.
- It emits `discriminator`, `spec` and `metadata` as null (F1b).
- Released v0.7.0 CLI, as a client (F2–F4): it encodes instruction data as risc0-serde u32 words, not borsh (borsh arrived on `main` with `ba8d7f1`); its PDA resolver encodes u64/u128 seeds big-endian right-aligned and rejects u32, unlike `ToSeed`; and it refuses struct args.
- The generator never compiles the file and matches literal tokens, so `#[cfg_attr(feature = "x", lez_program)]` is invisible to it and `#[cfg(feature …)]` items are dropped. The backend is therefore chosen in `Cargo.toml` (`spel = { package = "spel04-shim", … }`), not with cfg.

### 6.2 Program source in SPEL syntax, compiled by `spel04-shim`

`program/src/lib.rs` is real SPEL syntax:

```rust
use spel::prelude::*;   // Cargo.toml maps `spel` to spel04-shim
// No hand-written enum: the shim generates `Instruction` from the #[instruction] fns, in fn order (F1).
// New instructions are appended at the end of the module.

#[lez_program(instruction = "Instruction")]
mod lez_program_registry {
    #[instruction]
    pub fn register_deployer(
        #[account(mut, signer)] program: AccountWithMetadata,               // root (X, self)
        #[account(signer)] author: AccountWithMetadata,                     // may equal program; id from author_id (F5)
        #[account(init, pda = [literal("lezreg/v1/rel"), account("program"), arg("revision_seed")])] release: AccountWithMetadata, // no integer seeds (F3)
        #[account(mut, pda = [literal("lezreg/v1/head")])] head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/name"), arg("name")])] name_head: AccountWithMetadata,
        #[account(mut, pda = [literal("lezreg/v1/author"), account("author")])] author_head: AccountWithMetadata,
        #[account(init, pda = [literal("lezreg/v1/link"), account("program")])] link: AccountWithMetadata,
        name: String, version: String, author_label: String, description: String, tags: Vec<String>,
        idl: ContentRef, source: SourceRef, build: BuildConfig, image_id: [u8; 32],
        author_id: [u8; 32], revision: u32, revision_seed: [u8; 32], now: u64, via_pda_seed: Option<[u8; 32]>,
    ) -> Response { … }   // `-> Response` is accepted by the v0.7.0 generator (spike a)
    // … other instructions; #[effect] fns for internal handlers (ignored by SPEL's generator)
}
```

**`spel04-shim`** is a proc-macro crate of about 400–600 LOC (estimate). It implements `#[lez_program]`, `#[instruction]`, `#[account]`, `#[account_type]` and `#[effect]` for 0.4:
- **`AccountWithMetadata` becomes the shim's actor handle** `{account_id, program_account_id}`. The shim derives it from the seeds and arguments, so handlers *address* effects through it.
- Only the root parameter carries `pre_state`/`is_authorized`, from `ReceiveInput`.
- It **generates `enum Instruction`** from the `#[instruction]` fns (variants in fn order, fields in arg order, F1), and the hand-decoded `Message` (instruction tags, then effects from `0xC0`, `RequireSigner` at `0xFF`). It checks that new fns are only appended, against a committed tag list.
- It enforces the `<name>_id: [u8;32]` arg for every non-root, non-PDA account (F5) and rejects integer `arg` PDA seeds (F3), both at compile time.
- It strips `#[instruction]`, `#[account(..)]` and `#[effect]` before rustc sees them (rustc rejects unknown attributes on fn params).
- It generates the dispatcher with the root-only and `from_own_program` guards built in, plus `actor_entrypoint!`.
- It generates a host-side `actors_to_declare(&Instruction) -> Vec<Actor>`. The SDK uses it, so the guest, the SDK and the IDL all derive addresses from one source of truth.

**Deviations in the M1 code from the sketches above (behaviour-safe; the code is authoritative):**
- `PatchEntry` has no `by` field: the author is `from.account_id` (`program/src/lib.rs`, `patch_entry`). `IndexAuthorTransfer` was added and `WriteAuthorNode` carries `new_author` (§2.4).
- `DeployAndRegister` has no `via_pda_seed` argument at all (root-only by construction).
- `docker_env` is `Vec<EnvVar>`; `ContentRef`/`SourceRef`/`BuildConfig` are plain borsh structs (IDL `types[]`).
- `IndexCtx` carries `image_id`, `idl_sha256` and `born_with_deploy` for the `Registered` event.
- Validation is stricter or looser than §2.3 in four places (`crates/types/src/validate.rs`): versions are printable ASCII without spaces; a CID is 1..=64 bytes; `commit` may be empty when only an `archive` is given (otherwise 20 or 32 bytes); the repo URL scheme is not checked.
- The shim skips the co-signer probe when the signer equals the root or any other by-id account, and on the PDA path requires every signer to be the root (§2.4 rule 1).
- Event layouts in `x-lez04.event_types` are generated from the Rust event structs (`event!` in `crates/types/src/events.rs`), so they can't drift from the borsh layout.

### 6.3 IDL "generated via SPEL", literally

CI builds the released `spel` **v0.7.0** from its tag (pinned and checksummed) and runs `spel generate-idl program/src/lib.rs` over the **real program source**.

A deterministic `lezreg-idl-extend` step only **adds**: it writes `x-lez04` and fills `discriminator`, `spec` and `metadata`, which v0.7.0 leaves null (F1b), all from the same Rust constants the guest uses. It never changes a field the generator filled. Spike (a) confirmed that SPEL v0.7.0's `SpelIdl` parses the generator output with an extra top-level `x-lez04` key and round-trips it unchanged.

Drift tests:
1. Regenerate the IDL and fail on any diff.
2. `SpelIdl` parse, using `spel-framework-core` v0.7.0 as a host-only dev-dependency that never meets the 0.4 crates.
3. **Our own** IDL-driven borsh encoder (in the lezreg SDK, Rust and JS) produces bytes equal to `borsh::to_vec(&Instruction::…)` for every variant. The released `spel` v0.7.0 encoder can't serve as this oracle, because it emits risc0-serde words, not borsh (F2).
4. Every IDL `pda.seeds`, derived with SPEL's `compute_pda`, equals `actors_to_declare` and `lee_core::AccountId::for_public_pda` vectors.

**Fallbacks: not needed.** M0 spike (a) passed (`spikes/spel-idl/RESULT.md`): the released v0.7.0 generator accepts the shim-annotated toy module, `-> Response` returns included, and the same file builds a LEZ v0.4.0-rc1 guest through the in-repo shim. The fallbacks we had planned (a `-> SpelResult` tweak, then a sidecar `program/spel/interface.rs`) are dropped.

### 6.4 Adoption bonus: IDLs for other 0.4 programs

Most 0.4 programs have no IDL ("There is no IDL and no CLI for 0.4 yet", mmlado/lez-admin-authority README). A counted registration needs a "resolvable, parseable IDL", so we ship tools that produce one:

| Command | What it does |
|---|---|
| `lezreg idl gen --from spel <file>` | runs upstream `spel generate-idl` |
| `lezreg idl gen --from enum <crate>::<Instruction>` | `syn` over a plain borsh enum; writes SPEL-format instructions, args and types, plus an `x-lez04` skeleton (root actor = signer, by default). Actor lists are left for the author to fill |
| `lezreg idl gen --from-spel-idl <file>` | adds `x-lez04` to an existing SPEL IDL (lez-programs `artifacts/*-idl.json`) |
| `lezreg idl check <file> [--program X]` | parses as `SpelIdl`; validates `x-lez04`; cross-checks the variant count against simulated calls when a program id is given |

### 6.5 Upstream and disclosure

- We offer `spel04-shim` as SPEL's 0.4 backend: a PR to `logos-co/spel`, building on the `0x-r4bbit/spel` 0.3 work, with a comment on spel#291. We also propose `x-lez04` as SPEL's 0.4 actor/event section.
- We report the integer-seed disagreement between `ToSeed` and the CLI resolver (F3) upstream.
- The syntax is SPEL's, so if SPEL ships 0.4 the switch is a `Cargo.toml` change **plus** adopting whatever upstream picks for the conventions this shim had to invent: the `<name>_id` arg pairing (F5), `#[effect]` fns, the `0xC0` effect tag shift and the `RequireSigner` tag `0xFF`. If upstream picks differently, the wire format changes, which needs an `UpdateHeader` of the mutable registry, an `Update` of its self-entry and a client release.
- The FURPS self-assessment will say: "Program written in SPEL attribute syntax and compiled through an in-repo 0.4 backend (offered upstream). IDL generated by the released `spel generate-idl` v0.7.0 over the program source, plus a documented `x-lez04` extension. PDA seeds derive identically in the released `spel` CLI; transactions are encoded by `lezreg` (borsh), not by the stock `spel` CLI. SPEL upstream has no LEZ 0.4 support." This is **Q1, posted on day 0**.

**Rejected alternatives:**
- A hand-written IDL: not generated via SPEL, and the competitor self-rates it "partial".
- A sidecar shape file as the primary path: a second source of truth.
- Waiting for SPEL: it blocks the adoption clock.

---

## 7. Components and repo layout

```
lez-program-registry/                 (MIT OR Apache-2.0)
├─ program/            lez_program_registry: records, seeds, handlers (host-testable), SPEL-syntax module
├─ spel04-shim/        proc-macro: #[lez_program]/#[instruction]/#[account]/#[account_type]/#[effect] → 0.4 actor code
├─ methods/guest/      actor entrypoint via the shim; reproducible Docker build target
├─ artifacts/          lez_program_registry.bin, .imageid, build.json (committed; CI diffs them like LEZ's `artifacts` job)
├─ idl/                lez_program_registry.idl.json (generated, drift-tested)
├─ crates/types/       borsh records, SPEL-rule PDA derivation, validation, header decode; NO LEZ deps
├─ sdk/rust/           lezreg-sdk: read (JSON-RPC + borsh), fetch (Storage ladder), cache, verify-state; feature "tx" (lee/wallet)
├─ sdk/js/             @lezreg/sdk: dependency-free ESM reads, decoders, IDL fetch + hash check; tx via the LNS lez-tx.js encoder
├─ verify/             Dockerfile emitter mirroring risc0-build 3.0.5, image-id + segment-walk compare, report
├─ cli/                `lezreg` (bundles the LEZ wallet CLI as `lezreg wallet …`, as LNS does)
├─ module/             Logos core module `program_registry` (Rust-first mkLogosModule)
├─ ui-example/         `registry_browser` ui_qml + backend + RegistryLabel.qml (reference; not counted for adoption)
├─ action/             GitHub Action (composite): verify / prepare / publish
├─ explorer/           (post-launch) static site; browser reads the sequencer directly
├─ tools/engine-test/  real ELF on V03State (LNS/tools/engine-test pattern)
├─ tools/cycle-bench/  fork of LEZ tools/cycle_bench with our guest; + patches/cycle_bench-add-guest.patch (upstream, ~4 lines)
├─ tools/vectors/      Rust ↔ JS byte vectors
├─ chain/              start-standalone.sh, deploy.sh (REG itself: CreateHeader, then RegisterDeployer at (REG, REG)), resumable
├─ scripts/demo.sh     evaluator demo (clean env, local standalone sequencer + local Storage node)
├─ docs/               DESIGN.md, sdk.md (worked example), build-config.md, idl-ext.md, threat-model.md, CYCLES.md, COST.md
└─ .github/workflows/  ci.yml, repro.yml, e2e.yml, module.yml, cli-release.yml
```

Catalog repo (separate, required): `jefdiesel/lez-registry-catalog`, a fork of `logos-co/logos-modules-release-base`, with `program_registry` and `registry_browser` as submodules.

**Reuse from LNS 0.4 and elsewhere:**

| What | From |
|---|---|
| Instruction → Effect skeleton, `bounded_window`, `event_selector`, tests | `LNS/core/src/lib.rs` |
| `Client::transact/simulate/check_payer/finish/now` | `LNS/cli/src/main.rs:1376-1545`, `862-871` |
| Engine harness | `LNS/tools/engine-test` |
| JS reads and tx encoding, with vectors | `LNS/resolver/names.js`, `LNS/gateway/app-src/lez-tx.js` |
| Module layout and the CLI signing bridge | `LNS/basecamp-module` |
| Storage client | `logos-mirror` `mirror-sdk/storage.rs` |
| Release scripts | `LNS/scripts/publish-cli.sh` |

### 7.1 SDK (Rust, mirrored in JS)

```rust
let reg = Registry::new(Config { sequencer, registry: REG_TESTNET, storage: StorageCfg::local_default(),
                                 pin_peers: vec![], mirrors: vec![] });
let r = reg.resolve(&program_id)?;      // 1 getAccount: Resolved { deployer: Option<Entry>, header: HeaderStatus,
                                        //   third_party_count, verification: VerifyStatus, idl: IdlStatus<SpelIdl> }
println!("{} {} · {} · {}", r.name(), r.version(), r.kind_label(), r.badge());
if let IdlStatus::Ok(idl) = &r.idl { let call = idl.decode_instruction(&calldata)?; }
reg.list(Cursor::Newest, 50)?;          // global walk; incremental from cached seq
reg.search_name("token", 20)?;          // exact via name head + substring over local cache
reg.search_author(&acct, 20)?;          // proven list; search_author_label(q) is labelled unproven
reg.third_party(&program_id)?;
reg.verify_local(&program_id, opts)?;   // Docker; returns Verdict; writes a mark on request
let change = reg.prepare_register(input)?; // actors to declare + bare Instruction; sign via lee wallet, the module, or the CLI
```

- **Privacy mode:** `reg.sync_all()` pulls the whole registry and resolves locally, so the sequencer never learns which program the user cares about.
- **Caches:** keyed by `(address, revision)`, `Link` by `seq`, and blobs by sha256.
- **Read crates:** `crates/types` + `sdk` read have no LEZ dependencies, so the module and third-party apps link them cheaply.

### 7.2 CLI `lezreg`

Every command takes `--json`, which emits stable output with schemas in `docs/schemas/`, and `--dry-run`.

| Command | Purpose |
|---|---|
| `lezreg init [--detect] [--from-spel-idl f]` | write `lezreg.toml` and a CI snippet |
| `lezreg build` | canonical reproducible build (§4.2), printing `image_id` |
| **`lezreg publish`** | **The one command.** Build or reuse the `.bin` → if not deployed: write segments, then `DeployAndRegister`; if deployed: check `image_id` against the header → upload the IDL and source archive → register, or `Update` if registered → self-verify → print a share line. Idempotent: rerun after a version bump and it does an `Update`. Uses a deployer entry if the wallet holds the header key; otherwise it asks before making a third-party entry |
| `lezreg publish --from-release <tag-url>` | re-checks every hash in the Action's payload, uploads to the user's Storage node, and signs locally |
| `lezreg adopt <program> [--project dir]` | register an already-deployed program: reads the header, finds the header key in the wallet, takes metadata from the project or prompts |
| `lezreg deploy [--immutable] [--no-register]` | low-level deploy (`--no-register` gives a plain loader deploy) |
| `lezreg register [--third-party --program X] [--author A]`, `update`, `transfer --to B`, `freeze` | low-level instruction forms |
| `lezreg show <program\|name>`, `list [--kind] [--since-seq]`, `search name\|author\|label\|tag <q>` | discovery |
| `lezreg idl <program> [-o] [--connect]`, `decode <program> <hex>`, `idl push`, `idl gen`, `idl check` | IDL tools (§5, §6.4) |
| `lezreg verify <program> [--revision N\|--live] [--deep] [--mark] [--allow-network]`, `verified list\|forget` | verification (§4) |
| `lezreg stats [--by-month] [--by-author] [--verified] [--group-by-image] [--exclude-keys team.txt]` | the adoption evidence table straight from chain. `--group-by-image` counts distinct live `image_id`s, because one image can sit at many headers and the prize discounts duplicates. Entries with **no live header** (registered before deploy, §3) are left out of the counts, and `image_id` mismatches (stale) are flagged, because the prize counts only "genuinely deployed programs". `--check-idl` fetches every counted IDL and lists any that do not resolve or parse |
| `lezreg pin`, `pin-all`, `storage up`, `lezreg snapshot`, `lezreg doctor` | pinning; the self-hostable pinner (§5.3); a local Storage node for authors without one; static JSON snapshot; checks for Docker version and memory, Storage reachability, sequencer, clock skew and the registry `image_id` pin |
| `lezreg wallet …`, `lezreg module send` | the bundled LEZ wallet; the stdin-JSON signing bridge for the module |

### 7.3 Logos core module `program_registry`

The module is Rust-first `mkLogosModule`. Its layout is copied from `LNS/basecamp-module`, with `logos-module-builder` pinned to the rev that Basecamp 0.3.x locks. Trait: `ProgramRegistryModule`. Every method returns `{success, value, error}` JSON. Method names avoid C++ keywords, the lesson from LNS module 0.9.0 (`register`, `public`, `delete`, `default`).

| Group | Methods |
|---|---|
| Reads | `resolve_program(program_id)`, `program_label(program_id) → {text, badge, kind, verified, stale}`, `get_idl(program_id)`, `decode_instruction(program_id, data_hex)`, `list_entries(cursor, limit)`, `search_name(q, limit)`, `search_author(account, limit)`, `search_author_label(q, limit)`, `third_party_entries(program_id)`, `header_status(program_id)` |
| Verification | `verification_status(program_id)`, `local_verify(program_id)` (spawns `lezreg verify`, emits `verify_progress`) |
| Writes | `prepare_register(json)`, `prepare_update(json)`, `submit(change_json)` |
| Config | `config_get()`, `config_set(json)` (sequencer, Storage API, `pin_peers`, mirrors, trust list) |
| Events | `entry_resolved`, `idl_fetched`, `verify_progress` |

**How `submit` signs.** It uses `lez_core` once that module can declare actors (logos-execution-zone-module#72). Until then it uses the `lezreg module send` bridge. **We need no PDA evidence**, so any generic actor-declaring 0.4 wallet works as soon as one ships. LNS could not do this.

`storage_module ~3.0.0` is an *optional* dependency. An "already" error from `init`/`start` counts as success, because modules share one node.

**Integrating from a third-party UI module.** Declare `"optional_dependencies": [{"name":"program_registry","version":">=0.1.0"}]`. The app then neither bundles us nor breaks if we are absent; check `modules_state.is_ready("program_registry")`.

**Primary path: C++ backend** (`ui_qml` + `"interface":"universal"` + backend, scaffold `#ui-qml-backend`). QML core access is deprecated ("QML will lose core access in the next breaking release", `skills/create-qml-module.md:27`), and that release may well be the Basecamp that ships 0.4, exactly when the five integrators build. So `registry_browser`, `docs/integrate.md` and the outreach PRs all use the typed backend call: `auto r = modules().program_registry.resolve_program(id);`. We ship a copy-paste `RegistryLabel` backend class plus its QML view. We also track the unreleased `"backend":"module"` mode (logos-basecamp #529/#530) and switch the template when it ships.

Legacy QML path (**Basecamp 0.3.x only**; kept for integrators still on 0.3.2):

```qml
Text {
  property string programId
  text: programId.slice(0, 8) + "…"
  Component.onCompleted: logos.callModuleAsync("program_registry", "program_label", [programId],
    function (r) { const v = JSON.parse(r); if (v.success) text = v.value.text + (v.value.verified ? " ✓" : "") }, 5000)
}
```

The integration guide calls this module interface the **registry SDK for Logos modules**, so each integration plainly "integrates the registry SDK" as the adoption criterion words it.

### 7.4 Catalog and releases

**Catalog.**
- A fork of `logos-modules-release-base`, pinned to `logos-modules-release-action/.github/workflows/release.yml@v1.12`. We avoid `@v1`, which is stale at v1.8.
- Variants: `darwin-arm64,linux-amd64,linux-arm64`. `windows-x86_64` is added once the mingw cross-build is proven **(unverified)**. The release tooling has no Android variant.
- Signing mode `inline` with a published DID.
- The optional `storage_nodes` secret gives each `.lgx` a Storage CID as well.
- The `logos-repo.json` URL goes in the README and the solution file.

**CLI releases.** `cli-release.yml` builds linux-amd64 and linux-arm64 on Ubuntu 22.04 runners, darwin-arm64 on macOS runners, and windows-x86_64 on a Windows runner. The prize recommends Windows from 0.4; the read, register and IDL paths are plain Rust, and `verify` on Windows needs Docker Desktop. It writes sha256 files, an `ssh-keygen -Y` signature and `install.sh` (the `LNS/scripts/publish-cli.sh` pattern).

### 7.5 The registry registered in itself

`chain/deploy.sh` is resumable (LNS pattern, with the 0.4 header check through `getAccountView` at the loader). It:
1. writes the CI-built `.bin`'s segments;
2. sends the loader `CreateHeader` for REG and waits for it to settle;
3. sends `RegisterDeployer` rooted at `(REG, REG)`, signed by REG's header key, with the IDL from §6 and the full build tuple.

`DeployAndRegister` can't create the self-entry: its root `(REG, REG)` needs REG's code to exist already, and such a transaction is dropped as `Unknown program` (spike d, N5). The self-entry therefore has no `born_with_deploy`, and the header-key signature is its proof (§3).

Then `lezreg verify <REG> --deep` must report `Match` from a clean machine.

### 7.6 GitHub Action `lezreg-action@v1`

```yaml
on: { push: { tags: ["v*"] } }
jobs:
  registry:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: jefdiesel/lezreg-action@v1
        with: { mode: prepare, rpc: https://<testnet-0.4-rpc>, program-id: ${{ vars.PROGRAM_ID }} }
```

| Mode | What it does |
|---|---|
| `verify` | Canonical offline build, `image_id`, read the live header, compare. Writes a job summary with the three IDs, the builder digest and `network_denied`. Fails on mismatch |
| **`prepare`** (default) | `verify` + generate or check the IDL + JCS + the deterministic source tarball. Attaches `program.bin`, `idl.json`, `source.tar.gz` and `lezreg-release.json` (an unsigned register/update payload with all hashes) to the GitHub release. The author then runs **one local command**, `lezreg publish --from-release <tag-url>`. **The header and author keys never enter CI** |
| `publish` (opt-in) | For teams that accept a CI key: a third-party publisher key, or an author key (author ≠ X, so the header key stays cold). Needs secrets `LEZREG_KEY` and `LEZREG_STORAGE_API`, the latter being a Storage node the team runs |

`prepare` is the default for two reasons:
- an upload from an ephemeral runner would vanish when the runner stops;
- header keys don't belong in CI.

### 7.7 Hosting and cost (zero marginal cost)

**Invariant:** no client depends on a server we run. Every path below is either static or is a fixed-cost peer that the SDK can do without.

| Path | Kind | Expected req/month | Worst case: bot at 500 req/s |
|---|---|---|---|
| Docs, README, install script | static (GitHub Pages / Releases) | < 50k | $0 (static) |
| CLI binaries, `.lgx`, `logos-repo.json` | GitHub Releases + catalog repo | < 10k | $0 |
| Explorer (post-launch) | static assets; the **browser** calls the user-configured sequencer | < 50k page loads | $0. Sequencer load lands on whichever sequencer the user chose; we run none |
| Registry snapshot JSON (post-launch, optional) | written by a GitHub Actions cron every 15 min to Pages; the SDK verifies it against `getAccountView` before use | < 100k | $0. Cost is fixed by cron count, not visitors |
| Our Storage pinning nodes (M2 + one more) | fixed-cost peers, not on the critical path (`pin_peers` can be emptied) | n/a | bandwidth on our own line; no per-request bill |

There is no Worker, no `run_worker_first`, no database and no analytics. If Cloudflare ever fronts the static site, it uses static assets only, with `workers_dev: false`.

**Third-party services, disclosed (prize: "No mandatory centralised external services").** Register, update, lookup, enumerate, search and IDL resolution use only the user's sequencer and Storage node. Only `verify` and the author tooling reach third parties, and each one can be switched off:

| Service | Used by | Off switch / offline input | What still works with it off |
|---|---|---|---|
| Docker Hub (`risczero/risc0-guest-builder@sha256:…`) | `verify`, `build` | `--image-tar <file>` (`docker load`; the tar can come from Storage once mirrored) | everything; verify runs from the local image |
| crates.io (`cargo fetch --locked`) | `verify`, `build` | `--vendor-tar <file>` or the entry's `cargo vendor` archive on Storage (`--vendor`) | everything |
| Git forge (repo clone) | `verify` when no archive, the archive cross-check | `--no-forge`: use the Storage archive only | everything; verify needs the archive |
| Docker Hub `docker/dockerfile` frontend | only if `repro.yml` (which replaces M0 spike b) shows the `# syntax` line is needed | none needed if the built-in frontend works | — |
| GitHub Actions / Releases / Pages | the Action, `publish --from-release`, docs, the snapshot | don't use them; the CLI does everything locally | everything |
| HTTP IDL mirrors | fetch ladder step 5 | off by default | everything |

`lezreg verify --offline` refuses any step that would touch the network, apart from the user's Storage node and sequencer.

---

## 8. Testing and CI

### 8.1 Unit tests (`cargo test -p lezreg-program -p lezreg-types`, no zkVM)

- Every limit and charset; seed vectors (SPEL rule) shared with JS; borsh sizes against §2.3; the shim-generated `Message` decoder (instruction tags, effect tags, and the gap panics).
- The handler auth matrix, with `ReceiveInput` built by hand:
  - `is_authorized` × `from ∈ {None, Some(own), Some(other)}` × every instruction (`auth_matrix_every_instruction`);
  - the PDA-header rule, including a wrong seed, and "every signer is the root" on that path;
  - frozen entries;
  - a revision mismatch;
  - `RequireSigner` from a foreign program;
  - every effect variant from an outsider actor; an unauthorized root emits no effect;
  - `PatchEntry` refuses a record that is not an entry living at the receiving address;
  - inline patch vs `PatchEntry`.
- Index-chain message order, contents and `pda_seeds`.
- Transfer A→B→A and A→B→C→B are refused (named errors); the author-list walk stays correct after A→B→C, and `by_author` stops reading at its limit.

### 8.2 Engine tests (`tools/engine-test`; the real guest ELF on `V03State` via `transition_from_public_transaction`)

These are regression pins for the source facts in §2.1:

1. A deployer registers, with author = X and with author ≠ X. Then lookup, list, and the name and author walks.
2. **Unauthorized deployer entry is refused** (§3 case a). A second register is refused (d).
3. **Composed-transaction attack is refused.** A ~20-line `attacker` guest sends `RegisterDeployer` to `(X, REG)` in a transaction X signed (b). A variant sends `WriteEntry`/`PatchEntry` (c).
4. A third party registers; a duplicate publisher is refused; the third-party list walk works.
5. Update by a non-author is refused (e); a stale `revision` is refused; a transfer without the new author's signature is refused; freeze works; the inline-patch path works.
6. Two registrations in one block, and two with the same name in one block: the lists stay consistent.
7. **Self-admission:** no `PublicAccountEvidence` anywhere, and every transaction succeeds. This includes a brand-new author account, admitted as a signer.
8. **`DeployAndRegister`** against the real loader:
   - header and entry created, `born_with_deploy` set, segments undeclared;
   - a second run reverts because the header already exists;
   - the header's `image_id` equals the guest's.
9. PDA-owned header through a helper guest.
10. Self-registration of REG at `(REG, REG)`: `CreateHeader`, then `RegisterDeployer`; `born_with_deploy` unset.
11. Over-declared but untouched actor: legal (pins the "no unused-actor check" fact).
12. Third-party entry lifecycle (M1 review): update, transfer and freeze of an entry at `T`, refusals for non-authors, transfer to `T` itself and after freeze, then the third-party and author walks.
13. The PDA path can't attribute authorship (M1 review F1/F2): a program the user signed for registers its PDA with the user as author, transfers its entry to the user, or names another program's PDA it holds an inherited grant for. All refused, state unchanged.

### 8.3 End to end against a real standalone sequencer (`e2e.yml`, on push to `main` and on PRs)

1. Build `sequencer_service --features standalone` at `v0.4.0-rc1` (`lez/sequencer/service/Cargo.toml:62-63`), cached per LEZ commit.
2. `setup --config networks/local.json --funding-public-key … --create-channel`, then `start --rpc-listen-address 127.0.0.1:3062` (`Justfile:142-145`; the kill-check commands).
3. Start one Storage v0.5.3 node from the release binary. A second node is added for the two-node fetch test.
4. `chain/deploy.sh` deploys REG (`CreateHeader`, then `RegisterDeployer` at `(REG, REG)`, §7.5), then **asserts header `image_id` == `artifacts/*.imageid`**.
5. `scripts/demo.sh --verify` (the same script evaluators run, with the opt-in verify step on) with `RISC0_DEV_MODE=0` runs:
   - `lezreg publish` of a sample program (deploy and register);
   - a third-party entry;
   - **the rejected unauthorized deployer attempts (a)–(c)**;
   - fee pins from spike (d): each forced revert is charged its full declared gas limit; a `DeployAndRegister` rooted at an undeployed `(R, R)` is never included and charges nothing (N5);
   - update and release #2; transfer with both signatures; freeze;
   - list, search by name and author, and `stats`;
   - IDL fetch across two nodes;
   - degradation: bogus CID, hash mismatch, unparseable IDL, Storage stopped;
   - `UpdateHeader` of the sample, which shows **stale**;
   - `lezreg verify <REG> --deep` giving `Match`.
6. **`repro.yml`** builds with Docker `r0.1.91.1@sha256:fafb377a…`, diffs `artifacts/` (like LEZ `ci.yml:549-591`), and rebuilds once more with our `--network=none` Dockerfile, asserting identical bytes. It also runs the golden check that rebuilding rc1 `clock` gives `6f7b8745…b378`.
7. **`module.yml`:** `nix build .#lgx-portable`, then logosctl `package install`, `module load`, `call program_registry resolve_program <REG>` against the e2e sequencer (the `LNS/.github/workflows/basecamp-linux-test.yml` pattern).
8. JS vectors: JS-built transactions equal the Rust bytes and run through the engine.

The e2e job on `main` is the "CI green on the default branch" evidence.

**Evaluator demo, kept light enough to "succeed without modification".**
- `demo.sh` (local) deploys REG and the sample program from the committed `artifacts/*.bin`, so the functional path needs **no Docker and no risc0 toolchain**. It needs Rust, curl, and a `sequencer_service` binary: a prebuilt one if LEZ publishes one for rc1/0.4 (**unverified**), otherwise a cached `cargo build --features standalone` of the pinned commit, with the time printed up front. It also uses the Storage v0.5.3 release binary.
- `demo.sh --verify` is an explicit, separate step. It needs Docker and gives a time estimate before starting: native x86_64 is measured in `repro.yml` on GitHub Actions; Apple silicon (amd64 emulation, much slower) is not measured by us, since there is no local Docker build (M0 spike c dropped), so the script prints the x86_64 figure with a warning for arm64 hosts. It rebuilds a small sample guest first, then REG with `--deep`. `repro.yml` remains the always-on reproducibility evidence.
- **`demo.sh --testnet`** runs against the public testnet 0.4 RPC and the deployed REG id:
  - read-only by default: resolve `(REG, REG)`, list, search by name and author, fetch and parse the IDL, show a third-party entry, and `verify REG` (with `--verify`);
  - with `--key <funded key>`: one register and one update of a fresh sample program, plus the unauthorised deployer attempt (a) shown as a simulation refusal.
- A scheduled `testnet.yml` job runs the read-only `--testnet` path daily after D-day. It is the "deployed and tested against testnet 0.4" evidence, alongside the local e2e.

### 8.4 cycle_bench

`lez:tools/cycle_bench` hard-codes its guests (`src/main.rs:343-353`). We do both of the following:

- **The fork** in `tools/cycle-bench/` adds `("lez_program_registry", REG, guest)`. It drives every delivery of every operation through the `Meter` `ExecutionEnvironment` (`main.rs:299-341`) at typical and worst-case sizes. `cargo run --release -p lezreg-cycle-bench -- --exec-iters 50 [--prove]` writes `docs/CYCLES.md`: per-delivery and per-transaction `user_cycles`, transaction bytes, the fee at base fee 8, and po2/prove time with `--prove`. Totals are cross-checked against `simulate_public_transaction` gas from `lezreg --dry-run --json`. CI runs it in dev mode and **fails if cycles regress by more than 10%**.
- **Two small patches** to upstream `cycle_bench`: `patches/cycle_bench-add-guest.patch` (~4 lines) adds our guest to its `guest()` table, and `patches/cycle_bench-loader.patch` adds a loader branch to `Meter::handle_message` that calls `program_loader_core::handle_message` as `public_backend.rs:68-77` does. Without the second, a `DeployAndRegister` bench fails with "no guest is registered", because `Meter` only dispatches the native token itself (`main.rs:309-311`). The numbers then reproduce with the official tool (`cargo run --release -p cycle_bench`). Both are offered upstream as a way to bench external guests (Q14).
- **Licensing:** the fork copies LEZ code (`MIT or Apache-2.0`, workspace `Cargo.toml`). Keep its upstream copyright and licence notice in `tools/cycle-bench/NOTICE`, and say in the README that it is a fork, so the "original work / hold the rights" rule is not in doubt.

---

## 9. How the design enables adoption

The prize asks for: 50 programs from 20 independent authors over ≥ 2 months (≥ 10 per month), 20 verified entries, 5 independent UI modules, 20 + 20 testimonials, and macOS users.

| Requirement | Design lever | Action |
|---|---|---|
| **50 programs / 20 authors** | **The easiest way to (re)deploy on 0.4.** Every 0.3 program must be redeployed, and rc1 deploy needs N+1 hand-made accounts. `lezreg publish` does the reproducible build, segment sizing, `DeployAndRegister`, IDL and source upload, and a self-verify, all in one command. The verified deployer entry is a side effect of deploying. Plus `adopt`. No fee, no admin, no name squatting (names are not unique keys) | From day 0: PRs and issues to independent program repos adding `lezreg.toml` and the workflow. Start from the landscape pool (YodHeVauHe, evice-labs, bleu, gateway-fm, adysingh5711, corpetty, equilibriumco, hackyguru, xAlisher, paradoxcomputer, stevancarlon, bristinWild, vpavlin, and the RFP-005 0.4 multisig teams). **Authors sign their own registrations**, and a deployer entry needs their header key, so independence is built in |
| **Independent, weighty authors** | The `author` is a **proven co-signer**, separate from the often-fresh header key, so one developer with 5 programs counts as one author. `stats --exclude-keys team.txt` removes our own keys. **Fresh-chain problem:** on a new 0.4 chain every account starts with no history, and the prize says such accounts "carry little weight". rc1 still derives public account ids with the `/LEE/v0.3/AccountId/Public/` prefix (`lee/state_machine/src/signature/public_key.rs:76-78`), and the derivation is byte-identical to v0.3.1 (`1a003bf`, same file, lines 70-95 diffed). So the same key gives the **same account id on 0.3 and 0.4** (the live 0.3 testnet's version is **unverified**). An author's 0.3 testnet history then attaches to their 0.4 author account | Docs and `lezreg wallet import` tell authors to sign as `author` with their existing 0.3 testnet key, not a new one. Recommend author ≠ header key (it is also the safer setup, §10). Ask Q4 |
| **Parseable IDLs** | `idl gen --from enum\|spel`, `--from-spel-idl`, `idl check` | Offer to generate the IDL as part of each PR |
| **20 verified entries** | The Action's `prepare` mode does an offline rebuild against the live header, and then one local `publish --from-release`. The full tuple is on chain, so evaluators can rebuild | The workflow PR *is* the verified-source path. Each tagged release becomes a verified `Update` |
| **≥ 10 per month for 2 months** | `stats --by-month --group-by-image`, read from chain | Track weekly. Month 1: early adopters, the registry itself, and LEZ-org programs registered by their own maintainers if they want. Month 2: a verified-builds push and integrator-driven registrations. The target is 15 per month for slack |
| **5 UI modules** | `program_label` / `resolve_program` / `decode_instruction`; `RegistryLabel.qml`; `optional_dependencies`; the dependency-free `@lezreg/sdk` for web tools | Integration issues and PRs to paradoxcomputer/zonescan (it already has an ABI/alias registry), evice-labs/e-wallet-sdk (LP-0021), logos-kit, paradoxcomputer/medusa, hackyguru Persona, and the lez-programs AMM app (replacing `amm-registry.testnet.json`). Each integration is built by its own maintainers in their own repo with their own history. Our contributions are disclosed |
| **Testimonials** | Shareable output ("program X · deployer · verified ✓ · immutable"), with screenshots encouraged. No analytics anywhere | Ask each author and integrator after they have used it, and record what they used it for |
| **macOS** | darwin-arm64 CLI and module; `doctor` checks Docker memory; the Action verifies without local Docker | Recruit macOS developers. Our own machines don't count |

---

## 10. Threat model

| Threat | Mitigation | Residual risk |
|---|---|---|
| A non-deployer creates a deployer entry | `(X, REG)` creation needs X's authorization and an empty `pre_state` (§3). Demo cases (a) and (d) | None while X's key is safe |
| **Composed transaction**: a malicious program reached in a transaction X or A signed writes an entry | Root-only rule; effects only from REG; the PDA-header path recomputes the PDA and accepts no signer but the header (§2.4). Engine tests 3 and 13 | A wallet that lets a user sign a *direct* registry instruction they didn't intend. Wallets can render the call from the IDL. A dapp `P` the user signs for can still register or update `P`'s *own* PDA entries, as it may |
| The same composition against the **loader** (`UpdateHeader` to `(X, LOADER)` in a transaction X signed) | Outside the registry's control (a general 0.4 property). The registry *detects* it: claimed and live `image_id` diverge, the entry shows **stale**, and marks lapse | Wallets should flag any declared `(X, LOADER)` actor |
| Header key compromise | `Freeze`; deploy immutable; the UI shows **upgradeable** | With author = X, an attacker can rewrite a non-frozen entry. We recommend author ≠ X (cold header key) |
| Bait-and-switch | `name` is immutable; every revision leaves a write-once `Release` with the full tuple | — |
| Name squatting or impersonation | Names are not unique keys. Ranking: deployer, then fresh, then locally verified, then trusted attestations (post-launch), then the rest. The program-id fingerprint and the proven author are always shown. A restricted charset removes homoglyphs. `author_label` is labelled unproven | A scammer can legitimately verify *their own* look-alike program. Verification proves provenance, not quality; the UI says so |
| Index spam, flooding a popular name list | Every entry costs fees; at most one third-party entry per (program, publisher). Clients page with limits, filter by kind, and sync incrementally. Deployer entries are found by direct address, never through a list | Walking a heavily spammed name list is slow; the snapshot helps |
| **Concurrency or griefing races** | None by construction: no address depends on chain state (§2.4). The only conflict is an author racing their own `revision`, which fails before anything is written. A revert is charged the whole declared gas limit (spike d), so the client always sets the limit to simulated × 1.5 (§2.7), which caps a self-race revert | — |
| Storage griefing (huge or missing IDLs) | Size field plus a 120 KiB cap before fetch; the hash check; the pinner only pins entries with a live header | Authors who never pin lose availability; shown as "IDL unavailable" |
| Malicious IDL or source content | Hash checked before parse; size and depth limits; builds in Docker with no network at compile time and no host mounts except output | A Docker escape is out of scope; `doctor` recommends rootless Docker or Colima |
| `build.rs`/proc-macro tampering (Accretion) | `--network=none` compile; `NeedsNetwork` is shown as weaker; build-script crates are listed | Malicious *committed* source is visible to reviewers; verification doesn't judge intent |
| Builder tag moved | Pinned by digest on chain; refuse on mismatch | The image deleted from Docker Hub: post-launch mirror with `docker save` to Storage |
| Repo deleted or force-pushed | Storage archive with sha256 on chain; clone and archive cross-checked | Both gone means unverifiable, and shown as such |
| Timestamp lying | Validity window `[now, now+10 min)` against the block time | ≤ 10 min skew |
| Lying sequencer | Out of scope (the prize's trust model). Users can point at their own node; the privacy-mode sync reduces exposure | No light-client proofs for public state yet |
| Registry upgrade key abuse | No admin instructions. Entries are actor state and survive upgrades. Every upgrade shows as a stale self-entry, and the SDK pins `EXPECTED_REGISTRY_IMAGE_ID` and warns on change. Plan: mutable during the adoption window, then `UpdateHeader{immutable: true}`, announced ahead (Q11) | Until the freeze, users trust the upgrade key holder. Disclosed |
| Privacy | Reads go only to the user's sequencer and Storage node. `advertise=false` on reads. No analytics. Mirrors are off by default. `sync_all` privacy mode | The sequencer sees full-registry reads, which reveal nothing specific |
| Lost author key | `TransferAuthor` needs the old key, so an author with a lost key is stuck on that entry. The deployer (header key) can still publish a fresh third-party entry, and the docs recommend a backed-up author key | Disclosed |

---

## 11. Build order and milestones (one developer; focused days, estimate)

| # | Milestone | Effort | Exit check |
|---|---|---|---|
| M0 | Spikes:<br>(a) **DONE, PASS** (`spikes/spel-idl/RESULT.md`): released `spel generate-idl` v0.7.0 accepts the shim-annotated toy module unchanged, `SpelIdl` round-trips it with `x-lez04`, and the same source builds an rc1 guest through `spel04-shim`. Findings F1–F5 folded into §2.2, §2.4, §5.1, §6;<br>(b) **replaced** by GitHub Actions reproducible builds (`repro.yml`, §8.3): `RUN --network=none` byte-identity is checked there (decision: no local Docker);<br>(c) **replaced** likewise: no local Docker build on the laptop;<br>(d) **DONE, PASS** (`spikes/d-deploy-and-register/RESULT.md`): `DeployAndRegister` of a 427 KB guest in one transaction on the rc1 sequencer, loader 0 cycles, Mallory negatives refused. Findings folded into §2.1, §2.2, §2.7, §3, §7.5 (self-entry via `CreateHeader` + `RegisterDeployer`, N5);<br>(e) **deferred**: no home port forward (decision, security). IDLs ≤ 120 KiB go over the relay, source via the git pointer; a VPS pinner is decided near launch (§4.4, §5.3);<br>**Post the §12 questions in #builder-hub on day 0** | 2 d | (a), (d) passed; (b), (c) moved to CI; (e) deferred |
| M1 | `crates/types`, `spel04-shim`, program, guest, unit + engine tests 1–13, `artifacts/` repro job | 7 d | engine tests green; `image_id` stable across two CI runs. **Status 2026-11-03: done.** Repo public; `ci.yml` and `repro.yml` green on `1b1def2` (after installing PC/SC on the runners and fetching crates for all platforms before the network-off compile); two runners built byte-identical guests, image `5f42948a…4b55`, committed to `artifacts/` |
| M2 | Rust SDK (reads, walks, cache, tx) + CLI `register/update/transfer/freeze/show/list/search/deploy` + `chain/deploy.sh` | 4 d | the demo core passes on the Mini's local rc1 sequencer. **Status 2026-11-02: done**: `scripts/demo-m2.sh` passes (`docs/demo-m2.log`); SDK mock and e2e tests green. Deviations: IDL CIDs are supplied by the user (`--idl-cid raw`: a raw-codec CID of the IDL bytes) until M3; two-party transfer needs both keys in one wallet; no PDA-header (`via_pda_seed`) path in the CLI. After the 2026-11-02 M2 review, also deferred: §7.1 `prepare_*` builders (actors + bare `Instruction` for signing elsewhere; writes go only through `tx::Writer`, which M6 must split out), the privacy mode `sync_all()` and an entry cache keyed by `(address, revision)` (label/tag/substring search reads the newest `--scan` entries from the sequencer in JSON-RPC batches and reports `scanned`/`truncated`; `Link`s are cached), `IdlStatus`/`VerifyStatus` in `Resolved`, `docs/schemas/` for `--json`, `list --kind`/`--since-seq` (`--after` cursor instead), and `--json` for `lezreg wallet` (it prints the LEZ wallet's text) |
| M3 | Storage publish/fetch ladder, JCS, `IdlStatus`, `pin`, **`pin-all` pinner**, `storage up` | 3.5 d | two-node fetch and degradation tests; an IDL ≤ 120 KiB fetched over the relay. **Status 2026-11-03: partly done.** SDK `storage` (Storage v0.5.3 REST, ureq 3), `jcs` (RFC 8785), `idl` (fetch ladder: cache → `--dir` → node with retries and peer dials; `IdlStatus`; `prepare_idl`/`publish`); CLI `idl get`, `idl push`, `pin`, `pin-all [--every]`. Run on the Mini with three local Storage nodes and the rc1 sequencer: the registry self-entry updated to an uploaded IDL (block 19970); fetched through a second node (hash checked), from cache, and refused cleanly with no node; after `pin` on B and A stopped, a fresh node D fetched it through B. **Finding:** Storage v0.5.3 does not announce a streamed copy whatever `advertise` the stream asks for, so `pin` re-announces explicitly (before the fix a fresh node got 404); `/connect` to a pin peer does not help discovery (as logos-mirror SPIKE.md found). Also done: `register`/`update --idl-cid push --idl-file f` uploads inside the write (self-entry revision 3, block 20110, peer_hint recorded); `storage up` starts a node from a binary the user has (no auto-download), refuses `--block-ttl 0`, 0700 data dir, waits for an address. Not yet: a relay-only fetch, the module/JS side of `IdlStatus` |
| M4 | `verify` (Dockerfile emitter, three-way compare, `--deep`, releases, marks) | 3 d | `verify <REG> --deep` → `Match` |
| M5 | e2e CI + `demo.sh`; cycle-bench fork + upstream patch + `CYCLES.md` | 3 d | **CI green on `main`**; numbers published |
| M6 | Core module + `registry_browser` + `RegistryLabel.qml` + catalog fork; module CI | 4 d | logosctl `call program_registry resolve_program` in CI; loads in Basecamp on macOS  **Status 2026-11-03: module started.** `module/` (Rust-first `mkLogosModule`, builder `de169fd` like LNS; the read SDK comes in by pinned git rev because the builder copies only the crate): `resolve_program`, `program_label` (with the local verify mark), `get_idl`, `list_entries`, `search_name`, `third_party_entries`, `config_get/set`. `nix build .#lgx-portable` on the Mini; logosctl `install` + `module load` + calls against a local rc1 registry all OK. Not yet: writes (`prepare_*`/`submit`), events, `registry_browser`, catalog fork, module CI |
| M7 | JS SDK + vectors; `docs/sdk.md` worked example (resolve id → metadata + IDL); README; FURPS draft | 3 d | JS tests green  **Status 2026-11-03: SDK done.** `sdk/js/lezreg.js` (no dependencies; browser and Node 19+): addresses, borsh decoders for every record, `resolve`, `programLabel`, `list`, `byName`, `fetchIdl` with the sha256 check. `sdk/rust/tests/js_vectors.rs` writes `vectors.json` and fails when it is stale; 21 JS tests pass against it. Not yet: `docs/sdk.md`, README, FURPS |
| M8 | Adoption tools: `init --detect`, `publish` (+ `--from-release`), `adopt`, `idl gen/check`, `stats`, `doctor`, the Action (`verify/prepare/publish`) | 4 d | the Action runs end to end on a sample repo |
| **D-day** | Testnet 0.4 launch: `chain/deploy.sh` (REG self-entry via `CreateHeader`, then `RegisterDeployer`, §7.5), decide on a VPS pinner (§5.3), set network defaults, re-pin `logos-module-builder` (and the logosctl used in `module.yml`) to whatever the 0.4 Basecamp release locks, cut CLI and module releases, publish the program id, enable `testnet.yml`, start outreach | 1 d | `(REG, REG)` gives `Match` from a clean machine; `demo.sh --testnet` passes; the registry IDL resolves over the relay from outside our LAN |
| M9 | *(post-launch)* `Attest` (appended variant) + trust list | 2 d | attest and trust in the demo |
| M10 | *(post-launch)* snapshot cron, static explorer, builder image mirror (`--image-tar` source on Storage) | 2.5 d | the explorer works against a local node |
| A | Adoption window (continuous from M2): outreach, integration PRs, office hours, weekly `stats` | ≥ 2 months | §9 targets |

**Totals.** About **34 focused days before D-day** (after the 2026-11-02 review moved `pin-all` and `storage up` into M3 and added the testnet demo), and about 4.5 after. That is honest: the judges read the base design's "20 days" as 25+.

**Critical path to a deployable registry:** M0 → M1 → M2 → M3 → M5 (~19.5 d). M3 is on it because the D-day exit check needs IDL resolution over Storage (the direct-reachability gate was dropped with spike e). If testnet 0.4 launches earlier than expected, we deploy as soon as M5 is green and ship M3/M4/M6–M8 in the first weeks. The registry is mutable, and new instructions are only appended. Everything before D-day runs on rc1. If rc1 changes before 0.4 final, budget 2–3 days to re-pin.

---

## 12. Open questions for Logos

Posted in #builder-hub on day 0. The first two decide the most.

1. **SPEL.** Does this satisfy "built with SPEL / IDL generated via SPEL" while SPEL has no 0.4 support?
   - A program written in SPEL attribute syntax, compiled through an in-repo 0.4 backend (`spel04-shim`, offered upstream).
   - Its IDL produced by the released `spel generate-idl` v0.7.0 over the real source, plus an `x-lez04` extension.

   Would `logos-co/spel` take a 0.4 backend PR, or the `x-lez04` section as its 0.4 IDL? Is a SPEL 0.4 release planned before testnet 0.4?
2. **Testnet 0.4.** What are the launch date and RPC URL? Will state persist from launch? A reset inside the 2-month window would erase the on-chain timestamps that adoption is measured by. Would re-registered entries keep their month? Will rc1's types, RPC shapes and the PDA prefix be final? Is there a faucet for outside authors?
3. **Deployer definition.** Is "the account whose authorization the loader requires for `CreateHeader` (the header key, or the owning program via PDA)" accepted as "the account that deployed the program"? The 0.4 loader stores no deployer.
4. **Adoption counting.**
   - Do third-party entries count toward the 50, or only deployer entries?
   - Does an entry count for its co-signing `author` account rather than its header key?
   - Do Logos-org programs and UI modules (lez-programs, lez-explorer-ui) count as independent?
   - Does a program-owned (PDA) author count as an author account?
   - Testnet 0.4 starts empty, so every account is "fresh" at launch. Does activity on the 0.3 testnet by the same key (rc1 derives the same public account id, `public_key.rs:76-78`) count as "prior unrelated testnet activity"?
5. **Public sequencer.** Are CORS headers and JSON-RPC batch requests enabled, and what are the rate limits? These decide whether a static browser explorer and fast list walks work.
6. **Wallet path.** When will a 0.4 `lez_core` ship (logos-execution-zone-module#72), and will its generic public transaction let callers declare arbitrary actors? If so, our module signs without the CLI bridge, because it needs no evidence. When will Basecamp and logosctl ship with 0.4 support?
7. **Logos Storage.**
   - What is the relay data cap? We assume ~128 KiB.
   - When does a release with the #1540 and #1556 fixes ship?
   - Is there an official always-reachable pinning node we may list as a default `pin_peer`?
   - What is the recommended persistence for small public files?
8. **Loader read path.** Would LEZ accept a read-only `program_loader::Message::ReadHeader → StateReply` (like native-token `ReadState`)? `Register` could then check the header's `image_id` on chain instead of leaving it to clients. Today REG can't even check that X is a program (spike d): `RegisterDeployer` proves only that X signed, and `born_with_deploy` is the only on-chain proof of co-creation (§3).
9. **Sequencer discovery RPC.** Are accounts-by-program or `getEvents` planned for the sequencer (`getProgramIds` is a TODO, `rpc_server/src/actor/service.rs:280`)? We keep the on-chain lists either way.
10. **Verifiable builds.** Will LEZ standardise a build-config schema and a canonical builder image digest, and keep `risc0-guest-builder:r0.1.91.1` available? We would adopt theirs.
11. **Registry mutability.** Do evaluators prefer an immutable registry header from day one (upgrading by redeploy and migration), or a mutable one with a published freeze date?
12. **Engine semantics to confirm as stable API.**
    - `from == None` holds for exactly the root message of a public transaction.
    - Signers are always admitted.
    - `pda_seeds` self-admission.
    - Over-declared actors are legal.
    - What `pre_state` a second delivery to the same actor sees within one transaction. We avoid this case, but want it documented.
    - Is authorization scoping (for example per-program signer scopes) planned to change?
13. **PDA-owned headers and multisig authors.** Is the factory-deployed pattern expected on 0.4? Is our `via_pda_seed` recomputation the path you would recommend, and is an "author via program" path wanted for multisig authors?
14. **cycle_bench.** Is a fork plus a ~4-line upstream patch acceptable, or would you take an upstream option to bench external guests?
15. **Optional services.** Is an opt-in, disclosed, off-by-default HTTP mirror for IDL bytes acceptable under "no mandatory centralised services"?
16. **Platforms.** Is a Windows `.lgx` (mingw cross-build of Rust-first modules) expected now, and will an Android variant appear in the release tooling?

---

### Appendix A: Criteria map

| Prize criterion | Section |
|---|---|
| On-chain registry, SPEL, published IDL, pointers only, gas-aware | §2, §5, §6, §2.7 |
| Register: name, version, author, description, IDL pointer, code pointer, timestamp; lookup without an indexer | §2.3, §2.4, §2.6, §2.5 |
| Deployer vs third party; signed by the deployer; client distinguishes them; demo rejection | §3, §8.2, §8.3 |
| Update only by the author; transfer with two signatures | §2.4 |
| IDL on Logos Storage | §5 |
| Discovery: by id, enumerate, by name, by author | §2.5, §7.1 |
| Source verification, build config, local marks | §4 |
| Testnet 0.4 end to end | §11 (D-day), §7.5 |
| SDK, CLI, core module (`mkLogosModule`), catalog | §7 |
| Graceful degradation; no mandatory services; opt-in only; no analytics | §5.4, §7.7, §10 |
| cycle_bench | §2.7, §8.4 |
| E2E on a standalone sequencer in CI | §8.3 |
| Registry IDL via SPEL; registered in itself | §6.3, §7.5 |
| README, SDK worked example | §7.1, §11 M7 |
| Platforms (macOS, Linux; Windows and Android as able) | §7.4, §9 |
| Adoption | §9 |
| Design choices: what lives on chain, source hosting, trusted overlay | §2.3, §4.4, §4.5 |

### Appendix B: Comparison with the competitor (edenbd1 "Provenance", v0.3, `d7276eac`)

| Point | Provenance (v0.3) | This design (0.4) |
|---|---|---|
| Deployer proof | The header must sign (0.3 accounts) | Actor `(X, REG)` under the same `is_authorized` the loader uses, plus the root-only rule against composed transactions |
| Header check | In the guest, reading the loader shard. Impossible in 0.4 (no read message) | Client-side live-header compare in the same `getAccount` read; stale and upgradeable states; marks bound to `image_id` |
| Enumeration | Indexer and explorer | Race-free on-chain linked lists over `getAccountView`; the indexer is optional |
| Authors | Publisher account | A proven co-signing author plus an unproven label |
| SPEL | Hand-written IDL ("partial") | The released `spel generate-idl` over the real source, with actors and seeds in `accounts[]`; a 0.4 backend offered upstream |
| Hermetic build | An "unreproducible" state | The exact risc0-build Dockerfile, digest-pinned, `--network=none` compile, lock hash and host Rust on chain, byte compare of the segment chain |
| Onboarding | Manual | `publish` / `DeployAndRegister` / `adopt` / `idl gen` / Action `prepare` |
| Network | v0.3 local; testnet "pending funding" | 0.4 rc1 local now, testnet day one |

### Appendix C: Judge-raised flaws and how this design resolves them

| Flaw (judge, design) | Resolution |
|---|---|
| Hand-written IDL fails both SPEL bullets (lean) | Real source in SPEL syntax; released generator; spike M0(a) passed, so no sidecar is needed (§6) |
| Seal/page races, charged reverts, griefing (lean, adoption) | No state-dependent addresses at all; "pages beside the lists" rejected for the same reason (§2.4) |
| Nothing for third-party programs without an IDL (lean, trust) | `idl gen --from enum\|spel`, `--from-spel-idl`, `idl check` (§6.4) |
| Build tuple missing lock, risc0 and host Rust (lean) | `BuildConfig` has `lock_sha256`, `risc0_build`, `host_risc0_rust`, `image_digest`, `docker_root` (§2.3, §4.1) |
| Publisher = header key weakens the author count (adoption) | A proven co-signing `author`, separate from the header key (§2.4, §9) |
| Growing per-author and per-program records rewritten on every register (adoption) | Fixed-size `Head`s and write-once `Link`s; no record grows with history (§2.3) |
| "Root need not sign" and self-admission unproven (trust) | Both verified in source (§2.1). Self-admission is pinned by engine test 7. `Update` now roots at the signing author, so "root need not sign" is no longer load-bearing |
| `spel generate-idl` may produce an empty `accounts[]` (trust) | Actors are written as `AccountWithMetadata` params with SPEL seeds; spike M0(a) passed: the released generator fills `accounts[]` with flags and PDA seeds (§6.2, §6.3) |
| Verify switched to the live `image_id` while rebuilding the claimed tuple (trust) | Always rebuild a *recorded* tuple (entry or `Release`); `Match`/`MatchStale` verdicts (§4.2) |
| Fan-out cost unmeasured (all) | §2.7 estimates plus preliminary toy numbers from spike (d); measured in M5, with a CI regression gate (§8.4) |
| Optimistic schedule (trust) | Re-estimated at ~32 d before D-day, with a ~16 d critical path and post-launch deferrals (§11) |
| Root-composition threat (critic B3) | Root-only guard in the generated dispatcher (§2.4, §10) |
