//! IDL-driven Borsh encoding and decoding of messages, state and events.
//!
//! The registry stores whatever IDL an author publishes, so this codec reads three shapes without
//! depending on any program's Rust types: SPEL IDL v1 (`instructions[]`, `accounts[]`), SPEL IDL
//! v2 (`version: "2"`, `messages[]`, `state`, `events`), and v1 plus the registry's `x-lez04`
//! extension (state and event layouts live there). Format detection is shared with
//! [`crate::idl_format`]; this module is the byte-level half the wallet and explorer need.
//!
//! JSON values follow the LEZ CLI conventions: integers up to u64 as numbers, u128/i128 as decimal
//! strings, `account_id` and `program_id` as base58 or hex, `[u8; N]` and `vec<u8>` as lowercase
//! hex, other arrays/vecs as JSON arrays, `option` as `null` or the value, structs as objects keyed
//! by field name, enums as `{"Variant": {…}}` (a unit variant is just `"Variant"`).

use serde_json::{Map, Value, json};

use crate::idl_format;

type R<T> = Result<T, String>;

/// An IDL document, in any of the formats this SDK knows.
#[derive(Clone, Copy)]
pub struct Idl<'a> {
    pub root: &'a Value,
}

/// One message a client can encode, in either format.
#[derive(Debug, Clone, PartialEq)]
pub struct Message<'a> {
    pub name: &'a str,
    /// The leading Borsh byte (v2 `tag`, or v1 `discriminator[0]`).
    pub tag: u8,
    /// `(field name, IDL type)` in wire order.
    pub fields: Vec<(&'a str, &'a Value)>,
}

impl<'a> Idl<'a> {
    #[must_use]
    pub const fn new(root: &'a Value) -> Self {
        Self { root }
    }

    /// The format this SDK detects for the document.
    pub fn format(&self) -> R<idl_format::IdlFormat> {
        idl_format::detect(self.root)
            .map(|i| i.format)
            .map_err(|e| e.to_string())
    }

