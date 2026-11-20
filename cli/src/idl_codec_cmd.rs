//! Offline IDL tooling: encode/decode messages, state and events; compute and compare an image
//! id; plan a call from an IDL. Nothing here talks to a sequencer or signs anything.

use anyhow::{Result, anyhow, bail};
use lezreg_sdk::{idl_codec::Idl, verify};
use serde_json::{Value, json};
use std::path::Path;

fn load(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| anyhow!("cannot read {}: {e}", path.display()))
}

fn value(text: &str) -> Result<Value> {
    serde_json::from_str(text).map_err(|e| anyhow!("JSON value: {e}"))
}

/// The codec reports plain strings; anyhow needs an error that implements `std::error::Error`.
fn e<T>(result: std::result::Result<T, String>) -> Result<T> {
    result.map_err(anyhow::Error::msg)
}

/// `lezreg idl encode --idl F --kind message|state|event [--name N] --value JSON`
pub fn encode(
    idl: &Path,
    kind: &str,
    name: Option<&str>,
    text: &str,
    out: &mut crate::out::Out,
) -> Result<()> {
    let root: Value = serde_json::from_str(&load(idl)?).map_err(|e| anyhow!("IDL JSON: {e}"))?;
    let doc = Idl::new(&root);
    let v = value(text)?;
    let (what, bytes) = match kind {
        "message" => {
            let n = name.ok_or_else(|| anyhow!("--kind message needs --name <message>"))?;
            (n.to_owned(), e(doc.encode_message(n, &v))?)
        }
        "state" => {
            let n = name
                .map(str::to_owned)
                .or_else(|| doc.state_type().map(str::to_owned))
                .ok_or_else(|| anyhow!("the IDL names no state type: pass --name"))?;
            (n.clone(), e(doc.encode_named(&n, &v))?)
        }
        "event" => {
            let n = name.ok_or_else(|| anyhow!("--kind event needs --name <event>"))?;
            let (_, _, ty) = doc
                .events()
                .into_iter()
                .find(|(e, _, _)| *e == n)
                .ok_or_else(|| anyhow!("the IDL has no event {n}"))?;
            (ty.to_owned(), e(doc.encode_named(ty, &v))?)
        }
        other => bail!("--kind must be message, state or event, not {other}"),
    };
    let hex = hex::encode(&bytes);
    out.text(&format!("{what}: {hex}"));
    out.json(json!({"kind": kind, "name": what, "bytes": hex, "length": bytes.len()}));
    Ok(())
}

/// `lezreg idl decode --idl F --kind message|state|event [--name N] --data HEX`
pub fn decode(
    idl: &Path,
    kind: &str,
    name: Option<&str>,
    data: &str,
    out: &mut crate::out::Out,
) -> Result<()> {
    let root: Value = serde_json::from_str(&load(idl)?).map_err(|e| anyhow!("IDL JSON: {e}"))?;
    let doc = Idl::new(&root);
    let bytes = hex::decode(data).map_err(|e| anyhow!("--data is not hex: {e}"))?;
    let (what, decoded) = match kind {
        "message" => {
            let (n, args) = e(doc.decode_message(&bytes))?;
            (n, args)
        }
        "state" => {
            let n = name
                .map(str::to_owned)
                .or_else(|| doc.state_type().map(str::to_owned))
                .ok_or_else(|| anyhow!("the IDL names no state type: pass --name"))?;
            (n.clone(), e(doc.decode_named(&n, &bytes))?)
        }
        "event" => {
            let n = name.ok_or_else(|| anyhow!("--kind event needs --name <event>"))?;
            let (_, selector, ty) = doc
                .events()
                .into_iter()
                .find(|(e, _, _)| *e == n)
                .ok_or_else(|| anyhow!("the IDL has no event {n}"))?;
            if !data.starts_with(selector) {
                bail!("the data does not start with event {n}'s selector {selector}");
            }
            (n.to_owned(), e(doc.decode_named(ty, &bytes[8..]))?)
        }
        other => bail!("--kind must be message, state or event, not {other}"),
    };
    let pretty = serde_json::to_string_pretty(&decoded)?;
    out.text(&pretty);
    out.json(json!({"kind": kind, "name": what, "value": decoded}));
    Ok(())
}

/// `lezreg image-id <elf> [--compare HEX]`: the loader's id, with an optional verdict.
pub fn image_id(elf: &Path, compare: Option<&str>, out: &mut crate::out::Out) -> Result<()> {
    let bytes = std::fs::read(elf).map_err(|e| anyhow!("cannot read {}: {e}", elf.display()))?;
    let bin = verify::program_bin(&bytes)?;
    let id = verify::image_id_of_bin(&bin)?;
    let hex = hex::encode(id);
    out.text(&hex);
    let mut result = json!({"image_id": hex, "elf": elf.display().to_string()});
    if let Some(want) = compare {
        let want = want.trim().trim_start_matches("0x").to_ascii_lowercase();
        let verdict = if want == hex { "MATCH" } else { "MISMATCH" };
        out.text(&format!("compare {want}: {verdict}"));
        result["compare"] = json!(want);
        result["verdict"] = json!(verdict);
    }
    out.json(result);
    Ok(())
}

/// `lezreg call --idl F --message N --args JSON --plan`: build a call without sending it.
///
/// The plan is simulate-ready: the root message's bytes, the actors a v1 IDL says to declare,
/// the signers its `x-lez04` block requires, and the time window and revision rules it states.
/// v2 has no accounts by design (who receives a message is the caller's business), so its plan
/// lists only the message and says so. Sending is a separate command's job; nothing is signed.
pub fn plan(idl: &Path, message: &str, args: &str, out: &mut crate::out::Out) -> Result<()> {
    let root: Value = serde_json::from_str(&load(idl)?).map_err(|e| anyhow!("IDL JSON: {e}"))?;
    let doc = Idl::new(&root);
    let bytes = e(doc.encode_message(message, &value(args)?))?;
    let info = e(doc.message(message))?;
    let x = &doc.root["x-lez04"]["instructions"][kebab_key(info.name)];
    let accounts: Vec<Value> = doc.root["instructions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|ix| ix["name"] == info.name)
        .and_then(|ix| ix["accounts"].as_array())
        .cloned()
        .unwrap_or_default();
    let plan = json!({
        "idl": idl.display().to_string(),
        "format": e(doc.format())?.id(),
        "message": info.name,
        "tag": info.tag,
        "data": hex::encode(&bytes),
        "bytes": bytes.len(),
        "declared_actors": accounts.iter().map(|a| json!({
            "name": a["name"],
            "writable": a["writable"],
            "signer": a["signer"],
        })).collect::<Vec<Value>>(),
        "signers": x["signers"].clone(),
        "root": x["root"].clone(),
        "time": x["time"].clone(),
        "revision": x["revision"].clone(),
        "simulate": "not run: --plan builds the message and the actor/signer plan only; no sequencer was contacted",
    });
    out.text(&format!(
        "{} (tag {}): {}",
        info.name,
        info.tag,
        hex::encode(&bytes)
    ));
    if !accounts.is_empty() {
        out.text(&format!(
            "declare {} actor(s); signers: {}",
            accounts.len(),
            x["signers"].as_array().map_or("unknown".into(), |s| s
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "))
        ));
    } else {
        out.text(
            "the IDL names no actors: the caller chooses who receives this message (LEZ IDL v2)",
        );
    }
    out.json(plan);
    Ok(())
}

fn kebab_key(name: &str) -> String {
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
