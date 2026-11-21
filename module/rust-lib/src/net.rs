//! The module's one way to the sequencer: JSON-RPC over HTTP, shared by every call the module
//! answers, so a busy front end (a rate limit in front of a public sequencer, shared by every
//! machine behind one home IP) sees a few paced requests instead of a burst.
//!
//! - **One client per sequencer**, kept across calls: a refused batch is remembered, and reads
//!   go out one at a time (never the SDK's four threads).
//! - **Paced**: at most [`PER_SECOND`] requests in any second.
//! - **Batched**: many actor reads go [`MAX_BATCH`] to a request (the public filter's cap).
//! - **Cached reads**: an account or actor read is kept [`CACHE_TTL`]; `forget()` drops them
//!   (Refresh, a settled change, new settings). Writes read fresh ([`Seq::fresh`]).
//! - **Rate limits fail fast**: an HTTP 429 sets a quiet window (its `Retry-After`, clamped,
//!   [`DEFAULT_RETRY_AFTER`] without one) in which nothing is sent; the call answers at once with
//!   `rate_limited` and how long to wait. The module never sleeps out a limit: it answers one
//!   call at a time and hosts give up after about 20 s, so the app retries instead.
//! - **No response bodies in errors**: a refusal is "HTTP <status>: <its text without markup,
//!   200 characters at most>", kept for a "technical details" line only.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use lezreg_sdk::{
    Error, Result, Rpc,
    rpc::{AccountStates, parse_account},
    types::AccountId,
};
use serde_json::{Value, json};

/// Requests in any one second, at most.
pub const PER_SECOND: usize = 3;
/// Calls in one JSON-RPC batch (the public sequencer's filter refuses more).
pub const MAX_BATCH: usize = 20;
/// How long a read is kept.
pub const CACHE_TTL: Duration = Duration::from_secs(30);
/// The quiet window after a 429 that names none.
pub const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(10);
/// The longest quiet window a `Retry-After` can ask for.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);
/// Statuses with which a front end refuses a batch as such (the same calls may go one by one).
const BATCH_REFUSED: [u16; 4] = [400, 403, 405, 413];
/// The marker a rate-limited error carries, read by [`classify`].
const RATE_LIMITED: &str = "rate_limited retry_after_ms=";

/// One sequencer's client.
pub struct Net {
    url: String,
    agent: ureq::Agent,
    sent: Mutex<VecDeque<Instant>>,
    quiet_until: Mutex<Option<Instant>>,
    cache: Mutex<HashMap<String, (Instant, Value)>>,
    batches_refused: Mutex<bool>,
    requests: Mutex<u64>,
}

fn nets() -> &'static Mutex<HashMap<String, Arc<Net>>> {
    static NETS: OnceLock<Mutex<HashMap<String, Arc<Net>>>> = OnceLock::new();
    NETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The shared client for `url`.
pub fn net(url: &str) -> Arc<Net> {
    let mut all = nets().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    all.entry(url.to_owned()).or_insert_with(|| Arc::new(Net::new(url))).clone()
}

/// Drops every cached read of every sequencer.
pub fn forget_all() {
    let all = nets().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for n in all.values() {
        n.forget();
    }
}

impl Net {
    fn new(url: &str) -> Self {
        // As the SDK's client: the sequencer the user chose and nothing else (no redirects, no
        // proxy from the environment), statuses read here.
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .max_redirects(0)
            .proxy(None)
            .http_status_as_error(false)
            .build();
        Self {
            url: url.to_owned(),
            agent: config.into(),
            sent: Mutex::new(VecDeque::new()),
            quiet_until: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            batches_refused: Mutex::new(false),
            requests: Mutex::new(0),
        }
    }

