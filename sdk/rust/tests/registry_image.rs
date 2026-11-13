//! The image a writer trusts by default is the committed reproducible build.

#[test]
fn the_trusted_registry_image_is_the_committed_artifact() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/lez_program_registry.imageid");
    let committed = std::fs::read_to_string(&path).expect("artifacts/lez_program_registry.imageid");
    assert_eq!(
        lezreg_sdk::REGISTRY_IMAGE_HEX,
        committed.trim(),
        "update REGISTRY_IMAGE_HEX in sdk/rust/src/lib.rs to the new artifact"
    );
    assert_eq!(hex::encode(lezreg_sdk::REGISTRY_IMAGE), committed.trim());
}

#[test]
fn the_module_knows_the_committed_registry_image() {
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../module/rust-lib/src/lib.rs");
    let src = std::fs::read_to_string(&lib).expect("module/rust-lib/src/lib.rs");
    let line = src.lines().find(|l| l.contains("pub const KNOWN_REGISTRY_IMAGES")).expect("KNOWN_REGISTRY_IMAGES");
    assert!(line.contains(lezreg_sdk::REGISTRY_IMAGE_HEX), "add the new artifact image to KNOWN_REGISTRY_IMAGES in module/rust-lib/src/lib.rs");
}
