//! `registry_probe`: a test fixture. It is an ordinary app as far as `program_registry` can
//! tell (not `registry_browser`, not `names_ui`, not the host), and calls it through the typed
//! client the builder generates from `program_registry`'s contract, so each call arrives with
//! this module's own credential. Drive it with `logosctl call registry_probe <method> …`:
//! settings changes and signing must be refused or queued, reads must work.

pub trait RegistryProbeModule: Send + 'static {
    /// `program_registry.config_set(settings_json)` as this app.
    fn probe_config_set(&mut self, settings_json: String) -> String;
    /// `program_registry.program_label(program_id)` as this app.
    fn probe_label(&mut self, program_id: String) -> String;
    /// `program_registry.prepare_update(input_json)` as this app.
    fn probe_prepare_update(&mut self, input_json: String) -> String;
    /// `program_registry.submit(change_json)` as this app.
    fn probe_submit(&mut self, change_json: String) -> String;
    /// `program_registry.submit_status(job)` as this app.
    fn probe_status(&mut self, job: String) -> String;
    /// `program_registry.pending_requests()` as this app.
    fn probe_pending(&mut self) -> String;
    /// `program_registry.approve_request(request)` as this app.
    fn probe_approve(&mut self, request: String) -> String;

    /// Called once the module is wired to the host.
    fn on_context_ready(&mut self, _ctx: &RustModuleContext) {}
}

#[derive(Default)]
pub struct Probe;

fn shown<E: std::fmt::Display>(r: Result<String, E>) -> String {
    r.unwrap_or_else(|e| serde_json::json!({"probe_error": e.to_string()}).to_string())
}

impl RegistryProbeModule for Probe {
    fn probe_config_set(&mut self, settings_json: String) -> String {
        shown(modules().program_registry.config_set(&settings_json))
    }
    fn probe_label(&mut self, program_id: String) -> String {
        shown(modules().program_registry.program_label(&program_id))
    }
    fn probe_prepare_update(&mut self, input_json: String) -> String {
        shown(modules().program_registry.prepare_update(&input_json))
    }
    fn probe_submit(&mut self, change_json: String) -> String {
        shown(modules().program_registry.submit(&change_json))
    }
    fn probe_status(&mut self, job: String) -> String {
        shown(modules().program_registry.submit_status(&job))
    }
    fn probe_pending(&mut self) -> String {
        shown(modules().program_registry.pending_requests())
    }
    fn probe_approve(&mut self, request: String) -> String {
        shown(modules().program_registry.approve_request(&request))
    }
}

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<Probe>();
}