    /// HTTP requests sent so far.
    pub fn requests(&self) -> u64 {
        *self.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drops this sequencer's cached reads.
    pub fn forget(&self) {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }

    fn transport(&self, message: String) -> Error {
        Error::Transport { url: self.url.clone(), message }
    }

    /// Waits for a slot: at most PER_SECOND requests in any second. Inside a 429's quiet
    /// window: an error at once, nothing sent.
    fn slot(&self) -> Result<()> {
        if let Some(until) = *self.quiet_until.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
            let now = Instant::now();
            if now < until {
                return Err(self.transport(format!("{RATE_LIMITED}{} (waiting out the sequencer's rate limit; nothing sent)", (until - now).as_millis())));
            }
        }
        let mut sent = self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        while sent.front().is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1)) {
            sent.pop_front();
        }
        if sent.len() >= PER_SECOND {
            let wait = Duration::from_secs(1).saturating_sub(now.duration_since(*sent.front().expect("non-empty")));
            std::thread::sleep(wait);
            sent.pop_front();
        }
        sent.push_back(Instant::now());
        Ok(())
    }

    /// POSTs `body`: the status and the text. A 429 starts the quiet window and is an error.
    fn post(&self, body: &Value) -> Result<(u16, String)> {
        self.slot()?;
        *self.requests.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        let mut response = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| self.transport(e.to_string()))?;
        let status = response.status().as_u16();
        let retry_after = response.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if status == 429 {
            let wait = retry_after_of(retry_after.as_deref());
            *self.quiet_until.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now() + wait);
            return Err(self.transport(format!("{RATE_LIMITED}{} {}", wait.as_millis(), refusal(status, &text))));
        }
        Ok((status, text))
    }

    /// One call; its result.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        let (status, text) = self.post(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))?;
        if !(200..300).contains(&status) {
            return Err(self.transport(refusal(status, &text)));
        }
        let reply: Value = serde_json::from_str(&text).map_err(|_| self.transport(format!("not JSON-RPC: {}", plain(&text, 200))))?;
        result_of(method, &reply)
    }

    fn cached(&self, key: &str) -> Option<Value> {
        let cache = self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.get(key).filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, v)| v.clone())
    }

    fn keep(&self, key: String, v: &Value) {
        self.cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(key, (Instant::now(), v.clone()));
    }

    /// One read, from the cache when `use_cache` and fresh enough. Errors are never kept.
    fn read(&self, method: &str, params: Value, use_cache: bool) -> Result<Value> {
        let key = format!("{method} {params}");
        if use_cache {
            if let Some(v) = self.cached(&key) {
                return Ok(v);
            }
        }
        let v = self.call(method, params)?;
        self.keep(key, &v);
        Ok(v)
    }

    /// Many `getAccountView` reads: cached ones from the cache, the rest in batches of
    /// MAX_BATCH (one by one once the sequencer has refused a batch).
    fn views(&self, params: &[Value], use_cache: bool) -> Result<Vec<Value>> {
        let keys: Vec<String> = params.iter().map(|p| format!("getAccountView {p}")).collect();
        let mut out: Vec<Option<Value>> = keys.iter().map(|k| if use_cache { self.cached(k) } else { None }).collect();
        let missing: Vec<usize> = (0..params.len()).filter(|i| out[*i].is_none()).collect();
        for chunk in missing.chunks(MAX_BATCH) {
            let refused = *self.batches_refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let got = if chunk.len() > 1 && !refused { self.batch(chunk.iter().map(|i| &params[*i]))? } else { None };
            match got {
                Some(results) => {
                    for (i, v) in chunk.iter().zip(results) {
                        self.keep(keys[*i].clone(), &v);
                        out[*i] = Some(v);
                    }
                }
                None => {
                    for i in chunk {
                        out[*i] = Some(self.read("getAccountView", params[*i].clone(), false)?);
                        self.keep(keys[*i].clone(), out[*i].as_ref().expect("just set"));
                    }
                }
            }
        }
        Ok(out.into_iter().map(|v| v.expect("filled")).collect())
    }

    /// One batch of `getAccountView`: the results in order, or `None` when the sequencer
    /// refuses batches (remembered).
    fn batch<'a>(&self, params: impl Iterator<Item = &'a Value>) -> Result<Option<Vec<Value>>> {
        let params: Vec<&Value> = params.collect();
        let body: Vec<Value> = params.iter().enumerate()
            .map(|(i, p)| json!({"jsonrpc": "2.0", "id": i, "method": "getAccountView", "params": p}))
            .collect();
        let (status, text) = self.post(&Value::Array(body))?;
        let refuse = || {
            *self.batches_refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
            Ok(None)
        };
        if BATCH_REFUSED.contains(&status) {
            return refuse();
        }
        if !(200..300).contains(&status) {
            return Err(self.transport(refusal(status, &text)));
        }
        let reply: Value = serde_json::from_str(&text).map_err(|_| self.transport(format!("not JSON-RPC: {}", plain(&text, 200))))?;
        let Value::Array(replies) = reply else { return refuse() };
        let mut out = vec![None; params.len()];
        for r in &replies {
            let i = r.get("id").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).filter(|i| *i < params.len());
            let Some(i) = i else {
                return Err(Error::Rpc { method: "getAccountView".into(), message: "batch reply with an unknown id".into() });
            };
            out[i] = Some(result_of("getAccountView", r)?);
        }
        out.into_iter()
            .map(|v| v.ok_or_else(|| Error::Rpc { method: "getAccountView".into(), message: "batch reply missing an item".into() }))
            .collect::<Result<Vec<Value>>>()
            .map(Some)
    }
}

