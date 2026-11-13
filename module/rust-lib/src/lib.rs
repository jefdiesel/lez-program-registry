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
pub mod writes;

use lezreg_sdk::{
    HttpRpc, Kind, Registry, Rpc as _,
    apps::{self, Manifest, Names},
    idl::{self, FetchConfig, IdlStatus},
    json as js,
    types::AccountId,
};
use serde_json::{Value, json};

/// Until LEZ testnet 0.4 launches the only registry is a local one, so there is no default id:
/// `config_set` names it.
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
    /// `status` is `verified` (the name vouches for this exact package), `mismatch` (the name
    /// publishes this module but not this package: a copy, or a release not yet vouched for),
    /// `not_published`, `no_such_name`, or `unclaimed` (the package names no LNS publisher).
    /// `record.programs` are the programs the app says it calls. Reads the installed
    /// `manifest.json` from Basecamp's data folder and one LNS record from the sequencer.
    fn verify_app(&mut self, module_name: String) -> String;

    /// The same check for a package not yet installed, given its `manifest.json` as JSON text
    /// (the `manifest` of a catalog's index.json entry will do): for an install view.
    fn verify_package(&mut self, manifest_json: String) -> String;

    /// The apps an LNS name publishes: `{name, apps: [{module, version, roots, catalog,
    /// programs: [label]}]}`, each program with its `program_label`. Only someone who knows a name
    /// can read its apps: nothing lists them.
    fn publisher_apps(&mut self, name: String) -> String;

    /// `{sequencer, registry, storage_api, lns, basecamp_data, cli}`.
    fn config_get(&mut self) -> String;

    /// Changes any of `sequencer`, `registry`, `storage_api`, `lns` (the LNS program id, for
    /// the app checks; empty for the network's default), `basecamp_data` (Basecamp's data
    /// folder, if not the default) and `cli` (the `lezreg` that signs changes, if not found by
    /// itself) from a JSON object, and remembers them. The sequencer must be `https://`, or
    /// `http://` to this machine. **Only the registry browser, the Names Wallet or the host may
    /// call it** (`code: "not_allowed"` otherwise): the settings decide what every app is told.
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
    /// event fires when it ends. From any app other than the registry browser, the Names Wallet
    /// or the host, nothing is signed yet: the change waits as a request
    /// (`{request, state: "awaiting_approval"}`) until the person approves it in the registry
    /// browser (`approve_request`), and its job id then shows in `submit_status(request)`. Signed by the person's `lezreg` CLI wallet (`signer` absent or
    /// `"cli"`); `"signer": "lez_core"` answers `code: "lez_core_unavailable"` until a usable
    /// release of that module exists. A change prepared more than 10 minutes ago is refused
    /// (`code: "expired"`): the registry's time window has passed.
    fn submit(&mut self, change_json: String) -> String;

    /// A submitted change: `{job, what, entry, state (running|applied|failed), receipt,
    /// error, code, log_tail, requested_by}`, or a request still `awaiting_approval` /
    /// `denied`. An empty `job` lists the recent ones. An app sees only what it submitted.
    fn submit_status(&mut self, job: String) -> String;

    /// Who would sign: `{cli, cli_version, cli_error, lez_core}`. Trusted callers only.
    fn signer_status(&mut self) -> String;

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

pub struct ProgramRegistry {
    sequencer: String,
    registry: Option<AccountId>,
    storage_api: String,
    lns: Option<AccountId>,
    basecamp_data: Option<PathBuf>,
    /// The `lezreg` that signs changes, when set (else found on PATH and the usual places).
    cli: Option<PathBuf>,
    /// LNS reads, kept with their derived names, for the sequencer and LNS program they read.
    names: Option<(String, AccountId, Names<HttpRpc>)>,
    /// `(module, root)` → (verdict, when), for [`APP_CHECK_TTL_MS`].
    checked: std::collections::HashMap<(String, [u8; 32]), (Value, u64)>,
}

impl ProgramRegistry {
    fn with_settings(sequencer: String, registry: Option<AccountId>, storage_api: String, lns: Option<AccountId>, basecamp_data: Option<PathBuf>) -> Self {
        Self { sequencer, registry, storage_api, lns, basecamp_data, cli: None, names: None, checked: std::collections::HashMap::new() }
    }
}

