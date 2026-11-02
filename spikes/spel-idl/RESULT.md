# Spike (a): released SPEL `generate-idl` over SPEL-syntax source, compiled for LEZ 0.4

**Verdict: PASS.** The same source file, `program/src/lib.rs`, goes through two tools without any changes:

1. The released `spel generate-idl` (v0.7.0, `4107dcb`) accepts it. It emits spec-0.1.0 JSON with the instructions, args, accounts, PDA seeds and types we expected. SPEL v0.7.0's own `SpelIdl` type parses that JSON back, and serializing it again gives the same JSON.
2. A LEZ v0.4.0-rc1 guest ELF builds from it through the in-repo `spel04-shim`. The build uses the local risc0 toolchain on the Mini and no Docker. The shim's PDAs match SPEL's derivation.

The spike also found five DESIGN corrections, listed under "Findings" below. F1–F3 change the design. F4 and F5 are conventions to document.

## Layout

| Path | What |
|---|---|
| `program/src/lib.rs` | Toy registry in SPEL syntax: `#[lez_program(instruction = "Instruction")]` module; `register`/`update`/`freeze` with `AccountWithMetadata` actor params, `#[account(mut, signer, init, pda = [literal(..), account(..), arg(..)])]`; `#[account_type] Entry`, `Release`; helper `ContentRef`; `#[effect_handler]` fn; `-> Response` returns |
| `program/Cargo.toml` | Picks the backend: `spel = { package = "spel04-shim", path = .. }`. The source only says `use spel::prelude::*` |
| `spel04-shim/` | Runtime: `AccountWithMetadata` as a 0.4 actor handle, SPEL `ToSeed`/`compute_pda` seed rule, effect tag shift (0xC0..), RequireSigner probe (0xFF) |
| `spel04-shim-macros/` | `#[lez_program]` / `#[account_type]` proc macros (~330 LOC) |
| `methods/`, `methods/guest/` | risc0 `embed_methods`; guest = `lee_core::actor_entrypoint!(raw toy_registry::__lez_dispatch);` |
| `program/tests/vectors.rs` | Host tests: PDA vectors and wire bytes |
| `idl-check/` | Separate workspace on `spel-framework-core` git tag v0.7.0: `SpelIdl` parse, assertions, round trip, `x-lez04` tolerance |
| `seedcheck/lib.rs` | Copy of the toy with `revision: u64` plus a `probe` ix, used only to feed the spel CLI's PDA resolver |
| `out/` | Generated IDLs, spel dry-run JSON, guest build log |

On the Mini everything lives under `/Volumes/exos162/logos/lez-program-registry/spikes/spel-idl/`: binaries in `_deps/spel-root/bin/spel`, and target dirs `_deps/target-spel` (SPEL, 1.94.0) and `_deps/target-04` (0.4, 1.98.1).

## Commands and trimmed output (Mini)

```sh
export PATH=$HOME/.cargo/bin:$HOME/.risc0/bin:/opt/homebrew/bin:$PATH
cd /Volumes/exos162/logos/lez-program-registry/spikes/spel-idl
git clone --branch v0.7.0 --depth 1 https://github.com/logos-co/spel.git _deps/spel     # 4107dcb
# documented install (README: `cargo install --path spel-cli`), toolchain from its rust-toolchain.toml
CARGO_TARGET_DIR=$PWD/_deps/target-spel cargo +1.94.0 install --locked --path _deps/spel/spel-cli --root _deps/spel-root
#   Finished `release` profile in 2m 10s; Installed package `spel v0.7.0`
```

**1. The released generator over the real source.**
```sh
_deps/spel-root/bin/spel generate-idl program/src/lib.rs > out/toy_registry.idl.json   # exit 0, empty stderr, 6835 B
#   sha256 356e0707…1e70, identical on re-run (deterministic)
```
```
register accounts: program[w,s]  author[s]  release[w,init,pda: const "toyreg/v1/rel", account program, arg revision]
                   head[w,pda: const "toyreg/v1/head"]  name_head[w,pda: const "toyreg/v1/name", arg name]
         args:     author_id [u8;32], name string, version string, idl {defined ContentRef}, image_id [u8;32], revision u32, now u64
update   accounts: author[w,s] entry[w] release[w,init,pda: const, account entry, arg revision]
         args:     entry_id, revision u32, version option<string>, image_id option<[u8;32]>, now u64
freeze   accounts: author[s] entry[w]   args: entry_id, now
accounts[] = Entry, Release     types[] = ContentRef     instruction_type = "Instruction"
```

**2. Parse back with SPEL's own type** (`idl-check`, `spel-framework-core` tag v0.7.0):
```sh
CARGO_TARGET_DIR=$PWD/_deps/target-spel cargo +1.94.0 run -q --release --manifest-path idl-check/Cargo.toml -- out/toy_registry.idl.json
#   parsed: name=toy_registry version=0.1.0 instruction_type=Some("Instruction")
#   … (same instructions/accounts/args) …
#   roundtrip_equal=true
#   x-lez04_tolerated=true          (extra top-level "x-lez04" key still parses as SpelIdl)
#   IDL-CHECK PASS
```

