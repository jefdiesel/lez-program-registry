import { readFileSync, readdirSync } from "node:fs";
// Checks the browser's own logic (qml/logic.mjs, which qml/Main.qml imports) without a QML
// engine. Run with: node registry_browser/logic.test.mjs
import { clean, requestTitle, shortId, answer, kindOf, kindBadge, freshnessText, idlSummary, labelText, when, changeLines, jobText,
  titleCase, guideText, publisherOf, sectionOf, indexRows, guideOf, sourceLink, listedBy, day, hostOf, reverseName, searchForms } from "./qml/logic.mjs";

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

// ── The index (guidebook list), on the 8 entries of the live rc3 registry (tools/live-rc3.json) ──
const live = JSON.parse(readFileSync(new URL("tools/live-rc3.json", import.meta.url), "utf8")).items;
const byName = (n) => live.find((i) => i.entry.name === n);
check("titleCase keeps acronyms and versions", ["lez-token", "names-evm-bridge-v7", "lez_program_registry", "logos-names", "lez-nft", "my-idl-tool", "rpc  api"].map(titleCase),
  ["LEZ Token", "Names EVM Bridge v7", "LEZ Program Registry", "Logos Names", "LEZ NFT", "My IDL Tool", "RPC API"]);
check("titles of the live entries", live.map((i) => guideText(i.entry).title),
  ["LEZ Token", "Vest", "Logos Names", "Names EVM Bridge v7", "Names EVM Bridge v6", "Names EVM Bridge Token", "LEZ NFT", "LEZ Program Registry"]);
check("Vest: the text before the colon is the title, the rest one sentence", guideText(byName("vest").entry),
  { title: "Vest", sentence: "Privacy-preserving token vesting on LEZ 0.4: schedules, cliffs, private claims, batch, cancel.", about: byName("vest").entry.description });
check("a long lead is not a title (LNS (Logos Name Service) registry: …)", guideText(byName("logos-names").entry).sentence,
  "LNS registry: private names, sealed profiles and pages, optional name NFTs.");
check("a lead that restates the title is dropped", guideText(byName("lez_program_registry").entry).sentence,
  "On-chain registry of LEZ 0.4 programs with proven deployer entries.");
check("asides dropped, first sentence only, \"LEZ 0.4\" not a sentence end", guideText(byName("names-evm-bridge-v7").entry).sentence,
  "Private ETH bridge, instance for Sepolia vault v7.");
check("no description: title from the name, no sentence", guideText({ name: "my-dex", description: "" }), { title: "My DEX", sentence: "", about: "" });
check("a long sentence is cut at a word with …", guideText({ name: "x", description: "word ".repeat(60) }).sentence.length <= 150 && guideText({ name: "x", description: "word ".repeat(60) }).sentence.endsWith("…"), true);
check("chain text is cleaned before anything is derived", guideText({ name: "to‮ken", description: "Evil : line​ two" }), { title: "Evil", sentence: "Line two.", about: "Evil : line two" });
check("a colon lead with markup-ish or punctuation is not a title", guideText({ name: "abc", description: "Hi, there: x" }).title, "Abc");
check("publisher: lns label without its prefix, unproven", publisherOf(byName("vest").entry), { text: "jefdiesel", proven: false, account: false });
check("publisher: no label is the short account (proven by the chain)", publisherOf({ author: P, author_label: "" }), { text: "6YWH…VayN", proven: true, account: true });
check("publisher: a proven label", publisherOf({ author: P, author_label: "lns:alice", author_label_proven: true }).proven, true);
check("sections from tags", [["token", "vesting"], ["vesting", "token"], ["lns", "names"], ["nft", "lns"], ["bridge", "eth"], ["registry", "infrastructure"], ["games"], [], { 0: "dao", length: 1 }].map(sectionOf),
  ["Tokens", "Tokens", "Names", "NFTs", "Bridges", "Tools", "Games", "Other", "DAO"]);
const rows = indexRows(live);
const shown = rows.map((r) => r.type === "heading" ? "# " + r.text : (r.secondary ? "  - " : "") + r.title + (r.note ? " [" + r.note + "]" : ""));
check("the live index: sections in order, families together, A–Z", shown,
  ["# Tokens", "LEZ Token", "Vest", "# Names", "Logos Names", "# NFTs", "LEZ NFT", "# Bridges", "Names EVM Bridge [v7, the newest of 3]", "  - v6", "  - Token", "# Tools", "LEZ Program Registry"]);
