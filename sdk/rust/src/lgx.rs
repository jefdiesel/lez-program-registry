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
    /// SHA-256 of the `manifest.json` bytes (the package's, or the installed one), which an app
    /// record's `manifest` names: what covers the manifest where the root doesn't.
    pub manifest_sha256: [u8; 32],
    /// The whole manifest as parsed (keys unique at every depth).
    pub manifest_json: Value,
    /// The files the root was computed from, path → hex SHA-256 (archive paths for a `.lgx`,
    /// paths in the directory for an installed package).
    pub files: BTreeMap<String, String>,
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
        let manifest_bytes = manifest.ok_or_else(|| bad("the package has no manifest.json"))?;
        let json = crate::apps::strict_json(&manifest_bytes)?;
        let manifest = Manifest::parse(&json)?;
        let tree = merkle_tree(&files);
        let root = tree.get("root").ok_or_else(|| bad("the package has no files under variants/, assets/, docs/ or licenses/"))?;
        Ok(Self {
            manifest,
            root: hex32(root)?,
            form: Form::Lgx { sha256: lezreg_types::pda::sha256(bytes) },
            not_covered,
            manifest_sha256: lezreg_types::pda::sha256(&manifest_bytes),
            manifest_json: json,
            files,
        })
    }

    /// Reads an installed package's directory (`<Basecamp data>/modules/<module>` or
    /// `.../plugins/<module>`) and recomputes its root from the files on disk.
    pub fn from_installed(dir: &Path) -> Result<Self> {
        let read = |name: &str| std::fs::read(dir.join(name)).map_err(|e| bad(&format!("cannot read {}: {e}", dir.join(name).display())));
        let manifest_bytes = read("manifest.json")?;
        let json = crate::apps::strict_json(&manifest_bytes)?;
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
        Ok(Self {
            manifest,
            root,
            form: Form::Installed { variant },
            not_covered,
            manifest_sha256: lezreg_types::pda::sha256(&manifest_bytes),
            manifest_json: json,
            files: on_disk,
        })
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

/// One tar entry, as read: its header block, and for a file its data.
struct RawEntry {
    path: String,
    dir: bool,
    mode: u64,
    header: [u8; BLOCK],
    data: Vec<u8>,
}

/// A `.lgx` as read: its entries in order, and the tar's length (to check the end marker).
struct RawPackage {
    entries: Vec<RawEntry>,
    end_at: usize,
    tar_len: usize,
}

/// Exactly one gzip member and nothing after it. (lgx reads through `gzread`, which goes on into
/// a second member and stops quietly at trailing junk; refusing both leaves one reading.)
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>> {
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Err(bad("not a .lgx package (not gzip)"));
    }
    let mut decoder = flate2::bufread::GzDecoder::new(bytes);
    let mut tar = Vec::new();
    (&mut decoder)
        .take(MAX_UNPACKED as u64 + 1)
        .read_to_end(&mut tar)
        .map_err(|e| bad(&format!("a damaged .lgx package: {e}")))?;
    if tar.len() > MAX_UNPACKED {
        return Err(bad("the package unpacks to more than 1 GiB"));
    }
    if !decoder.into_inner().is_empty() {
        return Err(bad("bytes after the gzip stream (a second member, or junk): a package is one gzip member"));
    }
    Ok(tar)
}

/// A numeric header field written the one way every reader agrees on: octal digits from its
/// first byte, then only NULs or spaces. lgx's `readOctal` also skips leading spaces *and NULs*
/// and stops at the first non-digit, so a field such as `"\0" + "0001234" + "\0"` is 0 to a
/// reader that stops at the NUL and 0o1234 to lgx: a size like that hid a whole second package
/// (review B1). Anything but the plain form is refused, never interpreted.
fn strict_number(field: &[u8], what: &str) -> Result<u64> {
    let digits = field.iter().take_while(|b| (b'0'..=b'7').contains(b)).count();
    if digits == 0 || digits > 21 || !field[digits..].iter().all(|&b| b == 0 || b == b' ') {
        return Err(bad(&format!("a tar header whose {what} field is not plain octal ({field:?}); refused rather than guessed at")));
    }
    Ok(field[..digits].iter().fold(0, |v, d| v * 8 + u64::from(d - b'0')))
}

