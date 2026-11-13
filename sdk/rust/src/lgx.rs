//! A Logos package's root hash, recomputed from its files (docs/APPS.md §4).
//!
//! The root is defined by the packaging tool, `lgx` (logos-co/logos-package, `src/crypto/signing.cpp`,
//! `computeMerkleTree`; spec in its `docs/spec.md`, "Merkle Tree Hashing"):
//!
//! - every regular file is hashed with SHA-256 (64 lowercase hex characters);
//! - a **leaf directory** (`variants/<variant>`, `assets`, `docs`, `licenses`) hashes the sorted
//!   list of its files: `SHA-256(Σ path ‖ 0x00 ‖ hex(file hash) ‖ '\n')`, paths relative to it;
//! - `variants` hashes its variants the same way: `SHA-256(Σ variant ‖ 0x00 ‖ hex ‖ '\n')`;
//! - `root` hashes the top-level directories (`variants`, `assets`, `docs`, `licenses`) the same way.
//!
//! **`manifest.json` and `manifest.sig` are not covered.** The root binds the package's files,
//! not its manifest: two packages with the same files and different manifests (another `author`,
//! `main`, `version` or `dependencies`) have the same root. The whole `.lgx` file's SHA-256 is
//! what binds a manifest.
//!
//! An installed package holds one variant, flattened into its directory beside the root
//! `assets/`, and the installer's own files (`manifest.json`, `manifest.sig`, `variant`,
//! `.lgpm-source`). [`Contents::from_installed`] recomputes that variant's leaf and the `assets`
//! leaf from the files on disk and rebuilds the root, taking only the *other* variants' and
//! directories' hashes from the manifest (as Merkle siblings), so the root it returns proves
//! the files on disk, whatever the manifest says its root is.
//!
//! Reading is as strict as `lgx` or stricter: plain ustar entries only (regular files and
//! directories; no PAX or GNU headers, which `lgx` doesn't read, so another reader could see
//! other names), no duplicate paths, no path that `lgx` would refuse, and nothing the root
//! does not cover except `manifest.json` and `manifest.sig`.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read as _,
    path::Path,
};

use serde_json::Value;

use crate::{
    apps::Manifest,
    error::{Error, Result},
};

/// The top-level entries `lgx` allows in a package (`Package::ALLOWED_ROOT_ENTRIES`).
const ALLOWED_ROOT: [&str; 6] = ["manifest.json", "manifest.sig", "variants", "assets", "docs", "licenses"];
/// Files at the top of a package that the root does not cover.
pub const UNCOVERED_IN_PACKAGE: [&str; 2] = ["manifest.json", "manifest.sig"];
/// Files an installer writes beside an installed package's payload, none covered by the root:
/// lgx's `INSTALL_SIDECAR_FILES` (`manifest.json`, `manifest.sig`, `variant`) and the package
/// manager's `.lgpm-source` (the URL it was downloaded from).
pub const INSTALL_SIDECARS: [&str; 4] = ["manifest.json", "manifest.sig", "variant", ".lgpm-source"];
/// The root asset an installer may have merged with a variant's own `assets/` (lgx `Manifest::ICON_PATH`).
const ICON_PATH: &str = "assets/icon.png";
/// The most a package may unpack to.
pub const MAX_UNPACKED: usize = 1024 * 1024 * 1024;
const BLOCK: usize = 512;

fn hex_sha256(data: &[u8]) -> String {
    hex::encode(lezreg_types::pda::sha256(data))
}

/// `SHA-256(Σ name ‖ 0x00 ‖ hex hash ‖ '\n')` over `items` sorted by name; `None` when empty
/// (lgx leaves an empty directory out of the tree).
fn listing_hash(items: &BTreeMap<String, String>) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut concat = Vec::new();
    for (name, hash) in items {
        concat.extend_from_slice(name.as_bytes());
        concat.push(0);
        concat.extend_from_slice(hash.as_bytes());
        concat.push(b'\n');
    }
    Some(hex_sha256(&concat))
}

/// The leaf hash of the files under `prefix/`: `files` maps full paths to hex file hashes.
fn leaf(files: &BTreeMap<String, String>, prefix: &str) -> Option<String> {
    let prefix = format!("{prefix}/");
    let items: BTreeMap<String, String> = files
        .iter()
        .filter_map(|(p, h)| p.strip_prefix(&prefix).filter(|r| !r.is_empty()).map(|r| (r.to_owned(), h.clone())))
        .collect();
    listing_hash(&items)
}

