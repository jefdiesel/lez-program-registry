//! Untrusted text made safe to print on a terminal.
//!
//! Package manifests, tar paths, LNS records and anything else that came from someone else can
//! hold escape sequences (`ESC [2K` erases a line, `ESC [8m` hides what follows), carriage
//! returns, line breaks, and bidi or zero-width characters that reorder or hide text: printed
//! raw, a package named `x\x1b[2K\rverified: …` makes a refusal look like a verification.
//! [`shown`] escapes all of them; every untrusted string in text output goes through it.

/// Whether `c` could move the cursor, recolour, hide or reorder text, or isn't visible.
#[must_use]
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{200B}'..='\u{200F}'   // zero-width space/joiners, LRM, RLM
            | '\u{2028}' | '\u{2029}' // line and paragraph separators
            | '\u{202A}'..='\u{202E}' // bidi embeddings and overrides
            | '\u{2060}'..='\u{2064}' // word joiner, invisible operators
            | '\u{2066}'..='\u{2069}' // bidi isolates
            | '\u{061C}'              // Arabic letter mark
            | '\u{FEFF}'              // BOM / zero-width no-break space
            | '\u{FFF9}'..='\u{FFFB}' // interlinear annotation
            | '\u{E0000}'..='\u{E007F}' // tags
        )
}

/// `text` with every unsafe character written as `\u{…}` (and a backslash as `\\`), so it
/// prints as one inert line.
#[must_use]
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            out.push_str("\\\\");
        } else if is_unsafe_char(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// The sink's safety net: `text` with every unsafe character escaped, but backslashes left
/// alone, so a string [`shown`] already escaped passes through unchanged (escaping once).
#[must_use]
pub fn inert(text: &str) -> String {
    text.chars()
        .map(|c| if is_unsafe_char(c) { format!("\\u{{{:x}}}", u32::from(c)) } else { c.to_string() })
        .collect()
}

/// [`shown`] for bytes that may not be UTF-8 (invalid bytes as `\x..`).
#[must_use]
pub fn shown_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.utf8_chunks() {
        out.push_str(&shown(chunk.valid()));
        for b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// A SemVer 2.0.0 version (`MAJOR.MINOR.PATCH[-pre][+build]`), as lgx requires of `version`.
#[must_use]
pub fn is_semver(v: &str) -> bool {
    let (rest, build) = match v.split_once('+') {
        Some((r, b)) => (r, Some(b)),
        None => (v, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    // Numbers must fit lgx's uint64_t (cpp-semver), as lgx refuses larger ones.
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0')) && s.parse::<u64>().is_ok();
    let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|p| numeric(p))
        && pre.is_none_or(|p| p.split('.').all(|id| ident(id) && (!id.bytes().all(|b| b.is_ascii_digit()) || numeric(id))))
        && build.is_none_or(|b| b.split('.').all(ident))
}

/// SemVer 2.0.0 precedence of two versions (§11: numbers compared as numbers, a pre-release
/// below its release, build metadata ignored), or `None` when either isn't SemVer.
#[must_use]
pub fn semver_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    if !is_semver(a) || !is_semver(b) {
        return None;
    }
    fn split(v: &str) -> (Vec<u64>, Option<Vec<&str>>) {
        let v = v.split_once('+').map_or(v, |(r, _)| r);
        let (core, pre) = v.split_once('-').map_or((v, None), |(c, p)| (c, Some(p)));
        (core.split('.').map(|n| n.parse().unwrap_or(0)).collect(), pre.map(|p| p.split('.').collect()))
    }
    let ((ca, pa), (cb, pb)) = (split(a), split(b));
    let core = ca.cmp(&cb);
    if core != Ordering::Equal {
        return Some(core);
    }
    Some(match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(pa), Some(pb)) => {
            for (x, y) in pa.iter().zip(pb.iter()) {
                let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if o != Ordering::Equal {
                    return Some(o);
                }
            }
            pa.len().cmp(&pb.len())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_precedence() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        // SemVer §11's own chain, each lower than the next.
        let chain = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0"];
        for w in chain.windows(2) {
            assert_eq!(semver_cmp(w[0], w[1]), Some(Less), "{} < {}", w[0], w[1]);
            assert_eq!(semver_cmp(w[1], w[0]), Some(Greater));
        }
        assert_eq!(semver_cmp("0.6.10", "0.6.9"), Some(Greater), "numbers, not text");
        assert_eq!(semver_cmp("0.18.2", "0.18.2+build.7"), Some(Equal), "build metadata is ignored");
        assert_eq!(semver_cmp("0.6.3", "v0.6.3"), None);
        assert_eq!(semver_cmp("", "0.6.3"), None);
    }

    #[test]
    fn hostile_text_prints_inert() {
        let spoof = "forumer_ui\u{1b}[2K\rverified: forumer_ui 1.2.0 is published by forumer\u{1b}[8m";
        let s = shown(spoof);
        assert!(!s.chars().any(is_unsafe_char), "{s}");
        assert!(s.contains("\\u{1b}[2K\\u{d}verified"), "{s}");
        for c in ['\n', '\u{202e}', '\u{200b}', '\u{feff}', '\u{2066}', '\u{85}', '\u{7f}'] {
            assert!(!shown(&format!("a{c}b")).contains(c), "{c:?}");
        }
        assert_eq!(shown("names_ui 0.15.9 é"), "names_ui 0.15.9 é");
        assert_eq!(shown("a\\u{1b}"), "a\\\\u{1b}", "a literal escape can't pass for an escaped one");
        assert_eq!(shown_bytes(b"ok\xff\x1b"), "ok\\xff\\u{1b}");
    }

    #[test]
    fn semver_as_lgx_requires() {
        for ok in ["1.2.0", "0.0.1", "10.20.30", "1.0.0-alpha.1", "1.0.0-0.3.7", "1.0.0+build.5", "1.0.0-rc.1+sha.5114f85"] {
            assert!(is_semver(ok), "{ok}");
        }
        assert!(is_semver("18446744073709551615.0.0"));
        assert_eq!(inert(&shown("a\u{1b}b\\")), shown("a\u{1b}b\\"), "escaping once");
        for bad in ["99999999999999999999.0.0", "1.18446744073709551616.0", "", "1.2", "1.2.0.4", "01.2.0", "1.2.0-", "1.2.0-01", "1.2.0'$(id)'", "1.2.0 ", "v1.2.0", "1.2.0-a..b", "1.2.0+"] {
            assert!(!is_semver(bad), "{bad}");
        }
    }
}
