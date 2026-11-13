//! Which format another program's IDL is in, and a read-only view common to every format.
//!
//! The registry stores whatever IDL an author publishes, and on LEZ 0.4 there is more than one
//! format: SPEL's spec-0.1.0 IDL (`instructions[]`, "v1"), the IDL v2 of SPEL's 0.4 rewrite
//! (logos-co/spel#301: `version: "2"`, `messages[]`, `state`, `events`, `types`), and possibly
//! a `lez-idl` whose spec is not out yet. So an IDL is classified, not required to be one shape:
//!
//! - a known format ([`IdlFormat::SpelV1`], [`IdlFormat::SpelV2`]): its calls can be listed;
//! - [`IdlFormat::Unknown`]: a JSON object in no format listed here. It is still the content
//!   the chain names, so it is *available*; clients show it as "format not recognised";
//! - anything that is not a JSON object is not an IDL at all ([`detect`] returns `Err`).
//!
//! A file only counts as a known format if every part this module reads has the right shape;
//! a file that half-matches is `Unknown`, because a new format may reuse key names.
//!
//! Nothing here encodes or decodes calls. [`calls`] gives each message (v2) or instruction (v1)
//! with its argument names and types: enough for a wallet to label a call.
//!
//! **Adding a format:** write a module below with `claims`, `info` and `calls` functions, add
//! an [`IdlFormat`] variant and its id, and add one line to `FORMATS`. Then mirror it in
//! `sdk/js/lezreg.js` (`IDL_FORMATS`), put a sample under `sdk/fixtures/idl/` and add it to
//! `FIXTURES` in `sdk/rust/tests/js_vectors.rs`, which pins the two SDKs together.

use serde_json::{Map, Value};

/// An IDL format this SDK can tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdlFormat {
    /// SPEL spec 0.1.0 (`spel generate-idl` up to v0.7.0): `instructions[]` with accounts and
    /// args. The registry's own IDL is in this format.
    SpelV1,
    /// SPEL IDL v2 (logos-co/spel#301, LEZ 0.4): `version: "2"`, `messages[]` in Borsh tag
    /// order, `state`, `events`, `types`; no accounts.
    SpelV2,
    /// A JSON object in no format above.
    Unknown,
}

impl IdlFormat {
    /// The stable id used in JSON output and by the JS SDK.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::SpelV1 => "spel-v1",
            Self::SpelV2 => "spel-v2",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

impl std::fmt::Display for IdlFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// What an IDL says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdlInfo {
    pub format: IdlFormat,
    /// The format's version as the file states it: v1's `spec` (stock SPEL v0.7.0 leaves it
    /// out), v2's `version` (`"2"`). `None` if the file does not say, and for `Unknown`.
    pub format_version: Option<String>,
    /// The program name the file states, if it is a string.
    pub name: Option<String>,
    /// The program version the file states: v1's `version`. v2 carries none.
    pub program_version: Option<String>,
}

/// One message (v2) or instruction (v1) a program accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct IdlCall {
    pub name: String,
    /// The leading Borsh byte that selects it: v2's `tag`, or v1's `discriminator` when it is
    /// a single byte. `None` when the IDL does not say.
    pub tag: Option<u64>,
    pub args: Vec<IdlArg>,
}

/// One argument (v1) or field (v2) of a call.
#[derive(Debug, Clone, PartialEq)]
pub struct IdlArg {
    pub name: String,
    /// The type exactly as the IDL writes it.
    pub ty: Value,
    /// [`type_label`] of `ty`, for display.
    pub label: String,
}

type Obj = Map<String, Value>;

/// A format: `claims` decides membership (first match in `FORMATS` wins), `info` and `calls`
/// read a file `claims` accepted, so they may rely on the shape it checked.
struct Format {
    format: IdlFormat,
    claims: fn(&Obj) -> bool,
    info: fn(&Obj) -> IdlInfo,
    calls: fn(&Obj) -> Vec<IdlCall>,
}

const FORMATS: &[Format] = &[
    Format {
        format: IdlFormat::SpelV1,
        claims: spel_v1::claims,
        info: spel_v1::info,
        calls: spel_v1::calls,
    },
    Format {
        format: IdlFormat::SpelV2,
        claims: spel_v2::claims,
        info: spel_v2::info,
        calls: spel_v2::calls,
    },
];

fn format_of(o: &Obj) -> Option<&'static Format> {
    FORMATS.iter().find(|f| (f.claims)(o))
}

