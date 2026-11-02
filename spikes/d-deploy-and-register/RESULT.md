# Spike (d): DeployAndRegister in one transaction (M0)

**Verdict: PASS.** No fallback is needed for key-held headers.

- **Chain:** local LEZ `v0.4.0-rc1` (`1806e87`) standalone sequencer on the Mini, RPC `127.0.0.1:3062`, 2026-11-02, blocks 1351–1399.
- **Base fees:** exec 8 and storage 8 throughout.
- **Full log:** `out/run.log`. Account ids are in `out/accounts.env`.

## What was built

| Part | What it is |
|---|---|
| `core/` (`reg_core`) | Toy registry **REG**. `DeployAndRegister{first_segment, immutable, record}` at root `(X, REG)` asserts `from.is_none()`, `is_authorized` and `pre_state.is_empty()`. It then `set_state(Entry{born_with_deploy:1, record})` and sync-sends the loader `CreateHeader{first_segment, immutable}` to `(X, [0xFE;32])`. |
| `Register{record}` | The step-2 instruction of the two-transaction fallback. |
| `DeployAndRegisterNoCheck` | Test only. REG skips its own auth check, so the test reaches the loader's check. |
| `LoaderMessage` | Mirrors `program_loader_core::Message`. `reg_client check-encoding` confirms the bytes are identical. |
| `methods/` | risc0-build guest, in the same pattern as `names_methods`. REG is `lezreg_spike_reg.bin`: 316,008 B, user_elf 283,584 B, 3 segments, image `bbbe71d8…`. |
| `client/` (`reg_client`) | Built on the rc1 wallet, using the LNS 0.4 CLI's simulate/send/finish pattern. Segment states are fed to the simulator so the loader runs there exactly as it does at settlement. It refuses any RPC that is not loopback. |
| `run.sh` | Runs every step below. It writes to `out/run.log`. |

- **Target program X:** the real LNS 0.4 guest `logos_names.bin`. It is 459,452 B in total, with a **427,028 B user_elf in 5 segments**, image `a48b16bf…dd3a`.
- **Segment reuse:** one segment chain was written and every header below points at it, because one image can sit at many headers.

Commands (on the Mini):
```
cd /Volumes/exos162/logos/lez-program-registry/spikes/d-deploy-and-register
export PATH=$HOME/.cargo/bin:$HOME/.risc0/bin:/opt/homebrew/bin:$PATH CARGO_TARGET_DIR=$PWD/target
cargo build --release -p reg_methods -p reg_client
./run.sh            # wallet = ./wallet, a copy of lp0023-checks/wallet; own payer P funded 200M from 26pk…
```

## Results

