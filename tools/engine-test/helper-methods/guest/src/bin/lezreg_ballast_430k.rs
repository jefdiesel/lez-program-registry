//! A ~430 KB program binary (128 KiB of ballast): the deploy-cost fixture of CYCLES.md.
static BALLAST: [u8; 128 * 1024] = *include_bytes!(concat!(env!("OUT_DIR"), "/ballast_430k.bin"));
include!("../ballast.rs");
