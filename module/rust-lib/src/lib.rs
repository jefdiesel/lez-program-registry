//! `program_registry`: the LEZ Program Registry as a Logos core module (DESIGN §7.3), so any
//! Basecamp app, a wallet's approval prompt above all, can turn an opaque program id into a
//! name, an author and a verification status with one call.
//!
//! Every method returns JSON text in the shape Logos modules use:
//!
//! ```json
//! { "success": true,  "value": <answer>, "error": null }
//! { "success": false, "value": null,     "error": "why" }
//! ```
//!
//! It reads the sequencer the user configured and nothing else (the IDL also from the user's
//! own Logos Storage node). Method and parameter names avoid C++ keywords: the generated C++
//! wrappers use them verbatim (logos-cpp-sdk#183).

use std::path::PathBuf;

mod bridge;
mod labels;
mod net;
mod network_file;
pub mod writes;

use lezreg_sdk::{
    Kind, Registry, Rpc as _,
    apps::{self, Names},
    idl::{self, FetchConfig, IdlStatus},
    lgx::Contents,
    json as js,
    types::AccountId,
};
use serde_json::{Value, json};
use net::Seq;

/// The sequencer of a module with no network profile at all (unit tests; a broken build). A
/// normal build takes its defaults from the network profile: the built-in `network-rc3.json`
/// (our LEZ v0.4.0-rc3 chain and its registry), or the file the host names in `network_file`.
const DEFAULT_SEQUENCER: &str = "http://127.0.0.1:3062";
/// Most entries a list or search call returns.
const MAX_LIMIT: i64 = 200;
/// How long an app check is kept: a name's records change rarely, and deriving a name takes a
/// second of Argon2.
const APP_CHECK_TTL_MS: u64 = 5 * 60 * 1000;

pub trait ProgramRegistryModule: Send + 'static {
    /// Everything the registry says about a program: `{program, deployer, header, freshness,
    /// third_party_count}`. `deployer` is the entry only the program's own header key could
    /// write; `freshness` is `fresh`, `stale` (the program now runs another image) or
    /// `no-program`.
    fn resolve_program(&mut self, program_id: String) -> String;

    /// A one-line label for a wallet prompt: `{text, kind, verified, stale, upgradeable,
    /// name, version, author, author_short, author_label_unproven, community_count}`. `kind` is
    /// `deployer` (written by the program's own header key), `community` (no deployer entry, but
    /// others described it: unproven) or `unregistered`; `verified` is true only when this
    /// machine rebuilt the running image from its recorded source (`lezreg verify --mark`) and
    /// it still runs that image.
    fn program_label(&mut self, program_id: String) -> String;

    /// The program's IDL from Logos Storage, hash-checked against the chain: `{status, reason,
    /// format, format_version, calls, idl}`. `status` is `ok` (a format the SDK knows: SPEL v1 or
    /// v2), `unknown-format` (the right bytes, a JSON object, but no format the SDK knows: `idl`
    /// still holds it), `unavailable`, `hash-mismatch` or `unparseable`, with `reason` in words;
    /// `calls` is the common call view (`[{name, tag, args}]`). Never fails because of the IDL.
    fn get_idl(&mut self, program_id: String) -> String;

    /// The global list, newest first: `{items, next, total}`. Pass `next` back as `after`
    /// (empty for the first page).
    fn list_entries(&mut self, after: String, limit: i64) -> String;

    /// Entries whose name matches `query`: exact first, then containing it.
    fn search_name(&mut self, query: String, limit: i64) -> String;

    /// Community (third-party) entries for a program.
    fn third_party_entries(&mut self, program_id: String) -> String;

    /// Whether the installed Basecamp module `module_name` is really published by the LNS name
    /// its package names (docs/APPS.md): `{status, publisher, module, version, root, record}`.
    /// `status` is `verified` (the name vouches for this exact package), `files_verified` (the
    /// recorded root is not vouched for, but the installed files match it), `mismatch`,
    /// `root_mismatch`, `file_mismatch`, `manifest_mismatch`, `ambiguous`, `not_published`,
    /// `no_such_name`, or `unclaimed`. The root in the result is **recomputed from the files**,
    /// never copied from `manifest.json`. `claimed.record.programs` are the programs the app
    /// says it calls. Reads the installed package from Basecamp's data folder and one LNS
    /// record from the sequencer.
    fn verify_app(&mut self, module_name: String) -> String;

    /// The same check for a package not yet installed, given a path to an `.lgx` file or an
    /// installed package directory (not a bare manifest: a manifest's root is a claim, and the
    /// SDK cannot recompute it without the files). For an install view.
    fn verify_package(&mut self, package_path: String) -> String;

    /// The apps an LNS name publishes: `{name, apps: [{module, version, roots, catalog,
    /// programs: [label]}]}`, each program with its `program_label`. Only someone who knows a name
    /// can read its apps: nothing lists them.
    fn publisher_apps(&mut self, name: String) -> String;

    /// `{sequencer, registry, storage_api, lns, basecamp_data, cli, registry_images, network}`.
    /// `network` is the profile the defaults come from: `{name, chain_id, rpc, registry, lns,
    /// source, file, error}` (`source` is `built-in`, `file` or `none`). Nothing here asks the
    /// chain; compare `chain_id` with the sequencer's `getChannelId` to be sure.
    fn config_get(&mut self) -> String;

    /// Changes any of `storage_api` and `lns` (the LNS program id, for the app checks; empty for
    /// the network's default) from a JSON object, and remembers them (defaults are never
    /// written). **Only the registry browser or the host may call it** (`code: "not_allowed"`
    /// otherwise). `sequencer` (https://, or http:// to this machine), `registry` (empty for the
    /// network's default), `network_file` (an absolute path to a `names-network` file whose
    /// `rpc`, `programs.program_registry` and `programs.lns_registry` become the defaults; empty
    /// for the built-in profile), `basecamp_data`, `cli` (the `lezreg` that signs) and
    /// `registry_images` (registry code to trust beyond the committed build) are the host's
    /// alone, or the settings file's.
    fn config_set(&mut self, settings_json: String) -> String;

    /// A deployer entry for a program (DESIGN §2.4 `RegisterDeployer`), as a change to sign:
    /// input `{program, author?, name, version, author_label?, description?, tags?, idl, source,
    /// build, image_id?, payer?, allow_remote?}`. `idl`, `source` and `build` take the shapes
    /// `resolve_program` shows; `image_id` defaults to the live program's. The program's header
    /// key and `author` (default: the program) sign. Nothing is sent: pass the answer to
    /// `submit`. Errors carry a `code` (`bad_input`, `no_registry`, `no_program`, `exists`, …).
    fn prepare_register(&mut self, input_json: String) -> String;

    /// A community (third-party) entry for someone else's program, by `publisher`: the same
    /// input with `publisher` instead of `author`. The publisher signs.
    fn prepare_register_third_party(&mut self, input_json: String) -> String;

    /// An update by the entry's author: `{entry (or program, for its deployer entry), author?,
    /// version?, description?, tags?, author_label?, idl?, source?, build?, image_id?}`; only the
    /// fields given change. Reads the entry for its revision, author and frozen flag.
    fn prepare_update(&mut self, input_json: String) -> String;

    /// Hands an entry to `new_author`: `{entry (or program), new_author, author?}`. Both the
    /// current and the new author sign, in one transaction, so one wallet must hold both keys.
    fn prepare_transfer(&mut self, input_json: String) -> String;

    /// Freezes an entry for good (no more updates or transfers): `{entry (or program), author?}`.
    fn prepare_freeze(&mut self, input_json: String) -> String;

    /// Signs and sends a change a `prepare_*` method returned. Answers at once with
    /// `{job, state: "running"}`; `submit_status(job)` follows it and the `change_settled`
    /// event fires when it ends (with only the job and its state; details are for
    /// `submit_status`). Takes only a change this same app prepared, exactly as returned, once
    /// (`code: "unknown_change"` / `"changed"`). From any app other than the registry browser or
    /// the host, nothing is signed yet: the change waits as a request (`{request, state:
    /// "awaiting_approval"}`) until the person approves it in the registry browser
    /// (`approve_request`) or it expires with its window; its job then shows in
    /// `submit_status(request)`. Signed by the person's `lezreg` CLI wallet (`signer` absent or
    /// `"cli"`); `"signer": "lez_core"` answers `code: "lez_core_unavailable"` until a usable
    /// release of that module exists. A change prepared more than 10 minutes ago is refused
    /// (`code: "expired"`): the registry's time window has passed. Prepared changes, requests
    /// and jobs live in memory: a module restart forgets them (a `lezreg` already started keeps
    /// running and its transaction may still land; read the chain to know).
    fn submit(&mut self, change_json: String) -> String;

    /// A submitted change: `{job, what, entry, state (running|applied|failed), receipt,
    /// error, code, log_tail, requested_by}`, or a request still `awaiting_approval` /
    /// `denied`. An empty `job` lists the recent ones. An app sees only what it submitted.
    fn submit_status(&mut self, job: String) -> String;

    /// Who would sign: `{cli, cli_version, cli_error, lez_core}`. Trusted callers only.
    fn signer_status(&mut self) -> String;

    /// Drops the reads this module keeps for a while (about 30 s), so the next calls read the
    /// chain again: an app's Refresh. `{forgotten: true}`. Open to every caller (it only costs
    /// paced reads).
    fn forget_reads(&mut self) -> String;

    /// Changes other apps submitted that wait for the person's approval: `[{request, from,
    /// at, change}]`. Trusted callers only (the registry browser shows them).
    fn pending_requests(&mut self) -> String;

    /// Signs and sends a waiting request (as `submit` does) and answers its job. Trusted callers.
    fn approve_request(&mut self, request: String) -> String;

    /// Refuses a waiting request; nothing is signed. Trusted callers only.
    fn deny_request(&mut self, request: String) -> String;

    /// Called once the module is wired to the host.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

/// Events (the builder generates an `emit_<event>` for each).
pub trait ProgramRegistryModuleEvents {
    /// A change handed to `submit` ended: its `job`, `state` (`applied` or `failed`) and
    /// `submit_status`'s answer for it as JSON text.
    fn change_settled(&self, job: String, state: String, detail: String);
}

#[cfg(test)]
pub struct RustModuleContext;

/// Unit tests build without the generated scaffold and its event emitters.
#[cfg(test)]
fn emit_change_settled(_job: &str, _state: &str, _detail: &str) {}

/// Where the module's defaults come from: a `names-network` profile, read once.
#[derive(Clone, Debug)]
struct Defaults {
    sequencer: String,
    registry: Option<AccountId>,
    lns: Option<AccountId>,
    storage_api: String,
    /// The profile itself; `None` for a module with no profile (tests).
    file: Option<network_file::NetworkFile>,
    /// `built-in`, `file` or `none`.
    source: &'static str,
    /// The file named by `network_file`, when that is where they come from.
    path: Option<PathBuf>,
}