/// The module's reader of one sequencer: cached reads ([`Seq::cached`]) or fresh ones for
/// writes ([`Seq::fresh`]); both paced through the same shared client.
#[derive(Clone)]
pub struct Seq {
    net: Arc<Net>,
    use_cache: bool,
}

impl Seq {
    pub fn cached(url: &str) -> Self {
        Self { net: net(url), use_cache: true }
    }
    pub fn fresh(url: &str) -> Self {
        Self { net: net(url), use_cache: false }
    }
    /// One call that is not a read of state (fees, the clock): never cached.
    pub fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.net.call(method, params)
    }
}

fn actor_param(account: &AccountId, program: &AccountId) -> Value {
    json!([{"account_id": account.to_base58(), "program_account_id": program.to_base58()}])
}

fn state_of(v: &Value, program: &AccountId) -> Result<Vec<u8>> {
    Ok(parse_account(v)?.map(|a: AccountStates| a.state(program).to_vec()).unwrap_or_default())
}

impl Rpc for Seq {
    fn get_account(&self, id: &AccountId) -> Result<Option<AccountStates>> {
        parse_account(&self.net.read("getAccount", json!([id.to_base58()]), self.use_cache)?)
    }

    fn get_actor_state(&self, account: &AccountId, program: &AccountId) -> Result<Vec<u8>> {
        state_of(&self.net.read("getAccountView", actor_param(account, program), self.use_cache)?, program)
    }

    fn get_actor_states(&self, actors: &[(AccountId, AccountId)]) -> Result<Vec<Vec<u8>>> {
        let params: Vec<Value> = actors.iter().map(|(a, p)| actor_param(a, p)).collect();
        let views = self.net.views(&params, self.use_cache)?;
        actors.iter().zip(views).map(|((_, p), v)| state_of(&v, p)).collect()
    }
}

