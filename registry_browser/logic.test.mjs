// Checks the browser's own logic (qml/logic.mjs, which qml/Main.qml imports) without a QML
// engine. Run with: node registry_browser/logic.test.mjs
import { requestTitle, shortId, answer, kindOf, kindBadge, freshnessText, idlSummary, labelText, when, changeLines, jobText } from "./qml/logic.mjs";

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
check("changeLines freeze", changeLines({ what: "Freeze", entry: P, signers: [P], sequencer: "http://127.0.0.1:3062", summary: { irreversible: true } }),
  ["Freeze of entry 6YWH…VayN", "Signed by 6YWH…VayN with your lezreg wallet", "Sent to http://127.0.0.1:3062", "This can't be undone: a frozen entry never changes again."]);
check("jobText", [jobText({ what: "Freeze", state: "running" }), jobText({ what: "Freeze", state: "applied", receipt: { block: 9 } }), jobText({ what: "Update", state: "failed", code: "missing_key", error: "no key" })],
  ["Freeze: sending…", "Freeze: applied in block 9", "Update: failed (missing_key): no key"]);

check("requestTitle", requestTitle({ from: "evil_ui", change: { what: "Freeze" } }), "evil_ui asks to sign: Freeze");

check("changeLines update shows fields and payer", changeLines({ what: "Update (revision 2)", entry: P, signers: [P], sequencer: "s", summary: { fields: ["version"], payer: P } }),
  ["Update (revision 2) of entry 6YWH…VayN", "Changes: version", "Signed by 6YWH…VayN with your lezreg wallet", "Fee paid by 6YWH…VayN", "Sent to s"]);

console.log(failures ? `${failures} FAILED` : "all ok");
process.exit(failures ? 1 : 0);
