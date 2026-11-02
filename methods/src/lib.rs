//! The registry guest, built with the local RISC Zero toolchain. This build is not the
//! reproducible one: `.github/workflows/repro.yml` builds `methods/guest` with
//! `cargo risczero build` (Docker, digest-pinned) for the published `image_id`.

include!(concat!(env!("OUT_DIR"), "/methods.rs"));