**3. A LEZ 0.4 guest built from the same source.** The local risc0 toolchain does the build; Docker isn't used.
```sh
CARGO_TARGET_DIR=$PWD/_deps/target-04 cargo build --release -p toy_methods      # exit 0
#   riscv-guest/toy_methods/toy_registry_guest/riscv32im-risc0-zkvm-elf/release/toy_registry_guest
#   ELF 32-bit LSB executable, UCB RISC-V; 326168 B (.bin 358592 B, sha256 e613f6e9…0c7b)
#   TOY_REGISTRY_GUEST_ID = [3113189137, 2129548272, 3563567950, 223374838, 62010385, 2327838761, 1357747979, 2840777959]
#   the ELF contains the handler's and the shim's guard strings ("entry already exists", "instructions must be the transaction's root message", "co-signer did not sign")
```

**4. Host tests of the code the shim generates:**
```sh
CARGO_TARGET_DIR=$PWD/_deps/target-04 cargo test -p toy_registry -- --nocapture
#   test wire_is_borsh_enum_in_fn_order ... ok      (Freeze = tag 2, then [u8;32], then u64 LE)
#   test pdas_match_spel_rule_and_spel_cli ... ok   (actors_to_declare(Register): root, author, release, head, name_head)
```

**5. Cross-check against the released spel CLI's own PDA resolver** (`--dry-run=json`, program id `a5…a5`):
```
spel --idl out/seedcheck.idl.json -p a5…a5 --dry-run=json -- probe --author 33…33 --name alice
  head      7oS5b5cWo6iM46LfbvBdrAVgkM74XL6svxmEuHotKyPr   == shim 650b321f…47cd   OK
  name_head DYxSzNmxQy8KPZJahuVvnnSPQhijPTKyUCzJU4T6Zpok   == shim ba7bf18c…5a03   OK
spel … -- update … --revision 2   (u64 variant)
  release   9BqCZBWX…FmEz  = big-endian u64 seed;  framework ToSeed / DESIGN / shim (LE) give 3BnuJBK3…pwjq   MISMATCH (see F3)
spel … (toy, revision: u32) -- update …
  ❌ Arg 'revision' has unsupported type for PDA seed. Expected bytes32, u64, u128, or string.
spel … -- register --idl '{…}'
  ❌ --idl: 'ContentRef' is struct; only enum arguments are supported for now
```

## How "write once" works (the key question)

- **The generator is purely syntactic.** It never compiles the file or resolves imports. Source written for the shim is therefore valid generator input, as long as it uses the exact tokens the generator matches:
  - `#[lez_program]` on an inline `mod` (`path().is_ident("lez_program")`);
  - `#[instruction]` on fns;
  - param types whose last segment is `AccountWithMetadata`;
  - `#[account(mut|init|signer|owner = ..|pda = ..)]`. Any other constraint key is a hard error.
  - `#[account_type]` on structs.
- **The backend is chosen in Cargo.toml, not with cfg.** The program writes `use spel::prelude::*;` and the manifest maps `spel` to `spel04-shim`. If SPEL ships 0.4 support, the switch is a manifest change *plus* whatever conventions upstream picks for the things this shim had to invent: the `<name>_id` arg pairing, `#[effect_handler]`, the 0xC0 effect tag shift and the RequireSigner tag 0xFF. DESIGN §6.5 ("the switch is a dependency change") overclaims in the same way. **cfg gating would not work.** `#[cfg_attr(feature = "x", lez_program)]` is invisible to the generator, because it matches the literal path `lez_program`. The dependency collector also drops `#[cfg(feature …)]` items (`is_cfg_excluded`).
- **The shim's `#[lez_program]` strips the inner attributes.** It removes `#[instruction]`, `#[account(..)]` and `#[effect_handler]` before rustc sees them. rustc rejects unknown attributes on fn params, so only the outer macro makes this syntax compile.
- **`-> Response` is fine.** `parse_instruction` only walks `sig.inputs`. DESIGN §6.3 fallback 1 (`-> SpelResult`) is not needed.
- **Fallback 2 (a sidecar `interface.rs`) was not needed.** It was not exercised.

What the shim generates, and what the full shim still has to add, is in the "M1 sketch" section at the end.

## Findings: DESIGN corrections

- **F1. v0.7.0 does not order instructions by the enum (DESIGN §5.1, §6.1 are wrong for the release).**
  - `order_by_enum_variants` (`d58984a`) is in `v0.7.0-rc.3` and `main`, **not in `v0.7.0`**: `git tag --contains d58984a` gives `rc-latest, v0.7.0-rc.3`.
  - In v0.7.0, `instruction = "Instruction"` only sets the `instruction_type` string. Instructions come out in fn declaration order, and the generator never reads or checks the enum.
  - Fix, as done in the spike: the shim **generates** `enum Instruction` from the `#[instruction]` fns. Variants follow fn order and fields follow arg order. Wire tag order and borsh field order then equal the IDL's order by construction.
  - The source therefore has **no hand-written enum**. DESIGN §2.4/§6.2 should drop "hand-written; order = wire tags" and say "generated; append new fns at the end".
  - Cost: none; this is simpler. Drift test 3 still guards against it.