impl Defaults {
    /// No profile: a local sequencer and no registry (what every build had before 0.3.1).
    fn none() -> Self {
        Self { sequencer: DEFAULT_SEQUENCER.to_owned(), registry: None, lns: None, storage_api: lezreg_sdk::storage::DEFAULT_API.to_owned(), file: None, source: "none", path: None }
    }

    fn from_profile(file: network_file::NetworkFile, source: &'static str, path: Option<PathBuf>) -> Self {
        Self {
            sequencer: file.rpc.clone(),
            registry: file.registry,
            lns: file.lns,
            storage_api: file.storage_api.clone().unwrap_or_else(|| lezreg_sdk::storage::DEFAULT_API.to_owned()),
            file: Some(file),
            source,
            path,
        }
    }

    /// The built-in profile (our rc3 chain).
    fn builtin() -> Self {
        network_file::builtin().map_or_else(|_| Self::none(), |f| Self::from_profile(f, "built-in", None))
    }

    /// The profile in `path` (absolute), or the built-in one for `None`.
    fn load(path: Option<&std::path::Path>) -> Result<Self, String> {
        let Some(path) = path else { return Ok(Self::builtin()) };
        if !path.is_absolute() {
            return Err(format!("network_file '{}' must be an absolute path", path.display()));
        }
        let meta = std::fs::metadata(path).map_err(|e| format!("network_file {}: {e}", path.display()))?;
        if meta.len() > 64 * 1024 {
            return Err(format!("network_file {} is larger than 64 KiB", path.display()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| format!("network_file {}: {e}", path.display()))?;
        let file = network_file::parse(&text).map_err(|e| format!("network_file {}: {e}", path.display()))?;
        if !usable_sequencer(&file.rpc) {
            return Err(format!("network_file {}: rpc '{}' must be https://, or http:// to this machine", path.display(), file.rpc));
        }
        Ok(Self::from_profile(file, "file", Some(path.to_owned())))
    }

    fn shown(&self, error: Option<&str>) -> Value {
        let f = self.file.as_ref();
        json!({"name": f.map(|f| clean(&f.name)), "chain_id": f.map(|f| f.chain_id.clone()), "rpc": f.map(|f| f.rpc.clone()),
               "registry": self.registry.map(|r| r.to_base58()), "lns": self.lns.map(|l| l.to_base58()),
               "registry_image": f.and_then(|f| f.registry_image).map(hex::encode),
               "source": self.source, "file": self.path.as_ref().map(|p| p.display().to_string()), "error": error})
    }
}

pub struct ProgramRegistry {
    /// The profile the defaults below come from; settings equal to them are not saved, so a
    /// newer build's profile (a chain reset, a new registry) takes over by itself.
    defaults: Defaults,
    /// Why the `network_file` in the settings file could not be used (the built-in profile is).
    network_error: Option<String>,
    sequencer: String,
    registry: Option<AccountId>,
    storage_api: String,
    lns: Option<AccountId>,
    basecamp_data: Option<PathBuf>,
    /// The `lezreg` that signs changes, when set (else found on PATH and the usual places).
    cli: Option<PathBuf>,
    /// Registry images trusted beyond [`KNOWN_REGISTRY_IMAGES`] (host-pinned).
    registry_images: Vec<[u8; 32]>,
    /// LNS reads, kept with their derived names, for the sequencer and LNS program they read.
    names: Option<(String, AccountId, Names<Seq>)>,
    /// `(module, root)` → (verdict, when), for [`APP_CHECK_TTL_MS`].
    checked: std::collections::HashMap<(String, [u8; 32]), (Value, u64)>,
    /// LNS names derived for `author_label_proven` (labels.rs), kept across calls.
    label_keys: labels::Keys,
}

impl ProgramRegistry {
    fn with_settings(sequencer: String, registry: Option<AccountId>, storage_api: String, lns: Option<AccountId>, basecamp_data: Option<PathBuf>) -> Self {
        Self { defaults: Defaults::none(), network_error: None, sequencer, registry, storage_api, lns, basecamp_data, cli: None, registry_images: Vec::new(), names: None, checked: std::collections::HashMap::new(), label_keys: labels::Keys::default() }
    }

    /// A module on `defaults`, nothing else set.
    fn on(defaults: Defaults) -> Self {
        let mut me = Self::with_settings(defaults.sequencer.clone(), defaults.registry, defaults.storage_api.clone(), defaults.lns, None);
        me.defaults = defaults;
        me
    }

    /// Starts from the built-in profile, then the settings file. A `network_file` that can't be
    /// read leaves the built-in profile in place (said in `config_get`), the other settings
    /// still apply.
    fn from_saved(saved: Option<Value>) -> Self {
        let mut me = Self::on(Defaults::builtin());
        let Some(mut saved) = saved else { return me };
        // Builds before 0.3.1 wrote every setting, the old default `http://127.0.0.1:3062` too,
        // even when nothing was configured (no registry): that pin is not a choice, drop it.
        if saved.get("sequencer").and_then(Value::as_str) == Some(DEFAULT_SEQUENCER) && !matches!(saved.get("registry"), Some(Value::String(r)) if !r.is_empty()) {
            if let Some(o) = saved.as_object_mut() {
                o.remove("sequencer");
            }
        }
        if let Some(f) = saved.get("network_file").and_then(Value::as_str).filter(|f| !f.is_empty()) {
            match Defaults::load(Some(std::path::Path::new(f))) {
                Ok(d) => me = Self::on(d),
                Err(e) => me.network_error = Some(e),
            }
        }
        if let Some(o) = saved.as_object_mut() {
            o.remove("network_file");
        }
        let _ = me.apply(&saved);
        me
    }
}

impl Default for ProgramRegistry {
    fn default() -> Self {
        Self::from_saved(config_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()))
    }
}

fn reply(r: Result<Value, String>) -> String {
    match r {
        Ok(v) => json!({"success": true, "value": v, "error": null}),
        Err(e) => failed(&e, None),
    }
    .to_string()
}

/// A failure as an app shows it. Not reaching the sequencer (a rate limit, a 5xx, no
/// connection) is plain words and a stable code, with the raw cause (no response body: its
/// text without markup, 200 characters at most) in `detail` for a technical-details line, and
/// how long to wait in `retry_after_ms` after a rate limit.
fn failed(message: &str, code: Option<&str>) -> Value {
    match net::classify(message) {
        Some((code, words, detail, wait)) => json!({"success": false, "value": null, "error": words, "code": code, "detail": detail, "retry_after_ms": wait}),
        None => json!({"success": false, "value": null, "error": message, "code": code}),
    }
}

/// Like [`reply`], with a stable `code` beside the error text.
fn reply_coded(r: Result<Value, writes::Failure>) -> String {
    match r {
        Ok(v) => json!({"success": true, "value": v, "error": null, "code": null}),
        Err(f) => failed(&f.message, Some(&f.code)),
    }
    .to_string()
}

/// The one app that may change this module's settings, see other apps' requests and sign
/// through it without a request: its own browser. The host itself (Basecamp, or `logosctl`
/// driving the module) is trusted too; a caller that can't be named is not. Reads and
/// `prepare_*` stay open to everyone.
///
/// **What this trusts:** Basecamp names each app's credential after the package name the
/// package chose for itself. Until the module can check `registry_browser`'s publisher (the
/// LNS app checks, docs/APPS.md, once their root recomputation lands), any package that calls
/// itself `registry_browser` and gets installed in its place is trusted the same way.
const TRUSTED_APPS: &[&str] = &["registry_browser"];

fn trusted(caller: &logos_rust_sdk::LogosCaller) -> bool {
    matches!(caller, logos_rust_sdk::LogosCaller::HostAnchor) || TRUSTED_APPS.iter().any(|app| caller.is_module(app))
}

/// Settings that decide where changes are built and signed (`sequencer`, `registry`), point the
/// module at programs it runs or files it reads (`cli`, `basecamp_data`) or widen what it
/// trusts (`registry_images`): the host or the settings file only, never an app, not even the
/// browser (a package can take its name).
const HOST_ONLY_SETTINGS: &[&str] = &["sequencer", "registry", "network_file", "cli", "basecamp_data", "registry_images"];

#[cfg(not(test))]
fn calling() -> logos_rust_sdk::LogosCaller {
    logos_rust_sdk::current_caller()
}

// Tests run outside any dispatch; they act as the host unless a test says otherwise.
#[cfg(test)]
thread_local! {
    static TEST_CALLER: std::cell::RefCell<logos_rust_sdk::LogosCaller> =
        const { std::cell::RefCell::new(logos_rust_sdk::LogosCaller::HostAnchor) };
}

#[cfg(test)]
fn calling() -> logos_rust_sdk::LogosCaller {
    TEST_CALLER.with(|caller| caller.borrow().clone())
}

/// A name for who is calling, as the app chose it (shown, never trusted on its own).
fn caller_name(caller: &logos_rust_sdk::LogosCaller) -> String {
    match caller {
        logos_rust_sdk::LogosCaller::Module { name, .. } => clean(name),
        other => other.describe_for_human(),
    }
}

fn only_trusted() -> Result<(), writes::Failure> {
    let caller = calling();
    if trusted(&caller) {
        Ok(())
    } else {
        Err(writes::Failure::new("not_allowed", format!(
            "only the Program Registry app or the host may do this; refused for {}", caller.describe_for_human()
        )))
    }
}

/// Text from the chain or from an app, safe to put in a line someone reads, as ONE line: every
/// line or paragraph break (`\n`, `\r`, U+2028, U+2029, …) becomes a visible "⏎", so a value
/// can't add lines of its own; controls, bidirectional overrides and isolates, zero-width and
/// other invisible characters (soft hyphen, U+180E, U+2060–2064, U+206A–206F, U+FFF9–FFFB, tag
/// characters) are dropped.
pub fn clean(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => Some('⏎'),
            c if c.is_control() => None,
            '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{E007F}' => None,
            c => Some(c),
        })
        .collect()
}

/// The registry code this module builds changes for: the committed reproducible build
/// (`artifacts/lez_program_registry.imageid`, kept equal by `sdk/rust/tests/registry_image.rs`),
/// plus images the host pins with `config_set({"registry_images": [...]})`. `lezreg` checks the
/// same before it signs (`lezreg network set --registry-image`).
pub const KNOWN_REGISTRY_IMAGES: &[&str] = &["5ffbf8f5a8efa74ab8c1bb02396c9448daf4db2fba3a526150f9da411baf6bdc"];

/// A change `prepare_*` built: for the caller that prepared it, until it is submitted once or
/// its window passes. Submitting needs its random `change_id`, the same caller and the change
/// exactly as returned (M6 review A: keyed by the message alone, another app could prepare the
/// same message with its own payer and `allow_remote` and have it signed instead).
struct Prepared {
    by: String,
    change: Value,
    expires: u64,
}

static PREPARED: std::sync::Mutex<std::collections::BTreeMap<String, Prepared>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

const PREPARED_PER_CALLER: usize = 32;
const PREPARED_TOTAL: usize = 4096;

/// 16 bytes from the OS's random source, as hex: a change id no other app can guess.
fn fresh_id() -> Result<String, writes::Failure> {
    use std::io::Read as _;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| writes::Failure::new("error", format!("no randomness for a change id: {e}")))?;
    Ok(hex::encode(b))
}