    /// Every message/instruction, in wire order.
    pub fn messages(&self) -> R<Vec<Message<'a>>> {
        let v2 = self.root["version"].as_str() == Some("2");
        let list = if v2 { "messages" } else { "instructions" };
        let items = self.root[list]
            .as_array()
            .ok_or_else(|| format!("no {list} array in the IDL"))?;
        items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let name = item["name"]
                    .as_str()
                    .ok_or_else(|| format!("{list}[{i}] has no name"))?;
                let tag = if v2 {
                    item["tag"].as_u64().unwrap_or(i as u64)
                } else {
                    match item["discriminator"].as_array() {
                        Some(d) if d.len() == 1 => d[0].as_u64().unwrap_or(i as u64),
                        _ => i as u64,
                    }
                };
                let tag =
                    u8::try_from(tag).map_err(|_| format!("{name}: tag does not fit a byte"))?;
                let field_key = if v2 { "fields" } else { "args" };
                let fields = match item[field_key].as_array() {
                    Some(fs) => fs
                        .iter()
                        .map(|f| {
                            Ok((
                                f["name"]
                                    .as_str()
                                    .ok_or_else(|| format!("{name}: unnamed field"))?,
                                f.get("type")
                                    .ok_or_else(|| format!("{name}: field without a type"))?,
                            ))
                        })
                        .collect::<R<Vec<_>>>()?,
                    None => Vec::new(),
                };
                Ok(Message { name, tag, fields })
            })
            .collect()
    }

    /// Find a message by its variant name, CLI kebab-case name or snake_case name.
    pub fn message(&self, want: &str) -> R<Message<'a>> {
        let all = self.messages()?;
        all.into_iter()
            .find(|m| {
                m.name == want
                    || m.name.eq_ignore_ascii_case(want)
                    || kebab(m.name) == want
                    || m.name.replace('_', "-") == want
            })
            .ok_or_else(|| format!("no message {want} in the IDL"))
    }

    /// The actor state's type name, where each format records it.
    pub fn state_type(&self) -> Option<&'a str> {
        self.root["state"]["defined"].as_str().or_else(|| {
            // v1: the `Entry` account is the deployer entry (DESIGN §2.3); x-lez04 names the rest.
            self.root["accounts"].as_array()?.first()?["name"].as_str()
        })
    }

    /// `(event name, selector hex, payload type name)` in IDL order.
    pub fn events(&self) -> Vec<(&'a str, &'a str, &'a str)> {
        let v2 = self.root["version"].as_str() == Some("2");
        let list = if v2 {
            self.root["events"].as_array()
        } else {
            self.root["x-lez04"]["events"].as_array()
        };
        list.map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|e| {
                let name = e["name"].as_str()?;
                let selector = e["selector"].as_str()?;
                let ty = if v2 {
                    e["type"]["defined"].as_str()
                } else {
                    e["type"].as_str()
                }?;
                Some((name, selector, ty))
            })
            .collect()
    }

    /// `[tag] ‖ borsh(args…)` from a JSON object of named arguments.
    pub fn encode_message(&self, name: &str, args: &Value) -> R<Vec<u8>> {
        let m = self.message(name)?;
        let mut out = vec![m.tag];
        for (field, ty) in &m.fields {
            let value = args
                .get(*field)
                .ok_or_else(|| format!("{}: missing field {field}", m.name))?;
            self.encode(ty, value, &mut out)
                .map_err(|e| format!("{}.{field}: {e}", m.name))?;
        }
        Ok(out)
    }

    /// `(message name, named args)` from wire bytes; every byte must be consumed.
    pub fn decode_message(&self, bytes: &[u8]) -> R<(String, Value)> {
        let (&tag, rest) = bytes.split_first().ok_or("empty message")?;
        let m = self
            .messages()?
            .into_iter()
            .find(|m| m.tag == tag)
            .ok_or_else(|| format!("unknown message tag {tag}"))?;
        let mut rest = rest;
        let mut args = Map::new();
        for (field, ty) in &m.fields {
            args.insert((*field).into(), self.decode(ty, &mut rest)?);
        }
        if !rest.is_empty() {
            return Err(format!("{} trailing bytes", rest.len()));
        }
        Ok((m.name.to_owned(), Value::Object(args)))
    }

    /// Encode a named state, event payload or other type from `types[]`/`accounts[]`.
    pub fn encode_named(&self, name: &str, value: &Value) -> R<Vec<u8>> {
        let mut out = Vec::new();
        self.encode(&json!({ "defined": name }), value, &mut out)?;
        Ok(out)
    }

    /// Decode a named state, event payload or other type; every byte must be consumed.
    pub fn decode_named(&self, name: &str, bytes: &[u8]) -> R<Value> {
        let mut rest = bytes;
        let value = self.decode(&json!({ "defined": name }), &mut rest)?;
        if !rest.is_empty() {
            return Err(format!("{} trailing bytes", rest.len()));
        }
        Ok(value)
    }

    /// Encode one IDL-typed value, the same way Borsh does.
    pub fn encode(&self, ty: &Value, value: &Value, out: &mut Vec<u8>) -> R<()> {
        if let Some(p) = ty.as_str() {
            return self.encode_primitive(p, value, out);
        }
        let obj = ty
            .as_object()
            .ok_or_else(|| format!("unsupported type {ty}"))?;
        if let Some(inner) = obj.get("option") {
            if value.is_null() {
                out.push(0);
            } else {
                out.push(1);
                self.encode(inner, value, out)?;
            }
            return Ok(());
        }
        if let Some(inner) = obj.get("vec") {
            if inner == "u8" {
                let bytes = hex_bytes(value)?;
                out.extend_from_slice(
                    &u32::try_from(bytes.len())
                        .map_err(|_| "vector too long")?
                        .to_le_bytes(),
                );
                out.extend_from_slice(&bytes);
                return Ok(());
            }
            let items = value.as_array().ok_or("expected an array")?;
            out.extend_from_slice(
                &u32::try_from(items.len())
                    .map_err(|_| "vector too long")?
                    .to_le_bytes(),
            );
            for item in items {
                self.encode(inner, item, out)?;
            }
            return Ok(());
        }
        if let Some(arr) = obj.get("array") {
            let inner = &arr[0];
            let n = arr[1].as_u64().ok_or("array length")? as usize;
            if inner == "u8" {
                let bytes = hex_bytes(value)?;
                if bytes.len() != n {
                    return Err(format!("expected {n} bytes, got {}", bytes.len()));
                }
                out.extend_from_slice(&bytes);
                return Ok(());
            }
            let items = value.as_array().ok_or("expected an array")?;
            if items.len() != n {
                return Err(format!("expected {n} items, got {}", items.len()));
            }
            for item in items {
                self.encode(inner, item, out)?;
            }
            return Ok(());
        }
        if let Some(name) = obj.get("defined").and_then(Value::as_str) {
            return self.encode_defined(name, value, out);
        }
        Err(format!("unsupported type {ty}"))
    }

    /// Decode one IDL-typed value from a byte slice.
    pub fn decode(&self, ty: &Value, bytes: &mut &[u8]) -> R<Value> {
        if let Some(p) = ty.as_str() {
            return self.decode_primitive(p, bytes);
        }
        let obj = ty
            .as_object()
            .ok_or_else(|| format!("unsupported type {ty}"))?;
        if let Some(inner) = obj.get("option") {
            return match take(bytes, 1)?[0] {
                0 => Ok(Value::Null),
                1 => self.decode(inner, bytes),
                x => Err(format!("bad option tag {x}")),
            };
        }
        if let Some(inner) = obj.get("vec") {
            let n = u32::from_le_bytes(take(bytes, 4)?.try_into().expect("4")) as usize;
            if inner == "u8" {
                return Ok(json!(hex(take(bytes, n)?)));
            }
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(self.decode(inner, bytes)?);
            }
            return Ok(Value::Array(items));
        }
        if let Some(arr) = obj.get("array") {
            let inner = &arr[0];
            let n = arr[1].as_u64().ok_or("array length")? as usize;
            if inner == "u8" {
                return Ok(json!(hex(take(bytes, n)?)));
            }
            let mut items = Vec::with_capacity(n);
            for _ in 0..n {
                items.push(self.decode(inner, bytes)?);
            }
            return Ok(Value::Array(items));
        }
        if let Some(name) = obj.get("defined").and_then(Value::as_str) {
            return self.decode_defined(name, bytes);
        }
        Err(format!("unsupported type {ty}"))
    }

    fn typedef(&self, name: &str) -> R<&'a Value> {
        // v2 and v1 both have `types[]`; v1 state records are `accounts[]` with a nested `type`.
        let from_types = self.root["types"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == name));
        if let Some(t) = from_types {
            return if t.get("type").is_some() && t.get("kind").is_none() {
                Ok(&t["type"])
            } else {
                Ok(t)
            };
        }
        if let Some(t) = self.root["accounts"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == name))
        {
            return Ok(&t["type"]);
        }
        if let Some(t) = self.root["x-lez04"]["event_types"]
            .as_array()
            .and_then(|a| a.iter().find(|t| t["name"] == name))
        {
            return Ok(t);
        }
        Err(format!("type {name} is not defined in the IDL"))
    }

    fn encode_defined(&self, name: &str, value: &Value, out: &mut Vec<u8>) -> R<()> {
        let def = self.typedef(name)?;
        match def["kind"].as_str() {
            Some("struct") => {
                for f in def["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                    let field = f["name"].as_str().ok_or("field name")?;
                    let v = value
                        .get(field)
                        .ok_or_else(|| format!("{name}: missing field {field}"))?;
                    self.encode(&f["type"], v, out)
                        .map_err(|e| format!("{name}.{field}: {e}"))?;
                }
                Ok(())
            }
            Some("enum") => {
                let obj = value
                    .as_object()
                    .ok_or("enum value must be {\"Variant\": {...}}")?;
                let (variant, fields) = obj.iter().next().ok_or("empty enum value")?;
                let variants = def["variants"].as_array().ok_or("variants")?;
                let (index, def_v) = variants
                    .iter()
                    .enumerate()
                    .find(|(_, v)| v["name"] == *variant)
                    .ok_or("unknown variant")?;
                out.push(u8::try_from(index).map_err(|_| "variant index")?);
                for f in def_v["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                    let field = f["name"].as_str().ok_or("field name")?;
                    self.encode(&f["type"], fields.get(field).ok_or("missing field")?, out)?;
                }
                Ok(())
            }
            _ => Err(format!("type {name}: unsupported kind")),
        }
    }

    fn decode_defined(&self, name: &str, bytes: &mut &[u8]) -> R<Value> {
        let def = self.typedef(name)?;
        match def["kind"].as_str() {
            Some("struct") => {
                let mut out = Map::new();
                for f in def["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                    let field = f["name"].as_str().ok_or("field name")?;
                    out.insert(field.into(), self.decode(&f["type"], bytes)?);
                }
                Ok(Value::Object(out))
            }
            Some("enum") => {
                let index = take(bytes, 1)?[0] as usize;
                let variant = def["variants"]
                    .as_array()
                    .ok_or("variants")?
                    .get(index)
                    .ok_or("variant index out of range")?;
                let name = variant["name"].as_str().ok_or("variant name")?;
                let mut fields = Map::new();
                for f in variant["fields"].as_array().map_or(&[][..], Vec::as_slice) {
                    let field = f["name"].as_str().ok_or("field name")?;
                    fields.insert(field.into(), self.decode(&f["type"], bytes)?);
                }
                if fields.is_empty() {
                    Ok(json!(name))
                } else {
                    Ok(json!({ name: Value::Object(fields) }))
                }
            }
            _ => Err(format!("type {name}: unsupported kind")),
        }
    }

    fn encode_primitive(&self, p: &str, value: &Value, out: &mut Vec<u8>) -> R<()> {
        match p {
            "u8" => out.push(
                u8::try_from(value.as_u64().ok_or("u8 expected")?)
                    .map_err(|_| "u8 out of range")?,
            ),
            "u16" => out.extend_from_slice(
                &u16::try_from(value.as_u64().ok_or("u16 expected")?)
                    .map_err(|_| "u16 out of range")?
                    .to_le_bytes(),
            ),
            "u32" => out.extend_from_slice(
                &u32::try_from(value.as_u64().ok_or("u32 expected")?)
                    .map_err(|_| "u32 out of range")?
                    .to_le_bytes(),
            ),
            "u64" => out.extend_from_slice(&value.as_u64().ok_or("u64 expected")?.to_le_bytes()),
            "u128" => out.extend_from_slice(&num(value, "u128")?.to_le_bytes()),
            "i8" => out.push(
                i8::try_from(value.as_i64().ok_or("i8 expected")?).map_err(|_| "i8 out of range")?
                    as u8,
            ),
            "i16" => out.extend_from_slice(
                &i16::try_from(value.as_i64().ok_or("i16 expected")?)
                    .map_err(|_| "i16 out of range")?
                    .to_le_bytes(),
            ),
            "i32" => out.extend_from_slice(
                &i32::try_from(value.as_i64().ok_or("i32 expected")?)
                    .map_err(|_| "i32 out of range")?
                    .to_le_bytes(),
            ),
            "i64" => out.extend_from_slice(&value.as_i64().ok_or("i64 expected")?.to_le_bytes()),
            "i128" => out.extend_from_slice(
                &i128::from_str_radix(value.as_str().ok_or("i128 as string")?, 10)
                    .map_err(|e| e.to_string())?
                    .to_le_bytes(),
            ),
            "bool" => out.push(u8::from(value.as_bool().ok_or("bool expected")?)),
            "string" => {
                let s = value.as_str().ok_or("string expected")?;
                out.extend_from_slice(
                    &u32::try_from(s.len())
                        .map_err(|_| "string too long")?
                        .to_le_bytes(),
                );
                out.extend_from_slice(s.as_bytes());
            }
            "account_id" | "program_id" => {
                let bytes = account_bytes(value)?;
                if bytes.len() != 32 {
                    return Err(format!("{p} is 32 bytes, got {}", bytes.len()));
                }
                out.extend_from_slice(&bytes);
            }
            other if other.starts_with("option<") || other.starts_with("vec<") => {
                return Err(format!("use the object form for {other}"));
            }
            other => return Err(format!("unsupported primitive {other}")),
        }
        Ok(())
    }

    fn decode_primitive(&self, p: &str, bytes: &mut &[u8]) -> R<Value> {
        match p {
            "u8" => Ok(json!(take(bytes, 1)?[0])),
            "u16" => Ok(json!(u16::from_le_bytes(
                take(bytes, 2)?.try_into().expect("2")
            ))),
            "u32" => Ok(json!(u32::from_le_bytes(
                take(bytes, 4)?.try_into().expect("4")
            ))),
            "u64" => Ok(json!(u64::from_le_bytes(
                take(bytes, 8)?.try_into().expect("8")
            ))),
            "u128" => Ok(json!(
                u128::from_le_bytes(take(bytes, 16)?.try_into().expect("16")).to_string()
            )),
            "i8" => Ok(json!(i8::from_le_bytes([take(bytes, 1)?[0]]) as i64)),
            "i16" => Ok(json!(i16::from_le_bytes(
                take(bytes, 2)?.try_into().expect("2")
            ))),
            "i32" => Ok(json!(i32::from_le_bytes(
                take(bytes, 4)?.try_into().expect("4")
            ))),
            "i64" => Ok(json!(i64::from_le_bytes(
                take(bytes, 8)?.try_into().expect("8")
            ))),
            "i128" => Ok(json!(
                i128::from_le_bytes(take(bytes, 16)?.try_into().expect("16")).to_string()
            )),
            "bool" => Ok(json!(take(bytes, 1)?[0] != 0)),
            "string" => {
                let n = u32::from_le_bytes(take(bytes, 4)?.try_into().expect("4")) as usize;
                Ok(json!(
                    String::from_utf8(take(bytes, n)?.to_vec()).map_err(|e| e.to_string())?
                ))
            }
            "account_id" | "program_id" => Ok(json!(bs58::encode(take(bytes, 32)?).into_string())),
            other => Err(format!("unsupported primitive {other}")),
        }
    }
}

