//! IDL-driven borsh codec over the JSON IDL (SPEL 0.1.0 types). No dependency on the program's
//! Rust types: everything comes from the IDL file.
//!
//! JSON values: integers up to u64 as numbers, u128/i128 as decimal strings, `bool`, strings;
//! `account_id` as base58; `[u8; N]` arrays and `vec<u8>` as lowercase hex; other arrays and
//! vecs as JSON arrays; `option` as `null` or the value; structs as objects in field order.

use serde_json::{Map, Value, json};

pub struct Idl<'a> {
    pub root: &'a Value,
}

type R<T> = Result<T, String>;

impl<'a> Idl<'a> {
    #[must_use]
    pub const fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// A named type from `types[]`, `accounts[]` or `x-lez04.event_types[]`.
    fn typedef(&self, name: &str) -> R<&'a Value> {
        let find = |arr: Option<&'a Value>, inner: bool| -> Option<&'a Value> {
            arr?.as_array()?.iter().find(|t| t["name"] == name).map(|t| if inner { &t["type"] } else { t })
        };
        find(self.root.get("types"), false)
            .or_else(|| find(self.root.get("accounts"), true))
            .or_else(|| find(self.root.get("x-lez04").and_then(|x| x.get("event_types")), false))
            .ok_or_else(|| format!("type {name} is not defined in the IDL"))
    }

    pub fn instruction(&self, name: &str) -> R<(usize, &'a Value)> {
        self.root["instructions"]
            .as_array()
            .ok_or("no instructions")?
            .iter()
            .enumerate()
            .find(|(_, ix)| ix["name"] == name)
            .ok_or_else(|| format!("no instruction {name}"))
    }

    /// `[tag] ‖ borsh(args…)` from a JSON object of args.
    pub fn encode_instruction(&self, name: &str, args: &Value) -> R<Vec<u8>> {
        let (tag, ix) = self.instruction(name)?;
        let mut out = vec![u8::try_from(tag).map_err(|_| "tag overflow")?];
        for a in ix["args"].as_array().ok_or("no args")? {
            let n = a["name"].as_str().ok_or("arg name")?;
            let v = args.get(n).ok_or_else(|| format!("{name}: missing arg {n}"))?;
            self.encode(&a["type"], v, &mut out).map_err(|e| format!("{name}.{n}: {e}"))?;
        }
        Ok(out)
    }

    /// `(instruction name, args)` from wire bytes; all bytes must be consumed.
    pub fn decode_instruction(&self, data: &[u8]) -> R<(String, Value)> {
        let (&tag, mut rest) = data.split_first().ok_or("empty")?;
        let ix = self.root["instructions"].as_array().ok_or("no instructions")?.get(usize::from(tag)).ok_or("unknown tag")?;
        let mut args = Map::new();
        for a in ix["args"].as_array().ok_or("no args")? {
            let n = a["name"].as_str().ok_or("arg name")?;
            args.insert(n.into(), self.decode(&a["type"], &mut rest)?);
        }
        if !rest.is_empty() {
            return Err(format!("{} trailing bytes", rest.len()));
        }
        Ok((ix["name"].as_str().unwrap_or_default().to_string(), Value::Object(args)))
    }

    /// Decodes a named type (record or event) from bytes; all bytes must be consumed.
    pub fn decode_named(&self, name: &str, data: &[u8]) -> R<Value> {
        let mut rest = data;
        let v = self.decode(&json!({"defined": name}), &mut rest)?;
        if !rest.is_empty() {
            return Err(format!("{} trailing bytes", rest.len()));
        }
        Ok(v)
    }

    pub fn encode_named(&self, name: &str, v: &Value) -> R<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(&json!({"defined": name}), v, &mut out)?;
        Ok(out)
    }

    pub fn encode(&self, ty: &Value, v: &Value, out: &mut Vec<u8>) -> R<()> {
        if let Some(p) = ty.as_str() {
            return encode_primitive(p, v, out);
        }
        if let Some(inner) = ty.get("option") {
            if v.is_null() {
                out.push(0);
                return Ok(());
            }
            out.push(1);
            return self.encode(inner, v, out);
        }
        if let Some(inner) = ty.get("vec") {
            if inner == "u8" {
                let b = hex_bytes(v)?;
                out.extend_from_slice(&u32::try_from(b.len()).map_err(|_| "too long")?.to_le_bytes());
                out.extend_from_slice(&b);
                return Ok(());
            }
            let items = v.as_array().ok_or("expected an array")?;
            out.extend_from_slice(&u32::try_from(items.len()).map_err(|_| "too long")?.to_le_bytes());
            for it in items {
                self.encode(inner, it, out)?;
            }
            return Ok(());
        }
        if let Some(arr) = ty.get("array") {
            let (inner, n) = (&arr[0], arr[1].as_u64().ok_or("array length")?);
            let n = usize::try_from(n).map_err(|_| "array length")?;
            if inner == "u8" {
                let b = hex_bytes(v)?;
                if b.len() != n {
                    return Err(format!("expected {n} bytes, got {}", b.len()));
                }
                out.extend_from_slice(&b);
                return Ok(());
            }
            let items = v.as_array().ok_or("expected an array")?;
            if items.len() != n {
                return Err(format!("expected {n} items"));
            }
            for it in items {
                self.encode(inner, it, out)?;
            }
            return Ok(());
        }
        if let Some(name) = ty.get("defined").and_then(Value::as_str) {
            let def = self.typedef(name)?;
            match def["kind"].as_str() {
                Some("struct") => {
                    for f in def["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                        let n = f["name"].as_str().ok_or("field name")?;
                        let fv = v.get(n).ok_or_else(|| format!("{name}: missing field {n}"))?;
                        self.encode(&f["type"], fv, out).map_err(|e| format!("{name}.{n}: {e}"))?;
                    }
                    return Ok(());
                }
                Some("enum") => {
                    let o = v.as_object().ok_or("enum value must be {Variant: {..}}")?;
                    let (vn, fields) = o.iter().next().ok_or("empty enum value")?;
                    let variants = def["variants"].as_array().ok_or("variants")?;
                    let (i, var) = variants.iter().enumerate().find(|(_, x)| x["name"] == *vn).ok_or("unknown variant")?;
                    out.push(u8::try_from(i).map_err(|_| "variant")?);
                    for f in var["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                        let n = f["name"].as_str().ok_or("field name")?;
                        self.encode(&f["type"], fields.get(n).ok_or("missing field")?, out)?;
                    }
                    return Ok(());
                }
                _ => return Err(format!("type {name}: unsupported kind")),
            }
        }
        Err(format!("unsupported IDL type {ty}"))
    }

    pub fn decode(&self, ty: &Value, b: &mut &[u8]) -> R<Value> {
        if let Some(p) = ty.as_str() {
            return decode_primitive(p, b);
        }
        if let Some(inner) = ty.get("option") {
            return match take(b, 1)?[0] {
                0 => Ok(Value::Null),
                1 => self.decode(inner, b),
                x => Err(format!("bad option tag {x}")),
            };
        }
        if let Some(inner) = ty.get("vec") {
            let n = u32::from_le_bytes(take(b, 4)?.try_into().expect("4")) as usize;
            if inner == "u8" {
                return Ok(json!(hex(take(b, n)?)));
            }
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(self.decode(inner, b)?);
            }
            return Ok(Value::Array(items));
        }
        if let Some(arr) = ty.get("array") {
            let (inner, n) = (&arr[0], usize::try_from(arr[1].as_u64().ok_or("array length")?).map_err(|_| "len")?);
            if inner == "u8" {
                return Ok(json!(hex(take(b, n)?)));
            }
            let mut items = Vec::new();
            for _ in 0..n {
                items.push(self.decode(inner, b)?);
            }
            return Ok(Value::Array(items));
        }
        if let Some(name) = ty.get("defined").and_then(Value::as_str) {
            let def = self.typedef(name)?;
            match def["kind"].as_str() {
                Some("struct") => {
                    let mut o = Map::new();
                    for f in def["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                        let n = f["name"].as_str().ok_or("field name")?;
                        o.insert(n.into(), self.decode(&f["type"], b)?);
                    }
                    return Ok(Value::Object(o));
                }
                Some("enum") => {
                    let i = usize::from(take(b, 1)?[0]);
                    let var = def["variants"].as_array().and_then(|v| v.get(i)).ok_or("unknown variant")?;
                    let mut o = Map::new();
                    for f in var["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                        let n = f["name"].as_str().ok_or("field name")?;
                        o.insert(n.into(), self.decode(&f["type"], b)?);
                    }
                    return Ok(json!({ var["name"].as_str().unwrap_or_default(): o }));
                }
                _ => return Err(format!("type {name}: unsupported kind")),
            }
        }
        Err(format!("unsupported IDL type {ty}"))
    }
}