/// A text field: its bytes up to the first NUL, with only NULs after it.
fn strict_text<'a>(field: &'a [u8], what: &str) -> Result<&'a [u8]> {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    if !field[end..].iter().all(|&b| b == 0) {
        return Err(bad(&format!("a tar header with bytes after the end of its {what}")));
    }
    Ok(&field[..end])
}

/// Reads the ustar archive inside a `.lgx` strictly enough that lgx's `TarReader` can't read it
/// differently: every numeric field plain octal, the checksum the unsigned sum lgx computes,
/// `ustar\0` `00` magic (lgx reads name and prefix as ustar whatever the magic says), a regular
/// file or directory and nothing else, no link name, zero padding, a directory with no data,
/// and the archive ending with two zero blocks followed only by zeros (lgx skips a lone zero
/// block and keeps reading; other readers stop there).
fn read_raw(bytes: &[u8]) -> Result<RawPackage> {
    let tar = gunzip(bytes)?;
    let mut entries = Vec::new();
    let mut at = 0;
    loop {
        let header: [u8; BLOCK] = tar
            .get(at..at + BLOCK)
            .ok_or_else(|| bad("the package's tar has no end-of-archive marker (it ends inside or before a header)"))?
            .try_into()
            .expect("a block");
        at += BLOCK;
        if header.iter().all(|&b| b == 0) {
            if !tar.get(at..at + BLOCK).is_some_and(|b| b.iter().all(|&x| x == 0)) {
                return Err(bad("a lone zero block inside the tar (lgx skips it and reads on, other readers stop): refused"));
            }
            let end_at = at + BLOCK;
            if tar[end_at..].iter().any(|&b| b != 0) {
                return Err(bad("data after the tar's end-of-archive marker: refused"));
            }
            return Ok(RawPackage { entries, end_at, tar_len: tar.len() });
        }
        let stored = strict_number(&header[148..156], "checksum")?;
        let sum: u64 = header.iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u64::from(b) }).sum();
        if stored != sum {
            return Err(bad("a tar header with a wrong checksum"));
        }
        if &header[257..263] != b"ustar\0" || &header[263..265] != b"00" {
            return Err(bad("a tar header that is not POSIX ustar (lgx reads every header as ustar, so another format would read differently)"));
        }
        let mode = strict_number(&header[100..108], "mode")?;
        strict_number(&header[108..116], "uid")?;
        strict_number(&header[116..124], "gid")?;
        let size = strict_number(&header[124..136], "size")?;
        strict_number(&header[136..148], "mtime")?;
        let name = strict_text(&header[0..100], "name")?;
        let prefix = strict_text(&header[345..500], "prefix")?;
        if !strict_text(&header[157..257], "link name")?.is_empty() {
            return Err(bad("a tar entry with a link name"));
        }
        if header[500..].iter().any(|&b| b != 0) {
            return Err(bad("a tar header with bytes in its padding"));
        }
        let mut path = Vec::new();
        if !prefix.is_empty() {
            path.extend_from_slice(prefix);
            path.push(b'/');
        }
        path.extend_from_slice(name);
        let path = String::from_utf8(path).map_err(|_| bad("a tar entry whose name is not UTF-8"))?;
        let size = usize::try_from(size).map_err(|_| bad("an entry too large"))?;
        match header[156] {
            b'0' | 0 => {
                let data = tar.get(at..at + size).ok_or_else(|| bad(&format!("{path}: the tar ends inside its data")))?.to_vec();
                let end = at + size.div_ceil(BLOCK) * BLOCK;
                if !tar.get(at + size..end).is_some_and(|p| p.iter().all(|&b| b == 0)) {
                    return Err(bad(&format!("{path}: padding after its data that isn't zeros")));
                }
                at = end;
                entries.push(RawEntry { path, dir: false, mode, header, data });
            }
            b'5' => {
                if size != 0 {
                    return Err(bad(&format!("{path}: a directory with data")));
                }
                entries.push(RawEntry { path, dir: true, mode, header, data: Vec::new() });
            }
            other => {
                return Err(bad(&format!(
                    "{path}: a tar entry of type {:?}; a package holds plain files and directories only (lgx reads no links, PAX or GNU long-name headers)",
                    char::from(other)
                )));
            }
        }
    }
}