| # | Transaction | Signers | Simulated cycles | Tx bytes | Fee charged | Outcome |
|---|---|---|---|---|---|---|
| — | WriteSegment ×5 (target, 427 KB) | segment keys + P | 0 | ~98 KB each | 3,440,448 total (791,328 per 96 KiB segment) | applied |
| — | WriteSegment ×3 (REG) + CreateHeader R | | 0 / 0 | | 2,283,104 + 4,864 | REG deployed, image matches |
| **T1** | **DeployAndRegister X** (X signs, P pays) | X (1 nonce) + P | **9,738** | 693 | **83,456** | **APPLIED** in block 1377 |
| T2 | fallback step 1: CreateHeader X3 | X3 + P | **0** | 607 | 4,864 | applied |
| T3 | fallback step 2: Register X3 | X3 + P | 7,613 | 587 | 65,608 | applied |
| N1 | Mallory: Register `(X4, REG)`. X4 already has a header (the fallback's squat window) and does not sign | M | refused | 409 | 803,280 | **REVERTED**: `REG: X must sign` |
| N2 | Mallory: DeployAndRegister on a fresh X5 that does not exist | M | refused | 539 | 804,320 | **REVERTED**: `REG: X must sign` (a revert, not a rejection at admission) |
| N2b | the same after X5 exists (funded 1 unit) | M | refused | 506 | 804,056 | **REVERTED**: `REG: X must sign` |
| N3 | Mallory: DeployAndRegisterNoCheck (REG's check removed) | M | refused | 506 | 804,056 | **REVERTED** by the loader: `CreateHeader target must be an authorized account` |
| N4 | honest DeployAndRegister again on X | X + P | refused | 680 | 805,448 | **REVERTED**: `REG: entry already exists`; the entry is unchanged |
| N5 | self-entry: DeployAndRegister rooted at `(R2, R2)`, with R2 not yet deployed | R2 + P | refused | — | **0** | dropped at settlement and never included. Log: `failed fee classification: fee action failed: Unknown program, skipping it` |

The sequencer log for this run has exactly these panics: 6× `REG: X must sign`, 2× `CreateHeader target must be an authorized account`, 2× `REG: entry already exists`. Each one appears twice, once from the client's own simulation and once from the sequencer.

**Read-back. One `getAccount(X)` returns both actors:**
```
getAccount AWo5M9EgWBsHVzeTUxNXUHZPenrgt3474TbbJsz3rBiE nonce 1 actor_states:
   BLR5We4Zfe8X4HfEZ8DUyL5pSerC6dsNSPCFxd42cEnN 24 bytes 010112000000782d...   <- (X, REG) Entry, born_with_deploy=1
   JAQ5MVHbCkSYRzXunsrNuM2m1LS859PGveHfoYPAmcvZ 65 bytes a48b16bf0fb4e7e2...   <- (X, LOADER) header
```

The header's `image_id` is `a48b16bf…dd3a`, which equals `Program::new(logos_names.bin).id()`. Its `first_segment` is `4QFe…7Nno`.

## DESIGN.md claims checked

| Claim | Result |
|---|---|
| §2.1 / §2.7: the loader is unmetered (`public_backend.rs:68-77`) | **Confirmed.** Standalone CreateHeader over 427 KB of segments: **0 cycles**, and its fee of 4,864 is 607 tx bytes × 8. DeployAndRegister minus Register is **2,125 cycles**: the REG side's one `send_sync` plus a longer message, with nothing that scales with program size. The fee difference is 17,848, about 2,125 × 8 + 106 bytes × 8. The simulator's cycles match the fee charged. |
| §2.1: a program can deliver `CreateHeader` to the loader `[0xFE;32]` in the same transaction | **Confirmed** on a running sequencer (T1). Only `(X, REG)` and `(X, LOADER)` are declared; the segment accounts are not. |
| The header key signs once and is authorized in both programs | **Confirmed.** X's nonce is 1 after T1. The two-transaction path needs X to sign twice (X3's nonce is 2). |
| Only X can create `(X, REG)` | **Confirmed** (N1, N2, N2b). REG's `is_authorized` check fires first. Without it, the loader's check also refuses (N3), so this is defence in depth. |
| §2.7: a DeployAndRegister costs about what a RegisterDeployer costs, whatever the program size | **Confirmed** for this toy: 83,456 vs 65,608. The bulk of a deploy's cost is the WriteSegment bytes: about 0.79M per 96 KiB, or **3.44M for 427 KB**. |
| §2.2 / §11 D-day: the REG self-entry `(REG, REG)` is "created by `DeployAndRegister` at launch" | **Does not hold.** The root `(REG, REG)` needs REG's code already deployed, so the transaction is dropped as `Unknown program` (N5, no fee). The self-entry must be **CreateHeader, then RegisterDeployer**: two transactions, without `born_with_deploy`. This is a doc fix only. |

## Findings to fold into the design

1. **A charged revert costs the whole declared gas limit, not the cycles used.**
   - Each negative was sent with a 100,000 gas limit and paid 800,000 + bytes × 8.
   - The applied transactions paid only the cycles they used.
   - So the CLI's "simulate first, limit = 1.5 × simulated" rule (`LNS/cli/src/main.rs:2041-2050`) also caps what any revert can cost.
   - Mallory pays for her own failed attempts; the victim pays nothing.
2. **The two-transaction squat window is closed.**
   - Between CreateHeader and Register, nobody but X can write `(X, REG)` (N1).
   - So the fallback is safe; it only lacks `born_with_deploy`.
3. **REG cannot check that X has a header in `Register`.**
   - A 0.4 actor sees only its own state.
   - So `RegisterDeployer` (the fallback's step 2) proves only that X signed, not that X is a program.
   - Clients must read `(X, LOADER)` with the same `getAccount` call, which they already do.
   - `born_with_deploy = 1` is the only on-chain proof that the entry and the header were made together.
4. **A failed root dispatch costs nothing.** A transaction whose root program is not deployed is never included and never charged (N5).
5. **No admission evidence is needed for the new header X.** As a signer it is admitted automatically (T1).
   - A non-signing, non-existent X in Mallory's transaction was not refused at admission either (N2).
   - REG ran and refused it.

## If it had failed

The fallback would have been the DESIGN §2.7 two-transaction path: CreateHeader, then Register. Measured here, it costs 4,864 + 65,608 = 70,472 against 83,456 for one transaction. The cost is one more transaction and one more signature from X, and the loss of `born_with_deploy`. It is still needed for the self-entry `(REG, REG)` and for PDA-owned headers (Q13).

## Caveats

- REG here is a toy:
  - it has 2 declared actors, against 7–9 in the real `DeployAndRegister`;
  - it has no index chain and no Release.
  - The absolute cycles therefore say nothing about the real registry. Only the deltas, and the zero for the loader, carry over.
- The negative transactions were forced past a refusing simulation, so their fees reflect the forced 100k gas limit.
- The PDA-owned header case (Q13) was not tested.
