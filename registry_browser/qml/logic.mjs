// The registry browser's own logic, kept out of Main.qml so `node registry_browser/logic.test.mjs`
// can check it (a QML engine is not needed). Inputs are the program_registry module's answers.

/** Text from the chain or from an app, safe to show as ONE line: every line or paragraph break
 * (\n, \r, U+2028, U+2029, …) becomes a visible "⏎", so a value can't add lines of its own to a
 * card; controls, bidirectional overrides and isolates, zero-width and other invisible
 * characters (soft hyphen, U+180E, U+2060–2064, U+206A–206F, U+FFF9–FFFB, tags) are dropped.
 * Every Text in this app is also plain text (no markup). */
export function clean(s) {
  return String(s === undefined || s === null ? "" : s).replace(/\r\n|[\n\r\u000B\u000C\u0085\u2028\u2029]/g, "\u23CE").replace(/[\u0000-\u001F\u007F-\u009F\u00AD\u061C\u180E\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u206F\uFEFF\uFFF9-\uFFFB]|\uDB40[\uDC00-\uDC7F]/g, "");
}

/** "6YWH…VayN": an account id short enough for one line. */
export function shortId(id) {
  const s = String(id || "");
  return s.length <= 12 ? s : s.slice(0, 4) + "…" + s.slice(-4);
}

/** A module reply parsed as far as it goes: modules answer with JSON text, sometimes wrapped twice. */
export function parsed(reply) {
  for (let depth = 0; depth < 2 && typeof reply === "string"; depth++) {
    try { reply = JSON.parse(reply); } catch (e) { break; }
  }
  return reply;
}

/** `{ok, value, error, code}` from a module reply, whatever shape it came in. */
export function answer(reply) {
  const r = parsed(reply);
  if (r === undefined || r === null || r === "")
    return { ok: false, value: null, error: "The program_registry module didn't answer in time.", code: "timeout" };
  if (typeof r !== "object" || r.success === undefined)
    return { ok: false, value: null, error: "The program_registry module is missing or out of date: install it from the catalog.", code: "no_module" };
  return { ok: !!r.success, value: r.value, error: r.error || "", code: r.code || "" };
}

/** Which kind of entry a listed item is: "deployer", "community" or "unverified". */
export function kindOf(item) {
  const k = item && item.kind;
  if (k === "deployer") return "deployer";
  if (k && typeof k === "object" && k.third_party) return "community";
  return "unverified";
}

/** The badge for a kind: its words and whether it is proven by the chain. */
export function kindBadge(kind) {
  if (kind === "deployer") return { text: "Deployer", proven: true, note: "Written by the program's own header key: only who can deploy or upgrade the program could write it." };
  if (kind === "community") return { text: "Community", proven: false, note: "Written by someone else about this program. The chain proves who wrote it, not that it is right." };
  return { text: "Unverified", proven: false, note: "This record is not where the registry would put it." };
}

/** Freshness words for a `resolve_program` answer's `freshness`. */
export function freshnessText(f) {
  const s = f && f.status;
  if (s === "fresh") return { text: "Runs the registered code", tone: "good" };
  if (s === "stale") return { text: "Stale: the program now runs other code than registered", tone: "warn" };
  if (s === "no-program") return { text: "No program is deployed at this id", tone: "warn" };
  return { text: "", tone: "dim" };
}

/** One line about a `get_idl` answer: its calls when the format is known, "available, format not
 * recognised" when it is not, or why it is unavailable. Also reads the older `{status: "ok" |
 * "unavailable: …", idl}` answers of program_registry before 0.3. */
export function idlSummary(v) {
  if (!v) return { ok: false, text: "IDL not loaded", instructions: [] };
  const fmt = v.format ? ` (${v.format}${v.format_version ? " " + v.format_version : ""})` : "";
  if (v.status === "ok") {
    const calls = Array.isArray(v.calls) && v.calls.length ? v.calls.map((c) => c && c.name)
      : v.idl && Array.isArray(v.idl.instructions) ? v.idl.instructions.map((i) => i && i.name) : [];
    const ins = calls.filter(Boolean);
    return { ok: true, text: `IDL${fmt}: ${ins.length} instruction${ins.length === 1 ? "" : "s"} (checked against the chain's sha256)`, instructions: ins };
  }
  if (v.status === "unknown-format")
    return { ok: true, text: `IDL available${fmt}, format not recognised: the bytes match the chain, but this app can't read its calls.`, instructions: [] };
  const why = v.reason || String(v.status || "unknown").replace(/^unavailable:\s*/, "");
  return { ok: false, text: `IDL unavailable: ${String(why).replace(/^unavailable:\s*/, "")}. The entry still shows; its interface can't be read right now.`, instructions: [] };
}

