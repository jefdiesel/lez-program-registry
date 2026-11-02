//! Output: text on stdout, or with `--json` exactly one JSON object on stdout. Progress always
//! goes to stderr. In JSON mode stdout is pointed at /dev/null while the command runs, so
//! nothing the wallet prints can spoil the JSON.

use std::{io::Write as _, os::fd::AsRawFd as _};

use serde_json::{Value, json};

pub struct Out {
    json: bool,
    saved: Option<i32>,
    value: Value,
    /// Keys every result carries, an error included (e.g. a deploy's program id).
    context: serde_json::Map<String, Value>,
}

/// Points stdout at /dev/null; returns the saved descriptor.
fn silence() -> Option<i32> {
    let _ = std::io::stdout().flush();
    let null = std::fs::OpenOptions::new().write(true).open("/dev/null").ok()?;
    // SAFETY: plain descriptor calls on stdout.
    let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if saved < 0 {
        return None;
    }
    if unsafe { libc::dup2(null.as_raw_fd(), libc::STDOUT_FILENO) } < 0 {
        unsafe { libc::close(saved) };
        return None;
    }
    Some(saved)
}

fn restore(saved: i32) {
    let _ = std::io::stdout().flush();
    // SAFETY: `saved` came from `silence`.
    unsafe {
        libc::dup2(saved, libc::STDOUT_FILENO);
        libc::close(saved);
    }
}

/// Runs `f` with stdout silenced (the wallet announces things there).
pub async fn quietly<T, F: std::future::Future<Output = T>>(f: impl FnOnce() -> F) -> T {
    let saved = silence();
    let r = f().await;
    if let Some(s) = saved {
        restore(s);
    }
    r
}

impl Out {
    pub fn new(json: bool) -> Self {
        let saved = if json { silence() } else { None };
        Self { json, saved, value: Value::Null, context: serde_json::Map::new() }
    }

    /// A line of text (text mode only).
    pub fn text(&self, s: &str) {
        if !self.json {
            println!("{s}");
        }
    }

    /// The JSON result (JSON mode only).
    pub fn json(&mut self, v: Value) {
        self.value = v;
    }

    /// Adds keys to the JSON result.
    pub fn json_merge(&mut self, v: Value) {
        if let (Value::Object(into), Value::Object(from)) = (&mut self.value, v) {
            into.extend(from);
        }
    }

    /// A key the result carries whatever happens next, an error included.
    pub fn context(&mut self, key: &str, v: Value) {
        self.context.insert(key.to_owned(), v);
    }

    pub fn error(&mut self, kind: &str, message: &str) {
        eprintln!("error: {message}");
        let mut v = json!({"error": message, "kind": kind});
        if let Value::Object(o) = &mut v {
            o.extend(self.context.clone());
        }
        self.value = v;
    }

    /// Adds a key to the error object.
    pub fn error_field(&mut self, key: &str, v: Value) {
        if let Value::Object(o) = &mut self.value {
            o.insert(key.to_owned(), v);
        }
    }

    pub fn finish(mut self) {
        if !self.json {
            return;
        }
        if let Some(s) = self.saved.take() {
            restore(s);
        }
        let mut stdout = std::io::stdout();
        let _ = writeln!(stdout, "{}", serde_json::to_string_pretty(&self.value).unwrap_or_default());
        let _ = stdout.flush();
    }
}
