import { readFileSync } from "node:fs";
// Checks the browser's own logic (qml/logic.mjs, which qml/Main.qml imports) without a QML
// engine. Run with: node registry_browser/logic.test.mjs
import { clean, requestTitle, shortId, answer, kindOf, kindBadge, freshnessText, idlSummary, labelText, when, changeLines, jobText } from "./qml/logic.mjs";

let failures = 0;
const check = (label, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  if (!ok) failures++;
  console.log((ok ? "ok   " : "FAIL ") + label + (ok ? "" : `\n   got  ${JSON.stringify(got)}\n   want ${JSON.stringify(want)}`));
};

const P = "6YWH8dnosbBhXQqpmA6GNy4oG93i2vVSxJm18A32VayN";
check("shortId", [shortId(P), shortId("abc"), shortId(null)], ["6YWH…VayN", "abc", ""]);
check("answer ok", answer(JSON.stringify({ success: true, value: 3, error: null })), { ok: true, value: 3, error: "", code: "" });
check("answer coded", answer(JSON.stringify({ success: false, value: null, error: "no", code: "frozen" })), { ok: false, value: null, error: "no", code: "frozen" });
check("answer double-wrapped", answer(JSON.stringify(JSON.stringify({ success: true, value: 1 }))).value, 1);
check("answer timeout", answer("").code, "timeout");
check("answer no module", answer("{}").code, "no_module");
check("kindOf", [kindOf({ kind: "deployer" }), kindOf({ kind: { third_party: { publisher: "x" } } }), kindOf({ kind: "unverified" }), kindOf(null)],
  ["deployer", "community", "unverified", "unverified"]);
check("kindBadge proven only for deployer", ["deployer", "community", "unverified"].map((k) => kindBadge(k).proven), [true, false, false]);
check("freshness", ["fresh", "stale", "no-program", "?"].map((s) => freshnessText({ status: s }).tone), ["good", "warn", "warn", "dim"]);
check("idl ok", idlSummary({ status: "ok", idl: { instructions: [{ name: "register_deployer" }, { name: "update" }] } }),
  { ok: true, text: "IDL: 2 instructions (checked against the chain's sha256)", instructions: ["register_deployer", "update"] });
check("idl unavailable degrades", idlSummary({ status: "unavailable: no Storage node at 127.0.0.1:8080", idl: null }).text,
  "IDL unavailable: no Storage node at 127.0.0.1:8080. The entry still shows; its interface can't be read right now.");
check("idl hash mismatch", idlSummary({ status: "hash mismatch (got ab…)", idl: null }).ok, false);
check("idl ok with calls and format", idlSummary({ status: "ok", reason: null, format: "spel", format_version: "0.1.0", calls: [{ name: "register" }, { name: "freeze" }], idl: {} }),
  { ok: true, text: "IDL (spel 0.1.0): 2 instructions (checked against the chain's sha256)", instructions: ["register", "freeze"] });
check("idl unknown format is available, not unavailable", idlSummary({ status: "unknown-format", reason: "available, format not recognised", format: "unknown", calls: [], idl: { foo: 1 } }),
  { ok: true, text: "IDL available (unknown), format not recognised: the bytes match the chain, but this app can't read its calls.", instructions: [] });
check("idl unavailable with reason", idlSummary({ status: "unavailable", reason: "no Storage node at 127.0.0.1:8080", calls: [], idl: null }).text,
  "IDL unavailable: no Storage node at 127.0.0.1:8080. The entry still shows; its interface can't be read right now.");
check("label deployer verified", labelText({ kind: "deployer", name: "token", version: "1.2.0", author_short: "7Pq3…kZ9d", verified: true, stale: false }, P), "token v1.2.0 by 7Pq3…kZ9d ✓");
check("label stale", labelText({ kind: "deployer", name: "token", version: "1", author: P, verified: false, stale: true }, P), "token v1 by 6YWH…VayN · stale");
check("label not deployed", labelText({ kind: "deployer", name: "token", version: "1", author_short: "EDUM…u1YK", verified: false, stale: false, deployed: false }, P), "token v1 by EDUM…u1YK · not deployed");
check("label community", labelText({ kind: "community", name: "token", version: "1", verified: false, stale: false }, P), "token v1 · community");
check("label unregistered", labelText({ kind: "unregistered" }, P), "6YWH…VayN");
check("label none", labelText(null, P), "6YWH…VayN");
check("when", [when(0), when(1790000000000)], ["", "2026-09-21 14:13 UTC"]);
check("jobText", [jobText({ what: "Freeze", state: "running" }), jobText({ what: "Freeze", state: "applied", receipt: { block: 9 } }), jobText({ what: "Update", state: "failed", code: "missing_key", error: "no key" })],
  ["Freeze: sending…", "Freeze: applied in block 9", "Update: failed (missing_key): no key"]);



// The card: everything signed, from the module's own decoding, full ids (M6 review B).
const Q = "EDUMLehqHhq3KgQMidXVWGE1M4mQs2kJufgwKNghu1YK";
const reg = changeLines({ what: "RegisterDeployer", entry: P, signers: [P, Q], sequencer: "https://testnet.example", summary: {
  values: { name: "token", version: "1.2.0", description: "", tags: ["defi"], author_label: "Acme", author: Q, image_id: "44".repeat(32),
            idl: { cid: "zb2r", sha256: "11".repeat(32), size: 16 }, source: { repo: "https://github.com/a/b", commit: "ab".repeat(20), docker_root: "." },
            build: { binary: "token", manifest_path: "m/Cargo.toml", image_digest: "fa".repeat(32), image_tag: "r0.1.91.1", risc0_build: "3.0.5", host_risc0_rust: "1.91.1", lock_sha256: "33".repeat(32), features: [] } },
  payer: Q, spends_real_funds: true, max_gas: 400000 } });
