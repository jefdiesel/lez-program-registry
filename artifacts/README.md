# artifacts/

The canonical registry guest, built reproducibly by `.github/workflows/repro.yml` with
`cargo risczero build` (risc0-build 3.0.5) in `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a…48f0`,
like LEZ's own `artifacts` job.

| File | What |
|---|---|
| `lez_program_registry.bin` | the guest program binary that gets deployed |
| `lez_program_registry.imageid` | its image id (hex of LEZ `Program::id()`, 8 little-endian words) |
| `build.json` | builder image digest, risc0-build, host rzup Rust, manifest, binary, commit, `Cargo.lock` sha256 |

**Committed 2026-11-03 from `repro.yml` run on commit `1b1def2`:** two independent runners each
built the guest with the stock `cargo risczero build`, rebuilt it with the network off, and got
byte-identical binaries with image id `5f42948ad093b8b0101237c8639a37f90d9d0ddb06dcf029735b91a1b6944b55`.
Every later `repro.yml` run diffs its build against these files. Builds on a developer machine
without Docker (the M1/M2 smoke and demo runs) give a different image id and are not canonical.
`build.json`'s `host_risc0_rust` is empty: the workflow's `rzup show` parse needs fixing.
