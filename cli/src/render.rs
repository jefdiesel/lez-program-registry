//! Text and JSON rendering for reads and receipts.

use anyhow::Result;
use std::collections::HashMap;

use lezreg_sdk::{
    Found, Freshness, Header, HttpRpc, Kind, Listed, Page, Registry, cid, json as j,
    tx::Receipt,
};
use lezreg_types::{AccountId, Entry};
use serde_json::json;

use crate::out::Out;

/// Milliseconds since the epoch as `YYYY-MM-DD HH:MM:SSZ`.
pub fn readable(ms: u64) -> String {
    let seconds = ms / 1000;
    let days = seconds / 86_400 + 719_468;
    let (era, doe) = (days / 146_097, days % 146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60)
}

fn short(h: &[u8]) -> String {
    let s = hex::encode(h);
    if s.len() > 16 { format!("{}…{}", &s[..8], &s[s.len() - 8..]) } else { s }
}

fn freshness_text(f: &Freshness, claimed: &[u8; 32]) -> String {
    match f {
        Freshness::Fresh => format!("FRESH: the claimed image {} is what the program runs now", short(claimed)),
        Freshness::Stale { live } => {
            format!("STALE: the entry claims image {} but the program now runs {} (header repointed, or the claim was never updated)", short(claimed), short(live))
        }
        Freshness::NoProgram => "NO PROGRAM at this account (yet): nothing is deployed here".into(),
    }
}

fn entry_text(out: &Out, title: &str, address: &AccountId, kind: &Kind, e: &Entry, header: Option<&Header>, f: &Freshness) {
    out.text(&format!("{title}  {} v{}  (revision {})", e.name, e.version, e.revision));
    let why = match kind {
        Kind::Deployer => "the program's own account: only its header key could write it".to_owned(),
        Kind::ThirdParty { publisher } => format!("T(program, {publisher}): a community entry, NOT the deployer's"),
    };
    out.text(&format!("  address      {address}  ({why})"));
    out.text(&format!("  program      {}", e.program));
    out.text(&format!("  status       {}", freshness_text(f, &e.image_id)));
    if let Some(h) = header {
        out.text(&format!("  code         {}", if h.immutable { "immutable (can never be upgraded)" } else { "upgradeable (the header key can repoint it)" }));
    }
    out.text(&format!("  author       {}  (proven co-signer; the only account that may update)", e.author));
    if e.registrant != e.author {
        out.text(&format!("  registrant   {}  (original author; transferred since)", e.registrant));
    }
    if !e.author_label.is_empty() {
        out.text(&format!("  label        \"{}\"  (UNPROVEN free text)", e.author_label));
    }
    if !e.description.is_empty() {
        out.text(&format!("  description  {}", e.description));
    }
    if !e.tags.is_empty() {
        out.text(&format!("  tags         {}", e.tags.join(", ")));
    }
    out.text(&format!("  idl          {}  sha256 {}  {} B", cid::to_string(&e.idl.cid), short(&e.idl.sha256), e.idl.size));
    if !e.source.repo.is_empty() {
        out.text(&format!("  source       {} @ {} (docker root {})", e.source.repo, hex::encode(&e.source.commit), e.source.docker_root));
    }
    if let Some(a) = &e.source.archive {
        out.text(&format!("  archive      {}  sha256 {}  {} B", cid::to_string(&a.cid), short(&a.sha256), a.size));
    }
    out.text(&format!(
        "  build        {} (digest {}), risc0-build {}, binary {}, build hash {}{}",
        e.build.image_tag,
        short(&e.build.image_digest),
        e.build.risc0_build,
        e.build.binary,
        short(&lezreg_types::pda::build_hash(&e.build)),
        if e.build.lock_sha256 == [0; 32] { "\n               (UNVERIFIED: no Cargo.lock hash given; these may be lezreg's defaults, not the author's build, until `lezreg verify` checks them)" } else { "" }
    ));
    out.text(&format!("  image id     {}", hex::encode(e.image_id)));
    out.text(&format!("  registered   {}   updated {}", readable(e.registered_at), readable(e.updated_at)));
    let mut flags = vec![];
    if e.born_with_deploy() {
        flags.push("born with deploy (registered in the same transaction that deployed the program)");
    }
    if e.is_frozen() {
        flags.push("FROZEN (can no longer change)");
    }
    if !flags.is_empty() {
        out.text(&format!("  flags        {}", flags.join("; ")));
    }
}