/// The whole tree of hashes for a package's files (`path → hex`), as lgx `computeMerkleTree`
/// computes it: `variants/<v>`, `variants`, each other top-level directory, and `root`.
#[must_use]
pub fn merkle_tree(files: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let tops: BTreeSet<&str> = files.keys().filter_map(|p| p.split_once('/').map(|(t, _)| t)).collect();
    let mut top_hashes = BTreeMap::new();
    for top in tops {
        if top == "variants" {
            let variants: BTreeSet<&str> = files
                .keys()
                .filter_map(|p| p.strip_prefix("variants/")?.split_once('/').map(|(v, _)| v))
                .collect();
            let mut variant_hashes = BTreeMap::new();
            for v in variants {
                if let Some(h) = leaf(files, &format!("variants/{v}")) {
                    out.insert(format!("variants/{v}"), h.clone());
                    variant_hashes.insert(v.to_owned(), h);
                }
            }
            if let Some(h) = listing_hash(&variant_hashes) {
                out.insert("variants".into(), h.clone());
                top_hashes.insert("variants".to_owned(), h);
            }
        } else if let Some(h) = leaf(files, top) {
            out.insert(top.to_owned(), h.clone());
            top_hashes.insert(top.to_owned(), h);
        }
    }
    if let Some(root) = listing_hash(&top_hashes) {
        out.insert("root".into(), root);
    }
    out
}

/// Where a package's files were read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// A `.lgx` file, with the SHA-256 of the whole file (manifest included).
    Lgx { sha256: [u8; 32] },
    /// An installed package's directory, holding the named variant.
    Installed { variant: String },
}

/// A package's manifest, with the root recomputed from its files. Only [`Contents::from_lgx`]
/// and [`Contents::from_installed`] make one, so a verdict built from it always rests on files
/// that were hashed, never on the root a manifest claims.
/// `#[non_exhaustive]`: no other crate can write one by hand (say, with `root` copied from a
/// manifest).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Contents {
    /// What the manifest says, `root` included (the claim).
    pub manifest: Manifest,
    /// The root recomputed from the files (the fact).
    pub root: [u8; 32],
    pub form: Form,
    /// Files present but not covered by the root, by path: `manifest.json` always.
    pub not_covered: Vec<String>,
}

impl Contents {
    /// Whether the files are the ones the manifest's `hashes.root` names.
    #[must_use]
    pub fn root_matches(&self) -> bool {
        self.root == self.manifest.root
    }

    /// The whole file's SHA-256, for a `.lgx`.
    #[must_use]
    pub const fn file_sha256(&self) -> Option<[u8; 32]> {
        match self.form {
            Form::Lgx { sha256 } => Some(sha256),
            Form::Installed { .. } => None,
        }
    }

    /// Reads a `.lgx` package (a gzipped ustar archive) and recomputes its root.
    pub fn from_lgx(bytes: &[u8]) -> Result<Self> {
        let entries = read_lgx_entries(bytes)?;
        let mut files = BTreeMap::new();
        let mut manifest = None;
        let mut not_covered = Vec::new();
        for (path, data) in entries {
            if UNCOVERED_IN_PACKAGE.contains(&path.as_str()) {
                if path == "manifest.json" {
                    manifest = Some(data);
                }
                not_covered.push(path);
            } else {
                files.insert(path, hex_sha256(&data));
            }
        }
        let manifest = manifest.ok_or_else(|| bad("the package has no manifest.json"))?;
        let json: Value = serde_json::from_slice(&manifest).map_err(|_| bad("the package's manifest.json is not JSON"))?;
        let manifest = Manifest::parse(&json)?;
        let tree = merkle_tree(&files);
        let root = tree.get("root").ok_or_else(|| bad("the package has no files under variants/, assets/, docs/ or licenses/"))?;
        Ok(Self {
            manifest,
            root: hex32(root)?,
            form: Form::Lgx { sha256: lezreg_types::pda::sha256(bytes) },
            not_covered,
        })
    }

