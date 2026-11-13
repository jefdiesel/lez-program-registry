//! Writes the ballast blobs the `lezreg_ballast_*` guests embed: deterministic pseudo-random
//! bytes (an LCG), so the size of the program binary, not its content, is what they test.
use std::{env, fs, path::PathBuf};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    for (name, len) in [("ballast_430k.bin", 128 * 1024), ("ballast_1m.bin", 704 * 1024)] {
        let mut x: u32 = 0x1234_5678;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        fs::write(out.join(name), bytes).expect("write ballast");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