/// Keeps `change` for `caller` and stamps it with its `change_id`.
fn remember(change: &mut Value, caller: &logos_rust_sdk::LogosCaller) -> Result<(), writes::Failure> {
    let id = fresh_id()?;
    change["change_id"] = json!(id);
    let expires = change["expires_at"].as_u64().unwrap_or(0);
    let mut p = PREPARED.lock().map_err(|_| writes::Failure::new("error", "prepared changes unavailable"))?;
    let now = now_ms();
    p.retain(|_, c| c.expires > now);
    // Per app, so one app preparing in a loop can't lock the others out; a global backstop.
    let by = caller.identity();
    if p.values().filter(|c| c.by == by).count() >= PREPARED_PER_CALLER {
        return Err(writes::Failure::new("too_many_changes", format!("this app has {PREPARED_PER_CALLER} changes waiting to be submitted; submit them or let them expire")));
    }
    if p.len() >= PREPARED_TOTAL {
        return Err(writes::Failure::new("too_many_changes", "too many changes are waiting to be submitted; try again in a few minutes"));
    }
    p.insert(id, Prepared { by, change: change.clone(), expires });
    Ok(())
}

/// Takes (once) the change `given` names: prepared by this same caller, unexpired, unchanged.
fn take_prepared(given: &Value, caller: &logos_rust_sdk::LogosCaller) -> Result<Value, writes::Failure> {
    let unknown = || writes::Failure::new("unknown_change", "submit only a change a prepare_* method returned to this app, unchanged, within 10 minutes, once");
    let id = given["change_id"].as_str().ok_or_else(unknown)?;
    let mut p = PREPARED.lock().map_err(|_| writes::Failure::new("error", "prepared changes unavailable"))?;
    let now = now_ms();
    p.retain(|_, c| c.expires > now);
    let stored = p.get(id).ok_or_else(unknown)?;
    if stored.by != caller.identity() {
        return Err(unknown());
    }
    let mut asked = given.clone();
    if let Some(o) = asked.as_object_mut() {
        o.remove("signer");
    }
    if asked != stored.change {
        p.remove(id);
        return Err(writes::Failure::new("changed", "this change differs from the one prepared (nothing outside the message may change either): prepare it again"));
    }
    Ok(p.remove(id).map(|c| c.change).expect("checked above"))
}

/// A change another app submitted, waiting for the person's approval until its window passes.
struct Request {
    /// The app's own name for itself (unverified) and its identity.
    from: String,
    by: String,
    at: u64,
    expires: u64,
    change: Value,
    /// Set once approved (the job) or denied.
    outcome: Option<Result<String, ()>>,
}

static REQUESTS: std::sync::Mutex<std::collections::BTreeMap<String, Request>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// IDLs found unavailable, by content sha256, with when: asked again within
/// [`IDL_RETRY_MS`] they answer from here instead of waiting on Storage again.
static IDL_MISSES: std::sync::Mutex<std::collections::BTreeMap<String, (Value, u64)>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// Storage fetches each caller started, by caller identity: (window start, count).
static IDL_FETCHES: std::sync::Mutex<std::collections::BTreeMap<String, (u64, u32)>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
const IDL_RETRY_MS: u64 = 60_000;
/// Storage fetches one caller may start a minute (a fetch can hold the module up to ~12 s,
/// and the module answers one call at a time).
const IDL_FETCHES_PER_MINUTE: u32 = 4;
/// The browser opens entries one after another: a larger allowance.
const IDL_FETCHES_PER_MINUTE_TRUSTED: u32 = 30;

#[cfg(test)]
fn config_path() -> Option<PathBuf> {
    // Never the real settings file: a Basecamp-installed module reads that one.
    Some(std::env::temp_dir().join(format!("lezreg-module-test-{}", std::process::id())).join("module.json"))
}

#[cfg(not(test))]
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lez-registry").join("module.json"))
}

/// The CLI's marks file (`lezreg verify --mark`), the same path rules as `lezreg`.
fn marks_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        Some(if cfg!(target_os = "macos") { home.join("Library/Application Support") } else { home.join(".local/share") })
    })?;
    Some(base.join("lez-registry").join("verified.json"))
}

/// Whether a local mark covers `program` running `image`.
fn locally_verified(program: &str, image: &[u8; 32]) -> bool {
    let Some(text) = marks_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return false };
    let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&text) else { return false };
    let img = hex::encode(image);
    m.values().any(|v| v["program"].as_str() == Some(program) && v["image_id"].as_str() == Some(&img) && v["verdict"].as_str() == Some("Match"))
}

