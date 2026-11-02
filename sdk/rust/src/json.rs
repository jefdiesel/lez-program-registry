//! Stable JSON views of registry records (CLI `--json`, module, tests). Built here rather than
//! with serde derives on the records, because `crates/types/src/records.rs` is read by the
//! released `spel generate-idl` and must stay plain.

use lezreg_types::{BuildConfig, ContentRef, Entry, Link, Release, SourceRef};
use serde_json::{Value, json};

use crate::{
    cid,
    read::{Freshness, Header, Kind, Listed, Located, Resolved},
};

#[must_use]
pub fn content(c: &ContentRef) -> Value {
    json!({
        "cid": cid::to_string(&c.cid),
        "sha256": hex::encode(c.sha256),
        "size": c.size,
        "peer_hint": c.peer_hint.as_ref().map(hex::encode),
    })
}

#[must_use]
pub fn source(s: &SourceRef) -> Value {
    json!({
        "repo": s.repo,
        "commit": hex::encode(&s.commit),
        "docker_root": s.docker_root,
        "archive": s.archive.as_ref().map(content),
    })
}

#[must_use]
pub fn build(b: &BuildConfig) -> Value {
    json!({
        "builder": b.builder,
        "image_digest": hex::encode(b.image_digest),
        "image_tag": b.image_tag,
        "risc0_build": b.risc0_build,
        "host_risc0_rust": b.host_risc0_rust,
        "manifest_path": b.manifest_path,
        "binary": b.binary,
        "features": b.features,
        "lock_sha256": hex::encode(b.lock_sha256),
        "docker_env": b.docker_env.iter().map(|e| json!({"key": e.key, "value": e.value})).collect::<Vec<_>>(),
        "build_hash": hex::encode(lezreg_types::pda::build_hash(b)),
    })
}

#[must_use]
pub fn entry(e: &Entry) -> Value {
    json!({
        "kind": if e.kind == lezreg_types::KIND_DEPLOYER { "deployer" } else { "third-party" },
        "program": e.program.to_base58(),
        "author": e.author.to_base58(),
        "registrant": e.registrant.to_base58(),
        "name": e.name,
        "version": e.version,
        "author_label": e.author_label,
        "author_label_proven": false,
        "description": e.description,
        "tags": e.tags,
        "idl": content(&e.idl),
        "source": source(&e.source),
        "build": build(&e.build),
        "image_id": hex::encode(e.image_id),
        "registered_at": e.registered_at,
        "updated_at": e.updated_at,
        "revision": e.revision,
        "frozen": e.is_frozen(),
        "born_with_deploy": e.born_with_deploy(),
    })
}

#[must_use]
pub fn header(h: Option<&Header>) -> Value {
    h.map_or(Value::Null, |h| {
        json!({"image_id": hex::encode(h.image_id), "first_segment": h.first_segment.to_base58(), "immutable": h.immutable})
    })
}

#[must_use]
pub fn freshness(f: &Freshness) -> Value {
    match f {
        Freshness::Stale { live } => json!({"status": "stale", "live_image_id": hex::encode(live)}),
        other => json!({"status": other.label()}),
    }
}

#[must_use]
pub fn kind(k: Option<&Kind>) -> Value {
    match k {
        Some(Kind::Deployer) => json!("deployer"),
        Some(Kind::ThirdParty { publisher }) => json!({"third_party": {"publisher": publisher.to_base58()}}),
        None => json!("unverified"),
    }
}

#[must_use]
pub fn resolved(r: &Resolved) -> Value {
    json!({
        "program": r.program.to_base58(),
        "deployer_entry": r.deployer.as_ref().map(entry),
        "header": header(r.header.as_ref()),
        "freshness": r.freshness().as_ref().map(freshness),
    })
}

#[must_use]
pub fn located(l: &Located) -> Value {
    json!({
        "address": l.address.to_base58(),
        "kind": kind(Some(&l.kind)),
        "entry": entry(&l.entry),
        "header": header(l.header.as_ref()),
        "freshness": freshness(&l.freshness),
    })
}

#[must_use]
pub fn link(l: &Link) -> Value {
    json!({"seq": l.seq, "kind": l.kind, "program": l.program.to_base58(), "name": l.name, "registrant": l.author.to_base58()})
}

#[must_use]
pub fn listed(i: &Listed) -> Value {
    json!({
        "address": i.address.to_base58(),
        "seq": i.link.seq,
        "kind": kind(i.kind.as_ref()),
        "link": link(&i.link),
        "entry": i.entry.as_ref().map(entry),
    })
}

#[must_use]
pub fn release(r: &Release) -> Value {
    json!({
        "entry": r.entry.to_base58(),
        "revision": r.revision,
        "version": r.version,
        "idl": content(&r.idl),
        "source": source(&r.source),
        "build": build(&r.build),
        "image_id": hex::encode(r.image_id),
        "build_hash": hex::encode(r.build_hash),
        "at": r.at,
    })
}
