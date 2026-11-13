//! A ~1 MB program binary (704 KiB of ballast): the deploy-cost fixture of CYCLES.md.
static BALLAST: [u8; 704 * 1024] = *include_bytes!(concat!(env!("OUT_DIR"), "/ballast_1m.bin"));
include!("../ballast.rs");