fn num(value: &Value, what: &str) -> R<u128> {
    if let Some(s) = value.as_str() {
        return s.parse::<u128>().map_err(|e| format!("{what}: {e}"));
    }
    value
        .as_u64()
        .map(u128::from)
        .ok_or_else(|| format!("{what} expected"))
}

fn account_bytes(value: &Value) -> R<Vec<u8>> {
    let s = value.as_str().ok_or("an account id is a string")?;
    if let Some(hex) = s.strip_prefix("0x") {
        return hex::decode(hex).map_err(|e| e.to_string());
    }
    if s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()) {
        return hex::decode(s).map_err(|e| e.to_string());
    }
    bs58::decode(s).into_vec().map_err(|e| e.to_string())
}

fn hex_bytes(value: &Value) -> R<Vec<u8>> {
    let s = value.as_str().ok_or("expected lowercase hex")?;
    hex::decode(s.strip_prefix("0x").unwrap_or(s)).map_err(|e| e.to_string())
}

fn take<'a>(bytes: &mut &'a [u8], n: usize) -> R<&'a [u8]> {
    if bytes.len() < n {
        return Err(format!("not enough bytes: need {n}, have {}", bytes.len()));
    }
    let (head, tail) = bytes.split_at(n);
    *bytes = tail;
    Ok(head)
}