    /// Reads an installed package's directory (`<Basecamp data>/modules/<module>` or
    /// `.../plugins/<module>`) and recomputes its root from the files on disk.
    pub fn from_installed(dir: &Path) -> Result<Self> {
        let read = |name: &str| std::fs::read(dir.join(name)).map_err(|e| bad(&format!("cannot read {}: {e}", dir.join(name).display())));
        let json: Value = serde_json::from_slice(&read("manifest.json")?).map_err(|_| bad("the installed manifest.json is not JSON"))?;
        let manifest = Manifest::parse(&json)?;
        let variant = String::from_utf8(read("variant")?).map_err(|_| bad("the installed variant file is not text"))?;
        let variant = variant.trim().to_ascii_lowercase();
        if !is_child_name(&variant) {
            return Err(bad(&format!("the installed variant file names no variant: {variant:?}")));
        }
        let declared = declared_hashes(&json)?;
        let mut on_disk = BTreeMap::new();
        let mut not_covered = Vec::new();
        walk(dir, dir, &mut on_disk, &mut not_covered)?;
        not_covered.sort();
        let root = installed_root(&declared, &variant, &on_disk)?;
        Ok(Self { manifest, root, form: Form::Installed { variant }, not_covered })
    }
}

fn bad(m: &str) -> Error {
    Error::Invalid(m.to_owned())
}

fn hex32(text: &str) -> Result<[u8; 32]> {
    hex::decode(text).ok().and_then(|b| b.try_into().ok()).ok_or_else(|| bad(&format!("not a 32-byte hex hash: {text}")))
}

/// A directory name that can sit in a hash listing unambiguously: a non-empty path component
/// of printable ASCII, no `/`, no `\`.
fn is_child_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && name.bytes().all(|b| (0x21..0x7f).contains(&b) && b != b'/' && b != b'\\')
}

/// The manifest's `hashes`, each a 64-hex value under a well-formed key.
fn declared_hashes(manifest: &Value) -> Result<BTreeMap<String, String>> {
    let hashes = manifest.get("hashes").and_then(Value::as_object).ok_or_else(|| bad("the manifest has no hashes"))?;
    let mut out = BTreeMap::new();
    for (k, v) in hashes {
        let v = v.as_str().ok_or_else(|| bad(&format!("hashes.{k} is not a string")))?;
        if v.len() != 64 || !v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(bad(&format!("hashes.{k} is not 64 lower-case hex characters")));
        }
        let well_formed = match k.split_once('/') {
            None => is_child_name(k),
            Some(("variants", v)) => is_child_name(v),
            Some(_) => false,
        };
        if !well_formed {
            return Err(bad(&format!("the manifest's hashes have a key lgx never writes: {k:?}")));
        }
        out.insert(k.clone(), v.to_owned());
    }
    Ok(out)
}

/// Every regular file under `dir` (relative path → hex hash), refusing anything else; the
/// installer's sidecars at the top go to `not_covered`.
fn walk(base: &Path, dir: &Path, files: &mut BTreeMap<String, String>, not_covered: &mut Vec<String>) -> Result<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| bad(&format!("cannot read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| bad(&format!("cannot read {}: {e}", dir.display())))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| bad(&format!("cannot read {}: {e}", path.display())))?;
        let rel = path
            .strip_prefix(base)
            .ok()
            .and_then(Path::to_str)
            .map(|s| s.replace('\\', "/"))
            .ok_or_else(|| bad(&format!("not a UTF-8 path: {}", path.display())))?;
        if kind.is_dir() {
            walk(base, &path, files, not_covered)?;
        } else if kind.is_file() {
            if INSTALL_SIDECARS.contains(&rel.as_str()) {
                not_covered.push(rel);
            } else {
                let bytes = std::fs::read(&path).map_err(|e| bad(&format!("cannot read {}: {e}", path.display())))?;
                files.insert(rel, hex_sha256(&bytes));
            }
        } else {
            // Nothing a package installs is a link or a special file (lgx `collectListing`).
            return Err(bad(&format!("not a regular file: {rel} (a package installs none)")));
        }
    }
    Ok(())
}