/// The format of a parsed IDL. `Err` only if it is not a JSON object (not an IDL at all).
pub fn detect(v: &Value) -> Result<IdlInfo, String> {
    let o = v.as_object().ok_or("not a JSON object")?;
    Ok(match format_of(o) {
        Some(f) => {
            let info = (f.info)(o);
            debug_assert_eq!(info.format, f.format);
            info
        }
        None => IdlInfo {
            format: IdlFormat::Unknown,
            format_version: None,
            name: str_of(o, "name"),
            program_version: None,
        },
    })
}

/// The calls a known-format IDL lists, in file order. Empty for `Unknown` and non-objects.
#[must_use]
pub fn calls(v: &Value) -> Vec<IdlCall> {
    v.as_object()
        .and_then(|o| format_of(o).map(|f| (f.calls)(o)))
        .unwrap_or_default()
}

/// A short display form of an IDL type, the same in both SDKs: `u64`, `string`, `vec<string>`,
/// `option<[u8; 32]>`, `ContentRef` (`{"defined": …}`). A one-key object `{k: T}` is shown as
/// `k<T>`; any other shape as `?` (the raw type is kept beside the label).
#[must_use]
pub fn type_label(t: &Value) -> String {
    match t {
        Value::String(s) => s.clone(),
        Value::Object(o) if o.len() == 1 => {
            let (k, inner) = o.iter().next().expect("one key");
            match (k.as_str(), inner) {
                ("defined", Value::String(name)) => name.clone(),
                ("defined", Value::Object(d)) if d.get("name").is_some_and(Value::is_string) => {
                    d["name"].as_str().unwrap_or_default().to_owned()
                }
                ("array", Value::Array(a)) if a.len() == 2 && a[1].is_u64() => {
                    format!("[{}; {}]", type_label(&a[0]), a[1])
                }
                (_, inner) => format!("{k}<{}>", type_label(inner)),
            }
        }
        _ => "?".to_owned(),
    }
}