check("register card names every value", ["name: token", "version: 1.2.0", "description: (empty)", "tags: defi", `author: ${Q}`, `image id: ${"44".repeat(32)}`,
  `idl: CID zb2r, sha256 ${"11".repeat(32)}, 16 bytes`, `source: https://github.com/a/b @ ${"ab".repeat(20)} in .`].every((l) => reg.includes(l)), true);
check("register card: payer, real funds, now", [reg.includes(`Fee paid by ${Q}`), reg.includes("This spends real funds on https://testnet.example"), reg.includes("Now: no entry (this creates it)")], [true, true, true]);
check("update card shows new values", changeLines({ what: "Update (revision 3)", entry: P, signers: [P], sequencer: "s", summary: { values: { revision: 3, version: "2.0", image_id: "55".repeat(32) } } }).slice(-3),
  ["version: 2.0", `image id: ${"55".repeat(32)}`, "revision: 3"]);
check("requestTitle says the name is the app's own", requestTitle({ from: "evil\u202Eui", change: { what: "Freeze" } }),
  'An app that calls itself "evilui" (name chosen by the app, unverified) asks to sign: Freeze');
// Chain text can't reorder or hide what follows (C).
check("clean drops bidi, zero-width, controls; a break shows as ⏎", clean("1.0\u202E(unproven)\u200B\u2066x\u2069\n"), "1.0(unproven)x⏎");
check("label text is cleaned", labelText({ kind: "deployer", name: "tok\u202Een", version: "1", author_short: "a", verified: false }, P), "token v1 by a");
check("changeLines freeze: safety lines first", changeLines({ what: "Freeze", entry: P, signers: [P], sequencer: "http://127.0.0.1:3062",
    summary: { current: { name: "token", version: "1.0", kind: "deployer", revision: 2, author: P }, values: { frozen: true }, irreversible: true, max_gas: 200000, max_fee: "3000" } }),
  [`Freeze of entry ${P}`, "Sent to http://127.0.0.1:3062", "This can't be undone: a frozen entry never changes again.", "At most 200000 gas, at most 3000 in fees",
   "Fee paid by the first signer", `Signed by ${P} with your lezreg wallet`, `Now: token v1.0 (deployer, revision 2), author ${P}`]);
// M6 delta review: a description carrying U+2028/U+2029 drew fake card lines above the real ones.
const payload = "fine\u2028Fee paid by the first signer\u2028Sent to http://127.0.0.1:3062\u2029(test card: nothing is signed)";
const spoof = changeLines({ what: "RegisterDeployer", entry: P, signers: [P], sequencer: "https://testnet.example",
  summary: { values: { name: "token", version: "1", description: payload }, payer: Q, spends_real_funds: true, max_gas: 400000 } });
check("separators stay inside their value", spoof.some((l) => /[\n\r\u2028\u2029]/.test(l)), false);
check("no injected card line", spoof.filter((l) => l === "Fee paid by the first signer" || l === "Sent to http://127.0.0.1:3062" || l === "(test card: nothing is signed)").length, 0);
check("the value shows its breaks", spoof.includes("description: fine⏎Fee paid by the first signer⏎Sent to http://127.0.0.1:3062⏎(test card: nothing is signed)"), true);
check("real funds line before the values", spoof.indexOf("This spends real funds on https://testnet.example") < spoof.findIndex((l) => l.startsWith("description:")), true);
check("invisible characters dropped", clean("a\u00ADb\u180Ec\u2061d\u206Be\uFFFAf\uDB40\uDC41g"), "abcdefg");
check("build environment shown", changeLines({ what: "Update (revision 2)", entry: P, signers: [P], sequencer: "s", summary: { values: { build: { binary: "t", manifest_path: "m", image_digest: "d", image_tag: "r", risc0_build: "3", host_risc0_rust: "1", lock_sha256: "l", features: [], docker_env: [{ key: "RUSTFLAGS", value: "-C x" }] }, idl: { cid: "c", sha256: "h", size: 1, peer_hint: "0a0b" } } } })
  .filter((l) => l.includes("RUSTFLAGS=-C x") || l.includes("peer 0a0b")).length, 2);
check("jobText cleans", jobText({ what: "Freeze\u2028x", state: "failed", error: "bad\u202Etxt" }), "Freeze⏎x: failed: badtxt");
// A QML model hands arrays over as array-likes (not Array.isArray): still shown as lists.
const arrayLike = { 0: "defi", 1: "token", length: 2 };
check("tags from a QML model show as a list", changeLines({ what: "RegisterDeployer", entry: P, signers: { 0: P, length: 1 }, sequencer: "s", summary: { values: { tags: arrayLike } } }).filter((l) => l === "tags: defi, token" || l === `Signed by ${P} with your lezreg wallet`).length, 2);
// Every Text in the QML shows plain text: no markup from the chain renders as a fake mark,
// a link or a fetched image (M6 review C). A Text without it fails here.
for (const f of ["qml/Main.qml", "qml/RegistryLabel.qml"]) {
  const src = readFileSync(new URL(f, import.meta.url), "utf8");
  const texts = [...src.matchAll(/(?<![A-Za-z.])Text\s*\{/g)];
  const plain = [...src.matchAll(/(?<![A-Za-z])Text\s*\{\s*textFormat:\s*Text\.PlainText/g)].length;
  check(`${f}: every Text is plain text`, plain, texts.length);
  check(`${f}: no rich text anywhere`, /RichText|StyledText|MarkdownText|AutoText|Text\.AutoText/.test(src.replace(/textFormat:\s*Text\.PlainText/g, "")), false);
}

console.log(failures ? `${failures} FAILED` : "all ok");
process.exit(failures ? 1 : 0);