/// The root an installed tree proves: its variant's leaf and its `assets` leaf recomputed
/// from disk, the other children taken from `declared` as siblings. Root assets were merged
/// into the same directory as the variant's own files, so which `assets/` files came from the
/// root is guessed the way lgx `verifyInstalledTree` guesses (the icon only, or every asset;
/// and with or without the variant holding them too); a reading counts only if it rebuilds
/// the declared root, and without one the plain reading's root is returned (it won't match).
fn installed_root(declared: &BTreeMap<String, String>, variant: &str, on_disk: &BTreeMap<String, String>) -> Result<[u8; 32]> {
    let variant_key = format!("variants/{variant}");
    if !declared.contains_key(&variant_key) {
        return Err(bad(&format!("the manifest has no hash for the installed variant {variant}")));
    }
    let rebuild = |variant_files: &BTreeMap<String, String>, assets: Option<&BTreeMap<String, String>>| -> Option<String> {
        let mut variants: BTreeMap<String, String> = declared
            .iter()
            .filter_map(|(k, v)| k.strip_prefix("variants/").map(|n| (n.to_owned(), v.clone())))
            .collect();
        match listing_hash(variant_files) {
            Some(h) => variants.insert(variant.to_owned(), h),
            None => variants.remove(variant),
        };
        let mut top: BTreeMap<String, String> = declared
            .iter()
            .filter(|(k, _)| !k.contains('/') && *k != "root" && *k != "variants" && *k != "assets")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(h) = listing_hash(&variants) {
            top.insert("variants".into(), h);
        }
        if let Some(h) = assets.and_then(listing_hash) {
            top.insert("assets".into(), h);
        }
        listing_hash(&top)
    };
    let plain = rebuild(on_disk, None);
    let mut readings = vec![plain.clone()];
    if declared.contains_key("assets") {
        let (under, payload): (BTreeMap<_, _>, BTreeMap<_, _>) = on_disk.iter().map(|(k, v)| (k.clone(), v.clone())).partition(|(k, _)| k.starts_with("assets/"));
        for icon_only in [true, false] {
            let (taken, kept): (BTreeMap<_, _>, BTreeMap<_, _>) = under.clone().into_iter().partition(|(k, _)| !icon_only || k == ICON_PATH);
            let assets: BTreeMap<String, String> = taken.iter().map(|(k, v)| (k["assets/".len()..].to_owned(), v.clone())).collect();
            let mut without = payload.clone();
            without.extend(kept);
            readings.push(rebuild(&without, Some(&assets)));
            let mut with = payload.clone();
            with.extend(under.clone());
            readings.push(rebuild(&with, Some(&assets)));
        }
    }
    let want = declared.get("root");
    let found = readings.iter().flatten().find(|r| Some(*r) == want).or(plain.as_ref());
    hex32(found.ok_or_else(|| bad("the installed package has no files"))?)
}

