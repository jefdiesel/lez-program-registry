// The registry browser's own logic, kept out of Main.qml so `node registry_browser/logic.test.mjs`
// can check it (a QML engine is not needed). Inputs are the program_registry module's answers.

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
    let t = `${v.name} v${v.version}`;
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

/** Lines for the confirm sheet of a prepared change. */
export function changeLines(change) {
  if (!change) return [];
  const s = change.summary || {};
  const lines = [`${change.what} of entry ${shortId(change.entry)}`];
  if (/^Update/.test(change.what) && Array.isArray(s.fields) && s.fields.length) lines.push(`Changes: ${s.fields.join(", ")}`);
  lines.push(`Signed by ${(change.signers || []).map(shortId).join(" and ")} with your lezreg wallet`);
  if (s.payer) lines.push(`Fee paid by ${shortId(s.payer)}`);
  lines.push(`Sent to ${change.sequencer}`);
  if (s.irreversible) lines.push("This can't be undone: a frozen entry never changes again.");
  return lines;
}

/** A submitted change's state in words. */
export function jobText(job) {
  if (!job) return "";
  if (job.state === "running") return `${job.what}: sending…`;
  if (job.state === "applied") return `${job.what}: applied${job.receipt && job.receipt.block ? " in block " + job.receipt.block : ""}`;
  return `${job.what}: failed${job.code ? " (" + job.code + ")" : ""}: ${job.error || "no reason given"}`;
}

/** The heading of another app's waiting request. */
export function requestTitle(q) {
  if (!q || !q.change) return "";
  return `${q.from || "An app"} asks to sign: ${q.change.what}`;
}
