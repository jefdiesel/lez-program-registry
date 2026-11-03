//! RFC 8785 JSON Canonicalization Scheme, for the IDLs the registry hashes (DESIGN §5.2).
//!
//! Keys are sorted by their UTF-16 code units, there is no whitespace, and strings use the
//! shortest escapes RFC 8785 allows. Numbers: an IDL holds only integers, so integers are written
//! as is and any non-integer number is refused rather than risk an ECMAScript-formatting
//! mismatch with another implementation.

use serde_json::Value;

/// The canonical bytes of `v`.
pub fn canonical(v: &Value) -> Result<Vec<u8>, String> {
    let mut out = String::new();
    write(v, &mut out)?;
    Ok(out.into_bytes())
}

/// Parses `bytes` as JSON (with a nesting limit) and returns its canonical form.
pub fn canonicalize(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let v = crate::idl::parse_limited(bytes)?;
    canonical(&v)
}

fn write(v: &Value, out: &mut String) -> Result<(), String> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return Err(format!("non-integer number {n} has no canonical form here"));
            }
        }
        Value::String(s) => string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out)?;
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(k, out);
                out.push(':');
                write(&o[k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_keys_and_drops_whitespace() {
        let got = canonicalize(br#"{ "b": [1, 2, {"z": null, "a": true}], "a": "x" }"#).unwrap();
        assert_eq!(got, br#"{"a":"x","b":[1,2,{"a":true,"z":null}]}"#);
    }

    #[test]
    fn keys_sort_by_utf16_code_units() {
        // RFC 8785 §3.2.3: "\u{e9}" (U+00E9) sorts before "\u{1f600}" (a surrogate pair, 0xD83D…),
        // but after "z".
        let got = canonicalize("{\"\u{1f600}\":1,\"\u{e9}\":2,\"z\":3}".as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "{\"z\":3,\"\u{e9}\":2,\"\u{1f600}\":1}"
        );
    }

    #[test]
    fn escapes_only_what_rfc_8785_escapes() {
        let got = canonicalize(r#"["\u0001\b\t\n\f\r\"\\\/\u00e9"]"#.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "[\"\\u0001\\b\\t\\n\\f\\r\\\"\\\\/\u{e9}\"]"
        );
    }

    #[test]
    fn refuses_fractions() {
        assert!(canonicalize(b"[1.5]").is_err());
        assert_eq!(
            canonicalize(b"[-3, 18446744073709551615]").unwrap(),
            b"[-3,18446744073709551615]"
        );
    }

    #[test]
    fn is_idempotent() {
        let once = canonicalize(br#"{"x": {"c": 1, "b": [ "s" ] }, "a": -1}"#).unwrap();
        assert_eq!(canonicalize(&once).unwrap(), once);
    }
}