/// The entries of a `.lgx` (gzip, then ustar), path → contents for regular files, checked
/// against everything lgx refuses and everything the root would not cover.
fn read_lgx_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(bad("not a .lgx package (not gzip)"));
    }
    let mut tar = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes)
        .take(MAX_UNPACKED as u64 + 1)
        .read_to_end(&mut tar)
        .map_err(|e| bad(&format!("a damaged .lgx package: {e}")))?;
    if tar.len() > MAX_UNPACKED {
        return Err(bad("the package unpacks to more than 1 GiB"));
    }
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    let mut at = 0;
    let mut zero_blocks = 0;
    while at < tar.len() {
        let header = tar.get(at..at + BLOCK).ok_or_else(|| bad("the package's tar ends inside a header"))?;
        at += BLOCK;
        if header.iter().all(|&b| b == 0) {
            // lgx stops at two zero blocks in a row; a single one is skipped.
            zero_blocks += 1;
            if zero_blocks >= 2 {
                break;
            }
            continue;
        }
        zero_blocks = 0;
        let stored = octal(&header[148..156]).ok_or_else(|| bad("a tar header with a bad checksum field"))?;
        let sum: u64 = header.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) }).sum();
        if stored != sum {
            return Err(bad("a tar header with a wrong checksum"));
        }
        let field = |r: std::ops::Range<usize>| {
            let f = &header[r];
            &f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]
        };
        let (name, prefix) = (field(0..100), field(345..500));
        let mut path = Vec::new();
        if !prefix.is_empty() {
            path.extend_from_slice(prefix);
            path.push(b'/');
        }
        path.extend_from_slice(name);
        let path = String::from_utf8(path).map_err(|_| bad("a tar entry whose name is not UTF-8"))?;
        let size = usize::try_from(octal(&header[124..136]).ok_or_else(|| bad(&format!("{path}: a bad size field")))?)
            .map_err(|_| bad("an entry too large"))?;
        let kind = header[156];
        let data_end = at.checked_add(size.div_ceil(BLOCK) * BLOCK).ok_or_else(|| bad("an entry too large"))?;
        match kind {
            b'0' | 0 => {
                let data = tar.get(at..at + size).ok_or_else(|| bad(&format!("{path}: the tar ends inside its data")))?.to_vec();
                at = data_end;
                if path.ends_with('/') {
                    problems.push(format!("{path}: a file whose name ends in /"));
                    continue;
                }
                if let Err(why) = check_path(&path) {
                    problems.push(format!("{path}: {why}"));
                    continue;
                }
                if !seen.insert(path.clone()) {
                    problems.push(format!("{path}: in the package twice"));
                    continue;
                }
                let top = path.split('/').next().unwrap_or_default();
                let depth = path.split('/').count();
                if depth == 1 && !UNCOVERED_IN_PACKAGE.contains(&path.as_str()) {
                    problems.push(format!("{path}: a file at the top of the package, not covered by the root"));
                } else if top == "variants" && depth == 2 {
                    problems.push(format!("{path}: a file directly under variants/, not covered by the root"));
                } else if !ALLOWED_ROOT.contains(&top) {
                    problems.push(format!("{path}: lgx allows only manifest.json, manifest.sig, variants/, assets/, docs/ and licenses/ at the top"));
                } else {
                    out.push((path, data));
                }
            }
            b'5' => {
                if size != 0 {
                    return Err(bad(&format!("{path}: a directory with data")));
                }
                let trimmed = path.trim_end_matches('/');
                if let Err(why) = check_path(trimmed) {
                    problems.push(format!("{path}: {why}"));
                } else if !ALLOWED_ROOT.contains(&trimmed.split('/').next().unwrap_or_default()) {
                    problems.push(format!("{path}: lgx allows only variants/, assets/, docs/ and licenses/ directories at the top"));
                }
            }
            other => {
                return Err(bad(&format!(
                    "{path}: a tar entry of type {:?}; a package holds plain files and directories only (lgx reads no links, PAX or GNU long-name headers)",
                    char::from(other)
                )));
            }
        }
    }
    for (path, _) in &out {
        if let Some(dir) = seen.iter().find(|other| other.starts_with(&format!("{path}/"))) {
            problems.push(format!("{path}: both a file and the directory of {dir}"));
        }
    }
    if !problems.is_empty() {
        return Err(bad(&format!("the package is not one lgx accepts, or holds files its root does not cover: {}", problems.join("; "))));
    }
    if !seen.contains("manifest.json") {
        return Err(bad("the package has no manifest.json at its top"));
    }
    Ok(out)
}

/// An octal tar number field (NUL- or space-terminated); `None` for anything else (including
/// base-256 numbers, which lgx doesn't read).
fn octal(field: &[u8]) -> Option<u64> {
    let start = field.iter().position(|&b| b != b' ').unwrap_or(field.len());
    let rest = &field[start..];
    let digits = &rest[..rest.iter().position(|&b| b == 0 || b == b' ').unwrap_or(rest.len())];
    if digits.len() > 21 || !digits.iter().all(|b| (b'0'..=b'7').contains(b)) {
        return None;
    }
    Some(digits.iter().fold(0, |v, d| v * 8 + u64::from(d - b'0')))
}

