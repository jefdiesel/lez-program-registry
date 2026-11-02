# artifacts/

The canonical registry guest, built reproducibly by `.github/workflows/repro.yml` with
`cargo risczero build` (risc0-build 3.0.5) in `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a…48f0`,
like LEZ's own `artifacts` job.

| File | What |
|---|---|
| `lez_program_registry.bin` | the guest program binary that gets deployed |
| `lez_program_registry.imageid` | its image id (hex of LEZ `Program::id()`, 8 little-endian words) |
| `build.json` | builder image digest, risc0-build, host rzup Rust, manifest, binary, commit, `Cargo.lock` sha256 |

**Not committed yet (M1).** These come from CI, not from a developer machine: the M1 builds ran on
the Mini with the local rzup toolchain and no Docker, which gives a different image id. `repro.yml`
builds twice on separate runners, rebuilds once more with `RUN --network=none`, compares
everything, and uploads `guest-a`/`guest-b`. Commit those files here after the first green run;
from then on the job fails if a build differs from them.