fn result_of(method: &str, reply: &Value) -> Result<Value> {
    if let Some(error) = reply.get("error") {
        return Err(Error::Rpc { method: method.to_owned(), message: plain(&error.to_string(), 200) });
    }
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

/// How long a 429 asks to wait: `Retry-After` in seconds (an HTTP date counts as the default),
/// clamped to 1 s .. MAX_RETRY_AFTER.
pub fn retry_after_of(header: Option<&str>) -> Duration {
    header
        .and_then(|h| h.trim().parse::<u64>().ok())
        .map_or(DEFAULT_RETRY_AFTER, |s| Duration::from_secs(s.clamp(1, MAX_RETRY_AFTER.as_secs())))
}

/// "HTTP <status>: <the body as plain text, at most 200 characters>".
pub fn refusal(status: u16, body: &str) -> String {
    let text = plain(body, 200);
    if text.is_empty() { format!("HTTP {status}") } else { format!("HTTP {status}: {text}") }
}

/// Text with markup, comments, scripts and styles removed, spaces collapsed, at most `n`
/// characters: what a front end's error page says, never the page.
pub fn plain(s: &str, n: usize) -> String {
    let mut out = String::new();
    let lower = s.to_ascii_lowercase();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        if b[i] == b'<' {
            // Skip a whole <script>…</script> or <style>…</style>, a comment, or one tag.
            let rest = &lower[i..];
            let end = if rest.starts_with("<!--") {
                rest.find("-->").map(|e| e + 3)
            } else if rest.starts_with("<script") {
                rest.find("</script>").map(|e| e + 9)
            } else if rest.starts_with("<style") {
                rest.find("</style>").map(|e| e + 8)
            } else {
                rest.find('>').map(|e| e + 1)
            };
            i += end.unwrap_or(rest.len());
            out.push(' ');
            continue;
        }
        let ch = s[i..].chars().next().expect("in bounds");
        out.push(if ch.is_control() { ' ' } else { ch });
        i += ch.len_utf8();
    }
    let words = out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "‹").replace("&gt;", "›");
    let collapsed = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = collapsed.chars().take(n).collect();
    cut.replace(['<', '>'], "")
}