/// The entries of a `.lgx`, path → contents for regular files, checked against everything lgx
/// refuses and everything the root would not cover. A path given twice is refused whatever it
/// is (lgx keeps the later `manifest.json` or `manifest.sig`; another reader may keep the first).
fn read_lgx_entries(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let raw = read_raw(bytes)?;
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    let mut problems = Vec::new();
    for e in raw.entries {
        let path = e.path;
        if e.dir {
            let trimmed = path.strip_suffix('/').unwrap_or(&path).to_owned();
            if let Err(why) = check_path(&trimmed) {
                problems.push(format!("{path}: {why}"));
            } else if !ALLOWED_ROOT.contains(&trimmed.split('/').next().unwrap_or_default()) {
                problems.push(format!("{path}: lgx allows only variants/, assets/, docs/ and licenses/ directories at the top"));
            }
            dirs.insert(trimmed);
            continue;
        }
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
            out.push((path, e.data));
        }
    }
    for (path, _) in &out {
        if dirs.contains(path) {
            problems.push(format!("{path}: both a file and a directory"));
        }
        if let Some(other) = seen.iter().find(|other| other.starts_with(&format!("{path}/"))) {
            problems.push(format!("{path}: both a file and the directory of {other}"));
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

/// The header lgx's `DeterministicTarWriter::createHeader` writes for an entry.
fn canonical_header(path: &str, dir: bool, mode: u64, size: usize) -> Option<[u8; BLOCK]> {
    let full = if dir { format!("{}/", path.trim_end_matches('/')) } else { path.to_owned() };
    let (name, prefix) = split_ustar(&full)?;
    let mut h = [0_u8; BLOCK];
    h[..name.len()].copy_from_slice(name.as_bytes());
    let octal = |h: &mut [u8; BLOCK], at: usize, len: usize, v: u64| {
        let text = format!("{v:0width$o}", width = len - 1);
        h[at..at + len - 1].copy_from_slice(text.as_bytes());
    };
    octal(&mut h, 100, 8, if dir { 0o755 } else if mode & 0o777 == 0 { 0o644 } else { mode & 0o777 });
    octal(&mut h, 108, 8, 0);
    octal(&mut h, 116, 8, 0);
    octal(&mut h, 124, 12, if dir { 0 } else { size as u64 });
    octal(&mut h, 136, 12, 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = if dir { b'5' } else { b'0' };
    h[257..262].copy_from_slice(b"ustar");
    h[263..265].copy_from_slice(b"00");
    octal(&mut h, 329, 8, 0);
    octal(&mut h, 337, 8, 0);
    h[345..345 + prefix.len()].copy_from_slice(prefix.as_bytes());
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    h[154] = 0;
    h[155] = b' ';
    Some(h)
}

/// lgx's `splitPath`: the whole path as the name if it fits, else a prefix and a name.
fn split_ustar(path: &str) -> Option<(&str, &str)> {
    const NAME: usize = 100;
    const PREFIX: usize = 155;
    if path.len() <= NAME {
        return Some((path, ""));
    }
    let b = path.as_bytes();
    let mut i = path.len() - NAME;
    while i < path.len() && i <= PREFIX {
        if b[i] == b'/' && i <= PREFIX && path.len() - i - 1 <= NAME {
            return Some((&path[i + 1..], &path[..i]));
        }
        i += 1;
    }
    let mut i = PREFIX.min(path.len() - 1);
    while i > 0 {
        if b[i] == b'/' && path.len() - i - 1 <= NAME {
            return Some((&path[i + 1..], &path[..i]));
        }
        i -= 1;
    }
    None
}

/// Whether a `.lgx` is laid out exactly as lgx's own writer lays one out (`Package::save` through
/// `DeterministicTarWriter`): one gzip member, entries sorted by path, each header byte for byte
/// what lgx writes, two zero blocks and nothing after, and `manifest.json` in lgx's canonical
/// JSON (sorted keys, two-space indent: the bytes `lgx_get_manifest_json` returns, which lgpm
/// writes as the installed manifest). `lezreg app record` vouches only for such a package, so a
/// publisher can't be led to vouch for a file that some reader reads differently.
pub fn check_canonical_layout(bytes: &[u8]) -> Result<()> {
    let raw = read_raw(bytes)?;
    // Every check of the entries themselves (paths, duplicates, coverage) first.
    read_lgx_entries(bytes)?;
    let mut last: Option<String> = None;
    let mut used = 0;
    for e in &raw.entries {
        let sort_key = if e.dir { format!("{}/", e.path.trim_end_matches('/')) } else { e.path.clone() };
        if last.as_ref().is_some_and(|l| *l >= sort_key) {
            return Err(bad(&format!("{}: out of lgx's order (entries are sorted by path)", e.path)));
        }
        let want = canonical_header(&e.path, e.dir, e.mode, e.data.len()).ok_or_else(|| bad(&format!("{}: a path lgx can't write", e.path)))?;
        if want != e.header {
            return Err(bad(&format!("{}: its tar header is not the one lgx writes", e.path)));
        }
        used += BLOCK + e.data.len().div_ceil(BLOCK) * BLOCK;
        last = Some(sort_key);
    }
    if raw.end_at != used + 2 * BLOCK || raw.tar_len != raw.end_at {
        return Err(bad("the tar doesn't end with exactly two zero blocks, as lgx writes it"));
    }
    let manifest = &raw.entries.iter().find(|e| e.path == "manifest.json").expect("checked").data;
    let json = crate::apps::strict_json(manifest)?;
    if canonical_json(&json)?.as_bytes() != manifest.as_slice() {
        return Err(bad("manifest.json is not in lgx's canonical form (sorted keys, two-space indent), so an installed copy's manifest couldn't be matched to it"));
    }
    Ok(())
}

/// JSON as lgx's manifest writer prints it (nlohmann `dump(2)` of a sorted map): sorted keys,
/// two-space indent, `": "`, no trailing newline. Only integers (no floats in a manifest).
pub fn canonical_json(v: &Value) -> Result<String> {
    fn write(v: &Value, depth: usize, out: &mut String) -> Result<()> {
        let pad = |n: usize| "  ".repeat(n);
        match v {
            Value::Object(m) if m.is_empty() => out.push_str("{}"),
            Value::Array(a) if a.is_empty() => out.push_str("[]"),
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                out.push_str("{\n");
                for (i, k) in keys.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    out.push_str(&serde_json::to_string(k).expect("a string"));
                    out.push_str(": ");
                    write(&m[*k], depth + 1, out)?;
                    out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push('}');
            }
            Value::Array(a) => {
                out.push_str("[\n");
                for (i, x) in a.iter().enumerate() {
                    out.push_str(&pad(depth + 1));
                    write(x, depth + 1, out)?;
                    out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
                }
                out.push_str(&pad(depth));
                out.push(']');
            }
            Value::Number(n) if n.is_f64() => return Err(bad("a manifest with a fractional number")),
            other => out.push_str(&other.to_string()),
        }
        Ok(())
    }
    let mut out = String::new();
    write(v, 0, &mut out)?;
    Ok(out)
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
        gz(&raw_tar(entries))
    }

    /// A ustar header as common writers write it (Python's tarfile, GNU tar in ustar mode).
    pub fn header(path: &str, size: usize, kind: u8) -> [u8; 512] {
        let mut h = [0_u8; 512];
        h[..path.len()].copy_from_slice(path.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        reseal(&mut h);
        h
    }

    /// Recomputes a header's checksum after an edit.
    pub fn reseal(h: &mut [u8; 512]) {
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    }

    pub fn raw_tar(entries: &[(&str, Option<&[u8]>, u8)]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (path, data, kind) in entries {
            tar.extend_from_slice(&header(path, data.map_or(0, <[u8]>::len), *kind));
            if let Some(d) = data {
                tar.extend_from_slice(d);
                tar.resize(tar.len().div_ceil(512) * 512, 0);
            }
        }
        tar.extend_from_slice(&[0; 1024]);
        tar
    }

    pub fn gz(tar: &[u8]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, tar).unwrap();
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

    /// A small genuine package's tar entries, for the archive tests below.
    fn small() -> Vec<(&'static str, Option<&'static [u8]>, u8)> {
        let m: &'static [u8] = Box::leak(
            serde_json::json!({"name":"x","hashes": merkle_tree(&[("variants/v/a".to_owned(), hex_sha256(b"1"))].into_iter().collect())})
                .to_string()
                .into_bytes()
                .into_boxed_slice(),
        );
        vec![("manifest.json", Some(m), b'0'), ("variants/v/a", Some(b"1"), b'0')]
    }

    fn refused(bytes: &[u8], why: &str) {
        let e = Contents::from_lgx(bytes).unwrap_err().to_string();
        assert!(e.contains(why), "{why}: {e}");
    }

    /// Review B1 and its battery: every header field and archive shape where lgx's TarReader
    /// could read something other than what this reader reads is refused.
    #[test]
    fn archives_lgx_could_read_differently_are_refused() {
        assert!(Contents::from_lgx(&gz(&raw_tar(&small()))).unwrap().root_matches());
        let edit = |at: std::ops::Range<usize>, value: &[u8]| {
            let mut tar = raw_tar(&small());
            let mut h: [u8; 512] = tar[..512].try_into().unwrap();
            h[at].copy_from_slice(value);
            reseal(&mut h);
            tar[..512].copy_from_slice(&h);
            gz(&tar)
        };
        let size = 512 * 3;
        // B1: a size lgx reads past a leading NUL (or space) that a NUL-stopping reader reads as 0.
        refused(&edit(124..136, format!("\0{size:010o}\0").as_bytes()), "size field is not plain octal");
        refused(&edit(124..136, format!(" \0{size:09o}\0").as_bytes()), "size field is not plain octal");
        refused(&edit(124..136, format!(" {size:010o}\0").as_bytes()), "size field is not plain octal");
        // Junk after the digits (lgx stops there), base-256, an empty field.
        refused(&edit(124..136, format!("{:011o}9", 3).as_bytes()), "size field is not plain octal");
        let mut b256 = [0_u8; 12];
        b256[0] = 0x80;
        b256[11] = 3;
        refused(&edit(124..136, &b256), "size field is not plain octal");
        refused(&edit(100..108, &[0; 8]), "mode field is not plain octal");
        // A checksum with a leading NUL (lgx skips it).
        let mut tar = raw_tar(&small());
        let sum: u32 = tar[..512].iter().enumerate().map(|(i, &b)| if (148..156).contains(&i) { 32 } else { u32::from(b) }).sum();
        tar[148..156].copy_from_slice(format!("\0{sum:06o}\0").as_bytes());
        refused(&gz(&tar), "checksum field is not plain octal");
        // Not ustar (GNU's old magic puts other data where lgx reads the prefix).
        refused(&edit(257..265, b"ustar  \0"), "not POSIX ustar");
        // Bytes after the name's NUL, a link name, junk in the padding.
        refused(&edit(98..100, b"\0x"), "after the end of its name");
        refused(&edit(157..160, b"abc"), "link name");
        refused(&edit(500..502, b"zz"), "padding");
        // Non-zero padding after a file's data.
        let mut tar = raw_tar(&small());
        let pad_at = 512 + small()[0].1.unwrap().len();
        tar[pad_at] = b'!';
        refused(&gz(&tar), "padding after its data");
        // A lone zero block (lgx reads on past it), data after the end marker.
        let entries = small();
        let mut tar = Vec::new();
        tar.extend_from_slice(&raw_tar(&entries[..1])[..]);
        tar.truncate(tar.len() - 1024);
        tar.extend_from_slice(&[0; 512]);
        tar.extend_from_slice(&raw_tar(&entries[1..]));
        refused(&gz(&tar), "lone zero block");
        let mut tar = raw_tar(&small());
        tar.extend_from_slice(&header("variants/v/evil", 0, b'0'));
        tar.extend_from_slice(&[0; 1024]);
        refused(&gz(&tar), "after the tar's end-of-archive marker");
        // No end marker at all.
        let tar = raw_tar(&small());
        refused(&gz(&tar[..tar.len() - 1024]), "no end-of-archive marker");
        // Two gzip members, or junk after the member.
        let tar = raw_tar(&small());
        let mut two = gz(&tar[..1024]);
        two.extend(gz(&tar[1024..]));
        refused(&two, "bytes after the gzip stream");
        let mut junk = gz(&tar);
        junk.extend_from_slice(b"GARBAGE");
        refused(&junk, "bytes after the gzip stream");
        // manifest.sig twice (lgx keeps the later one).
        let mut e = small();
        e.insert(1, ("manifest.sig", Some(b"{}"), b'0'));
        e.insert(2, ("manifest.sig", Some(b"{}"), b'0'));
        refused(&gz(&raw_tar(&e)), "twice");
        // A file and a directory with the same path.
        let mut e = small();
        e.push(("variants/v/a/", None, b'5'));
        refused(&gz(&raw_tar(&e)), "both a file and a directory");
        // A directory with a size.
        let mut tar = raw_tar(&[("variants/", None, b'5')]);
        let mut h: [u8; 512] = tar[..512].try_into().unwrap();
        h[124..136].copy_from_slice(b"00000001000\0");
        reseal(&mut h);
        tar[..512].copy_from_slice(&h);
        refused(&gz(&tar), "directory with data");
    }

    /// What `record` accepts: only a package laid out exactly as lgx writes one.
    #[test]
    fn only_lgxs_own_layout_is_canonical() {
        let files: BTreeMap<String, String> = [("variants/v/a".to_owned(), hex_sha256(b"1"))].into_iter().collect();
        let manifest = canonical_json(&serde_json::json!({"name": "x", "version": "1.0.0", "hashes": merkle_tree(&files)})).unwrap();
        let entries: Vec<(&str, bool, &[u8])> =
            vec![("manifest.json", false, manifest.as_bytes()), ("variants/", true, b""), ("variants/v/", true, b""), ("variants/v/a", false, b"1")];
        let build = |entries: &[(&str, bool, &[u8])]| {
            let mut tar = Vec::new();
            for (p, dir, d) in entries {
                tar.extend_from_slice(&canonical_header(p, *dir, 0o644, d.len()).unwrap());
                tar.extend_from_slice(d);
                tar.resize(tar.len().div_ceil(512) * 512, 0);
            }
            tar.extend_from_slice(&[0; 1024]);
            tar
        };
        let tar = build(&entries);
        check_canonical_layout(&gz(&tar)).unwrap();
        assert!(Contents::from_lgx(&gz(&tar)).unwrap().root_matches());
        // Out of order.
        let mut swapped = entries.clone();
        swapped.swap(0, 3);
        assert!(check_canonical_layout(&gz(&build(&swapped))).unwrap_err().to_string().contains("order"));
        // Readable, but not lgx's header (Python-style mode field) or not its manifest form.
        let mut other = tar.clone();
        other[..512].copy_from_slice(&header("manifest.json", manifest.len(), b'0'));
        assert!(Contents::from_lgx(&gz(&other)).is_ok());
        assert!(check_canonical_layout(&gz(&other)).unwrap_err().to_string().contains("header"));
        let compact = serde_json::json!({"name": "x", "version": "1.0.0", "hashes": merkle_tree(&files)}).to_string();
        let mut e2 = entries.clone();
        e2[0].2 = compact.as_bytes();
        assert!(check_canonical_layout(&gz(&build(&e2))).unwrap_err().to_string().contains("canonical form"));
        // Extra zero blocks after the end marker: readable, not lgx's.
        let mut padded = tar.clone();
        padded.extend_from_slice(&[0; 512]);
        assert!(Contents::from_lgx(&gz(&padded)).is_ok());
        assert!(check_canonical_layout(&gz(&padded)).is_err());
        assert_eq!(canonical_json(&serde_json::json!({"b": [1, {}], "a": "\u{e9}\n", "c": []})).unwrap(), "{\n  \"a\": \"\u{e9}\\n\",\n  \"b\": [\n    1,\n    {}\n  ],\n  \"c\": []\n}");
    }
}
