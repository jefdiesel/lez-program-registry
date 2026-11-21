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

/** `{ok, value, error, code, detail, retryAfterMs}` from a module reply, whatever shape it came
 * in. A failure to reach the network is plain words with code "rate_limited" or "unreachable"
 * (as program_registry 0.3.3 answers; an older module's raw text is read the same way), and
 * `detail` holds its cause as plain text for a technical-details line, never a response body. */
export function answer(reply) {
  const r = parsed(reply);
  if (r === undefined || r === null || r === "")
    return { ok: false, value: null, error: "The program_registry module didn't answer in time.", code: "timeout", detail: "", retryAfterMs: 0 };
  if (typeof r !== "object" || r.success === undefined)
    return { ok: false, value: null, error: "The program_registry module is missing or out of date: install it from the catalog.", code: "no_module", detail: "", retryAfterMs: 0 };
  if (r.success) return { ok: true, value: r.value, error: "", code: r.code || "", detail: "", retryAfterMs: 0 };
  const n = networkError(r.error, r.code, r.detail, r.retry_after_ms);
  return n ? Object.assign({ ok: false, value: null }, n) : { ok: false, value: null, error: plainText(r.error, 300), code: r.code || "", detail: "", retryAfterMs: 0 };
}

/** Text with markup (tags, comments, scripts, styles) removed and spaces collapsed, at most `n`
 * characters: what an error page says, never the page. Also cleaned as `clean` does. */
export function plainText(s, n) {
  let t = String(s === undefined || s === null ? "" : s)
    .replace(/<!--[\s\S]*?(-->|$)/g, " ").replace(/<script[\s\S]*?(<\/script>|$)/gi, " ").replace(/<style[\s\S]*?(<\/style>|$)/gi, " ")
    .replace(/<[^>]*(>|$)/g, " ").replace(/&nbsp;/g, " ").replace(/&amp;/g, "&").replace(/&lt;/g, "‹").replace(/&gt;/g, "›");
  t = clean(t).replace(/\u23CE/g, " ").replace(/\s+/g, " ").trim().replace(/[<>]/g, "");
  return n && t.length > n ? t.slice(0, n) : t;
}

export const BUSY_WORDS = "The network is busy. Trying again in a few seconds.";
export const STILL_BUSY_WORDS = "Still busy. Try again in a minute.";
export const UNREACHABLE_WORDS = "Can't reach the network right now.";

/** A failure about reaching the network, as `{error, code, detail, retryAfterMs}`, or null. */
export function networkError(error, code, detail, retryAfterMs) {
  const raw = String(error || "");
  const why = plainText(detail || raw.replace(/^.*?cannot reach the sequencer at \S+?:\s+/i, ""), 200);
  if (code === "rate_limited" || /\bHTTP 429\b|rate.?limit|Error 1015/i.test(raw))
    return { error: BUSY_WORDS, code: "rate_limited", detail: why || "HTTP 429", retryAfterMs: Number(retryAfterMs) || 0 };
  if (code === "unreachable" || /cannot reach the sequencer|\bHTTP 5\d\d\b|not JSON|<!doctype|<html/i.test(raw))
    return { error: UNREACHABLE_WORDS, code: "unreachable", detail: why, retryAfterMs: 0 };
  return null;
}

/** Automatic tries after a rate limit before saying "Still busy". */
export const MAX_RETRIES = 4;

/** How long to wait before try `attempt` (0-based) after a rate limit: 2, 4, 8, 16 s, or what
 * the sequencer asked for when that is longer; never more than a minute. */
export function retryDelay(attempt, retryAfterMs) {
  const base = 2000 * Math.pow(2, Math.max(0, Math.min(attempt, 4)));
  return Math.min(60000, Math.max(base, Number(retryAfterMs) || 0));
}

/** Module methods an app may retry by itself: reads only, never a change or an answer to one. */
export function retryable(method) {
  return ["config_get", "list_entries", "search_name", "resolve_program", "program_label", "third_party_entries", "get_idl"].includes(method);
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
  const raw = v.reason || String(v.status || "unknown").replace(/^unavailable:\s*/, "");
  const n = networkError(raw);
  const why = n ? n.error.replace(/\.$/, "") : plainText(raw, 300);
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
  const n = networkError(job.error, job.code);
  if (n) return `${clean(job.what)}: failed: ${n.error}`;
  return `${clean(job.what)}: failed${job.code ? " (" + clean(job.code) + ")" : ""}: ${plainText(job.error, 300) || "no reason given"}`;
}