fn take<'b>(b: &mut &'b [u8], n: usize) -> R<&'b [u8]> {
    if b.len() < n {
        return Err("unexpected end of data".into());
    }
    let (h, t) = b.split_at(n);
    *b = t;
    Ok(h)
}

#[must_use]
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_bytes(v: &Value) -> R<Vec<u8>> {
    let s = v.as_str().ok_or("expected a hex string")?;
    if s.len() % 2 != 0 {
        return Err("odd hex length".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

fn int_le(v: &Value, bytes: usize, signed: bool, out: &mut Vec<u8>) -> R<()> {
    let n: i128 = match v {
        Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)).ok_or("not an integer")?,
        Value::String(s) => s.parse::<i128>().map_err(|e| e.to_string())?,
        _ => return Err("expected an integer".into()),
    };
    let le = n.to_le_bytes();
    let fits = if signed {
        let max = (1i128 << (bytes * 8 - 1)) - 1;
        (-(max + 1)..=max).contains(&n)
    } else {
        n >= 0 && (bytes == 16 || n < (1i128 << (bytes * 8)))
    };
    if !fits {
        return Err(format!("{n} out of range"));
    }
    out.extend_from_slice(&le[..bytes]);
    Ok(())
}

fn encode_primitive(p: &str, v: &Value, out: &mut Vec<u8>) -> R<()> {
    match p {
        "u8" => int_le(v, 1, false, out),
        "u16" => int_le(v, 2, false, out),
        "u32" => int_le(v, 4, false, out),
        "u64" => int_le(v, 8, false, out),
        "u128" => {
            let n: u128 = match v {
                Value::String(s) => s.parse().map_err(|_| "u128")?,
                Value::Number(n) => u128::from(n.as_u64().ok_or("u128")?),
                _ => return Err("u128".into()),
            };
            out.extend_from_slice(&n.to_le_bytes());
            Ok(())
        }
        "i8" => int_le(v, 1, true, out),
        "i16" => int_le(v, 2, true, out),
        "i32" => int_le(v, 4, true, out),
        "i64" => int_le(v, 8, true, out),
        "bool" => {
            out.push(u8::from(v.as_bool().ok_or("expected a bool")?));
            Ok(())
        }
        "string" => {
            let s = v.as_str().ok_or("expected a string")?;
            out.extend_from_slice(&u32::try_from(s.len()).map_err(|_| "too long")?.to_le_bytes());
            out.extend_from_slice(s.as_bytes());
            Ok(())
        }
        "account_id" => {
            let a = lezreg_types::AccountId::from_base58(v.as_str().ok_or("expected base58")?).ok_or("bad account id")?;
            out.extend_from_slice(&a.0);
            Ok(())
        }
        other => Err(format!("unsupported primitive {other}")),
    }
}

fn decode_primitive(p: &str, b: &mut &[u8]) -> R<Value> {
    macro_rules! le {
        ($t:ty, $n:expr) => {{
            let x = <$t>::from_le_bytes(take(b, $n)?.try_into().expect("width"));
            x
        }};
    }
    Ok(match p {
        "u8" => json!(take(b, 1)?[0]),
        "u16" => json!(le!(u16, 2)),
        "u32" => json!(le!(u32, 4)),
        "u64" => json!(le!(u64, 8)),
        "u128" => json!(le!(u128, 16).to_string()),
        "i8" => json!(le!(i8, 1)),
        "i16" => json!(le!(i16, 2)),
        "i32" => json!(le!(i32, 4)),
        "i64" => json!(le!(i64, 8)),
        "bool" => match take(b, 1)?[0] {
            0 => json!(false),
            1 => json!(true),
            x => return Err(format!("bad bool {x}")),
        },
        "string" => {
            let n = le!(u32, 4) as usize;
            json!(std::str::from_utf8(take(b, n)?).map_err(|e| e.to_string())?)
        }
        "account_id" => json!(lezreg_types::AccountId(take(b, 32)?.try_into().expect("32")).to_base58()),
        other => return Err(format!("unsupported primitive {other}")),
    })
}
