//! What lezreg reads in `.lgx` packages, as one JSON line each: whether it reads the package at
//! all, the files it hashed, the manifest, the recomputed root, and whether `record` would vouch
//! for it (lgx's own layout). Used by the differential test against lgx itself
//! (`tests/fixtures/apps/differential.py`). Usage: `cargo run --example lgx_view -- a.lgx ...`
use lezreg_sdk::lgx::{Contents, check_canonical_layout};
use serde_json::json;

fn main() {
    for path in std::env::args().skip(1) {
        let line = match std::fs::read(&path) {
            Err(e) => json!({"path": path, "read": false, "error": e.to_string()}),
            Ok(bytes) => match Contents::from_lgx(&bytes) {
                Err(e) => json!({"path": path, "read": false, "error": e.to_string()}),
                Ok(c) => json!({
                    "path": path,
                    "read": true,
                    "root": hex::encode(c.root),
                    "root_matches": c.root_matches(),
                    "manifest": c.manifest_json,
                    "manifest_sha256": hex::encode(c.manifest_sha256),
                    "files": c.files,
                    "canonical": check_canonical_layout(&bytes).err().map_or_else(|| "yes".to_owned(), |e| e.to_string()),
                }),
            },
        };
        println!("{line}");
    }
}