/** The heading of another app's waiting request. */
export function requestTitle(q) {
  if (!q || !q.change) return "";
  return `An app that calls itself "${clean(q.from) || "?"}" (name chosen by the app, unverified) asks to sign: ${clean(q.change.what)}`;
}

// ── The index: entries read like a guidebook, not a technical record ──────────────────────
// Everything below is derived from the entry's own text (name, description, tags, author label):
// nothing here knows any particular program.

/** Words kept in capitals when a name becomes a title. */
const ACRONYMS = { lez: "LEZ", nft: "NFT", nfts: "NFTs", evm: "EVM", lns: "LNS", idl: "IDL", eth: "ETH", rpc: "RPC", api: "API", ui: "UI", zk: "ZK", dao: "DAO", dex: "DEX", amm: "AMM", rfp: "RFP", lp: "LP" };

/** A program name as a title: "names-evm-bridge-v7" → "Names EVM Bridge v7". */
export function titleCase(name) {
  return clean(name).split(/[-_\s]+/).filter(Boolean).map((w) => {
    const low = w.toLowerCase();
    if (ACRONYMS[low]) return ACRONYMS[low];
    if (/^v\d+(\.\d+)*$/.test(low)) return low;
    return w.charAt(0).toUpperCase() + w.slice(1);
  }).join(" ");
}

/** A program name as an app title, when its description gives none: in title case, without a
 * leading "LEZ" (the coin) or "Logos" (the network), which name no app ("lez-token" → "Token"). */
export function fallbackTitle(name) {
  const t = titleCase(name);
  const rest = t.replace(/^((LEZ|Logos)\s+)+/i, "");
  return rest || t;
}

/** Text without its (parenthetical asides), and tidy spaces around what is left. */
function withoutAsides(s) {
  let t = s, prev;
  do { prev = t; t = t.replace(/\s*\([^()]*\)/g, ""); } while (t !== prev);
  return t.replace(/\s+([,.;:])/g, "$1").replace(/\s{2,}/g, " ").trim();
}

/** The first sentence: up to a full stop followed by a space (so "LEZ 0.4" stays whole). */
function firstSentence(s) {
  const m = /^(.*?[.!?])\s+\S/.exec(s);
  return m ? m[1] : s;
}

/** Capital first letter, full stop at the end, at most `max` characters (cut at a word). */
function tidySentence(s, max) {
  let t = s.trim().replace(/[\s,;:]+$/, "");
  if (!t) return "";
  t = t.charAt(0).toUpperCase() + t.slice(1);
  if (t.length > max) {
    const cut = t.slice(0, max - 1);
    t = cut.slice(0, Math.max(cut.lastIndexOf(" "), max * 0.6)).replace(/[\s,;:.]+$/, "") + "…";
  } else if (!/[.!?…]$/.test(t)) t += ".";
  return t;
}

/** An entry's title and one plain sentence. The description's text before ": " is the title when
 * it is short and plain ("Vest: privacy-preserving …" → "Vest"); a longer lead that only restates
 * the title is dropped; otherwise the title is the name in title case. The sentence is the first
 * sentence of what is left, without (asides). `about` is the whole description, cleaned; `more` is
 * what it says after its first sentence (the guide page's About, "" when there is nothing more). */
export function guideText(entry, fallbackName) {
  const e = entry || {};
  const name = clean(e.name || fallbackName || "");
  const about = clean(e.description || "").replace(/⏎/g, " ").replace(/\s+/g, " ").trim();
  let title = fallbackTitle(name);
  let rest = about;
  const colon = about.indexOf(": ");
  if (colon > 0) {
    const lead = about.slice(0, colon).trim();
    const plain = !/[,()[\].;]/.test(lead) && lead.split(/\s+/).length <= 3 && lead.length <= 32;
    if (plain) { title = lead; rest = about.slice(colon + 2); }
    else if (title && withoutAsides(lead).toLowerCase().replace(/^the\s+/, "").replace(/^((lez|logos)\s+)+/, "").startsWith(title.toLowerCase())) rest = about.slice(colon + 2);
  }
  const more = about.slice(firstSentence(about).length).trim();
  return { title: title || "Unnamed program", sentence: tidySentence(withoutAsides(firstSentence(rest)), 150), about, more };
}