impl Default for ProgramRegistry {
    fn default() -> Self {
        let mut me = Self::with_settings(DEFAULT_SEQUENCER.to_owned(), None, lezreg_sdk::storage::DEFAULT_API.to_owned(), None, None);
        if let Some(saved) = config_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            let _ = me.apply(&saved);
        }
        me
    }
}

fn reply(r: Result<Value, String>) -> String {
    match r {
        Ok(v) => json!({"success": true, "value": v, "error": null}),
        Err(e) => json!({"success": false, "value": null, "error": e}),
    }
    .to_string()
}

/// Like [`reply`], with a stable `code` beside the error text.
fn reply_coded(r: Result<Value, writes::Failure>) -> String {
    match r {
        Ok(v) => json!({"success": true, "value": v, "error": null, "code": null}),
        Err(f) => json!({"success": false, "value": null, "error": f.message, "code": f.code}),
    }
    .to_string()
}

/// The apps that may change this module's settings and sign through it without asking: its own
/// browser and the Names Wallet (whose approval sheet relies on `verify_app`). Basecamp names
/// each app's credential after the app, so the module knows which app calls. The host itself
/// (Basecamp, or `logosctl` driving the module) is trusted too; a caller that can't be named is
/// not. Reads stay open to everyone. (The Names module gates its settings the same way.)
const TRUSTED_APPS: &[&str] = &["registry_browser", "names_ui"];

fn trusted(caller: &logos_rust_sdk::LogosCaller) -> bool {
    matches!(caller, logos_rust_sdk::LogosCaller::HostAnchor) || TRUSTED_APPS.iter().any(|app| caller.is_module(app))
}

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

/// A stable name for who is calling, kept with what it submitted.
fn caller_name(caller: &logos_rust_sdk::LogosCaller) -> String {
    match caller {
        logos_rust_sdk::LogosCaller::Module { name, .. } => name.clone(),
        other => other.describe_for_human(),
    }
}

fn only_trusted() -> Result<(), writes::Failure> {
    let caller = calling();
    if trusted(&caller) {
        Ok(())
    } else {
        Err(writes::Failure::new("not_allowed", format!(
            "only the registry browser, the Names Wallet or the host may do this; refused for {}", caller.describe_for_human()
        )))
    }
}

/// The LNS program the app checks read by default on a network the module knows: the LNS
/// registry on the public LEZ testnet. A local chain has none (set `lns`).
const TESTNET_SEQUENCER: &str = "https://testnet.lez.logos.co";
const TESTNET_LNS: &str = "38jEfVV1RCyMzu1FGbYcCn5FqxCxKyY58VgzjruAh2gq";

fn default_lns(sequencer: &str) -> Option<AccountId> {
    (sequencer.trim_end_matches('/') == TESTNET_SEQUENCER).then(|| AccountId::from_base58(TESTNET_LNS).expect("a valid id"))
}

/// Every change `prepare_*` built, by its message, until it expires. `submit` takes only these,
/// and uses this copy: what a confirm sheet shows (what, entry, signers, summary, payer) is the
/// module's own, so a caller can't relabel a Freeze as an Update around the same message.
static PREPARED: std::sync::Mutex<std::collections::BTreeMap<String, Value>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

fn remember(change: &Value) {
    if let (Some(m), Ok(mut p)) = (change["message"].as_str(), PREPARED.lock()) {
        let now = now_ms();
        p.retain(|_, c| c["expires_at"].as_u64().is_some_and(|e| e > now));
        p.insert(m.to_owned(), change.clone());
    }
}

/// The change this module prepared with the same message as `given`.
fn prepared(given: &Value) -> Result<Value, writes::Failure> {
    let m = given["message"].as_str().unwrap_or_default();
    PREPARED.lock().ok().and_then(|p| p.get(m).cloned()).ok_or_else(|| {
        writes::Failure::new("unknown_change", "submit only a change a prepare_* method of this module returned (within 10 minutes, since it last started)")
    })
}

