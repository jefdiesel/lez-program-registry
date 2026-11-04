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

use lezreg_sdk::{
    HttpRpc, Kind, Registry,
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
    /// name, version, author}`. `kind` is `deployer`, `community` or `unregistered`;
    /// `verified` is true only when this machine rebuilt the running image from its recorded
    /// source (`lezreg verify --mark`) and it still runs that image.
    fn program_label(&mut self, program_id: String) -> String;

    /// The program's IDL from Logos Storage, hash-checked against the chain: `{status, idl}`.
    /// `status` is `ok` or why not (`unavailable: …`, `hash mismatch …`). Never fails because
    /// of the IDL.
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

    /// `{sequencer, registry, storage_api, lns, basecamp_data}`.
    fn config_get(&mut self) -> String;

    /// Changes any of `sequencer`, `registry`, `storage_api`, `lns` (the LNS program id, for
    /// the app checks) and `basecamp_data` (Basecamp's data folder, if not the default) from a
    /// JSON object, and remembers them. The sequencer must be `https://`, or `http://` to this
    /// machine.
    fn config_set(&mut self, settings_json: String) -> String;

    /// Called once the module is wired to the host.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

#[cfg(test)]
pub struct RustModuleContext;

pub struct ProgramRegistry {
    sequencer: String,
    registry: Option<AccountId>,
    storage_api: String,
    lns: Option<AccountId>,
    basecamp_data: Option<PathBuf>,
    /// LNS reads, kept with their derived names, for the sequencer and LNS program they read.
    names: Option<(String, AccountId, Names<HttpRpc>)>,
    /// `(module, root)` → (verdict, when), for [`APP_CHECK_TTL_MS`].
    checked: std::collections::HashMap<(String, [u8; 32]), (Value, u64)>,
}

impl ProgramRegistry {
    fn with_settings(sequencer: String, registry: Option<AccountId>, storage_api: String, lns: Option<AccountId>, basecamp_data: Option<PathBuf>) -> Self {
        Self { sequencer, registry, storage_api, lns, basecamp_data, names: None, checked: std::collections::HashMap::new() }
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
        Ok(())
    }

    fn shown(&self) -> Value {
        json!({"sequencer": self.sequencer, "registry": self.registry.map(|r| r.to_base58()), "storage_api": self.storage_api,
               "lns": self.lns.map(|l| l.to_base58()), "basecamp_data": self.basecamp_data.as_ref().map(|d| d.display().to_string())})
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
            return Ok(json!({"text": format!("{short} (not in the registry)"), "kind": "unregistered", "verified": false, "stale": false, "upgradeable": upgradeable, "deployed": live.is_some()}));
        };
        let stale = live != Some(e.image_id);
        let verified = !stale && live.is_some_and(|i| locally_verified(program_id, &i));
        let mut text = format!("{} {}", e.name, e.version);
        if verified {
            text.push_str(" ✓ verified");
        }
        if stale {
            text.push_str(" (stale: the program runs other code than registered)");
        }
        Ok(json!({"text": text, "kind": "deployer", "verified": verified, "stale": stale, "upgradeable": upgradeable,
                  "name": e.name, "version": e.version, "author": e.author.to_base58(), "author_label_unproven": e.author_label}))
    }

    /// LNS reads for the configured sequencer and LNS program, kept across calls.
    fn names(&mut self) -> Result<&mut Names<HttpRpc>, String> {
        let lns = self.lns.ok_or("no LNS program configured: call config_set with {\"lns\": \"<program id>\"}")?;
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
        let verdict = if manifest.publisher().is_none() {
            apps::Verdict::Unclaimed
        } else {
            self.names()?.verify(manifest, now).map_err(|e| e.to_string())?
        };
        let mut shown = verdict.to_json(manifest);
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
            // Two quick attempts: a Basecamp call must not hang for the CLI's full ladder.
            let cfg = FetchConfig { storage_api: Some(self.storage_api.clone()), backoff: vec![std::time::Duration::from_secs(2)], ..FetchConfig::default() };
            Ok(match idl::fetch_idl(&e.idl, &cfg) {
                IdlStatus::Ok(v) => json!({"status": "ok", "idl": v}),
                other => json!({"status": other.to_string(), "idl": null}),
            })
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
        reply((|| {
            let v: Value = serde_json::from_str(&settings_json).map_err(|e| format!("not JSON: {e}"))?;
            let mut next = Self::with_settings(self.sequencer.clone(), self.registry, self.storage_api.clone(), self.lns, self.basecamp_data.clone());
            next.apply(&v)?;
            next.save()?;
            *self = next;
            Ok(self.shown())
        })())
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