/** Who published an entry, as people read it: the author label without its "lns:" prefix, or the
 * short account when there is no label. `proven` is false for a label the chain doesn't confirm. */
export function publisherOf(entry) {
  const e = entry || {};
  const label = clean(e.author_label || "").trim().replace(/^lns:/i, "");
  if (label) return { text: label, proven: !!e.author_label_proven, account: false };
  return { text: shortId(e.author), proven: true, account: true };
}

/** Sections of the index, in reading order, and the tags that put an entry in one. */
const SECTIONS = [
  ["Tokens", ["token", "tokens", "fungible", "vesting", "defi", "finance", "payments", "stablecoin"]],
  ["Names", ["lns", "names", "name", "naming", "identity"]],
  ["NFTs", ["nft", "nfts", "collectible", "collectibles", "deeds"]],
  ["Bridges", ["bridge", "bridges", "eth", "evm", "interop", "cross-chain"]],
  ["Tools", ["registry", "infrastructure", "tools", "tool", "developer", "dev", "utility"]],
];

/** The index section for an entry's tags: the first tag that names a known section wins; an
 * unknown first tag is its own section; no tags at all is "Other". */
export function sectionOf(tags) {
  const t = list(tags).map((x) => clean(x).toLowerCase().trim()).filter(Boolean);
  for (const tag of t) for (const [section, words] of SECTIONS) if (words.includes(tag)) return section;
  return t.length ? titleCase(t[0]) : "Other";
}


/** The family key of a name: the name without a trailing version ("bridge-v7" → "bridge"). */
function familyKey(name) {
  return clean(name).toLowerCase().replace(/[-_]v\d+$/, "");
}

const nameOfItem = (i) => (i.entry ? i.entry.name : i.link.name) || "";
const authorOfItem = (i) => (i.entry ? i.entry.author : i.link.registrant) || "";

/** Versions of one app: entries by the SAME author account whose names differ only by a
 * trailing version ("x-v6", "x-v7"). A different program stays its own app, even when its
 * name extends another's ("x-token"). Each version list newest first (highest seq). */
function versionsOf(items) {
  const groups = new Map();
  for (const i of list(items).filter((i) => i && (i.entry || i.link))) {
    const versioned = familyKey(nameOfItem(i)) !== clean(nameOfItem(i)).toLowerCase();
    const id = versioned ? authorOfItem(i) + "\u0000" + familyKey(nameOfItem(i)) : "\u0001" + i.address;
    if (!groups.has(id)) groups.set(id, []);
    groups.get(id).push(i);
  }
  const all = [...groups.values()];
  for (const g of all) g.sort((a, b) => Number(b.seq || 0) - Number(a.seq || 0));
  return all;
}

/** An app's title without a trailing version ("Bridge v7" → "Bridge"), for the index. */
function appTitle(title) {
  return title.replace(/\s+v\d+(\.\d+)*$/i, "") || title;
}

/** The index as rows, in reading order:
 *   {type: "heading", text}
 *   {type: "entry", item, title, sentence, publisher, proven, note, secondary, older}
 * One plain line per app, A–Z by title, no headings. Of several versions of one app (see
 * versionsOf) only the newest is listed, under its title without the version; the older ones
 * (`older`, newest first) are on its guide page, not in the index. (`type` stays so a caller can
 * still add headings of its own.) */
export function indexRows(items) {
  const groups = versionsOf(items).map((versions) => {
    const lead = versions[0];
    const t = guideText(lead.entry, nameOfItem(lead));
    const p = publisherOf(lead.entry || { author: authorOfItem(lead) });
    const title = versions.length > 1 ? appTitle(t.title) : t.title;
    return {
      section: sectionOf(lead.entry ? lead.entry.tags : []), title,
      row: { type: "entry", item: lead, title, sentence: t.sentence, publisher: p.text, proven: p.proven,
             note: versions.length > 1 ? `the newest of ${versions.length} versions` : "", secondary: false, older: versions.slice(1) },
    };
  });
  // One flat list, A–Z by title (no section headings: Jef, 2026-11-21).
  groups.sort((a, b) => a.title.localeCompare(b.title, undefined, { sensitivity: "base" }) || a.title.localeCompare(b.title));
  return groups.map((g) => g.row);
}