/// A path lgx accepts and that hashes the same for every reader: relative, `/`-separated,
/// printable ASCII (lgx also takes NFC Unicode; this refuses it rather than normalise it), no
/// empty, `.` or `..` component.
fn check_path(path: &str) -> std::result::Result<(), &'static str> {
    if path.is_empty() {
        return Err("an empty path");
    }
    if !path.bytes().all(|b| (0x20..0x7f).contains(&b)) {
        return Err("not printable ASCII");
    }
    if path.contains('\\') {
        return Err("a backslash");
    }
    if path.starts_with('/') {
        return Err("an absolute path");
    }
    if path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err("an empty, . or .. component");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A ustar archive of `entries` (path, data; `None` = directory), gzipped.
    pub fn lgx(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        lgx_typed(&entries.iter().map(|(p, d)| (*p, *d, if d.is_some() { b'0' } else { b'5' })).collect::<Vec<_>>())
    }

    pub fn lgx_typed(entries: &[(&str, Option<&[u8]>, u8)]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (path, data, kind) in entries {
            let mut h = [0_u8; 512];
            h[..path.len()].copy_from_slice(path.as_bytes());
            h[100..107].copy_from_slice(b"0000644");
            let size = data.map_or(0, <[u8]>::len);
            h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
            h[136..147].copy_from_slice(b"00000000000");
            h[156] = *kind;
            h[257..263].copy_from_slice(b"ustar\0");
            h[263..265].copy_from_slice(b"00");
            h[148..156].copy_from_slice(b"        ");
            let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
            h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
            tar.extend_from_slice(&h);
            if let Some(d) = data {
                tar.extend_from_slice(d);
                tar.resize(tar.len().div_ceil(512) * 512, 0);
            }
        }
        tar.extend_from_slice(&[0; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    /// A package with a correct root: its files, and a manifest naming that root.
    pub fn genuine(author: &str, files: &[(&str, &[u8])]) -> (Vec<u8>, String) {
        genuine_with(author, files, |_| {})
    }

    pub fn genuine_with(author: &str, files: &[(&str, &[u8])], edit: impl FnOnce(&mut Value)) -> (Vec<u8>, String) {
        let hashed: BTreeMap<String, String> = files.iter().map(|(p, d)| ((*p).to_owned(), hex_sha256(d))).collect();
        let tree = merkle_tree(&hashed);
        let mut manifest = serde_json::json!({
            "name": "forumer_ui", "version": "1.2.0", "author": author, "type": "ui_qml",
            "hashes": tree, "main": {"darwin-arm64": "forumer_ui_plugin.dylib"}
        });
        edit(&mut manifest);
        let manifest = manifest.to_string();
        let mut entries: Vec<(&str, Option<&[u8]>)> = vec![("manifest.json", Some(manifest.as_bytes()))];
        entries.extend(files.iter().map(|(p, d)| (*p, Some(*d))));
        (lgx(&entries), tree["root"].clone())
    }

    pub const FILES: &[(&str, &[u8])] = &[
        ("variants/darwin-arm64/forumer_ui_plugin.dylib", b"the publisher's build"),
        ("variants/darwin-arm64/qml/Main.qml", b"Item {}"),
        ("variants/linux-amd64/forumer_ui_plugin.so", b"the publisher's linux build"),
        ("assets/icon.png", b"\x89PNG icon"),
        ("assets/lidl/forumer.lidl", b"lidl"),
    ];

    #[test]
    fn the_root_is_recomputed_from_the_files() {
        let (bytes, root) = genuine("lns:forumer", FILES);
        let c = Contents::from_lgx(&bytes).unwrap();
        assert_eq!(hex::encode(c.root), root);
        assert!(c.root_matches());
        assert_eq!(c.not_covered, vec!["manifest.json"]);
        assert_eq!(c.file_sha256(), Some(lezreg_types::pda::sha256(&bytes)));
    }

    /// The spec's definition, by hand for a two-file package.
    #[test]
    fn matches_the_lgx_definition() {
        let h = |b: &[u8]| hex_sha256(b);
        let leaf = h(format!("a\0{}\nb/c\0{}\n", h(b"1"), h(b"2")).as_bytes());
        let variants = h(format!("darwin-arm64\0{leaf}\n").as_bytes());
        let docs = h(format!("readme\0{}\n", h(b"3")).as_bytes());
        let root = h(format!("docs\0{docs}\nvariants\0{variants}\n").as_bytes());
        let files: BTreeMap<String, String> = [
            ("variants/darwin-arm64/a", b"1"),
            ("variants/darwin-arm64/b/c", b"2"),
            ("docs/readme", b"3"),
        ]
        .iter()
        .map(|(p, d)| ((*p).to_owned(), h(*d)))
        .collect();
        let tree = merkle_tree(&files);
        assert_eq!(tree["root"], root);
        assert_eq!(tree["variants/darwin-arm64"], leaf);
    }

    #[test]
    fn other_files_under_the_same_manifest_are_caught() {
        let (genuine_bytes, _) = genuine("lns:forumer", FILES);
        let real = Contents::from_lgx(&genuine_bytes).unwrap();
        assert!(real.root_matches());
        // The genuine manifest.json, byte for byte, next to another binary.
        let manifest = read_lgx_entries(&genuine_bytes).unwrap().into_iter().find(|(p, _)| p == "manifest.json").unwrap().1;
        let evil = lgx(&[
            ("manifest.json", Some(&manifest)),
            ("variants/darwin-arm64/forumer_ui_plugin.dylib", Some(b"MALICIOUS")),
        ]);
        let c = Contents::from_lgx(&evil).unwrap();
        assert_eq!(c.manifest, real.manifest);
        assert!(!c.root_matches());
    }

    #[test]
    fn what_the_root_does_not_cover_is_refused() {
        let m: &[u8] = br#"{"name":"x","hashes":{"root":"00"}}"#;
        let refused = |entries: &[(&str, Option<&[u8]>, u8)], why: &str| {
            let e = Contents::from_lgx(&lgx_typed(entries)).unwrap_err().to_string();
            assert!(e.contains(why), "{e}");
        };
        let file = b'0';
        refused(&[("manifest.json", Some(m), file), ("variants/evil.dylib", Some(b"x"), file)], "directly under variants/");
        refused(&[("manifest.json", Some(m), file), ("run.sh", Some(b"x"), file)], "at the top of the package");
        refused(&[("manifest.json", Some(m), file), ("bin/x", Some(b"x"), file)], "lgx allows only");
        refused(&[("manifest.json", Some(m), file), ("variants/v/a", Some(b"1"), file), ("variants/v/a", Some(b"2"), file)], "twice");
        refused(&[("manifest.json", Some(m), file), ("variants/v/../a", Some(b"1"), file)], "component");
        refused(&[("manifest.json", Some(m), file), ("./variants/v/a", Some(b"1"), file)], "component");
        refused(&[("manifest.json", Some(m), file), ("variants/v/a\\b", Some(b"1"), file)], "backslash");
        refused(&[("manifest.json", Some(m), file), ("/variants/v/a", Some(b"1"), file)], "absolute");
        refused(&[("manifest.json", Some(m), file), ("variants/v/l", Some(b""), b'2')], "type '2'");
        refused(&[("manifest.json", Some(m), file), ("././@PaxHeader", Some(b"30 path=variants/v/x\n"), b'x')], "type 'x'");
        refused(&[("manifest.json", Some(m), file), ("././@LongLink", Some(b"variants/v/x"), b'L')], "type 'L'");
        refused(&[("x/manifest.json", Some(m), file), ("variants/v/a", Some(b"1"), file)], "lgx allows only");
        refused(&[("variants/v/a", Some(b"1"), file)], "no manifest.json");
        refused(&[("manifest.json", Some(m), file), ("manifest.json", Some(m), file), ("variants/v/a", Some(b"1"), file)], "twice");
        assert!(Contents::from_lgx(b"{\"name\":\"x\"}").is_err(), "a bare manifest has no files");
    }

    #[test]
    fn an_installed_package_proves_its_files_up_to_the_root() {
        let (bytes, root) = genuine("lns:forumer", FILES);
        let installed = Contents::from_lgx(&bytes).unwrap();
        let dir = std::env::temp_dir().join(format!("lezreg-lgx-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |p: &str, d: &[u8]| {
            let p = dir.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, d).unwrap();
        };
        // As lgpm installs it: the darwin variant flattened, root assets merged in, sidecars.
        let manifest = {
            let entries = read_lgx_entries(&bytes).unwrap();
            entries.into_iter().find(|(p, _)| p == "manifest.json").unwrap().1
        };
        write("manifest.json", &manifest);
        write("variant", b"darwin-arm64");
        write(".lgpm-source", b"https://example.org/forumer_ui.lgx");
        write("forumer_ui_plugin.dylib", b"the publisher's build");
        write("qml/Main.qml", b"Item {}");
        write("assets/icon.png", b"\x89PNG icon");
        write("assets/lidl/forumer.lidl", b"lidl");
        let c = Contents::from_installed(&dir).unwrap();
        assert_eq!(hex::encode(c.root), root);
        assert!(c.root_matches());
        assert_eq!(c.not_covered, vec![".lgpm-source", "manifest.json", "variant"]);
        assert_eq!(c.manifest, installed.manifest);
        // A swapped binary: the recomputed root moves, whatever the manifest says.
        write("forumer_ui_plugin.dylib", b"MALICIOUS");
        let c = Contents::from_installed(&dir).unwrap();
        assert!(!c.root_matches());
        assert_eq!(c.manifest.root, installed.manifest.root);
        // An extra file is part of the variant's listing, so it moves the root too.
        write("forumer_ui_plugin.dylib", b"the publisher's build");
        assert!(Contents::from_installed(&dir).unwrap().root_matches());
        write("extra.dylib", b"x");
        assert!(!Contents::from_installed(&dir).unwrap().root_matches());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