/// What a failed read means for a person: `(code, words, detail, retry_after_ms)`, or `None`
/// for an error that is not about reaching the sequencer.
pub fn classify(message: &str) -> Option<(&'static str, &'static str, String, Option<u64>)> {
    if let Some(at) = message.find(RATE_LIMITED) {
        let rest = &message[at + RATE_LIMITED.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let detail = plain(rest[digits.len()..].trim(), 200);
        return Some(("rate_limited", "The network is busy. Trying again in a few seconds.", if detail.is_empty() { "HTTP 429".into() } else { detail }, digits.parse().ok()));
    }
    if let Some(at) = message.find("cannot reach the sequencer") {
        let rest = &message[at..];
        let detail = rest.split_once(": ").map_or(rest, |(_, d)| d);
        return Some(("unreachable", "Can't reach the network right now.", plain(detail, 200), None));
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
    };

    /// Cloudflare's rate-limit page, as the public sequencer's front end sends it.
    pub const CLOUDFLARE_429: &str = "<!doctype html><!--[if lt IE 7]> <html class=\"no-js ie6 oldie\" lang=\"en-US\"> <![endif]--><head><title>Access denied | lez.nameswallet.app used Cloudflare to restrict access</title><style>body{color:red}</style><script>var a=1;</script></head><body><h1>Error 1015</h1><p>You are being rate limited</p></body></html>";

    /// A one-thread HTTP server answering every request with `status`, `headers` and `body`;
    /// its URL and the number of requests it got.
    pub fn serve(status: u16, headers: &'static str, body: &'static str) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/", listener.local_addr().expect("addr"));
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 65536];
                let mut got = Vec::new();
                // Read the head, then the body by Content-Length.
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 { break; }
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if got.len() >= h + 4 + len { break; }
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}", body.len());
                let _ = s.write_all(reply.as_bytes());
            }
        });
        (url, hits)
    }

    #[test]
    fn a_429_is_plain_words_and_starts_a_quiet_window() {
        let (url, hits) = serve(429, "Retry-After: 7\r\n", CLOUDFLARE_429);
        let s = Seq::cached(&url);
        let a = AccountId::from_base58("7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82").expect("id");
        let e = s.get_actor_state(&a, &a).expect_err("429").to_string();
        let (code, words, detail, wait) = classify(&e).expect("classified");
        assert_eq!((code, words, wait), ("rate_limited", "The network is busy. Trying again in a few seconds.", Some(7000)));
        assert!(detail.starts_with("HTTP 429: Access denied"), "{detail}");
        for bad in ["<", ">", "doctype", "script", "var a", "color:red"] {
            assert!(!detail.to_lowercase().contains(bad), "{bad} in {detail}");
        }
        assert!(detail.chars().count() <= 210);
        // Inside the window nothing more is sent, and the answer says how long is left.
        let again = s.get_account(&a).expect_err("still quiet").to_string();
        assert_eq!(hits.load(Ordering::SeqCst), 1, "no request inside the quiet window");
        let (code, _, _, left) = classify(&again).expect("classified");
        assert_eq!(code, "rate_limited");
        assert!(left.is_some_and(|ms| ms > 0 && ms <= 7000));
    }

    #[test]
    fn a_429_without_retry_after_waits_the_default_and_long_asks_are_clamped() {
        assert_eq!(retry_after_of(None), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after_of(Some("Wed, 21 Oct 2026 07:28:00 GMT")), DEFAULT_RETRY_AFTER);
        assert_eq!(retry_after_of(Some("3600")), MAX_RETRY_AFTER);
        assert_eq!(retry_after_of(Some("0")), Duration::from_secs(1));
    }

    #[test]
    fn a_5xx_is_cant_reach_with_the_page_stripped() {
        let (url, _) = serve(502, "", "<html><body><h1>502 Bad Gateway</h1></body></html>");
        let e = Seq::fresh(&url).call("getFeeState", json!([])).expect_err("502").to_string();
        let (code, words, detail, wait) = classify(&e).expect("classified");
        assert_eq!((code, words, detail.as_str(), wait), ("unreachable", "Can't reach the network right now.", "HTTP 502: 502 Bad Gateway", None));
    }

    #[test]
    fn nothing_listening_is_cant_reach() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}/", listener.local_addr().expect("addr"));
        drop(listener);
        let e = Seq::fresh(&url).call("getFeeState", json!([])).expect_err("closed").to_string();
        assert_eq!(classify(&e).map(|c| c.0), Some("unreachable"));
    }

    #[test]
    fn reads_are_cached_and_forgotten() {
        let (url, hits) = serve(200, "", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":null}");
        let s = Seq::cached(&url);
        let a = AccountId::from_base58("7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82").expect("id");
        assert!(s.get_account(&a).expect("read").is_none());
        assert!(s.get_account(&a).expect("cached").is_none());
        assert_eq!(hits.load(Ordering::SeqCst), 1, "the second read came from the cache");
        Seq::fresh(&url).get_account(&a).expect("fresh");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "a fresh read always asks");
        net(&url).forget();
        s.get_account(&a).expect("after forget");
        assert_eq!(hits.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn at_most_three_requests_a_second() {
        let (url, hits) = serve(200, "", "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}");
        let s = Seq::fresh(&url);
        let start = Instant::now();
        for _ in 0..7 {
            s.call("getFeeState", json!([])).expect("ok");
        }
        assert_eq!(hits.load(Ordering::SeqCst), 7);
        assert!(start.elapsed() >= Duration::from_millis(1900), "7 requests at 3 a second take 2 s, took {:?}", start.elapsed());
    }

    #[test]
    fn plain_text_from_markup() {
        assert_eq!(plain(CLOUDFLARE_429, 200), "Access denied | lez.nameswallet.app used Cloudflare to restrict access Error 1015 You are being rate limited");
        assert_eq!(plain("a <b>bold</b> & <i", 200), "a bold &");
        assert_eq!(plain(&"x".repeat(500), 200).len(), 200);
    }
}