/// A change another app submitted, waiting for the person's approval.
struct Request {
    from: String,
    at: u64,
    change: Value,
    /// Set once approved (the job) or denied.
    outcome: Option<Result<String, ()>>,
}

static REQUESTS: std::sync::Mutex<std::collections::BTreeMap<String, Request>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

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
        if let Some(s) = v.get("sequencer").and_then(Value::as_str) {
            if !usable_sequencer(s) {
                return Err(format!("sequencer '{s}' must be https://, or http:// to this machine"));
            }
            s.clone_into(&mut self.sequencer);
        }
        if let Some(r) = v.get("registry").and_then(Value::as_str) {
            self.registry = if r.is_empty() { None } else { Some(program_arg(r)?) };
        }
        if let Some(s) = v.get("storage_api").and_then(Value::as_str) {
            if !usable_sequencer(s) {
                return Err(format!("storage_api '{s}' must be https://, or http:// to this machine"));
            }
            s.clone_into(&mut self.storage_api);
        }
        if let Some(l) = v.get("lns").and_then(Value::as_str) {
            self.lns = if l.is_empty() { None } else { Some(program_arg(l)?) };
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
        Ok(())
    }

    fn shown(&self) -> Value {
        json!({"sequencer": self.sequencer, "registry": self.registry.map(|r| r.to_base58()), "storage_api": self.storage_api,
               "lns": self.lns.or_else(|| default_lns(&self.sequencer)).map(|l| l.to_base58()), "lns_is_default": self.lns.is_none() && default_lns(&self.sequencer).is_some(), "basecamp_data": self.basecamp_data.as_ref().map(|d| d.display().to_string()),
               "cli": self.cli.as_ref().map(|d| d.display().to_string())})
    }

    fn reader(&self) -> Result<Registry<HttpRpc>, String> {
        let reg = self.registry.ok_or("no registry configured: call config_set with {\"registry\": \"<id>\"}")?;
        Ok(Registry::new(HttpRpc::new(&self.sequencer), reg))
    }

    fn save(&self) -> Result<(), String> {
        let p = config_path().ok_or("no config directory")?;
        let dir = p.parent().expect("has a parent");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let tmp = dir.join(".module.json.tmp");
        std::fs::write(&tmp, self.shown().to_string()).map_err(|e| e.to_string())?;
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
                return Ok(json!({"text": format!("{} {} (community entry, unproven)", c.name, c.version), "kind": "community", "verified": false,
                                  "stale": live.is_some_and(|l| l != c.image_id), "upgradeable": upgradeable, "deployed": live.is_some(),
                                  "name": c.name, "version": c.version, "author": c.author.to_base58(), "author_short": short_id(&c.author.to_base58()),
                                  "author_label_unproven": c.author_label, "community_count": community.len()}));
            }
            return Ok(json!({"text": format!("{short} (not in the registry)"), "kind": "unregistered", "verified": false, "stale": false, "upgradeable": upgradeable, "deployed": live.is_some(), "community_count": 0}));
        };
        // Stale: a program runs here, but not the code registered. No program at all is said so.
        let stale = live.is_some_and(|l| l != e.image_id);
        let verified = !stale && live.is_some_and(|i| locally_verified(program_id, &i));
        let mut text = format!("{} {}", e.name, e.version);
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
                  "name": e.name, "version": e.version, "author": e.author.to_base58(), "author_short": short_id(&e.author.to_base58()),
                  "author_label_unproven": e.author_label}))
    }

    /// The registry a write is for, coded.
    fn write_registry(&self) -> Result<AccountId, writes::Failure> {
        self.registry.ok_or_else(|| writes::Failure::new("no_registry", "no registry configured: call config_set with {\"registry\": \"<id>\"}"))
    }

    /// The chain's clock (ms), which the registry checks a change's `now` against.
    fn chain_now(&self) -> Result<u64, writes::Failure> {
        let (account, program) = writes::clock_actor();
        let state = HttpRpc::new(&self.sequencer).get_actor_state(&account, &program).map_err(|e| writes::Failure::new("transport", e.to_string()))?;
        writes::clock_time(&state).ok_or_else(|| writes::Failure::new("no_clock", format!("the sequencer at {} has no clock account to time the change by", self.sequencer)))
    }

    /// A built change, with the caller's send options and a summary for a confirm sheet.
    fn change(&self, built: &writes::Built, input: &Value, now: u64) -> Result<Value, writes::Failure> {
        let reg = self.write_registry()?;
        let payer = input.get("payer").filter(|p| !p.is_null()).map(|p| writes::account(p, "payer")).transpose()?;
        // What a confirm sheet shows. Built here from the instruction, never taken from a caller.
        let fields: Vec<&str> = ["name", "version", "description", "tags", "author_label", "idl", "source", "build", "image_id", "new_author"]
            .into_iter().filter(|k| input.get(*k).is_some_and(|v| !v.is_null())).collect();
        let summary = json!({"what": built.what, "entry": built.entry.to_base58(), "signers": built.signers.iter().map(AccountId::to_base58).collect::<Vec<_>>(),
                             "registry": reg.to_base58(), "sequencer": self.sequencer, "irreversible": built.what == "Freeze",
                             "payer": payer.map(|p| p.to_base58()), "fields": fields});
        let mut c = built.change(&self.sequencer, &reg, now, summary);
        if let Some(p) = payer {
            c["payer"] = json!(p.to_base58());
        }
        c["allow_remote"] = json!(input.get("allow_remote") == Some(&json!(true)));
        remember(&c);
        Ok(c)
    }

    /// `prepare_register` and `prepare_register_third_party`.
    fn prepare_new(&self, input_json: &str, third_party: bool) -> Result<Value, writes::Failure> {
        let input: Value = serde_json::from_str(input_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
        let reg = self.write_registry()?;
        let reader = Registry::new(HttpRpc::new(&self.sequencer), reg);
        let program = writes::account(&input["program"], "program")?;
        let live = reader.header(&program).map_err(|e| writes::Failure::new("transport", e.to_string()))?.map(|h| h.image_id);
        let now = self.chain_now()?;
        let built = if third_party { writes::register_third_party(&reg, &input, now, live)? } else { writes::register(&reg, &input, now, live)? };
        if reader.entry(&built.entry).map_err(|e| writes::Failure::new("transport", e.to_string()))?.is_some() {
            return Err(writes::Failure::new("exists", format!("{} already holds an entry: use prepare_update", built.entry.to_base58())));
        }
        self.change(&built, &input, now)
    }

    /// `prepare_update`, `prepare_transfer` and `prepare_freeze`: they read the entry first.
    fn prepare_change(&self, input_json: &str, build: fn(&AccountId, &Value, u64, &writes::Stored) -> writes::Res<writes::Built>) -> Result<Value, writes::Failure> {
        let input: Value = serde_json::from_str(input_json).map_err(|e| writes::Failure::input(format!("not JSON: {e}")))?;
        let reg = self.write_registry()?;
        let at = writes::target(&input)?;
        let reader = Registry::new(HttpRpc::new(&self.sequencer), reg);
        let e = reader.entry(&at).map_err(|e| writes::Failure::new("transport", e.to_string()))?
            .ok_or_else(|| writes::Failure::new("no_entry", format!("there is no registry entry at {}", at.to_base58())))?;
        let stored = writes::Stored { revision: e.revision, author: e.author, registrant: e.registrant, frozen: e.is_frozen() };
        let now = self.chain_now()?;
        let built = build(&reg, &input, now, &stored)?;
        self.change(&built, &input, now)
    }

    /// Starts signing `change` with the lezreg CLI, for the app named `requested_by`.
    fn start_job(&self, change: &Value, requested_by: &str) -> Result<Value, writes::Failure> {
        let cli = bridge::find_cli(self.cli.as_deref())?;
        let job = bridge::start(&cli, change, now_ms(), requested_by, |job| {
            let detail = job.json().to_string();
            emit_change_settled(&job.id, job.state, &detail);
        })?;
        Ok(job.json())
    }

    /// LNS reads for the configured sequencer and LNS program, kept across calls.
    fn names(&mut self) -> Result<&mut Names<HttpRpc>, String> {
        let lns = self.lns.or_else(|| default_lns(&self.sequencer)).ok_or("no LNS program configured: call config_set with {\"lns\": \"<program id>\"}")?;
        let current = matches!(&self.names, Some((seq, l, _)) if *seq == self.sequencer && *l == lns);
        if !current {
            self.names = Some((self.sequencer.clone(), lns, Names::new(HttpRpc::new(&self.sequencer), lns)));
        }
        Ok(&mut self.names.as_mut().expect("just set").2)
    }

    /// A package checked against its publisher, kept for [`APP_CHECK_TTL_MS`].
    fn check(&mut self, manifest: &Manifest) -> Result<Value, String> {
        let now = now_ms();
        let key = (manifest.name.clone(), manifest.root);
        if let Some((verdict, at)) = self.checked.get(&key) {
            if now.saturating_sub(*at) < APP_CHECK_TTL_MS {
                return Ok(verdict.clone());
            }
        }
        let (verdict, via) = if manifest.publisher().is_none() {
            (apps::Verdict::Unclaimed, None)
        } else {
            self.names()?.verify(manifest, now).map_err(|e| e.to_string())?
        };
        let mut shown = verdict.to_json(manifest);
        // A publisher name that forwards was checked where it points; the prompt says so.
        shown["forwarded_from"] = json!(via);
        if let Some(programs) = shown["record"]["programs"].as_array().cloned() {
            let labels: Vec<Value> = programs.iter().filter_map(Value::as_str).map(|p| self.label(p).unwrap_or_else(|e| json!({"text": p, "error": e}))).collect();
            shown["record"]["program_labels"] = json!(labels);
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

/// The installed manifest of `module`: core modules under `modules/`, ui apps under `plugins/`.
fn installed_manifest(data: &std::path::Path, module: &str) -> Result<Manifest, String> {
    // A module name becomes a path: only the characters an app record's key allows, no `..`.
    if !apps::is_module_name(module) || module.contains("..") || module.starts_with('.') {
        return Err(format!("'{module}' is not a module name"));
    }
    for dir in ["modules", "plugins"] {
        let path = data.join(dir).join(module).join("manifest.json");
        if let Ok(text) = std::fs::read_to_string(&path) {
            let json: Value = serde_json::from_str(&text).map_err(|_| format!("{} is not JSON", path.display()))?;
            let manifest = Manifest::parse(&json).map_err(|e| e.to_string())?;
            if manifest.name != module {
                return Err(format!("{} names another module ({})", path.display(), manifest.name));
            }
            return Ok(manifest);
        }
    }
    Err(format!("{module} is not installed in {}", data.display()))
}

impl ProgramRegistryModule for ProgramRegistry {
    fn resolve_program(&mut self, program_id: String) -> String {
        reply((|| {
            let program = program_arg(&program_id)?;
            let reader = self.reader()?;
            let r = reader.resolve(&program).map_err(|e| e.to_string())?;
            let third = reader.third_party(&program, 50).map_err(|e| e.to_string())?.len();
            let mut v = js::resolved(&r);
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
            Ok(v)
        })())
    }

    fn list_entries(&mut self, after: String, limit: i64) -> String {
        reply((|| {
            let after = if after.trim().is_empty() { None } else { Some(program_arg(&after)?) };
            let page = self.reader()?.list(after.as_ref(), limit_arg(limit)).map_err(|e| e.to_string())?;
            Ok(json!({"items": page.items.iter().map(js::listed).collect::<Vec<_>>(), "next": page.next.map(|n| n.to_base58()), "total": page.total}))
        })())
    }

    fn search_name(&mut self, query: String, limit: i64) -> String {
        reply((|| {
            let found = self.reader()?.search_name(&query, limit_arg(limit), 1000).map_err(|e| e.to_string())?;
            Ok(json!({"items": found.items.iter().map(js::listed).collect::<Vec<_>>(), "scanned": found.scanned, "truncated": found.truncated}))
        })())
    }

    fn third_party_entries(&mut self, program_id: String) -> String {
        reply((|| {
            let program = program_arg(&program_id)?;
            let items = self.reader()?.third_party(&program, 50).map_err(|e| e.to_string())?;
            Ok(json!(items.iter().filter(|i| matches!(i.kind, Some(Kind::ThirdParty { .. }))).map(js::listed).collect::<Vec<_>>()))
        })())
    }

    // TODO(appfix): the SDK is about to recompute a package's root from its files instead of
    // trusting manifest.json (branch `appfix`, not merged; its verdicts and the apps.rs/lgx.rs API
    // will change). verify_app and verify_package stay on the current API until it lands; then
    // bump the SDK pin and report its new verdict here.
    fn verify_app(&mut self, module_name: String) -> String {
        reply((|| {
            let data = self.basecamp_data().ok_or("no Basecamp data folder: call config_set with {\"basecamp_data\": \"<path>\"}")?;
            let manifest = installed_manifest(&data, module_name.trim())?;
            self.check(&manifest)
        })())
    }

    fn verify_package(&mut self, manifest_json: String) -> String {
        reply((|| {
            let json: Value = serde_json::from_str(&manifest_json).map_err(|e| format!("not JSON: {e}"))?;
            let manifest = Manifest::parse(&json).map_err(|e| e.to_string())?;
            self.check(&manifest)
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
                        shown.push(json!({"module": module, "version": r.version, "roots": r.roots.iter().map(hex::encode).collect::<Vec<_>>(), "catalog": r.catalog, "programs": labels}));
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
            let mut next = Self::with_settings(self.sequencer.clone(), self.registry, self.storage_api.clone(), self.lns, self.basecamp_data.clone());
            next.cli = self.cli.clone();
            next.apply(&v).map_err(writes::Failure::input)?;
            next.save()?;
            *self = next;
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
            // Only what this module prepared, and its own copy of it.
            let change = prepared(&given)?;
            // A change is only sent to the network this module is pointed at.
            let reg = self.write_registry()?;
            if change["registry"].as_str() != Some(reg.to_base58().as_str()) || change["sequencer"].as_str() != Some(self.sequencer.as_str()) {
                return Err(writes::Failure::new("wrong_network", "this change was prepared for another registry or sequencer than the module is set to: prepare it again"));
            }
            if change["expires_at"].as_u64().is_some_and(|e| now_ms() >= e) {
                return Err(writes::Failure::new("expired", "this change was prepared more than 10 minutes ago and the registry would refuse it: prepare it again"));
            }
            let caller = calling();
            if !trusted(&caller) {
                // Nothing is signed for an app the person has not trusted: it waits for them.
                let from = caller_name(&caller);
                let at = now_ms();
                static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let id = format!("req-{at}-{}", SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
                if let Ok(mut r) = REQUESTS.lock() {
                    r.retain(|_, q| q.outcome.is_none() || at.saturating_sub(q.at) < 3_600_000);
                    if r.values().filter(|q| q.from == from && q.outcome.is_none()).count() >= 5 {
                        return Err(writes::Failure::new("too_many_requests", "this app already has 5 changes waiting for approval"));
                    }
                    r.insert(id.clone(), Request { from: from.clone(), at, change, outcome: None });
                }
                return Ok(json!({"request": id, "state": "awaiting_approval", "from": from,
                                 "note": "the person approves or refuses it in the Program Registry app"}));
            }
            self.start_job(&change, &caller_name(&caller))
        })())
    }

    fn submit_status(&mut self, job: String) -> String {
        let caller = calling();
        let me = caller_name(&caller);
        let all = trusted(&caller);
        let job = job.trim();
        reply_coded(if job.is_empty() {
            Ok(json!(bridge::jobs().into_iter().filter(|j| all || j["requested_by"] == me.as_str()).collect::<Vec<_>>()))
        } else if let Some(state) = REQUESTS.lock().ok().and_then(|r| r.get(job).filter(|q| all || q.from == me).map(|q| match &q.outcome {
            None => json!({"request": job, "state": "awaiting_approval"}),
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
            let r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
            Ok(json!(r.iter().filter(|(_, q)| q.outcome.is_none()).map(|(id, q)| json!({"request": id, "from": q.from, "at": q.at, "change": q.change})).collect::<Vec<_>>()))
        })())
    }

    fn approve_request(&mut self, request: String) -> String {
        reply_coded((|| {
            only_trusted()?;
            let (change, from) = {
                let r = REQUESTS.lock().map_err(|_| writes::Failure::new("error", "request list unavailable"))?;
                let q = r.get(request.trim()).filter(|q| q.outcome.is_none()).ok_or_else(|| writes::Failure::new("no_request", format!("no request '{request}' is waiting")))?;
                (q.change.clone(), q.from.clone())
            };
            let job = self.start_job(&change, &from)?;
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
        let change = json!({"format": writes::FORMAT, "registry": "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN", "sequencer": "http://127.0.0.1:9", "expires_at": u64::MAX, "message": "0a0b"});
        assert_eq!(parsed(&m.submit(change.to_string()))["code"], "unknown_change", "only what this module prepared");
        remember(&change);
        assert_eq!(parsed(&m.submit(change.to_string()))["code"], "wrong_network");
        let mut lez = change.clone();
        lez["signer"] = json!("lez_core");
        assert_eq!(parsed(&m.submit(lez.to_string()))["code"], "lez_core_unavailable");
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

    /// Settings and signing are for the registry browser, the Names Wallet and the host; any
    /// other app may read, and its changes wait for the person's approval.
    #[test]
    fn only_trusted_apps_change_settings_or_sign() {
        use logos_rust_sdk::LogosCaller;
        assert!(trusted(&LogosCaller::HostAnchor));
        assert!(trusted(&app("registry_browser")));
        assert!(trusted(&app("names_ui")));
        assert!(trusted(&LogosCaller::Module { name: "names_ui".into(), instance: Some("a41f".into()) }));
        assert!(!trusted(&app("evil_ui")));
        assert!(!trusted(&LogosCaller::Unknown));
        assert!(!trusted(&LogosCaller::Derived { parent: "registry_browser".into(), leaf: "x".into() }));

        let reg = "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN";
        let mut m = fresh();
        m.registry = AccountId::from_base58(reg);
        as_caller(app("evil_ui"));
        let v = parsed(&m.config_set(json!({"lns": reg}).to_string()));
        assert_eq!((v["success"].as_bool(), v["code"].as_str()), (Some(false), Some("not_allowed")), "{v}");
        assert!(m.lns.is_none(), "nothing changed");
        assert_eq!(parsed(&m.signer_status())["code"], "not_allowed");
        assert_eq!(parsed(&m.pending_requests())["code"], "not_allowed");
        assert_eq!(parsed(&m.config_get())["success"], true, "reads stay open");
        // A change from an untrusted app waits; nothing is signed.
        let change = json!({"format": writes::FORMAT, "what": "Freeze", "registry": reg, "sequencer": DEFAULT_SEQUENCER, "expires_at": u64::MAX, "entry": reg,
                            "message": "04aa", "summary": {"what": "Freeze", "irreversible": true}});
        remember(&change);
        // The app relabels it: the module keeps showing (and sending) its own copy.
        let mut relabelled = change.clone();
        relabelled["what"] = json!("Update");
        relabelled["summary"] = json!({"what": "Update", "irreversible": false});
        let v = parsed(&m.submit(relabelled.to_string()));
        assert_eq!(v["value"]["state"], "awaiting_approval", "{v}");
        let request = v["value"]["request"].as_str().unwrap().to_owned();
        assert_eq!(parsed(&m.approve_request(request.clone()))["code"], "not_allowed", "it can't approve itself");
        assert_eq!(parsed(&m.submit_status(request.clone()))["value"]["state"], "awaiting_approval");
        as_caller(app("other_ui"));
        assert_eq!(parsed(&m.submit_status(request.clone()))["code"], "no_job", "another app can't see it");
        // The person (through the browser) sees it and refuses it.
        as_caller(app("registry_browser"));
        let pending = parsed(&m.pending_requests());
        assert!(pending["value"].as_array().unwrap().iter().any(|q| q["request"] == request.as_str() && q["from"] == "evil_ui"
            && q["change"]["what"] == "Freeze" && q["change"]["summary"]["irreversible"] == true), "the card shows the module's copy: {pending}");
        assert_eq!(parsed(&m.deny_request(request.clone()))["value"]["state"], "denied");
        assert_eq!(parsed(&m.deny_request(request.clone()))["code"], "no_request");
        as_caller(app("evil_ui"));
        assert_eq!(parsed(&m.submit_status(request))["value"]["state"], "denied");
        as_caller(logos_rust_sdk::LogosCaller::HostAnchor);
    }

    /// An approved request is signed through the CLI like a trusted submit.
    #[test]
    fn an_approved_request_is_sent_and_settles() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("lezreg-approve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("lezreg");
        std::fs::write(&cli, "#!/bin/sh\ncat > /dev/null\necho '{\"tx\":\"abc\",\"block\":9}'\n").unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reg = "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN";
        let mut m = fresh();
        m.registry = AccountId::from_base58(reg);
        m.cli = Some(cli);
        let change = json!({"format": writes::FORMAT, "what": "Freeze", "registry": reg, "sequencer": DEFAULT_SEQUENCER, "expires_at": u64::MAX, "entry": reg, "message": "04bb"});
        remember(&change);
        as_caller(app("some_dapp_ui"));
        let request = parsed(&m.submit(change.to_string()))["value"]["request"].as_str().unwrap().to_owned();
        as_caller(app("registry_browser"));
        let v = parsed(&m.approve_request(request.clone()));
        assert_eq!((v["value"]["state"].as_str(), v["value"]["requested_by"].as_str()), (Some("running"), Some("some_dapp_ui")), "{v}");
        assert_eq!(parsed(&m.approve_request(request.clone()))["code"], "no_request", "approved once");
        as_caller(app("some_dapp_ui"));
        let mut state = Value::Null;
        for _ in 0..100 {
            state = parsed(&m.submit_status(request.clone()))["value"].clone();
            if state["state"] != "running" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!((state["state"].as_str(), state["receipt"]["block"].as_u64()), (Some("applied"), Some(9)), "{state}");
        as_caller(logos_rust_sdk::LogosCaller::HostAnchor);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_testnet_has_a_default_lns_program() {
        let mut m = fresh();
        assert_eq!(m.shown()["lns"], Value::Null, "a local chain has none");
        assert!(m.apply(&json!({"sequencer": "https://testnet.lez.logos.co"})).is_ok());
        assert_eq!((m.shown()["lns"].as_str(), m.shown()["lns_is_default"].as_bool()), (Some(TESTNET_LNS), Some(true)));
        assert!(m.apply(&json!({"lns": "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN"})).is_ok());
        assert_eq!(m.shown()["lns_is_default"], false);
    }

    #[test]
    fn limits_are_clamped() {
        assert_eq!(limit_arg(-5), 1);
        assert_eq!(limit_arg(10_000), 200);
    }

    /// A Basecamp data folder with one core module and one ui app installed.
    fn basecamp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lezreg-module-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = "ab".repeat(32);
        for (kind, name, author) in [("modules", "names", "lns:jef"), ("plugins", "forumer_ui", "")] {
            std::fs::create_dir_all(dir.join(kind).join(name)).unwrap();
            let manifest = json!({"name": name, "version": "1.0.0", "author": author, "hashes": {"root": root}});
            std::fs::write(dir.join(kind).join(name).join("manifest.json"), manifest.to_string()).unwrap();
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
        // A package not yet installed, as a catalog index lists it.
        let v = parsed(&m.verify_package(json!({"name": "faucet_ui", "version": "0.1.0", "author": "", "hashes": {"root": "cd".repeat(32)}}).to_string()));
        assert_eq!(v["value"]["status"], "unclaimed");
        assert_eq!(parsed(&m.verify_package("{}".into()))["success"], false);
        assert!(parsed(&m.publisher_apps("jef".into()))["error"].as_str().unwrap().contains("no LNS program"));
        let shown = parsed(&m.config_get());
        assert_eq!(shown["value"]["basecamp_data"], data.display().to_string());
        let _ = std::fs::remove_dir_all(data);
    }
}