pub fn show(out: &mut Out, r: &Registry<HttpRpc>, target: &str, publisher: Option<&str>, revision: Option<u32>) -> Result<()> {
    let Ok(program) = crate::args::parse_account(target).map(|p| lezreg_sdk::tx::tid(&p)) else {
        return show_name(out, r, target);
    };
    if let Some(p) = publisher {
        let publisher = lezreg_sdk::tx::tid(&crate::args::parse_account(p)?);
        let (at, _) = r.third_party_entry(&program, &publisher)?;
        let loc = r.locate(&at)?.ok_or(lezreg_sdk::Error::NoEntry(at))?;
        entry_text(out, "third-party entry", &loc.address, &loc.kind, &loc.entry, loc.header.as_ref(), &loc.freshness);
        let mut v = j::located(&loc);
        release(out, r, &at, revision, &mut v)?;
        out.json(v);
        return Ok(());
    }
    // One getAccount: the deployer entry and the program header together.
    let resolved = r.resolve(&program)?;
    let tps = r.third_party(&program, SHOWN)?;
    let tp_total = r.third_party_count(&program)?;
    out.text(&format!("program {program}"));
    match &resolved.header {
        Some(h) => out.text(&format!("  deployed     image {}  ({})", hex::encode(h.image_id), if h.immutable { "immutable" } else { "upgradeable" })),
        None => out.text("  deployed     NO: no program header at this account"),
    }
    let mut v = j::resolved(&resolved);
    match &resolved.deployer {
        Some(e) => {
            out.text("");
            entry_text(out, "deployer entry", &program, &Kind::Deployer, e, resolved.header.as_ref(), &resolved.freshness().expect("entry"));
            release(out, r, &program, revision, &mut v)?;
        }
        None => out.text("\nno deployer entry: the program's header key has not registered it"),
    }
    out.text(&format!("\nthird-party (community) entries: {tp_total}{}", more(tp_total, tps.len())));
    let mut tj = Vec::new();
    for i in &tps {
        let Some(e) = &i.entry else {
            out.text(&format!("  {}  (link without a public entry: private or unverifiable)", i.address));
            continue;
        };
        let f = Freshness::of(e, resolved.header.as_ref());
        match i.kind {
            Some(Kind::ThirdParty { publisher }) => {
                out.text(&format!("  community entry by {publisher}: {} v{} (revision {}), {}  [{}]", e.name, e.version, e.revision, f.label(), i.address));
            }
            _ => out.text(&format!("  {} (address does not match its record: ignored)", i.address)),
        }
        let mut x = j::listed(i);
        x["freshness"] = j::freshness(&f);
        tj.push(x);
    }
    v["third_party"] = json!(tj);
    v["third_party_count"] = json!(tp_total);
    out.json(v);
    Ok(())
}

/// How many entries `show` prints from one list.
const SHOWN: usize = 50;

fn more(total: u64, shown: usize) -> String {
    let shown = shown as u64;
    if total > shown { format!(" (newest {shown} shown; {} more)", total - shown) } else { String::new() }
}

/// Each listed entry's program status, from one batched header read.
fn statuses(r: &Registry<HttpRpc>, items: &[Listed]) -> anyhow::Result<Vec<Option<Freshness>>> {
    let mut programs: Vec<_> = items.iter().filter_map(|i| i.entry.as_ref().map(|e| e.program)).collect();
    programs.sort_unstable();
    programs.dedup();
    let headers: HashMap<_, _> = programs.iter().copied().zip(r.headers(&programs)?).collect();
    Ok(items.iter().map(|i| i.entry.as_ref().map(|e| Freshness::of(e, headers.get(&e.program).and_then(Option::as_ref)))).collect())
}

fn listed_json(i: &Listed, f: Option<&Freshness>) -> serde_json::Value {
    let mut x = j::listed(i);
    if let Some(f) = f {
        x["freshness"] = j::freshness(f);
    }
    x
}

fn release(out: &Out, r: &Registry<HttpRpc>, entry: &AccountId, revision: Option<u32>, v: &mut serde_json::Value) -> Result<()> {
    let Some(n) = revision else { return Ok(()) };
    match r.release(entry, n)? {
        Some(rel) => {
            out.text(&format!(
                "  release #{n}   v{} image {} idl {} build hash {} at {}",
                rel.version,
                short(&rel.image_id),
                cid::to_string(&rel.idl.cid),
                short(&rel.build_hash),
                readable(rel.at)
            ));
            v["release"] = j::release(&rel);
        }
        None => {
            out.text(&format!("  release #{n}   none"));
            v["release"] = serde_json::Value::Null;
        }
    }
    Ok(())
}

/// Orders entries that share a name: deployer entries of a deployed program first, then other
/// entries of deployed programs, then entries whose program does not exist (anyone can claim
/// any name for any account). Otherwise newest first.
fn rank(i: &Listed, f: Option<&Freshness>) -> u8 {
    let deployed = matches!(f, Some(Freshness::Fresh | Freshness::Stale { .. }));
    match (deployed, i.kind == Some(Kind::Deployer)) {
        (true, true) => 0,
        (true, false) => 1,
        (false, _) => 2,
    }
}