fn str_of(o: &Obj, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// `items` is an array of objects, each with a string `name`, and each passing `each`.
fn named_list(items: Option<&Value>, each: impl Fn(&Obj) -> bool) -> bool {
    items.and_then(Value::as_array).is_some_and(|a| {
        a.iter().all(|x| {
            x.as_object()
                .is_some_and(|x| x.get("name").is_some_and(Value::is_string) && each(x))
        })
    })
}

/// Absent, or a list of `{name, type}` objects.
fn fields_ok(f: Option<&Value>) -> bool {
    f.is_none() || named_list(f, |a| a.contains_key("type"))
}

fn args(list: Option<&Value>) -> Vec<IdlArg> {
    list.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_object)
                .map(|a| {
                    let ty = a.get("type").cloned().unwrap_or(Value::Null);
                    IdlArg {
                        name: str_of(a, "name").unwrap_or_default(),
                        label: type_label(&ty),
                        ty,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

mod spel_v1 {
    use super::*;

    /// `instructions` is a list of `{name, args?: [{name, type}]}`.
    pub fn claims(o: &Obj) -> bool {
        named_list(o.get("instructions"), |ix| fields_ok(ix.get("args")))
    }

    pub fn info(o: &Obj) -> IdlInfo {
        IdlInfo {
            format: IdlFormat::SpelV1,
            format_version: str_of(o, "spec"),
            name: str_of(o, "name"),
            program_version: str_of(o, "version"),
        }
    }

    pub fn calls(o: &Obj) -> Vec<IdlCall> {
        o["instructions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|ix| IdlCall {
                name: str_of(ix, "name").unwrap_or_default(),
                tag: match ix.get("discriminator").and_then(Value::as_array) {
                    Some(d) if d.len() == 1 => d[0].as_u64().filter(|b| *b <= 255),
                    _ => None,
                },
                args: args(ix.get("args")),
            })
            .collect()
    }
}

mod spel_v2 {
    use super::*;

    /// `version` is `"2"` and `messages` is a list of `{name, tag?, fields?: [{name, type}]}`
    /// (unit variants have no `fields`).
    pub fn claims(o: &Obj) -> bool {
        o.get("version").and_then(Value::as_str) == Some("2")
            && named_list(o.get("messages"), |m| {
                m.get("tag").is_none_or(Value::is_u64) && fields_ok(m.get("fields"))
            })
    }

    pub fn info(o: &Obj) -> IdlInfo {
        IdlInfo {
            format: IdlFormat::SpelV2,
            format_version: Some("2".into()),
            name: str_of(o, "name"),
            program_version: None,
        }
    }

    pub fn calls(o: &Obj) -> Vec<IdlCall> {
        o["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
            .map(|m| IdlCall {
                name: str_of(m, "name").unwrap_or_default(),
                tag: m.get("tag").and_then(Value::as_u64),
                args: args(m.get("fields")),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const OURS: &[u8] = include_bytes!("../../../idl/lez_program_registry.idl.json");
    const V2: &[u8] = include_bytes!("../../fixtures/idl/spel-v2-state-entry.json");
    const V2_NO_STATE: &[u8] = include_bytes!("../../fixtures/idl/spel-v2-no-state.json");
    const UNKNOWN: &[u8] = include_bytes!("../../fixtures/idl/unknown-object.json");

    fn parse(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    #[test]
    fn our_registry_idl_is_spel_v1() {
        let v = parse(OURS);
        let i = detect(&v).unwrap();
        assert_eq!(i.format, IdlFormat::SpelV1);
        assert_eq!(i.format_version.as_deref(), Some("0.1.0"));
        assert_eq!(i.name.as_deref(), Some("lez_program_registry"));
        assert_eq!(i.program_version.as_deref(), Some("0.1.0"));
        let c = calls(&v);
        assert_eq!(c.len(), 6);
        assert_eq!(
            (c[0].name.as_str(), c[0].tag),
            ("register_deployer", Some(0))
        );
        assert_eq!(
            (c[5].name.as_str(), c[5].tag),
            ("deploy_and_register", Some(5))
        );
        let tags = &c[0].args.iter().find(|a| a.name == "tags").unwrap().label;
        assert_eq!(tags, "vec<string>");
    }

    #[test]
    fn spel_301_output_is_spel_v2() {
        for b in [V2, V2_NO_STATE] {
            let v = parse(b);
            let i = detect(&v).unwrap();
            assert_eq!(i.format, IdlFormat::SpelV2);
            assert_eq!(i.format_version.as_deref(), Some("2"));
            assert_eq!(i.name.as_deref(), Some("lez_program_registry"));
            assert_eq!(i.program_version, None);
            let c = calls(&v);
            let names: Vec<_> = c.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(
                names,
                [
                    "RegisterDeployer",
                    "RegisterThirdParty",
                    "Update",
                    "TransferAuthor",
                    "Freeze",
                    "DeployAndRegister"
                ]
            );
            assert!(c.iter().enumerate().all(|(i, c)| c.tag == Some(i as u64)));
            let labels: Vec<_> = c[0].args.iter().map(|a| a.label.as_str()).collect();
            assert_eq!(
                &labels[..6],
                [
                    "string",
                    "string",
                    "string",
                    "string",
                    "vec<string>",
                    "ContentRef"
                ]
            );
            assert!(c[0].args.iter().any(|a| a.label == "option<[u8; 32]>"));
        }
    }

    #[test]
    fn a_unit_variant_and_a_minimal_v2_file_are_v2() {
        let v = json!({"version": "2", "name": "t", "messages": [{"name": "Ping", "tag": 0}]});
        assert_eq!(detect(&v).unwrap().format, IdlFormat::SpelV2);
        assert!(calls(&v)[0].args.is_empty());
    }

    #[test]
    fn other_objects_are_unknown_and_non_objects_are_not_idls() {
        let unknown = parse(UNKNOWN);
        let i = detect(&unknown).unwrap();
        assert_eq!(i.format, IdlFormat::Unknown);
        assert!(calls(&unknown).is_empty());
        for half in [
            json!({"hello": "this is not an IDL"}),
            json!({"name": "x", "version": "1"}),
            // A key name shared with a known format is not enough.
            json!({"name": "x", "instructions": "not a list"}),
            json!({"name": "x", "instructions": [{"args": []}]}),
            json!({"version": "3", "messages": [{"name": "A", "tag": 0}]}),
            json!({"version": 2, "messages": []}),
            json!({"version": "2", "messages": [{"name": "A", "tag": -1}]}),
            json!({"version": "2", "messages": [{"name": "A", "fields": [{"name": "x"}]}]}),
        ] {
            assert_eq!(detect(&half).unwrap().format, IdlFormat::Unknown, "{half}");
        }
        for not in [json!([]), json!("idl"), json!(1), Value::Null] {
            assert!(detect(&not).is_err());
            assert!(calls(&not).is_empty());
        }
    }

    #[test]
    fn type_labels() {
        for (t, want) in [
            (json!("u128"), "u128"),
            (json!({"vec": "string"}), "vec<string>"),
            (json!({"option": {"array": ["u8", 32]}}), "option<[u8; 32]>"),
            (json!({"defined": "ContentRef"}), "ContentRef"),
            (json!({"defined": {"name": "Gen", "generics": []}}), "Gen"),
            (json!({"box": {"vec": "u8"}}), "box<vec<u8>>"),
            (json!({"a": 1, "b": 2}), "?"),
            (json!(7), "?"),
        ] {
            assert_eq!(type_label(&t), want, "{t}");
        }
    }
}