/** What the guide page leads with, for one listed item: its own title, sentence, About text and
 * publisher, its section, and the other versions of the same app (`versions`: [{address, label,
 * newest}], newest first, without itself). `newest` says whether this item is the newest. */
export function guideOf(item, items) {
  if (!item) return null;
  const all = list(items).some((i) => i && i.address === item.address) ? items : list(items).concat([item]);
  const versions = versionsOf(all).find((g) => g.some((i) => i.address === item.address)) || [item];
  const name = nameOfItem(item);
  const t = guideText(item.entry, name);
  const p = publisherOf(item.entry || { author: item.link ? item.link.registrant : "" });
  return {
    title: t.title, sentence: t.sentence, about: t.about, more: t.more, publisher: p.text, proven: p.proven, account: p.account,
    section: sectionOf(item.entry ? item.entry.tags : []), newest: versions[0].address === item.address,
    versions: versions.filter((i) => i.address !== item.address).map((i, n) => ({ address: i.address, label: guideText(i.entry, nameOfItem(i)).title, newest: i.address === versions[0].address })),
  };
}

/** Where to read an entry's source: an https link (to the commit on GitHub) and its words. null
 * when the entry names no repository, or one this app won't open. */
export function sourceLink(source) {
  const repo = clean(source && source.repo).trim();
  if (!/^https:\/\/[^\s"'<>]+$/i.test(repo)) return null;
  const commit = clean(source.commit).trim();
  const base = repo.replace(/\.git$/, "").replace(/\/+$/, "");
  const github = /^https:\/\/github\.com\/[^/]+\/[^/]+$/i.test(base);
  return { url: github && /^[0-9a-f]{7,40}$/i.test(commit) ? `${base}/tree/${commit}` : base, text: base.replace(/^https:\/\//i, "") };
}

/** How an entry came to be listed, in plain words. */
export function listedBy(kind) {
  if (kind === "deployer") return "Listed by the program's own deployer.";
  if (kind === "community") return "A community listing: written by someone other than the program's deployer.";
  return "This record is not where the registry would put it.";
}

/** A day as people write it ("20 Nov 2026", UTC), or "" for none. */
export function day(ms) {
  if (!ms) return "";
  const d = new Date(Number(ms));
  if (isNaN(d.getTime())) return "";
  return `${d.getUTCDate()} ${["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][d.getUTCMonth()]} ${d.getUTCFullYear()}`;
}

/** The host of an address, for a network line ("lez.nameswallet.app"). */
export function hostOf(url) {
  const m = /^[a-z]+:\/\/([^/:?#]+)/i.exec(clean(url));
  return m ? m[1] : clean(url);
}

/** Whether a Names app reply failed for a network reason worth trying again (busy or unreachable). */
export function reverseRetry(reply) {
  const a = answer(reply);
  return !a.ok && (a.code === "rate_limited" || a.code === "unreachable");
}

/** A name the Names app's `reverse` gave for an account, or "" (no name, no Names app, odd reply). */
export function reverseName(reply) {
  const a = answer(reply);
  const v = a.ok ? a.value : null;
  const name = v && typeof v === "object" ? v.name : typeof v === "string" ? v : "";
  return /^[a-z0-9][a-z0-9-]{0,62}$/i.test(clean(name)) ? clean(name) : "";
}

/** What to ask `search_name` for what a person typed, in order, until one finds something:
 * registry names are lower case with "-" or "_" between words, while the index shows titles
 * ("Names EVM Bridge"). One word is asked as it is; several are tried joined by "-", then by "_",
 * then the longest word alone. */
export function searchForms(typed) {
  const words = clean(typed).toLowerCase().trim().split(/[\s\-_]+/).filter(Boolean);
  if (words.length <= 1) return words;
  const longest = words.slice().sort((a, b) => b.length - a.length)[0];
  return [words.join("-"), words.join("_"), longest].filter((q, i, all) => all.indexOf(q) === i);
}