check("every live entry appears exactly once", rows.filter((r) => r.type === "entry").map((r) => r.item.address).sort(), live.map((i) => i.address).sort());
check("index rows carry no ids, numbers or versions", rows.filter((r) => r.type === "entry").some((r) => /[1-9A-HJ-NP-Za-km-z]{32,}|#\d|v0\.1\.0|Community|Deployer/.test(r.title + r.sentence + r.publisher + r.note)), false);
check("the family lead is the newest (highest seq)", rows.find((r) => r.family && !r.secondary).item.entry.name, "names-evm-bridge-v7");
// A family is one author's: someone else's "names-evm-bridge-v8" must not lead it.
const intruder = JSON.parse(JSON.stringify(byName("names-evm-bridge-v7")));
Object.assign(intruder, { address: "Intruder1111111111111111111111111111111111", seq: 9 });
Object.assign(intruder.entry, { name: "names-evm-bridge-v8", author: Q, description: "Evil: steals funds." });
const mixed = indexRows(live.concat([intruder]));
check("another author's look-alike stays out of the family", [mixed.find((r) => r.note).item.entry.name, mixed.find((r) => r.item && r.item.address === intruder.address).secondary,
  mixed.find((r) => r.item && r.item.address === intruder.address).title], ["names-evm-bridge-v7", false, "Evil"]);
check("names sharing only a first word are not a family", indexRows([byName("lez-token"), byName("lez-nft")]).filter((r) => r.secondary).length, 0);
check("a search result goes through the same index", indexRows([byName("names-evm-bridge-v6")]).map((r) => r.type === "heading" ? r.text : r.title), ["Bridges", "Names EVM Bridge v6"]);
check("an empty list is an empty index", [indexRows([]), indexRows(null)], [[], []]);
check("a link-only item still shows", indexRows([{ address: "A", seq: 1, kind: "unverified", link: { name: "odd-one", registrant: P } }]).map((r) => r.text || r.title), ["Other", "Odd One"]);
const g6 = guideOf(byName("names-evm-bridge-v6"), live);
check("detail of a family member: full title, the others by variant", [g6.title, g6.family.map((f) => f.label)], ["Names EVM Bridge v6", ["v7 (newest)", "Token"]]);
check("detail of a single entry", (({ title, sentence, publisher, proven, family, section }) => ({ title, sentence, publisher, proven, family, section }))(guideOf(byName("vest"), live)),
  { title: "Vest", sentence: "Privacy-preserving token vesting on LEZ 0.4: schedules, cliffs, private claims, batch, cancel.", publisher: "jefdiesel", proven: false, family: [], section: "Tokens" });
check("sourceLink: GitHub at the commit", sourceLink(byName("vest").entry.source),
  { url: "https://github.com/jefdiesel/vest/tree/c230c15744e3e23120b8b5ec2d72dbcbf7db6416", text: "github.com/jefdiesel/vest" });
check("sourceLink: only https, nothing odd", [sourceLink({ repo: "http://x.org/a", commit: "" }), sourceLink({ repo: "javascript:alert(1)" }), sourceLink({ repo: "https://a b" }), sourceLink(null),
  sourceLink({ repo: "https://git.example/a.git", commit: "abc1234" })], [null, null, null, null, { url: "https://git.example/a", text: "git.example/a" }]);
check("listedBy", ["deployer", "community", "unverified"].map((k) => listedBy(k).split(":")[0].split(" ").slice(0, 3).join(" ")), ["Listed by the", "A community listing", "This record is"]);
check("day", [day(0), day(1795200292766)], ["", "20 Nov 2026"]);
check("hostOf", [hostOf("https://lez.nameswallet.app/"), hostOf("http://127.0.0.1:3040"), hostOf("odd")], ["lez.nameswallet.app", "127.0.0.1", "odd"]);
check("reverseName: the Names app's answer, or nothing", [reverseName(JSON.stringify({ success: true, value: { name: "jefdiesel" } })), reverseName(JSON.stringify({ success: true, value: null })),
  reverseName(""), reverseName(JSON.stringify({ success: false, error: "no" })), reverseName(JSON.stringify({ success: true, value: { name: "<b>x</b>" } }))], ["jefdiesel", "", "", "", ""]);
check("searchForms: one word as typed, titles as names", [searchForms("Vest"), searchForms(" Names EVM  Bridge "), searchForms("program registry"), searchForms("lez_program_registry"), searchForms("")],
  [["vest"], ["names-evm-bridge", "names_evm_bridge", "bridge"], ["program-registry", "program_registry", "registry"], ["lez-program-registry", "lez_program_registry", "registry"], []]);
// Every Text in the QML shows plain text: no markup from the chain renders as a fake mark,
// a link or a fetched image (M6 review C). A Text without it fails here.
for (const f of ["qml/Main.qml", "qml/RegistryLabel.qml", ...readdirSync(new URL("qml/kit/", import.meta.url)).filter((n) => n.endsWith(".qml")).map((n) => "qml/kit/" + n)]) {
  const src = readFileSync(new URL(f, import.meta.url), "utf8");
  const texts = [...src.matchAll(/(?<![A-Za-z.])Text\s*\{/g)];
  const plain = [...src.matchAll(/(?<![A-Za-z])Text\s*\{\s*textFormat:\s*Text\.PlainText/g)].length;
  check(`${f}: every Text is plain text`, plain, texts.length);
  check(`${f}: no rich text anywhere`, /RichText|StyledText|MarkdownText|AutoText|Text\.AutoText/.test(src.replace(/textFormat:\s*Text\.PlainText/g, "")), false);
}

console.log(failures ? `${failures} FAILED` : "all ok");
process.exit(failures ? 1 : 0);