fn show_name(out: &mut Out, r: &Registry<HttpRpc>, name: &str) -> Result<()> {
    let name = name.to_ascii_lowercase();
    let items = r.by_name(&name, SHOWN)?;
    let total = r.name_count(&name)?;
    let fs = statuses(r, &items)?;
    let mut rows: Vec<_> = items.iter().zip(fs).collect();
    rows.sort_by_key(|(i, f)| rank(i, f.as_ref()));
    out.text(&format!("name '{name}': {total} entries{} (deployed programs' deployer entries first; NO-PROGRAM entries last)", more(total, items.len())));
    let mut v = Vec::new();
    for (i, f) in &rows {
        out.text(&format!("  {}", line(i, f.as_ref())));
        v.push(listed_json(i, f.as_ref()));
    }
    out.json(json!({"name": name, "count": total, "entries": v}));
    Ok(())
}

/// One line per listed entry. `f`: the program's status (`None` for a link with no entry).
fn line(i: &Listed, f: Option<&Freshness>) -> String {
    let Some(e) = &i.entry else {
        return format!("#{:<4} {}  {}  (no public entry: private or unverifiable)", i.link.seq, i.link.name, i.address);
    };
    let kind = match &i.kind {
        Some(Kind::Deployer) => "deployer   ".to_owned(),
        Some(Kind::ThirdParty { publisher }) => format!("community by {}", short_id(publisher)),
        None => "UNVERIFIED ".to_owned(),
    };
    let fresh = match f {
        Some(Freshness::NoProgram) => "  NO-PROGRAM (nothing deployed at this program id)".to_owned(),
        Some(f) => format!("  {}", f.label()),
        None => String::new(),
    };
    let frozen = if e.is_frozen() { "  frozen" } else { "" };
    format!("#{:<4} {:<24} v{:<10} {kind}  program {}  author {}{fresh}{frozen}", i.link.seq, e.name, e.version, short_id(&e.program), short_id(&e.author))
}

fn short_id(a: &AccountId) -> String {
    let s = a.to_base58();
    if s.len() > 12 { format!("{}…{}", &s[..6], &s[s.len() - 4..]) } else { s }
}

pub fn list(out: &mut Out, r: &Registry<HttpRpc>, page: &Page) -> Result<()> {
    out.text(&format!("{} entries in the registry {}; showing {} (newest first)", page.total, r.registry(), page.items.len()));
    let fs = statuses(r, &page.items)?;
    for (i, f) in page.items.iter().zip(&fs) {
        out.text(&format!("  {}", line(i, f.as_ref())));
    }
    if let Some(n) = page.next {
        out.text(&format!("more: lezreg list --after {n}"));
    }
    out.json(json!({
        "registry": r.registry().to_base58(),
        "total": page.total,
        "items": page.items.iter().zip(&fs).map(|(i, f)| listed_json(i, f.as_ref())).collect::<Vec<_>>(),
        "next": page.next.map(|n| n.to_base58()),
    }));
    Ok(())
}

/// `exact`: for a name search, the query; exact matches stay first, and within each group
/// entries of deployed programs come before NO-PROGRAM ones.
pub fn search(out: &mut Out, r: &Registry<HttpRpc>, title: &str, found: &Found, proven: bool, exact: Option<&str>) -> Result<()> {
    let fs = statuses(r, &found.items)?;
    let mut rows: Vec<_> = found.items.iter().zip(fs).collect();
    if let Some(q) = exact {
        rows.sort_by_key(|(i, f)| (i.link.name != q, rank(i, f.as_ref())));
    }
    out.text(&format!("search {title}: {} found ({} entries examined)", rows.len(), found.scanned));
    for (i, f) in &rows {
        out.text(&format!("  {}", line(i, f.as_ref())));
    }
    if found.truncated {
        out.text("  (the search stopped at its --scan bound; older entries were not examined: raise --scan to look further)");
    }
    out.json(json!({
        "query": title,
        "proven": proven,
        "items": rows.iter().map(|(i, f)| listed_json(i, f.as_ref())).collect::<Vec<_>>(),
        "scanned": found.scanned,
        "truncated": found.truncated,
    }));
    Ok(())
}

pub fn receipt(out: &mut Out, r: &Receipt) {
    match (&r.hash, r.block) {
        (Some(h), None) => out.text(&format!(
            "{}: applied (tx {h}), confirmed by reading the chain back; its block was not found in time, so the fee was not measured{}",
            r.what,
            r.entry.map_or_else(String::new, |e| format!("; entry {e}"))
        )),
        (Some(h), Some(b)) => out.text(&format!(
            "{}: applied in block {b} (tx {h}); simulated {} cycles, gas limit {}, fee {}, {} bytes{}",
            r.what,
            r.cycles.map_or_else(|| "refused".into(), |c| c.to_string()),
            r.gas_limit,
            r.fee,
            r.tx_bytes,
            r.entry.map_or_else(String::new, |e| format!("; entry {e}"))
        )),
        _ => out.text(&format!(
            "{}: dry run, nothing sent; simulated {} cycles, gas limit would be {}; the payer {} must hold about {} for the fee reserve",
            r.what,
            r.cycles.map_or_else(|| "refused".into(), |c| c.to_string()),
            r.gas_limit,
            r.payer,
            r.reserve
        )),
    }
    out.json(r.json());
}