/** A `program_label` answer as the words a RegistryLabel shows. */
export function labelText(v, programId) {
  if (!v) return shortId(programId);
  if (v.kind === "deployer" || v.kind === "community") {
    let t = `${clean(v.name)} v${clean(v.version)}`;
    t += v.kind === "deployer" ? ` by ${v.author_short || shortId(v.author)}` : " · community";
    if (v.verified) t += " ✓";
    if (v.stale) t += " · stale";
    if (v.deployed === false) t += " · not deployed";
    return t;
  }
  return shortId(programId);
}

/** A date for a millisecond timestamp, or "" for none. */
export function when(ms) {
  if (!ms) return "";
  const d = new Date(Number(ms));
  return isNaN(d.getTime()) ? "" : d.toISOString().slice(0, 16).replace("T", " ") + " UTC";
}

/** An array, or an array-like a QML model hands over (it fails Array.isArray), as an array. */
function list(x) {
  return Array.isArray(x) ? x : x && typeof x === "object" && typeof x.length === "number" ? Array.prototype.slice.call(x) : [];
}

/** A value a change writes, as one line of text. */
function valueText(key, v) {
  if (v === null || v === undefined) return "";
  if (key === "idl" || key === "archive") return `CID ${clean(v.cid)}, sha256 ${clean(v.sha256)}, ${v.size} bytes${v.peer_hint ? `, peer ${clean(v.peer_hint)}` : ""}`;
  if (key === "source") return `${clean(v.repo) || "(no repo)"} @ ${clean(v.commit)} in ${clean(v.docker_root)}${v.archive ? `; archive ${valueText("archive", v.archive)}` : ""}`;
  if (key === "build") return `binary ${clean(v.binary)} from ${clean(v.manifest_path)}; builder image ${clean(v.image_digest)} (${clean(v.image_tag)}), risc0-build ${clean(v.risc0_build)}, rust ${clean(v.host_risc0_rust)}, Cargo.lock ${clean(v.lock_sha256)}${list(v.features).length ? `, features ${list(v.features).map(clean).join(" ")}` : ""}${list(v.docker_env).length ? `, build environment ${list(v.docker_env).map((e) => `${clean(e.key)}=${clean(e.value)}`).join(" ")}` : ", no build environment"}`;
  if (Array.isArray(v) || (typeof v === "object" && typeof v.length === "number")) return list(v).map(clean).join(", ") || "(none)";
  if (typeof v === "object") return clean(JSON.stringify(v));
  return clean(v) === "" ? "(empty)" : clean(v);
}

const VALUE_ORDER = ["name", "version", "description", "tags", "author_label", "author", "new_author", "program", "image_id", "idl", "source", "build", "revision", "frozen"];

/** Lines for the confirm sheet of a prepared change: everything that will be signed, from the
 * module's own decoding of it (summary.values), with full ids. */
export function changeLines(change) {
  if (!change) return [];
  const s = change.summary || {};
  // What matters most comes first, before any long value: where it spends, whether it can be
  // undone, what it can cost, who pays and signs.
  const lines = [`${clean(change.what)} of entry ${clean(change.entry)}`];
  lines.push(s.spends_real_funds ? `This spends real funds on ${clean(change.sequencer)}` : `Sent to ${clean(change.sequencer)}`);
  if (s.irreversible) lines.push("This can't be undone: a frozen entry never changes again.");
  if (s.max_gas) lines.push(`At most ${s.max_gas} gas${s.max_fee ? `, at most ${clean(s.max_fee)} in fees` : ""}`);
  lines.push(s.payer ? `Fee paid by ${clean(s.payer)}` : "Fee paid by the first signer");
  lines.push(`Signed by ${list(change.signers).map(clean).join(" and ")} with your lezreg wallet`);
  if (s.current) lines.push(`Now: ${clean(s.current.name)} v${clean(s.current.version)} (${clean(s.current.kind)}, revision ${s.current.revision}), author ${clean(s.current.author)}`);
  else if (/^Register/.test(change.what || "")) lines.push("Now: no entry (this creates it)");
  const values = s.values || {};
  for (const k of VALUE_ORDER.concat(Object.keys(values).filter((k) => !VALUE_ORDER.includes(k))))
    if (k in values && k !== "frozen") lines.push(`${k.replace(/_/g, " ")}: ${valueText(k, values[k])}`);
  return lines;
}

/** A submitted change's state in words. */
export function jobText(job) {
  if (!job) return "";
  if (job.state === "running") return `${clean(job.what)}: sending…`;
  if (job.state === "applied") return `${clean(job.what)}: applied${job.receipt && job.receipt.block ? " in block " + job.receipt.block : ""}`;
  if (job.state === "awaiting_approval" || job.state === "denied" || job.state === "expired") return `request ${clean(job.state)}`;
  return `${clean(job.what)}: failed${job.code ? " (" + clean(job.code) + ")" : ""}: ${clean(job.error) || "no reason given"}`;
}

/** The heading of another app's waiting request. */
export function requestTitle(q) {
  if (!q || !q.change) return "";
  return `An app that calls itself "${clean(q.from) || "?"}" (name chosen by the app, unverified) asks to sign: ${clean(q.change.what)}`;
}