- **F1b. Some §5.1 fields are not filled by the generator.** v0.7.0 emits `discriminator: null`, `spec: null` and `metadata: null`. DESIGN §5.1 lists `discriminator`/`metadata` as part of the format; they must come from `lezreg-idl-extend`, not from the generator.
- **F2. The v0.7.0 CLI's wire format is not borsh.**
  - `spel` v0.7.0 encodes instruction data with risc0 serde: a u32 variant, then one u32 word per u8. For example, `probe --name alice` gives `03000000 05000000 616c6963 65000000`.
  - borsh (u8 variant + args) arrived on `main` with the LEZ 0.2.5 port (`ba8d7f1`, `serialize_to_borsh`), after v0.7.0.
  - The IDL itself has no wire format, so the IDL work is unaffected. But DESIGN §2.4's "A SPEL-style client that encodes only the instruction produces a valid transaction" is **false for the released CLI**. It holds only for unreleased `main`.
  - Fix: don't claim released-SPEL-client compatibility for tx encoding. Drift test 3 needs our own IDL-driven borsh encoder (lezreg SDK). v0.7.0's encoder can't serve as that oracle.
- **F3. SPEL's two PDA rules disagree on integer seeds.**
  - `spel-framework-core::pda::ToSeed` (on-chain side, what DESIGN §2.2 and the shim follow) encodes u32/u64 as **little-endian, left-aligned**.
  - The v0.7.0 CLI resolver (`spel-cli/src/pda.rs`) encodes u64/u128 as **big-endian, right-aligned**, and **rejects u32** outright.
  - So `spel` can't derive the `Release` PDA (`arg revision: u32`). With u64 it derives a different address (9BqC… vs 3Bnu…).
  - Const, account and string seeds match exactly (verified above).
  - Fix, cheap: use **no integer `arg` seeds** in any `pda = [...]`. For `Release`, either take `revision_seed: [u8; 32]` with the guest asserting it equals `revision.to_le_bytes()` padded, or key the release by `arg("version")` (a string, already ≤ 32 B). The cleaner option is a `[u8;32]` arg.
  - Also report the inconsistency upstream (§6.5).
- **F4. The v0.7.0 CLI can't take struct args** (`'ContentRef' is struct; only enum arguments are supported`).
  - The IDL still describes them correctly (`types[]` has `ContentRef`). Only "drive the registry with the stock `spel` CLI" breaks.
  - To keep that possible, flatten `ContentRef`/`SourceRef`/`BuildConfig` into scalar args. Otherwise accept it, since `lezreg` is the real client. Recommendation: accept it and drop any claim that the stock spel CLI can submit registry transactions.
- **F5. Non-root, non-PDA actors need an id arg.**
  - A 0.4 guest sees only its receiver. An account such as `author` (a co-signer) can't be resolved from the transaction.
  - Shim convention, enforced at compile time: account `foo` requires an arg `foo_id: [u8; 32]`. The IDL then shows `author` under accounts **and** `author_id` under args.
  - This matches DESIGN's `RegisterDeployer { author: AccountId, .. }`, but the arg can't be named `author`, because Rust forbids two params with one name.
  - Document the `<name>_id` pairing in `x-lez04`.

## M1 sketch: what the full `spel04-shim` must do (the spike already does the starred items)

- \* Parse SPEL syntax and strip helper attributes.
- \* Generate `enum Instruction` (F1).
- \* Generate `__lez_dispatch` with these guards:
  - root-only (`from.is_none()`);
  - `from_own_program` for tags ≥ 0xC0;
  - root `signer` ⇒ `is_authorized`.
- \* Derive PDA actors with SPEL's rule and attach `with_pda_seeds` when sending to them.
- \* Send a RequireSigner probe for non-root signers, skipped when the signer equals the root (the deployer-entry single-key path).
- \* Generate host `actors_to_declare(root, program, &Instruction)`.
- \* Apply borsh derives for `#[account_type]`.
- \* Find the aliased crate name (`proc-macro-crate`).
- Still to do:
  - the `via_pda_seed` PDA-header exception and `timestamp_window` (DESIGN §2.4 rules 1 and 4);
  - typed `#[effect]` fns instead of one raw `#[effect_handler]`;
  - `init` ⇒ an `assert!(pre_state.is_empty())` hook in the receiving effect handler;
  - emitting the `x-lez04` constants;
  - compile errors for integer `arg` seeds (F3);
  - a check that new `#[instruction]` fns are only appended (compare against a committed tag list).

  Estimated at 400–600 LOC total, which still fits DESIGN's estimate.

## Not done (out of scope for this spike)

- No deploy, and no call of the toy guest on the running rc1 sequencer. Spike (d) covers deploy; engine tests come in M1.
- No `x-lez04` generation; the spike only checked that the extension is tolerated.
