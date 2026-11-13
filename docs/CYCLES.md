# Cycles, gas and fees of the LEZ Program Registry (LEZ v0.4.0-rc3)

This document gives the cycle count of every on-chain operation of the registry, and the gas and
fee a real transaction pays for it on LEZ v0.4.0-rc3 (`bafd07be`). The LP-0023 spec asks for it
("Document cycle count usage for each on-chain operation (register, update)"). Every number
below was measured, not estimated. The last section says how to reproduce each one.

## What an author pays, in plain words

Registry operations are cheap and flat. A registration costs 165k to 345k execution gas
(about 1.3M to 2.8M native units at today's minimum base fees). An update costs 67k to 233k
(0.5M to 1.9M). A transfer costs about 100k and a freeze 27k to 48k. These costs depend only
on the size of the metadata, not on the program. The program's size is paid once, when it is
deployed, and since rc3 it is paid twice per byte:

- **Segment writes.** Every byte of the program is sent in a `WriteSegment` transaction and
  pays storage gas.
- **The loader's reads.** The loader now charges 1 execution gas per byte it reads (LEZ #943).
  Each `WriteSegment` that links to a next segment reads that segment, about 98k gas. The
  `CreateHeader` inside `DeployAndRegister` reads the whole chain to compute the image id,
  about one gas per byte of the program.

At base fees of 8, a deploy therefore costs about 24 native units per byte of program plus about
1.5M for the registration. That is about 10.5M for a 430 KB program, 13.3M for the registry's
own 555 KB binary and 24.7M for a 1 MB program. The registration part (`DeployAndRegister`'s
registry transitions, 187,776 cycles) is the same at every size. Updating metadata never
touches the program, so an update costs the same for a 10 KB program as for a 2 MB one.
Upgrading the code (`UpdateHeader`, outside the registry) costs the program's size in gas
again, plus the new segments.

## Gas and fees in rc3 (what the numbers mean)

- **Execution gas** of a public transaction is the cycles settlement charges, `ExecutionCharge::cycles`:
  - the risc0 user cycles of every guest transition (each delivery of the root message, and of
    every message it sends), plus
  - since rc3, `LOADER_CYCLES_PER_BYTE` = 1 per loader actor state byte the loader reads, its
    own pre-state included (`lee/state_machine/src/validated_state_diff/mod.rs`, `run_program_loader`).
  The client's simulation charges the same (`Simulator::handle_message`), so lezreg's gas limit
  (simulated × 1.5) covers the loader.
- **Storage gas** is the transaction's borsh size as a `LeeTransaction` (`lez/chain_state/src/classify.rs`).
  We call it "data bytes"; it is the `PublicTransaction` size + 1.
- **Fee** = `min(execution gas, gas limit) × base_fee_exec + data bytes × base_fee_stor + tip`
  (`fee_core::assess::fee_actual_base`, `lez/chain_state/src/apply.rs`). Both base fees start, and
  stay on a quiet chain, at their minimum of 8 (`lez/programs/fee/core/src/market.rs`). lezreg
  sends tip 0. The "fee" columns below are at 8/8.
- **Reserve.** Before the action runs, settlement holds `gas limit × base_fee_exec + data bytes ×
  base_fee_stor` from the payer and refunds the rest afterwards. The payer must hold at least
  this, not just the fee. lezreg checks it before sending, priced at the next block's ceiling
  base fees.
- **A revert** is charged its whole declared gas limit when the guest panics, which is how the
  registry refuses (rc3 `charged_cycles`: a panic or a session-limit bail pays the full budget).
  Measured on chain below.

## Every operation

Measured on the committed registry binary `artifacts/lez_program_registry.bin` (image
`5ffbf8f5…6bdc`, 555,272 B). Every transaction declares a fee, as a real client's does. The
transition cells give the user cycles of each guest transition, in execution order. `R` is a
registry delivery, `F` the forwarder (the owner program in the PDA case) and `L` the loader.

Field sets:

- **min:** every field at its smallest valid value.
- **typical** and **worst:** the §2.3 size-table fixtures (DESIGN.md). Typical is a 13-character
  name, one sentence, two tags, a 38-byte CID and a source archive. Worst is every field at
  its limit.

Every registration uses a new name and a new author, so it creates both index heads, which is
the most a registration writes. The exception is the row marked "taken name".

| Operation | Case | Transitions (user cycles) | Execution gas | Data bytes | Fee at 8/8 |
|---|---|---|---:|---:|---:|
| RegisterDeployer | min fields, author = X (one signature) | R 43,764 + 24,249 + 24,679 + 25,125 + 25,029 + 21,663 | 164,509 | 1,043 | 1,324,416 |
| RegisterDeployer | typical fields, author ≠ X (two signatures) | R 53,570 + 27,682 + 25,116 + 25,367 + 25,260 + 22,006 + 8,437 | 187,438 | 1,591 | 1,512,232 |
| RegisterDeployer | worst-case fields, author ≠ X | R 106,510 + 45,259 + 25,152 + 25,352 + 25,233 + 22,185 + 8,437 | 258,128 | 3,317 | 2,091,560 |
| RegisterDeployer | typical fields, taken name, author = X | R 52,584 + 27,682 + 25,116 + 26,565 + 26,016 + 22,749 | 180,712 | 1,351 | 1,456,504 |
| RegisterDeployer | the registry's own entry at launch (REG signs), typical | R 52,449 + 27,682 + 23,725 + 24,466 + 24,354 + 21,173 | 173,849 | 1,358 | 1,401,656 |
| RegisterDeployer | `via_pda_seed` through an owner program, typical | F 15,727 + R 55,916 + 27,682 + 25,170 + 25,314 + 25,205 + 21,851 | 196,865 ¹ | 1,553 ¹ | 1,587,344 ¹ |
| RegisterThirdParty | min fields | R 48,279 + 27,119 + 24,249 + 24,306 + 24,972 + 24,827 + 24,848 + 21,385 | 219,985 | 1,171 | 1,769,248 |
| RegisterThirdParty | typical fields | R 58,162 + 32,784 + 27,682 + 24,977 + 25,338 + 25,187 + 25,208 + 21,877 | 241,215 | 1,481 | 1,941,568 |
| RegisterThirdParty | worst-case fields | R 114,142 + 59,535 + 45,259 + 25,152 + 25,352 + 25,189 + 26,477 + 22,904 | 344,010 | 3,204 | 2,777,712 |
| Update | one field (version), author = X (inline) | R 42,730 + 24,249 | 66,979 | 553 | 540,256 |
| Update | one field (version), author ≠ X (`PatchEntry`) | R 17,721 + 47,424 + 27,682 | 92,827 | 621 | 747,584 |
| Update | typical: version, description, IDL; author ≠ X | R 20,431 + 49,845 + 27,682 | 97,958 | 776 | 789,872 |
| Update | every field at its limit, author ≠ X | R 61,468 + 126,595 + 45,259 | 233,322 | 2,857 | 1,889,432 |
| Update | third-party entry, one field | R 17,721 + 49,856 + 27,682 | 95,259 | 621 | 767,040 |
| TransferAuthor | both sign | R 21,744 + 36,918 + 20,867 + 15,504 + 8,437 | 103,470 | 904 | 834,992 |
| Freeze | author = X (inline) | R 26,962 | 26,962 | 440 | 219,216 |
| Freeze | author ≠ X (`PatchEntry`) | R 12,122 + 35,496 | 47,618 | 504 | 384,976 |
| DeployAndRegister | typical fields, author ≠ X, any program size | R 53,908 + L *(program size)* + R 27,682 + 25,116 + 25,367 + 25,260 + 22,006 + 8,437 | 187,776 + loader | 1,687 | see below |

¹ The forwarder is a test guest built on the measuring machine, so its 15,727 cycles (and this
row's totals) can differ by a few cycles elsewhere. The registry rows do not.

What the transitions are:

- **The root delivery** validates and writes the entry, or the patch. It is the largest
  transition and the one that grows with the field sizes.
- **Then one transition each** for the release snapshot, the global head, the name head, the
  author head and the link.
- **The co-signer probe.** Its 8,437 cycles appear only when someone other than the root must
  also sign.
- **Third-party entries** add the `IndexProgram` hop.
- **Update and Freeze by a non-root author** go through `PatchEntry` at the entry: one more
  transition than the inline path (3 instead of 2 for Update, 2 instead of 1 for Freeze).

### Deploys, by program size

A deploy is one `WriteSegment` per 96 KiB of the program's user ELF, written tail first, then
one `DeployAndRegister` (or a plain loader `CreateHeader`, then `RegisterDeployer`).

- **`WriteSegment`** runs no guest. Its execution gas is the loader reading the next segment:
  98,342 for a full one, 0 for the tail. Its data bytes are the segment plus about 436 bytes of
  transaction.
- **`DeployAndRegister`** is the registry part above plus the loader's `CreateHeader`, which
  reads every segment: user ELF bytes + 38 per segment − 32.

| Program | Binary | User ELF | Segments | `WriteSegment` gas / fee (all) | `DeployAndRegister` gas | loader part | `DeployAndRegister` fee | **Deploy + register, total fee at 8/8** |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| forwarder (test guest) ² | 329,920 | 297,496 | 4 | 199,274 / 3,987,856 | 485,392 | 297,616 | 3,896,632 | **7,884,488** |
| ~430 KB (`lezreg_ballast_430k`) ² | 438,380 | 405,956 | 5 | 307,772 / 5,727,008 | 593,890 | 406,114 | 4,764,616 | **10,491,624** |
| the registry itself (`artifacts/`) | 555,272 | 522,848 | 6 | 424,702 / 7,601,072 | 710,820 | 523,044 | 5,700,056 | **13,301,128** |
| ~1 MB (`lezreg_ballast_1m`) ² | 1,028,196 | 995,772 | 11 | 897,816 / 15,186,816 | 1,183,934 | 996,158 | 9,484,968 | **24,671,784** |

² Built on the measuring machine from `tools/engine-test/helper-methods/guest`. The ballast
programs embed 128 KiB and 704 KiB of fixed bytes. Their sizes, and so their gas, can differ by
a few bytes with another toolchain. The loader's charge is checked against the formula above on
every run, whatever the size.

The plain loader path on the registry's chain, for comparison: `CreateHeader` costs 523,044 gas
and 432 data bytes (4,187,808). `UpdateHeader`, which repoints a program at new code, costs
523,110: the chain plus the 66-byte header.

## On chain (rc3 standalone sequencer)

`tools/cycle-bench/chain-costs.sh` ran on a fresh local v0.4.0-rc3 sequencer. It deployed the
registry from `artifacts/` with `chain/deploy.sh`, then sent each operation with lezreg. The
payer was LEZ's local-test genesis account, and the base fees stayed at 8/8 throughout.

- **"Spent"** is the payer's balance before minus after.
- **"Must hold"** is the reserve lezreg's `--dry-run` reports and refuses to go below. For a
  deploy it covers every segment write plus the final transaction, priced at the next blocks'
  ceiling base fees. Settlement refunds the unused part, so the payer needs this much while the
  command runs but spends only the "spent" column.

| Operation (lezreg command) | Gas (simulated = charged) | Data bytes | Spent | Must hold |
|---|---:|---:|---:|---:|
| Deploy the registry itself (`chain/deploy.sh`: 6 segments, `CreateHeader`, self-entry) | | | 13,181,752 | |
| Plain deploy, ~430 KB (`deploy --no-register`: 5 segments + `CreateHeader`) | 406,114 (`CreateHeader`) | 608 | 8,987,824 | 14,780,529 |
| **(a)** `RegisterDeployer` of that deployed program (`register`, typical, author ≠ X) | 187,190 | 1,587 | **1,510,216** | **2,550,726** |
| **(b)** Deploy + register ~430 KB (`deploy`: 5 segments + `DeployAndRegister`) | 598,337 | 1,683 | **10,534,208** | **17,381,187** |
| **(b)** Deploy + register the registry's 555 KB binary | 715,267 | 1,683 | 13,345,120 | 21,136,986 |
| **(b)** Deploy + register ~1 MB | 1,188,381 | 1,683 | **24,722,816** | **37,406,421** |
| **(c)** `Update`, one field (`update --version`) | 92,716 | 621 | **746,696** | **1,270,521** |
| **(c)** `Update`, version, description and IDL | 98,543 | 774 | 794,536 | 1,350,567 |
| `RegisterThirdParty`, typical (publisher Q signs, P pays) | 245,784 | 1,650 | 1,979,472 | 3,341,736 |
| `TransferAuthor` (both sign) | 103,467 | 904 | 834,968 | 1,415,475 |
| `Freeze` | 47,484 | 504 | 383,904 | 659,412 |
| Forced revert: a duplicate `RegisterDeployer` sent with `--force-send --gas-limit 300000` | 300,000 (whole limit) | 1,763 | 2,414,104 | |

What the run confirmed:

- **The fee identity.** Every applied transaction paid exactly 8 × simulated gas + 8 × data
  bytes. lezreg's simulation therefore includes the loader's per-byte charge, and the data
  bytes are the `PublicTransaction` + 1.
- **The revert rule.** The forced revert paid its whole 300,000 gas limit, not the cycles it
  used (2,414,104 = 8 × 300,000 + 8 × 1,763).

Why the chain differs slightly from the engine table:

- The CLI's IDL and archive CIDs are 36 bytes, not 38.
- On chain the payer had registered before and every registration reused the name
  `token-program`, so its author and name heads already existed: about +4,400 cycles in
  `DeployAndRegister`.
- Every segment write also carries the payer's signature, about +176 bytes each.

## How each number was produced

**Machine-independent.** Every number in the "Every operation" table, and the registry row of
the deploy table, comes from the fixed bytes of `artifacts/lez_program_registry.bin` and fixed
inputs. The same command gives the same numbers on any machine: risc0 user cycles are
deterministic for a given binary and input. The forwarder and ballast rows depend on a locally
built guest (notes ¹ and ²).

**The method is LEZ's `cycle_bench`.** `cycle_bench`
(`tools/cycle_bench/src/main.rs` at v0.4.0-rc3) measures an operation as all its transitions:

- it drives `WholeExecution::execute` with its own `ExecutionEnvironment` (`Meter`);
- it runs each guest transition through `risc0_zkvm::default_executor()`;
- each transition's `info.cycles()` is one row.

As released, it can't measure the registry. Its guests (`guest()`) and inputs (`cases()`) are
hard-coded, and its `Meter` has no branch for the program loader. So:

1. **Our harness, `tools/engine-test/src/bench.rs`**, runs the same method in our workspace.
   - Actor states come from a live engine state (`V03State`, LEZ's real state machine), not
     fixtures.
   - Guests come from the committed binary.
   - Loader transitions run `program_loader_core::handle_message` at 1 gas per byte read, as
     settlement's `run_program_loader` does.
   - Every operation is then applied to the real state machine, and **the per-transition rows
     plus the loader's bytes must equal the charged gas** (`ExecutionCharge::cycles`), or the
     run fails. So the cycle_bench method and the engine test agree exactly, on every row of
     this document.

   ```sh
   cargo run --release -p lezreg-engine-test --bin lezreg-cycles -- --json cycles.json --cases cases
   ```

   Run on branch `cycles` (based on main `ca7c8bf`; the registry binary is that commit's `artifacts/`) on the Mac mini (aarch64, macOS) with the risc0 3.0.5 executor.
2. **LEZ's own tool, patched.** `tools/cycle-bench/cycle_bench-external-cases.patch` adds
   `--elf`, `--case` and a loader branch to LEZ's `cycle_bench`. Run on the case files from
   step 1, it reproduced all 113 guest transitions of the 21 cases that run a guest, cycle for cycle, and all 32 loader transitions byte for byte. That is every row of this document; we compared its `cycle_bench.json` with `lezreg-cycles --json` (`tools/cycle-bench/README.md` has the commands).
3. **CI keeps this document honest.** `tools/engine-test/tests/cycles.rs` (run by `ci.yml`)
   checks:
   - the execution gas and data bytes of every artifact-only row, exactly;
   - the loader's charge against the formula, for every deploy size;
   - that `DeployAndRegister`'s registry part does not change with program size.

   A change to the program, to LEZ or to these fixtures that moves a number fails CI until
   this document is updated.
4. **On chain** is `tools/cycle-bench/chain-costs.sh` against a fresh standalone sequencer built
   from v0.4.0-rc3 (`--features standalone`, as `e2e.yml`), with lezreg built from this branch before its rebase onto `74a0010`. That rebase changed only
   how the CLI reports an IDL's format, not what it sends, so the chain run was not repeated.
   - Each fee there is the payer's balance before and after.
   - Each reserve is what lezreg's `--dry-run` printed.
   - The chain's own state, account ids and clock differ from the engine run, so its cycles
     can differ slightly from the table. The identity `fee = 8 × gas + 8 × data bytes` held
     for every transaction.

Not measured: prover cycles and proving time (`cycle_bench --prove`). Public transactions are
executed, not proven, by the sequencer. The Mini this was run on must not run unthrottled
proofs.