/// A sequencer URL the module will talk to: `https://`, or `http://` only to this machine.
fn usable_sequencer(url: &str) -> bool {
    if url.starts_with("https://") {
        return url.len() > 8;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = if let Some(b) = host.strip_prefix('[') { b.split(']').next().unwrap_or("") } else { host.split(':').next().unwrap_or("") };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn program_arg(s: &str) -> Result<AccountId, String> {
    AccountId::from_base58(s.trim()).ok_or_else(|| format!("'{s}' is not a program id"))
}

fn limit_arg(limit: i64) -> usize {
    usize::try_from(limit.clamp(1, MAX_LIMIT)).unwrap_or(20)
}

impl ProgramRegistry {
    fn apply(&mut self, v: &Value) -> Result<(), String> {
        // First, so values given beside it override the new profile's.
        if let Some(f) = v.get("network_file").and_then(Value::as_str) {
            let next = Defaults::load((!f.is_empty()).then(|| std::path::Path::new(f)))?;
            // Whatever was the old profile's default follows the new profile; a value set by
            // hand stays.
            let old = std::mem::replace(&mut self.defaults, next);
            let new = &self.defaults;
            if self.sequencer == old.sequencer {
                self.sequencer.clone_from(&new.sequencer);
            }
            if self.registry == old.registry {
                self.registry = new.registry;
            }
            if self.lns == old.lns {
                self.lns = new.lns;
            }
            if self.storage_api == old.storage_api {
                self.storage_api.clone_from(&new.storage_api);
            }
            self.network_error = None;
        }
        if let Some(s) = v.get("sequencer").and_then(Value::as_str) {
            if !usable_sequencer(s) {
                return Err(format!("sequencer '{s}' must be https://, or http:// to this machine"));
            }
            s.clone_into(&mut self.sequencer);
        }
        if let Some(r) = v.get("registry").and_then(Value::as_str) {
            self.registry = if r.is_empty() { self.defaults.registry } else { Some(program_arg(r)?) };
        }
        if let Some(s) = v.get("storage_api").and_then(Value::as_str) {
            if !usable_sequencer(s) {
                return Err(format!("storage_api '{s}' must be https://, or http:// to this machine"));
            }
            s.clone_into(&mut self.storage_api);
        }
        if let Some(l) = v.get("lns").and_then(Value::as_str) {
            self.lns = if l.is_empty() { self.defaults.lns } else { Some(program_arg(l)?) };
        }
        if let Some(d) = v.get("basecamp_data").and_then(Value::as_str) {
            self.basecamp_data = if d.is_empty() { None } else { Some(PathBuf::from(d)) };
        }
        if let Some(c) = v.get("cli").and_then(Value::as_str) {
            if !c.is_empty() && !std::path::Path::new(c).is_absolute() {
                return Err(format!("cli '{c}' must be an absolute path"));
            }
            self.cli = if c.is_empty() { None } else { Some(PathBuf::from(c)) };
        }
        if let Some(list) = v.get("registry_images") {
            let list = list.as_array().ok_or("registry_images must be a list of 64-hex image ids")?;
            self.registry_images = list
                .iter()
                .map(|i| hex::decode(i.as_str().unwrap_or_default()).ok().and_then(|b| <[u8; 32]>::try_from(b).ok()).ok_or_else(|| format!("'{i}' is not a 64-hex image id")))
                .collect::<Result<_, _>>()?;
        }
        Ok(())
    }

    /// The settings as set (defaults left out, so a default never gets pinned in the file).
    fn settings(&self) -> Value {
        let d = &self.defaults;
        let mut out = json!({"basecamp_data": self.basecamp_data.as_ref().map(|d| d.display().to_string()),
               "cli": self.cli.as_ref().map(|d| d.display().to_string()),
               "registry_images": self.registry_images.iter().map(hex::encode).collect::<Vec<_>>()});
        if let Some(p) = &d.path {
            out["network_file"] = json!(p.display().to_string());
        }
        if self.sequencer != d.sequencer {
            out["sequencer"] = json!(self.sequencer);
        }
        if self.registry != d.registry {
            out["registry"] = json!(self.registry.map(|r| r.to_base58()));
        }
        if self.lns != d.lns {
            out["lns"] = json!(self.lns.map(|l| l.to_base58()));
        }
        if self.storage_api != d.storage_api {
            out["storage_api"] = json!(self.storage_api);
        }
        out
    }

    fn shown(&self) -> Value {
        json!({"sequencer": self.sequencer, "registry": self.registry.map(|r| r.to_base58()), "storage_api": self.storage_api,
               "lns": self.lns.map(|l| l.to_base58()), "lns_is_default": self.lns.is_some() && self.lns == self.defaults.lns,
               "network": self.defaults.shown(self.network_error.as_deref()), "basecamp_data": self.basecamp_data.as_ref().map(|d| d.display().to_string()),
               "cli": self.cli.as_ref().map(|d| d.display().to_string()),
               "registry_images": KNOWN_REGISTRY_IMAGES.iter().map(|i| (*i).to_owned()).chain(self.registry_images.iter().map(hex::encode)).collect::<Vec<_>>()})
    }

    /// `author_label_proven` for one answer: `lns:<name>` labels checked against the network's
    /// LNS program, through the same paced, cached reads (labels.rs).
    fn labels(&mut self) -> labels::Check<'_, Seq> {
        labels::Check::new(&mut self.label_keys, Seq::cached(&self.sequencer), self.lns, now_ms())
    }

    /// Reads for the apps: paced, batched and cached for a while (net.rs).
    fn reader(&self) -> Result<Registry<Seq>, String> {
        let reg = self.registry.ok_or("no registry configured: call config_set with {\"registry\": \"<id>\"}")?;
        Ok(Registry::new(Seq::cached(&self.sequencer), reg))
    }

    fn save(&self) -> Result<(), String> {
        let p = config_path().ok_or("no config directory")?;
        let dir = p.parent().expect("has a parent");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // Unique per save, so two saves at once can't rename each other's file.
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = dir.join(format!(".module.json.{}.{}.tmp", std::process::id(), SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(&tmp, self.settings().to_string()).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    fn label(&self, program_id: &str) -> Result<Value, String> {
        let program = program_arg(program_id)?;
        let r = self.reader()?.resolve(&program).map_err(|e| e.to_string())?;
        let short = format!("{}…{}", &program_id.get(..6).unwrap_or(program_id), program_id.get(program_id.len().saturating_sub(4)..).unwrap_or(""));
        let live = r.header.as_ref().map(|h| h.image_id);
        let upgradeable = r.header.as_ref().is_some_and(|h| !h.immutable);
        let Some(e) = &r.deployer else {
            // No deployer entry: what others say about it, shown as theirs (unproven).
            let community: Vec<lezreg_sdk::types::Entry> = self.reader()?.third_party(&program, 20).map_err(|e| e.to_string())?
                .into_iter().filter(|i| matches!(i.kind, Some(Kind::ThirdParty { .. }))).filter_map(|i| i.entry).collect();
            if let Some(c) = community.first() {
                return Ok(json!({"text": format!("{} {} (community entry, unproven)", clean(&c.name), clean(&c.version)), "kind": "community", "verified": false,
                                  "stale": live.is_some_and(|l| l != c.image_id), "upgradeable": upgradeable, "deployed": live.is_some(),
                                  "name": clean(&c.name), "version": clean(&c.version), "author": c.author.to_base58(), "author_short": short_id(&c.author.to_base58()),
                                  "author_label_unproven": clean(&c.author_label), "community_count": community.len()}));
            }
            return Ok(json!({"text": format!("{short} (not in the registry)"), "kind": "unregistered", "verified": false, "stale": false, "upgradeable": upgradeable, "deployed": live.is_some(), "community_count": 0}));
        };
        // Stale: a program runs here, but not the code registered. No program at all is said so.
        let stale = live.is_some_and(|l| l != e.image_id);
        let verified = !stale && live.is_some_and(|i| locally_verified(program_id, &i));
        // Names and versions are anyone's text: cleaned of characters that reorder or hide what a
        // reader sees (the registry's own validation allows them inside versions and labels).
        let mut text = format!("{} {}", clean(&e.name), clean(&e.version));
        if verified {
            text.push_str(" ✓ verified");
        }
        if stale {
            text.push_str(" (stale: the program runs other code than registered)");
        }
        if live.is_none() {
            text.push_str(" (no program deployed at this id)");
        }
        Ok(json!({"text": text, "kind": "deployer", "verified": verified, "stale": stale, "upgradeable": upgradeable, "deployed": live.is_some(),
                  "name": clean(&e.name), "version": clean(&e.version), "author": e.author.to_base58(), "author_short": short_id(&e.author.to_base58()),
                  "author_label_unproven": clean(&e.author_label)}))
    }

    /// The registry a write is for, coded.
    fn write_registry(&self) -> Result<AccountId, writes::Failure> {
        self.registry.ok_or_else(|| writes::Failure::new("no_registry", "no registry configured: call config_set with {\"registry\": \"<id>\"}"))
    }

    /// Builds changes only for a registry running known code (M6 review E: a program that
    /// answers with a fake self-entry and receives the signer's account can move its funds).
    /// `lezreg` checks the same before it signs.
    fn check_registry_code(&self, reader: &Registry<Seq>, reg: &AccountId) -> Result<(), writes::Failure> {
        let header = reader.header(reg).map_err(|e| writes::Failure::new("transport", e.to_string()))?
            .ok_or_else(|| writes::Failure::new("unknown_registry", format!("no program is deployed at the registry id {}", reg.to_base58())))?;
        let live = hex::encode(header.image_id);
        // The network profile's registry image counts too (the built-in file's, or the host's
        // `network_file`), as `lezreg` trusts its network file's.
        let profile_image = self.defaults.file.as_ref().and_then(|f| f.registry_image).map(hex::encode);
        if KNOWN_REGISTRY_IMAGES.contains(&live.as_str())
            || self.registry_images.iter().any(|i| hex::encode(i) == live)
            || profile_image.as_deref() == Some(live.as_str())
        {
            return Ok(());
        }
        Err(writes::Failure::new("unknown_registry", format!(
            "the registry id {} runs image {live}, which is not LEZ Program Registry code this module knows: nothing was prepared. \
             The host can pin a newer registry image with config_set({{\"registry_images\": [...]}}), and lezreg with `lezreg network set --registry-image`",
            reg.to_base58()
        )))
    }

    /// The most a change of `gas` gas and `len` message bytes can cost now (the fee reserve the
    /// signer holds: the next block's ceilings), or `None` if the sequencer won't say.
    fn max_fee(&self, gas: u64, len: usize) -> Option<u128> {
        let q = Seq::fresh(&self.sequencer).call("getFeeState", json!([])).ok()?;
        let exec = u128::from(q["next_base_fee_exec_ceiling"].as_u64()?);
        let stor = u128::from(q["next_base_fee_stor_ceiling"].as_u64()?);
        Some(u128::from(gas) * exec + (u128::try_from(len).ok()? + 2_000) * stor)
    }

    /// The chain's clock (ms), which the registry checks a change's `now` against.
    fn chain_now(&self) -> Result<u64, writes::Failure> {
        let (account, program) = writes::clock_actor();
        let state = Seq::fresh(&self.sequencer).get_actor_state(&account, &program).map_err(|e| writes::Failure::new("transport", e.to_string()))?;
        writes::clock_time(&state).ok_or_else(|| writes::Failure::new("no_clock", format!("the sequencer at {} has no clock account to time the change by", self.sequencer)))
    }

    /// A built change, with the caller's send options and a summary for a confirm sheet. Every
    /// word of the summary comes from the instruction and the chain, never from the caller.
    fn change(&self, built: &writes::Built, input: &Value, now: u64, current: Option<&lezreg_sdk::types::Entry>) -> Result<Value, writes::Failure> {
        let reg = self.write_registry()?;
        let payer = input.get("payer").filter(|p| !p.is_null()).map(|p| writes::account(p, "payer")).transpose()?;
        let allow_remote = input.get("allow_remote") == Some(&json!(true));
        let gas = writes::max_gas(&built.ix);
        let current = current.map(|e| json!({"name": clean(&e.name), "version": clean(&e.version), "author": e.author.to_base58(),
                                            "revision": e.revision, "kind": if e.kind == lezreg_sdk::types::KIND_DEPLOYER { "deployer" } else { "community" }}));
        let summary = json!({
            "what": built.what, "entry": built.entry.to_base58(), "current": current,
            "values": built.ix.values(),
            "signers": built.signers.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
            "payer": payer.map(|p| p.to_base58()),
            "registry": reg.to_base58(), "sequencer": self.sequencer,
            "spends_real_funds": allow_remote, "irreversible": built.what == "Freeze",
            "max_gas": gas, "max_fee": self.max_fee(gas, built.message().len()).map(|f| f.to_string()),
        });
        let mut c = built.change(&self.sequencer, &reg, now, summary);
        if let Some(p) = payer {
            c["payer"] = json!(p.to_base58());
        }
        c["allow_remote"] = json!(allow_remote);
        c["max_gas"] = json!(gas);
        remember(&mut c, &calling())?;
        Ok(c)
    }

    /// `prepare_register` and `prepare_register_third_party`.
    fn prepare_new(&self, input_json: &str, third_party: bool) -> Result<Value, writes::Failure> {
        let input: Value = serde_json::from_str(input_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
        let reg = self.write_registry()?;
        // A change is built from the chain as it is now: fresh reads, never the cache.
        let reader = Registry::new(Seq::fresh(&self.sequencer), reg);
        self.check_registry_code(&reader, &reg)?;
        let program = writes::account(&input["program"], "program")?;
        let live = reader.header(&program).map_err(|e| writes::Failure::new("transport", e.to_string()))?.map(|h| h.image_id);
        let now = self.chain_now()?;
        let built = if third_party { writes::register_third_party(&reg, &input, now, live)? } else { writes::register(&reg, &input, now, live)? };
        if reader.entry(&built.entry).map_err(|e| writes::Failure::new("transport", e.to_string()))?.is_some() {
            return Err(writes::Failure::new("exists", format!("{} already holds an entry: use prepare_update", built.entry.to_base58())));
        }
        self.change(&built, &input, now, None)
    }

    /// `prepare_update`, `prepare_transfer` and `prepare_freeze`: they read the entry first.
    fn prepare_change(&self, input_json: &str, build: fn(&AccountId, &Value, u64, &writes::Stored) -> writes::Res<writes::Built>) -> Result<Value, writes::Failure> {
        let input: Value = serde_json::from_str(input_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
        let reg = self.write_registry()?;
        let at = writes::target(&input)?;
        // A change is built from the chain as it is now: fresh reads, never the cache.
        let reader = Registry::new(Seq::fresh(&self.sequencer), reg);
        self.check_registry_code(&reader, &reg)?;
        let e = reader.entry(&at).map_err(|e| writes::Failure::new("transport", e.to_string()))?
            .ok_or_else(|| writes::Failure::new("no_entry", format!("there is no registry entry at {}", at.to_base58())))?;
        let stored = writes::Stored { revision: e.revision, author: e.author, registrant: e.registrant, frozen: e.is_frozen() };
        let now = self.chain_now()?;
        let built = build(&reg, &input, now, &stored)?;
        self.change(&built, &input, now, Some(&e))
    }

    /// Starts signing `change` with the lezreg CLI, for the app named `requested_by`.
    fn start_job(&self, change: &Value, requested_by: &str) -> Result<Value, writes::Failure> {
        let cli = bridge::find_cli(self.cli.as_deref())?;
        let job = bridge::start(&cli, change, now_ms(), requested_by, |job| {
            // Every subscriber sees this: only the job and how it ended. The rest (entry, who
            // asked, the CLI's output) is for submit_status, which shows each app only its own.
            let detail = json!({"job": job.id, "state": job.state}).to_string();
            // A change settled: what was read before it may be out of date.
            net::forget_all();
            emit_change_settled(&job.id, job.state, &detail);
        })?;
        Ok(job.json())
    }

    /// LNS reads for the configured sequencer and LNS program, kept across calls.
    fn names(&mut self) -> Result<&mut Names<Seq>, String> {
        let lns = self.lns.ok_or("no LNS program configured: call config_set with {\"lns\": \"<program id>\"}")?;
        let current = matches!(&self.names, Some((seq, l, _)) if *seq == self.sequencer && *l == lns);
        if !current {
            self.names = Some((self.sequencer.clone(), lns, Names::new(Seq::cached(&self.sequencer), lns)));
        }
        Ok(&mut self.names.as_mut().expect("just set").2)
    }

    /// A package checked against its publisher, kept for [`APP_CHECK_TTL_MS`].
    fn check(&mut self, contents: &Contents) -> Result<Value, String> {
        let now = now_ms();
        let key = (contents.manifest.name.clone(), contents.root);
        if let Some((verdict, at)) = self.checked.get(&key) {
            if now.saturating_sub(*at) < APP_CHECK_TTL_MS {
                return Ok(verdict.clone());
            }
        }
        let (verdict, via) = if !contents.root_matches() {
            (apps::Verdict::RootMismatch, None)
        } else if contents.manifest.publisher().is_none() {
            (apps::Verdict::Unclaimed, None)
        } else {
            self.names()?.verify(contents, now).map_err(|e| e.to_string())?
        };
        let mut shown = verdict.to_json(contents);
        // A publisher name that forwards was checked where it points; the prompt says so.
        shown["forwarded_from"] = json!(via);
        if let Some(programs) = shown["claimed"]["record"]["programs"].as_array().cloned() {
            let labels: Vec<Value> = programs.iter().filter_map(Value::as_str).map(|p| self.label(p).unwrap_or_else(|e| json!({"text": p, "error": e}))).collect();
            shown["claimed"]["record"]["program_labels"] = json!(labels);
        }
        if self.checked.len() > 256 {
            self.checked.clear();
        }
        self.checked.insert(key, (shown.clone(), now));
        Ok(shown)
    }

    /// Basecamp's data folder: the configured one, else where Basecamp keeps it.
    fn basecamp_data(&self) -> Option<PathBuf> {
        self.basecamp_data.clone().or_else(default_basecamp_data)
    }
}

/// `7Pq3…kZ9d`: an account id short enough for one line.
fn short_id(id: &str) -> String {
    if id.len() <= 12 {
        return id.to_owned();
    }
    format!("{}…{}", &id[..4], &id[id.len() - 4..])
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Where Basecamp keeps installed modules: Qt's app data folder for `Logos/LogosBasecamp`.
fn default_basecamp_data() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Logos/LogosBasecamp")
    } else {
        std::env::var_os("XDG_DATA_HOME").map_or_else(|| home.join(".local/share"), PathBuf::from).join("Logos/LogosBasecamp")
    })
}

/// The installed package of `module`: core modules under `modules/`, ui apps under `plugins/`.
/// The SDK recomputes the root from the files; the manifest's root stays only a claim.
fn installed_contents(data: &std::path::Path, module: &str) -> Result<Contents, String> {
    // A module name becomes a path: only the characters an app record's key allows, no `..`.
    if !apps::is_module_name(module) || module.contains("..") || module.starts_with('.') {
        return Err(format!("'{module}' is not a module name"));
    }
    for dir in ["modules", "plugins"] {
        let path = data.join(dir).join(module);
        if path.is_dir() {
            let contents = Contents::from_installed(&path).map_err(|e| e.to_string())?;
            if contents.manifest.name != module {
                return Err(format!("{} names another module ({})", path.display(), contents.manifest.name));
            }
            return Ok(contents);
        }
    }
    Err(format!("{module} is not installed in {}", data.display()))
}

/// A path to an unpacked directory or a `.lgx` package, for `verify_package`.
fn contents_from_path(text: &str) -> Result<Contents, String> {
    let path = std::path::PathBuf::from(text.trim());
    if path.is_dir() {
        Contents::from_installed(&path).map_err(|e| e.to_string())
    } else {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Contents::from_lgx(&bytes).map_err(|e| e.to_string())
    }
}

impl ProgramRegistryModule for ProgramRegistry {
    fn resolve_program(&mut self, program_id: String) -> String {
        reply((|| {
            let program = program_arg(&program_id)?;
            let reader = self.reader()?;
            let r = reader.resolve(&program).map_err(|e| e.to_string())?;
            let third = reader.third_party(&program, 50).map_err(|e| e.to_string())?.len();
            let mut v = self.labels().resolved(&r);
            v["third_party_count"] = json!(third);
            Ok(v)
        })())
    }

    fn program_label(&mut self, program_id: String) -> String {
        reply(self.label(&program_id))
    }

    fn get_idl(&mut self, program_id: String) -> String {
        reply((|| {
            let program = program_arg(&program_id)?;
            let r = self.reader()?.resolve(&program).map_err(|e| e.to_string())?;
            let e = r.deployer.ok_or("the program has no deployer entry")?;
            // An IDL just found unavailable answers from memory for a minute; each app may start
            // only a few Storage fetches a minute (one holds the module up to ~12 s, and the
            // module answers one call at a time, so a loop on get_idl would starve every label).
            let key = hex::encode(e.idl.sha256);
            let now = now_ms();
            if let Some((v, at)) = IDL_MISSES.lock().ok().and_then(|m| m.get(&key).cloned()) {
                if now.saturating_sub(at) < IDL_RETRY_MS {
                    return Ok(v);
                }
            }
            {
                let caller = calling();
                let allowance = if trusted(&caller) { IDL_FETCHES_PER_MINUTE_TRUSTED } else { IDL_FETCHES_PER_MINUTE };
                let mut f = IDL_FETCHES.lock().map_err(|_| "busy")?;
                let slot = f.entry(caller.identity()).or_insert((now, 0));
                if now.saturating_sub(slot.0) >= 60_000 {
                    *slot = (now, 0);
                }
                if slot.1 >= allowance {
                    return Ok(json!({"status": "unavailable", "reason": "rate limited: this app asked for too many IDLs in the last minute; try again shortly",
                                     "format": null, "format_version": null, "calls": [], "idl": null}));
                }
                slot.1 += 1;
            }
            // One bounded attempt (at most ~12 s): a host call gives up after about 20 s (logosctl)
            // and the module answers one call at a time, so it must not wait out the CLI's
            // full ladder. Content no node has comes back as `unavailable`, not as a timeout.
            let s = std::time::Duration::from_secs;
            let cfg = FetchConfig {
                storage_api: Some(self.storage_api.clone()),
                backoff: Vec::new(),
                timeouts: lezreg_sdk::storage::Timeouts { manifest: s(4), stream: s(6), dial: s(2) },
                ..FetchConfig::default()
            };
            let fetched = idl::fetch_idl(&e.idl, &cfg);
            let mut v = js::idl_status(&fetched);
            // An IDL in a format the SDK does not know is still the IDL: hand it over.
            v["idl"] = match &fetched {
                IdlStatus::Ok(i) | IdlStatus::UnknownFormat(i) => i.clone(),
                IdlStatus::Pending | IdlStatus::Unavailable(_) | IdlStatus::HashMismatch { .. } | IdlStatus::ParseError(_) => Value::Null,
            };
            if matches!(fetched, IdlStatus::Unavailable(_)) {
                if let Ok(mut m) = IDL_MISSES.lock() {
                    m.retain(|_, (_, at)| now.saturating_sub(*at) < IDL_RETRY_MS);
                    m.insert(key, (v.clone(), now));
                }
            }
            Ok(v)
        })())
    }

    fn list_entries(&mut self, after: String, limit: i64) -> String {
        reply((|| {
            let after = if after.trim().is_empty() { None } else { Some(program_arg(&after)?) };
            let page = self.reader()?.list(after.as_ref(), limit_arg(limit)).map_err(|e| e.to_string())?;
            let mut labels = self.labels();
            Ok(json!({"items": page.items.iter().map(|i| labels.listed(i)).collect::<Vec<_>>(), "next": page.next.map(|n| n.to_base58()), "total": page.total}))
        })())
    }

    fn search_name(&mut self, query: String, limit: i64) -> String {
        reply((|| {
            let found = self.reader()?.search_name(&query, limit_arg(limit), 1000).map_err(|e| e.to_string())?;
            let mut labels = self.labels();
            Ok(json!({"items": found.items.iter().map(|i| labels.listed(i)).collect::<Vec<_>>(), "scanned": found.scanned, "truncated": found.truncated}))
        })())
    }

    fn third_party_entries(&mut self, program_id: String) -> String {
        reply((|| {
            let program = program_arg(&program_id)?;
            let items = self.reader()?.third_party(&program, 50).map_err(|e| e.to_string())?;
            let mut labels = self.labels();
            Ok(json!(items.iter().filter(|i| matches!(i.kind, Some(Kind::ThirdParty { .. }))).map(|i| labels.listed(i)).collect::<Vec<_>>()))
        })())
    }

    fn verify_app(&mut self, module_name: String) -> String {
        reply((|| {
            let data = self.basecamp_data().ok_or("no Basecamp data folder: call config_set with {\"basecamp_data\": \"<path>\"}")?;
            let contents = installed_contents(&data, module_name.trim())?;
            self.check(&contents)
        })())
    }

    fn verify_package(&mut self, package_path: String) -> String {
        reply((|| {
            let contents = contents_from_path(&package_path)?;
            self.check(&contents)
        })())
    }

    fn publisher_apps(&mut self, name: String) -> String {
        reply((|| {
            let name = name.trim().strip_prefix("lns:").unwrap_or(name.trim()).to_owned();
            let profile = self.names()?.profile(&name, now_ms()).map_err(|e| e.to_string())?.ok_or_else(|| format!("{name} is not a registered LNS name"))?;
            let mut shown = Vec::new();
            for (module, record) in apps::apps_of(&profile) {
                match record {
                    Ok(r) => {
                        let labels: Vec<Value> = r.programs.iter().map(|p| self.label(&p.to_base58()).unwrap_or_else(|e| json!({"text": p.to_base58(), "error": e}))).collect();
                        shown.push(json!({"module": module, "version": r.newest().version, "roots": r.roots().iter().map(hex::encode).collect::<Vec<_>>(), "catalog": r.catalog, "programs": labels}));
                    }
                    Err(e) => shown.push(json!({"module": module, "error": e.to_string()})),
                }
            }
            Ok(json!({"name": name, "apps": shown}))
        })())
    }

    fn config_get(&mut self) -> String {
        reply(Ok(self.shown()))
    }

    fn config_set(&mut self, settings_json: String) -> String {
        reply_coded((|| {
            only_trusted()?;
            let v: Value = serde_json::from_str(&settings_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
            let caller = calling();
            if let Some(k) = HOST_ONLY_SETTINGS.iter().find(|k| v.get(**k).is_some()) {
                if !matches!(caller, logos_rust_sdk::LogosCaller::HostAnchor) {
                    return Err(writes::Failure::new("not_allowed", format!(
                        "{k} names a program to run, files to read or code to trust: only the host (or the settings file) may set it; refused for {}",
                        caller.describe_for_human()
                    )));
                }
            }
            let mut next = Self::with_settings(self.sequencer.clone(), self.registry, self.storage_api.clone(), self.lns, self.basecamp_data.clone());
            next.defaults = self.defaults.clone();
            next.network_error.clone_from(&self.network_error);
            next.cli = self.cli.clone();
            next.registry_images = self.registry_images.clone();
            next.apply(&v).map_err(writes::Failure::input)?;
            next.save()?;
            *self = next;
            net::forget_all();
            Ok(self.shown())
        })())
    }

    fn prepare_register(&mut self, input_json: String) -> String {
        reply_coded(self.prepare_new(&input_json, false))
    }

    fn prepare_register_third_party(&mut self, input_json: String) -> String {
        reply_coded(self.prepare_new(&input_json, true))
    }

    fn prepare_update(&mut self, input_json: String) -> String {
        reply_coded(self.prepare_change(&input_json, writes::update))
    }

    fn prepare_transfer(&mut self, input_json: String) -> String {
        reply_coded(self.prepare_change(&input_json, writes::transfer))
    }

    fn prepare_freeze(&mut self, input_json: String) -> String {
        reply_coded(self.prepare_change(&input_json, writes::freeze))
    }

    fn submit(&mut self, change_json: String) -> String {
        reply_coded((|| {
            let given: Value = serde_json::from_str(&change_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
            match given.get("signer").and_then(Value::as_str).unwrap_or("cli") {
                "cli" => {}
                "lez_core" => return Err(writes::Failure::new("lez_core_unavailable", bridge::LEZ_CORE_BLOCKED)),
                other => return Err(writes::Failure::input(format!("unknown signer '{other}': use \"cli\""))),
            }
            if given["format"] != writes::FORMAT {
                return Err(writes::Failure::input("not a change: pass what a prepare_* method returned"));
            }
            let caller = calling();
            // Only what this app prepared, unchanged, once; and the module's own copy of it.
            let change = take_prepared(&given, &caller)?;
            // A change is only sent to the network this module is pointed at.
            let reg = self.write_registry()?;
            if change["registry"].as_str() != Some(reg.to_base58().as_str()) || change["sequencer"].as_str() != Some(self.sequencer.as_str()) {
                return Err(writes::Failure::new("wrong_network", "this change was prepared for another registry or sequencer than the module is set to: prepare it again"));
            }
            let expires = change["expires_at"].as_u64().unwrap_or(0);
            let at = now_ms();
            if at >= expires {
                return Err(writes::Failure::new("expired", "this change was prepared more than 10 minutes ago and the registry would refuse it: prepare it again"));
            }
            if !trusted(&caller) {
                // Nothing is signed for an app the person has not trusted: it waits for them.
                let from = caller_name(&caller);
                let by = caller.identity();
                let id = format!("req-{}", fresh_id()?);
                let mut r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
                r.retain(|_, q| at < q.expires || (q.outcome.is_some() && at.saturating_sub(q.at) < 3_600_000));
                if r.values().filter(|q| q.by == by && q.outcome.is_none()).count() >= 5 {
                    return Err(writes::Failure::new("too_many_requests", "this app already has 5 changes waiting for approval"));
                }
                r.insert(id.clone(), Request { from: from.clone(), by, at, expires, change, outcome: None });
                return Ok(json!({"request": id, "state": "awaiting_approval", "from": from, "expires_at": expires,
                                 "note": "the person approves or refuses it in the Program Registry app before it expires"}));
            }
            self.start_job(&change, &caller.identity())
        })())
    }

    fn submit_status(&mut self, job: String) -> String {
        let caller = calling();
        let me = caller.identity();
        let all = trusted(&caller);
        let job = job.trim();
        let now = now_ms();
        reply_coded(if job.is_empty() {
            Ok(json!(bridge::jobs().into_iter().filter(|j| all || j["requested_by"] == me.as_str()).collect::<Vec<_>>()))
        } else if let Some(state) = REQUESTS.lock().ok().and_then(|r| r.get(job).filter(|q| all || q.by == me).map(|q| match &q.outcome {
            None if now >= q.expires => json!({"request": job, "state": "expired"}),
            None => json!({"request": job, "state": "awaiting_approval", "expires_at": q.expires}),
            Some(Err(())) => json!({"request": job, "state": "denied"}),
            Some(Ok(j)) => bridge::status(j).map_or_else(|| json!({"request": job, "job": j}), |s| s.json()),
        })) {
            Ok(state)
        } else {
            bridge::status(job).filter(|j| all || j.requested_by == me).map(|j| j.json())
                .ok_or_else(|| writes::Failure::new("no_job", format!("no change '{job}' was submitted by this app since the module started")))
        })
    }

    fn pending_requests(&mut self) -> String {
        reply_coded((|| {
            only_trusted()?;
            let now = now_ms();
            let r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
            Ok(json!(r.iter().filter(|(_, q)| q.outcome.is_none() && now < q.expires).map(|(id, q)| json!({
                "request": id, "from": q.from, "from_note": "the name this app gave itself: not verified",
                "at": q.at, "expires_at": q.expires, "change": q.change,
            })).collect::<Vec<_>>()))
        })())
    }

    fn approve_request(&mut self, request: String) -> String {
        reply_coded((|| {
            only_trusted()?;
            let (change, by) = {
                let r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
                let q = r.get(request.trim()).filter(|q| q.outcome.is_none()).ok_or_else(|| writes::Failure::new("no_request", format!("no request '{request}' is waiting")))?;
                if now_ms() >= q.expires {
                    return Err(writes::Failure::new("expired", "this request's change has expired: the app must prepare it again"));
                }
                (q.change.clone(), q.by.clone())
            };
            let job = self.start_job(&change, &by)?;
            if let Ok(mut r) = REQUESTS.lock() {
                if let Some(q) = r.get_mut(request.trim()) {
                    q.outcome = Some(Ok(job["job"].as_str().unwrap_or_default().to_owned()));
                }
            }
            Ok(job)
        })())
    }

    fn deny_request(&mut self, request: String) -> String {
        reply_coded((|| {
            only_trusted()?;
            let mut r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
            let q = r.get_mut(request.trim()).filter(|q| q.outcome.is_none()).ok_or_else(|| writes::Failure::new("no_request", format!("no request '{request}' is waiting")))?;
            q.outcome = Some(Err(()));
            Ok(json!({"request": request.trim(), "state": "denied"}))
        })())
    }

    fn forget_reads(&mut self) -> String {
        net::forget_all();
        reply(Ok(json!({"forgotten": true})))
    }

    fn signer_status(&mut self) -> String {
        if let Err(f) = only_trusted() {
            return reply_coded(Err(f));
        }
        let cli = bridge::find_cli(self.cli.as_deref());
        let version = cli.as_ref().ok().map(|c| bridge::cli_version(c));
        reply(Ok(json!({
            "cli": cli.as_ref().ok().map(|c| c.display().to_string()),
            "cli_version": version.as_ref().and_then(|v| v.as_ref().ok()).cloned(),
            "cli_error": cli.as_ref().err().map(|f| f.message.clone()).or_else(|| version.and_then(Result::err).map(|f| f.message)),
            "lez_core": bridge::LEZ_CORE_BLOCKED,
        })))
    }
}

// The builder injects the generated scaffold here (the `install` entry point). Unit tests build
// without it.
#[cfg(not(test))]
include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

#[cfg(not(test))]
#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<ProgramRegistry>();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    fn fresh() -> ProgramRegistry {
        ProgramRegistry::with_settings(DEFAULT_SEQUENCER.into(), None, lezreg_sdk::storage::DEFAULT_API.into(), None, None)
    }

    #[test]
    fn without_a_registry_reads_say_what_to_do() {
        let v = parsed(&fresh().program_label("6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN".into()));
        assert_eq!(v["success"], false);
        assert!(v["error"].as_str().unwrap().contains("config_set"));
    }

    #[test]
    fn bad_ids_and_remote_plain_http_are_refused() {
        assert!(parsed(&fresh().resolve_program("nope".into()))["error"].as_str().unwrap().contains("not a program id"));
        let mut m = fresh();
        assert!(m.apply(&json!({"sequencer": "http://evil.example:3062"})).is_err());
        assert!(m.apply(&json!({"sequencer": "http://127.0.0.1:1@evil.example/"})).is_err());
        assert!(m.apply(&json!({"storage_api": "file:///etc"})).is_err());
        assert!(m.apply(&json!({"sequencer": "https://testnet.lez.logos.co", "registry": "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN"})).is_ok());
        assert_eq!(m.shown()["registry"], "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN");
    }

    #[test]
    fn writes_need_a_registry_and_go_only_to_the_configured_network() {
        let mut m = fresh();
        let v = parsed(&m.prepare_freeze(json!({"program": "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN"}).to_string()));
        assert_eq!((v["success"].as_bool(), v["code"].as_str()), (Some(false), Some("no_registry")));
        assert_eq!(parsed(&m.prepare_update("nope".into()))["code"], "bad_input");
        m.registry = AccountId::from_base58("6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN");
        let raw = json!({"format": writes::FORMAT, "registry": "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN", "sequencer": "http://127.0.0.1:9", "expires_at": u64::MAX, "message": "0a0b"});
        assert_eq!(parsed(&m.submit(raw.to_string()))["code"], "unknown_change", "only what this module prepared");
        let mut change = raw.clone();
        remember(&mut change, &calling()).unwrap();
        let mut lez = change.clone();
        lez["signer"] = json!("lez_core");
        assert_eq!(parsed(&m.submit(lez.to_string()))["code"], "lez_core_unavailable");
        assert_eq!(parsed(&m.submit(change.to_string()))["code"], "wrong_network");
        assert_eq!(parsed(&m.submit_status("nope".into()))["code"], "no_job");
        assert!(m.apply(&json!({"cli": "lezreg"})).is_err(), "the signer's path must be absolute");
        assert!(m.apply(&json!({"cli": "/opt/lezreg/bin/lezreg"})).is_ok());
        assert_eq!(m.shown()["cli"], "/opt/lezreg/bin/lezreg");
        assert_eq!(short_id("6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN"), "6YWH…VayN");
    }

    fn as_caller(caller: logos_rust_sdk::LogosCaller) {
        TEST_CALLER.with(|c| *c.borrow_mut() = caller);
    }

    fn app(name: &str) -> logos_rust_sdk::LogosCaller {
        logos_rust_sdk::LogosCaller::Module { name: name.into(), instance: None }
    }

    const REG: &str = "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN";

    /// A change as `prepare_*` returns it, kept for `caller`.
    fn prepared_by(caller: logos_rust_sdk::LogosCaller, message: &str, payer: &str, allow_remote: bool) -> Value {
        let mut c = json!({"format": writes::FORMAT, "what": "Freeze", "registry": REG, "sequencer": DEFAULT_SEQUENCER, "expires_at": u64::MAX,
                           "entry": REG, "message": message, "payer": payer, "allow_remote": allow_remote,
                           "summary": {"what": "Freeze", "irreversible": true, "payer": payer, "spends_real_funds": allow_remote}});
        remember(&mut c, &caller).unwrap();
        c
    }

    /// A stand-in lezreg that records its arguments and answers like `lezreg --json module send`.
    fn fake_cli(tag: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("lezreg-mod-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("lezreg");
        std::fs::write(&cli, "#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/args.txt\"\ncat > /dev/null\necho '{\"tx\":\"abc\",\"block\":9}'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        cli
    }

    fn wait(m: &mut ProgramRegistry, id: &str) -> Value {
        for _ in 0..100 {
            let s = parsed(&m.submit_status(id.into()))["value"].clone();
            if s["state"] != "running" {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("{id} never settled");
    }

    /// M6 review A: another app prepares the same message with its own payer and allow_remote;
    /// the trusted app's submit must still sign the trusted app's own change, once.
    #[test]
    fn a_prepared_change_is_the_preparers_alone_unchanged_and_used_once() {
        let mut m = fresh();
        m.registry = AccountId::from_base58(REG);
        let cli = fake_cli("a");
        m.cli = Some(cli.clone());
        let mine = prepared_by(app("registry_browser"), "04cc", "PayerMine", false);
        // Same message, the attacker's payer and allow_remote: before the fix this overwrote mine.
        let theirs = prepared_by(app("evil_ui"), "04cc", "PayerTheirs", true);
        assert_ne!(mine["change_id"], theirs["change_id"]);
        // The attacker can't submit the browser's change (by id or by copying it) …
        as_caller(app("evil_ui"));
        assert_eq!(parsed(&m.submit(mine.to_string()))["code"], "unknown_change");
        as_caller(app("registry_browser"));
        // … nor can the browser's change be altered outside the message …
        let mut altered = mine.clone();
        altered["payer"] = json!("PayerTheirs");
        altered["allow_remote"] = json!(true);
        assert_eq!(parsed(&m.submit(altered.to_string()))["code"], "changed");
        // (an altered submit uses the change up: it must be prepared again)
        let mine = prepared_by(app("registry_browser"), "04cc", "PayerMine", false);
        let v = parsed(&m.submit(mine.to_string()));
        let job = v["value"]["job"].as_str().unwrap_or_else(|| panic!("{v}")).to_owned();
        assert_eq!(wait(&mut m, &job)["state"], "applied");
        let args = std::fs::read_to_string(cli.parent().unwrap().join("args.txt")).unwrap();
        assert!(args.contains("--payer PayerMine") && !args.contains("--yes") && !args.contains("PayerTheirs"), "signed the browser's own change: {args}");
        // … and it is sent once.
        assert_eq!(parsed(&m.submit(mine.to_string()))["code"], "unknown_change", "a second submit is refused");
        let _ = std::fs::remove_dir_all(cli.parent().unwrap());
    }

    /// One app preparing in a loop can't lock another out (M6 delta review).
    #[test]
    fn prepared_changes_are_capped_per_app() {
        for i in 0..PREPARED_PER_CALLER {
            let mut c = json!({"format": writes::FORMAT, "expires_at": u64::MAX, "message": format!("ff{i:04x}")});
            remember(&mut c, &app("flood_ui")).unwrap();
        }
        let mut c = json!({"format": writes::FORMAT, "expires_at": u64::MAX, "message": "ffff"});
        assert_eq!(remember(&mut c, &app("flood_ui")).unwrap_err().code, "too_many_changes");
        assert!(remember(&mut c, &app("registry_browser")).is_ok(), "the browser can still prepare");
    }

    /// Settings and signing are for the registry browser and the host; any other app may read
    /// and prepare, and its changes wait for the person's approval.
    #[test]
    fn only_the_browser_and_the_host_change_settings_or_sign() {
        use logos_rust_sdk::LogosCaller;
        assert!(trusted(&LogosCaller::HostAnchor));
        assert!(trusted(&app("registry_browser")));
        assert!(!trusted(&app("names_ui")), "the Names Wallet only reads");
        assert!(!trusted(&app("evil_ui")));
        assert!(!trusted(&LogosCaller::Unknown));
        assert!(!trusted(&LogosCaller::Derived { parent: "registry_browser".into(), leaf: "x".into() }));

        let mut m = fresh();
        m.registry = AccountId::from_base58(REG);
        as_caller(app("evil_ui"));
        let v = parsed(&m.config_set(json!({"lns": REG}).to_string()));
        assert_eq!((v["success"].as_bool(), v["code"].as_str()), (Some(false), Some("not_allowed")), "{v}");
        assert!(m.lns.is_none(), "nothing changed");
        assert_eq!(parsed(&m.signer_status())["code"], "not_allowed");
        assert_eq!(parsed(&m.pending_requests())["code"], "not_allowed");
        assert_eq!(parsed(&m.config_get())["success"], true, "reads stay open");
        // Even the browser can't point the module at a program to run or files to read (D).
        as_caller(app("registry_browser"));
        for k in ["sequencer", "registry", "network_file", "cli", "basecamp_data", "registry_images"] {
            let v = parsed(&m.config_set(json!({k: "/tmp/x"}).to_string()));
            assert_eq!(v["code"], "not_allowed", "{k}: {v}");
        }
        assert_eq!(parsed(&m.config_set(json!({"storage_api": "http://127.0.0.1:8095/api/storage/v1"}).to_string()))["success"], true);
        // An untrusted app's change waits; the card shows the module's copy, not the app's labels.
        as_caller(app("evil_ui"));
        let mut relabelled = prepared_by(app("evil_ui"), "04aa", "P", false);
        relabelled["what"] = json!("Update");
        assert_eq!(parsed(&m.submit(relabelled.to_string()))["code"], "changed", "relabelled: refused");
        let change = prepared_by(app("evil_ui"), "04aa", "P", false);
        let v = parsed(&m.submit(change.to_string()));
        assert_eq!(v["value"]["state"], "awaiting_approval", "{v}");
        let request = v["value"]["request"].as_str().unwrap().to_owned();
        assert_eq!(parsed(&m.approve_request(request.clone()))["code"], "not_allowed", "it can't approve itself");
        assert_eq!(parsed(&m.submit_status(request.clone()))["value"]["state"], "awaiting_approval");
        as_caller(app("other_ui"));
        assert_eq!(parsed(&m.submit_status(request.clone()))["code"], "no_job", "another app can't see it");
        as_caller(app("registry_browser"));
        let pending = parsed(&m.pending_requests());
        assert!(pending["value"].as_array().unwrap().iter().any(|q| q["request"] == request.as_str() && q["from"] == "evil_ui"
            && q["from_note"].as_str().unwrap().contains("not verified") && q["change"]["what"] == "Freeze"), "{pending}");
        assert_eq!(parsed(&m.deny_request(request.clone()))["value"]["state"], "denied");
        assert_eq!(parsed(&m.deny_request(request.clone()))["code"], "no_request");
        as_caller(app("evil_ui"));
        assert_eq!(parsed(&m.submit_status(request))["value"]["state"], "denied");
        as_caller(LogosCaller::HostAnchor);
    }

    /// A request waits only as long as its change can still be signed.
    #[test]
    fn requests_expire_with_their_change() {
        let mut m = fresh();
        m.registry = AccountId::from_base58(REG);
        let mut c = json!({"format": writes::FORMAT, "what": "Freeze", "registry": REG, "sequencer": DEFAULT_SEQUENCER,
                           "expires_at": now_ms() + 300, "entry": REG, "message": "04dd"});
        remember(&mut c, &app("dapp_ui")).unwrap();
        as_caller(app("dapp_ui"));
        let request = parsed(&m.submit(c.to_string()))["value"]["request"].as_str().unwrap().to_owned();
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert_eq!(parsed(&m.submit_status(request.clone()))["value"]["state"], "expired");
        as_caller(app("registry_browser"));
        assert!(parsed(&m.pending_requests())["value"].as_array().unwrap().is_empty());
        assert_eq!(parsed(&m.approve_request(request))["code"], "expired");
        as_caller(logos_rust_sdk::LogosCaller::HostAnchor);
    }

    /// An approved request is signed through the CLI like a trusted submit.
    #[test]
    fn an_approved_request_is_sent_and_settles() {
        let cli = fake_cli("approve");
        let mut m = fresh();
        m.registry = AccountId::from_base58(REG);
        m.cli = Some(cli.clone());
        let change = prepared_by(app("some_dapp_ui"), "04bb", "P", false);
        as_caller(app("some_dapp_ui"));
        let request = parsed(&m.submit(change.to_string()))["value"]["request"].as_str().unwrap().to_owned();
        as_caller(app("registry_browser"));
        let v = parsed(&m.approve_request(request.clone()));
        assert_eq!((v["value"]["state"].as_str(), v["value"]["requested_by"].as_str()), (Some("running"), Some("module:some_dapp_ui")), "{v}");
        assert_eq!(parsed(&m.approve_request(request.clone()))["code"], "no_request", "approved once");
        as_caller(app("some_dapp_ui"));
        let state = wait(&mut m, &request);
        assert_eq!((state["state"].as_str(), state["receipt"]["block"].as_u64()), (Some("applied"), Some(9)), "{state}");
        as_caller(logos_rust_sdk::LogosCaller::HostAnchor);
        let _ = std::fs::remove_dir_all(cli.parent().unwrap());
    }

    #[test]
    fn defaults_come_from_the_built_in_rc3_profile_and_are_never_saved() {
        let m = ProgramRegistry::from_saved(None);
        let shown = m.shown();
        assert_eq!(shown["sequencer"], "https://lez.nameswallet.app/");
        assert_eq!(shown["registry"], "7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82");
        assert_eq!(shown["lns"], "6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA");
        assert_eq!(shown["lns_is_default"], true);
        assert_eq!(shown["network"]["source"], "built-in");
        assert_eq!(shown["network"]["chain_id"], "2da7f7dfb6e51a28c942ea80391aa518253ec515e98d28cb4d83cc9e16ff5374");
        let saved = m.settings();
        for k in ["sequencer", "registry", "lns", "storage_api", "network_file"] {
            assert!(saved.get(k).is_none(), "a default is not pinned into the settings file: {k} in {saved}");
        }
        // A value set by hand is saved, and an empty one goes back to the profile's.
        let mut m = ProgramRegistry::from_saved(None);
        assert!(m.apply(&json!({"sequencer": "http://127.0.0.1:3290", "lns": REG})).is_ok());
        assert_eq!((m.settings()["sequencer"].as_str(), m.settings()["lns"].as_str()), (Some("http://127.0.0.1:3290"), Some(REG)));
        assert!(m.apply(&json!({"lns": ""})).is_ok());
        assert_eq!(m.shown()["lns"], "6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA");
        // An old 0.3 URL no longer selects the 0.3 LNS registry by itself.
        assert!(m.apply(&json!({"sequencer": "https://testnet.lez.logos.co"})).is_ok());
        assert_ne!(m.shown()["lns"], "38jEfVV1RCyMzu1FGbYcCn5FqxCxKyY58VgzjruAh2gq");
    }

    #[test]
    fn an_old_builds_pinned_local_default_does_not_hide_the_profile() {
        // What every build before 0.3.1 wrote after any config_set, with nothing configured.
        let old = json!({"sequencer": DEFAULT_SEQUENCER, "registry": null, "storage_api": lezreg_sdk::storage::DEFAULT_API, "lns": null,
                         "basecamp_data": null, "cli": null, "registry_images": []});
        let m = ProgramRegistry::from_saved(Some(old));
        assert_eq!((m.shown()["sequencer"].as_str(), m.shown()["registry"].as_str()), (Some("https://lez.nameswallet.app/"), Some("7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82")));
        // A local chain someone really configured stays.
        let m = ProgramRegistry::from_saved(Some(json!({"sequencer": DEFAULT_SEQUENCER, "registry": REG})));
        assert_eq!((m.shown()["sequencer"].as_str(), m.shown()["registry"].as_str()), (Some(DEFAULT_SEQUENCER), Some(REG)));
    }

    #[test]
    fn a_network_file_moves_the_defaults_and_keeps_what_was_set_by_hand() {
        let dir = std::env::temp_dir().join(format!("lezreg-module-netfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut file: Value = serde_json::from_str(network_file::BUILTIN_RAW).unwrap();
        file["kind"] = json!("local");
        file["rpc"] = json!("http://127.0.0.1:3290/");
        file["programs"]["program_registry"]["id"] = json!(REG);
        let path = dir.join("local.json");
        std::fs::write(&path, file.to_string()).unwrap();
        let mut m = ProgramRegistry::from_saved(None);
        assert!(m.apply(&json!({"storage_api": "http://127.0.0.1:8095/api/storage/v1"})).is_ok());
        assert!(m.apply(&json!({"network_file": path.display().to_string()})).is_ok());
        let shown = m.shown();
        assert_eq!((shown["sequencer"].as_str(), shown["registry"].as_str()), (Some("http://127.0.0.1:3290/"), Some(REG)));
        assert_eq!(shown["storage_api"], "http://127.0.0.1:8095/api/storage/v1", "set by hand: kept");
        assert_eq!(shown["network"]["source"], "file");
        assert_eq!(m.settings()["network_file"], path.display().to_string());
        assert!(m.settings().get("sequencer").is_none(), "the file's rpc is its default, not a pin");
        // Read back from the settings file the same way.
        let again = ProgramRegistry::from_saved(Some(m.settings()));
        assert_eq!(again.shown()["registry"], REG);
        // Back to the built-in profile.
        assert!(m.apply(&json!({"network_file": ""})).is_ok());
        assert_eq!(m.shown()["registry"], "7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82");
        // Refused: relative paths, missing files, a remote plain-http rpc; a broken saved file
        // leaves the built-in profile, said in config_get.
        assert!(m.apply(&json!({"network_file": "local.json"})).is_err());
        assert!(m.apply(&json!({"network_file": dir.join("missing.json").display().to_string()})).is_err());
        file["rpc"] = json!("http://example.org/");
        std::fs::write(&path, file.to_string()).unwrap();
        assert!(m.apply(&json!({"network_file": path.display().to_string()})).is_err());
        let broken = ProgramRegistry::from_saved(Some(json!({"network_file": path.display().to_string(), "lns": REG})));
        assert_eq!(broken.shown()["network"]["source"], "built-in");
        assert!(broken.shown()["network"]["error"].as_str().is_some());
        assert_eq!(broken.shown()["lns"], REG, "the other settings still apply");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live read of our rc3 chain's LNS (the built-in network file): jef05 carries an NFT deed,
    /// a version-7 record, which SDKs before cec2133 refused to decode. Needs the network, so it
    /// is not in the default run: `cargo test -- --ignored live_`.
    /// A rate-limited sequencer (Cloudflare's 429 page) reaches an app as plain words and a code,
    /// never the page, and the module answers at once inside the limit's window.
    #[test]
    fn a_rate_limited_sequencer_is_plain_words() {
        let (url, hits) = net::tests::serve(429, "Retry-After: 5\r\n", net::tests::CLOUDFLARE_429);
        let reg = AccountId::from_base58(REG).expect("id");
        let mut m = ProgramRegistry::with_settings(url, Some(reg), "http://127.0.0.1:9".into(), None, None);
        let raw = m.list_entries(String::new(), 25);
        let v = parsed(&raw);
        assert_eq!((v["success"].clone(), v["code"].clone(), v["error"].clone()), (json!(false), json!("rate_limited"), json!("The network is busy. Trying again in a few seconds.")), "{v}");
        assert_eq!(v["retry_after_ms"], 5000);
        assert!(v["detail"].as_str().is_some_and(|d| d.starts_with("HTTP 429: Access denied")), "{v}");
        for bad in ["<", "doctype", "script"] {
            assert!(!raw.to_lowercase().contains(bad), "{bad} in {raw}");
        }
        // Inside the window: answered at once, nothing sent.
        let started = std::time::Instant::now();
        let again = parsed(&m.resolve_program(REG.into()));
        assert_eq!(again["code"], "rate_limited", "{again}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A sequencer that answers 502 with a page: "Can't reach the network right now."
    #[test]
    fn an_unreachable_sequencer_is_plain_words() {
        let (url, _) = net::tests::serve(502, "", "<html><body>Bad gateway</body></html>");
        let mut m = ProgramRegistry::with_settings(url, Some(AccountId::from_base58(REG).expect("id")), "http://127.0.0.1:9".into(), None, None);
        let v = parsed(&m.program_label(REG.into()));
        assert_eq!((v["code"].as_str(), v["error"].as_str(), v["detail"].as_str()), (Some("unreachable"), Some("Can't reach the network right now."), Some("HTTP 502: Bad gateway")), "{v}");
    }

    /// forget_reads answers for anyone.
    #[test]
    fn forget_reads_answers() {
        assert_eq!(parsed(&ProgramRegistry::default().forget_reads())["value"]["forgotten"], true);
    }

    /// What a cold open of the registry browser asks the module, in order: settings, the first
    /// page, then the newest entry opened (its four reads and the label RegistryLabel asks
    /// again), then a second entry opened. Run against a counting forwarder to count the
    /// sequencer requests: `LEZREG_COLD_RPC=<url> LEZREG_COLD_REG=<id> cargo test -- --ignored cold_open --nocapture`.
    #[test]
    #[ignore = "needs a sequencer and a registry"]
    fn cold_open_reads() {
        let rpc = std::env::var("LEZREG_COLD_RPC").expect("LEZREG_COLD_RPC");
        let reg = AccountId::from_base58(&std::env::var("LEZREG_COLD_REG").expect("LEZREG_COLD_REG")).expect("registry id");
        let mut m = ProgramRegistry::with_settings(rpc, Some(reg), "http://127.0.0.1:9".into(), None, None);
        let _ = m.config_get();
        let list = parsed(&m.list_entries(String::new(), 25));
        assert_eq!(list["success"], true, "{list}");
        let items = list["value"]["items"].as_array().expect("items").clone();
        for (n, item) in items.iter().take(2).enumerate() {
            let program = item["entry"]["program"].as_str().or_else(|| item["link"]["program"].as_str()).expect("program").to_owned();
            for (what, r) in [("resolve_program", m.resolve_program(program.clone())), ("program_label", m.program_label(program.clone())),
                              ("third_party_entries", m.third_party_entries(program.clone())), ("get_idl", m.get_idl(program.clone())),
                              ("program_label", m.program_label(program.clone()))] {
                assert!(parsed(&r).get("success").is_some(), "{what}: {r}");
            }
            eprintln!("COLD-OPEN step {} done (entry {n} opened)", n + 1);
        }
    }

    #[test]
    #[ignore = "reads the live rc3 chain"]
    fn live_a_deeded_lns_name_reads() {
        let mut m = ProgramRegistry::default();
        let v = parsed(&m.publisher_apps("jef05".into()));
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(v["value"]["name"], "jef05", "{v}");
    }

    /// Chain text that reorders or hides what a reader sees is cleaned out of labels (C).
    #[test]
    fn labels_drop_bidi_and_zero_width_characters() {
        assert_eq!(clean("1.0\u{202E}nevorpnu"), "1.0nevorpnu");
        assert_eq!(clean("a\u{200B}b\u{2066}c\u{2069}\u{FEFF}d\n"), "abcd⏎");
        // The reviewer's payload: separators that would add fake lines to a card stay on one line.
        assert_eq!(clean("ok\u{2028}Fee paid by the first signer\u{2029}Sent to x\r\n(test)"), "ok⏎Fee paid by the first signer⏎Sent to x⏎(test)");
        assert_eq!(clean("a\u{00AD}b\u{180E}c\u{2061}d\u{206B}e\u{FFFA}f\u{E0041}g"), "abcdefg");
        assert_eq!(clean("token 1<b>&#10003;</b>"), "token 1<b>&#10003;</b>", "markup is the view's job: it shows plain text");
    }

    #[test]
    fn limits_are_clamped() {
        assert_eq!(limit_arg(-5), 1);
        assert_eq!(limit_arg(10_000), 200);
    }

    /// A Basecamp data folder with one core module and one ui app installed,
    /// each with files whose hashes make the declared root true. The SDK
    /// recomputes the root; a bare manifest with a made-up root is refused.
    fn basecamp(label: &str) -> PathBuf {
        use std::collections::BTreeMap;
        let dir = std::env::temp_dir().join(format!("lezreg-module-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (kind, name, author) in [("modules", "names", "lns:jef"), ("plugins", "forumer_ui", "")] {
            let package = dir.join(kind).join(name);
            std::fs::create_dir_all(&package).unwrap();
            let payload = b"the publisher's build";
            let file_hash = hex::encode(lezreg_sdk::types::pda::sha256(payload));
            let files = BTreeMap::from([(
                format!("variants/darwin-arm64/{name}.bin"),
                file_hash,
            )]);
            let hashes = lezreg_sdk::lgx::merkle_tree(&files);
            let manifest = json!({"name": name, "version": "1.0.0", "author": author, "hashes": hashes});
            std::fs::write(package.join("manifest.json"), manifest.to_string()).unwrap();
            std::fs::write(package.join("variant"), "darwin-arm64").unwrap();
            std::fs::write(package.join(format!("{name}.bin")), payload).unwrap();
        }
        dir
    }

    #[test]
    fn installed_apps_are_read_from_basecamp_and_checked() {
        let data = basecamp("apps");
        let mut m = fresh();
        assert_eq!(parsed(&m.config_set(json!({"basecamp_data": data.display().to_string()}).to_string()))["success"], true);
        // A ui app that names no publisher is unclaimed, without any chain read.
        let v = parsed(&m.verify_app("forumer_ui".into()));
        assert_eq!(v["value"]["status"], "unclaimed", "{v}");
        assert_eq!(v["value"]["module"], "forumer_ui");
        // One that does needs the LNS program to check it.
        let v = parsed(&m.verify_app("names".into()));
        assert!(v["error"].as_str().unwrap().contains("no LNS program"), "{v}");
        // Module names can't reach outside the data folder.
        for bad in ["../names", "..", ".hidden", "a/b", "Names", ""] {
            assert_eq!(parsed(&m.verify_app(bad.into()))["success"], false, "{bad}");
        }
        assert!(parsed(&m.verify_app("missing".into()))["error"].as_str().unwrap().contains("not installed"));
        // A package not yet installed: give the directory, not a bare manifest.
        let path = data.join("plugins").join("forumer_ui").display().to_string();
        let v = parsed(&m.verify_package(path));
        assert_eq!(v["value"]["status"], "unclaimed");
        assert_eq!(parsed(&m.verify_package("no-such-package".into()))["success"], false);
        // A swapped file moves the recomputed root and is refused up front.
        let plugin = data.join("plugins").join("forumer_ui").join("forumer_ui.bin");
        std::fs::write(&plugin, b"MALICIOUS").unwrap();
        let v = parsed(&m.verify_app("forumer_ui".into()));
        assert_eq!(v["value"]["status"], "root_mismatch", "{v}");
        std::fs::write(&plugin, b"the publisher's build").unwrap();
        assert!(parsed(&m.publisher_apps("jef".into()))["error"].as_str().unwrap().contains("no LNS program"));
        let shown = parsed(&m.config_get());
        assert_eq!(shown["value"]["basecamp_data"], data.display().to_string());
        let _ = std::fs::remove_dir_all(data);
    }
}
