//! `lezreg-imageid <file>...`: prints `<image_id hex> <sha256 of file> <file>` for a risc0
//! program binary (`.bin`) or a raw guest ELF (wrapped with V1COMPAT like the LEZ loader does).
//! The image id is LEZ's `Program::id()`, as 32 little-endian bytes of its 8 words.
use sha2::{Digest as _, Sha256};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: lezreg-imageid <file.bin|elf>...");
    for f in files {
        let mut bytes = std::fs::read(&f).unwrap_or_else(|e| panic!("{f}: {e}"));
        if bytes.starts_with(b"\x7fELF") {
            bytes = risc0_binfmt::ProgramBinary::new(&bytes, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
        }
        let id = lee::program::Program::new(bytes.into()).unwrap_or_else(|e| panic!("{f}: {e}")).id();
        let file_sha: String = Sha256::digest(std::fs::read(&f).expect("read")).iter().map(|b| format!("{b:02x}")).collect();
        println!("{} {file_sha} {f}", hex(&lezreg_engine_test::image_bytes(&id)));
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