fn hex(bytes: &[u8]) -> String {
    ::hex::encode(bytes)
}

fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('-');
            }
            for l in c.to_lowercase() {
                out.push(l);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = include_str!("../../../idl/lez_program_registry.idl.json");
    const V2: &str = include_str!("../../../idl/lez_program_registry.v2.json");

    fn json_of(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn detects_both_idl_formats() {
        let one = json_of(V1);
        let two = json_of(V2);
        assert_eq!(
            Idl::new(&one).format().unwrap(),
            idl_format::IdlFormat::SpelV1
        );
        assert_eq!(
            Idl::new(&two).format().unwrap(),
            idl_format::IdlFormat::SpelV2
        );
    }

    #[test]
    fn the_two_idls_describe_the_same_message_bytes() {
        let one_json = json_of(V1);
        let two_json = json_of(V2);
        let v1 = Idl::new(&one_json);
        let v2 = Idl::new(&two_json);
        let one = v1.messages().unwrap();
        let two = v2.messages().unwrap();
        assert_eq!(one.len(), two.len());
        for (a, b) in one.iter().zip(two.iter()) {
            assert_eq!(a.tag, b.tag, "{} vs {}", a.name, b.name);
            assert_eq!(
                a.fields.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                b.fields.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
                "{} vs {}",
                a.name,
                b.name,
            );
            for ((an, at), (bn, bt)) in a.fields.iter().zip(b.fields.iter()) {
                assert_eq!(
                    crate::idl_format::type_label(at),
                    crate::idl_format::type_label(bt),
                    "{an} vs {bn}"
                );
            }
        }
    }

    #[test]
    fn encodes_and_decodes_a_message_in_both_formats() {
        let args = json!({
            "entry_id": "0x0101010101010101010101010101010101010101010101010101010101010101",
            "now": 1_700_000_000u64,
            "via_pda_seed": null
        });
        let mut encodings = Vec::new();
        for text in [V1, V2] {
            let json = json_of(text);
            let idl = Idl::new(&json);
            let bytes = idl.encode_message("freeze", &args).unwrap();
            assert_eq!(bytes[0], 4, "Freeze's wire tag");
            let (name, back) = idl.decode_message(&bytes).unwrap();
            assert_eq!(name, "freeze");
            assert_eq!(back["now"], json!(1_700_000_000u64));
            assert_eq!(back["via_pda_seed"], Value::Null);
            encodings.push(bytes);
        }
        assert_eq!(
            encodings[0], encodings[1],
            "v1 and v2 encode the same bytes"
        );
    }

    #[test]
    fn state_and_events_have_the_same_layout_in_both_formats() {
        let one_json = json_of(V1);
        let two_json = json_of(V2);
        let v1 = Idl::new(&one_json);
        let v2 = Idl::new(&two_json);
        assert_eq!(v1.state_type(), Some("Entry"));
        assert_eq!(v2.state_type(), Some("Entry"));
        assert_eq!(v1.events().len(), 4);
        assert_eq!(v1.events(), v2.events());
        for (name, selector, ty) in v1.events() {
            assert_eq!(selector.len(), 16, "{name}");
            assert_eq!(v2.typedef(ty).unwrap()["kind"], "struct");
        }
    }

    #[test]
    fn a_type_round_trips_through_borsh_shape() {
        let json = json_of(V2);
        let idl = Idl::new(&json);
        let head = json!({
            "schema": 1u64,
            "count": 3u64,
            "newest": "0x0202020202020202020202020202020202020202020202020202020202020202"
        });
        let bytes = idl.encode_named("Head", &head).unwrap();
        assert_eq!(
            bytes.len(),
            42,
            "Head is 42 bytes with a newest entry (DESIGN §2.3)"
        );
        let back = idl.decode_named("Head", &bytes).unwrap();
        assert_eq!(back["schema"], json!(1));
        assert_eq!(back["count"], json!(3));
        assert_eq!(back["newest"], json!(bs58::encode([2u8; 32]).into_string()));
        let ev =
            json!({"entry": "0x0202020202020202020202020202020202020202020202020202020202020202"});
        let bytes = idl.encode_named("Frozen", &ev).unwrap();
        assert_eq!(bytes.len(), 32);
        let back = idl.decode_named("Frozen", &bytes).unwrap();
        assert_eq!(back["entry"], json!(bs58::encode([2u8; 32]).into_string()));
    }
}
