// node sdk/js/lezreg.test.mjs — the JS SDK against the Rust SDK's vectors (vectors.json, written
// by sdk/rust/tests/js_vectors.rs), plus a mocked sequencer and Storage node.
import { readFileSync } from "node:fs";
import {
  LOADER, Registry, addr, accountId, decodeEntry, decodeHead, decodeLink, decodeRelease, fetchIdl,
  fromBase58, fromHex, hex, idlFormat, parseIdl, sha256, toBase58,
} from "./lezreg.js";

const v = JSON.parse(readFileSync(new URL("./vectors.json", import.meta.url)));
let failed = 0;
// Compares as JSON with object keys sorted (the Rust side writes sorted keys).
const canon = (x) => JSON.stringify(x, (_, val) => (val && typeof val === "object" && !Array.isArray(val) ? Object.fromEntries(Object.entries(val).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) : val));
const check = (what, got, want) => {
  const ok = canon(got) === canon(want);
  if (!ok) failed++;
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${ok ? "" : `\n     got  ${JSON.stringify(got)}\n     want ${JSON.stringify(want)}`}`);
};
const throws = async (what, f) => {
  try {
    await f();
    failed++;
    console.log(`FAIL ${what}: did not throw`);
  } catch {
    console.log(`ok   ${what}`);
  }
};

const reg = accountId(v.registry);
const id = (b) => new Uint8Array(32).fill(b);

check("sha256", hex(await sha256(new TextEncoder().encode("abc"))), v["sha256:abc"]);
check("base58 round trip", toBase58(fromBase58(v.registry)), v.registry);
// The loader is 32 bytes of 0xFE (lee_core PROGRAM_LOADER_ACCOUNT_ID; Rust SDK read::LOADER).
check("loader id", LOADER, toBase58(new Uint8Array(32).fill(0xfe)));
check("global head", toBase58(await addr.globalHead(reg)), v.addresses.global_head);
check("name head", toBase58(await addr.nameHead(reg, "alice")), v.addresses["name_head:alice"]);
check("author head", toBase58(await addr.authorHead(reg, id(8))), v.addresses["author_head:08"]);
check("program head", toBase58(await addr.programHead(reg, id(7))), v.addresses["program_head:07"]);
check("link", toBase58(await addr.link(reg, id(7))), v.addresses["link:07"]);
check("release", toBase58(await addr.release(reg, id(7), 2)), v.addresses["release:07:2"]);
check("third-party entry", toBase58(await addr.thirdParty(reg, id(7), id(9))), v.addresses["third_party:07:09"]);

const e = decodeEntry(fromHex(v.entry.borsh));
check("entry fields", [e.program, e.name, e.revision, e.imageId, e.idl.size, e.updatedAt.toString(), e.bornWithDeploy, e.frozen],
  [v.entry.program, v.entry.name, v.entry.revision, v.entry.image_id, v.entry.idl_size, v.entry.updated_at, true, false]);
check("entry nested", [e.tags, e.source.repo, e.build.binary, e.build.dockerEnv, e.idl.peerHint?.length], [["token", "defi"], "https://github.com/acme/token", "token", [{ key: "K", value: "v" }], 4]);
const l = decodeLink(fromHex(v.link.borsh));
check("link fields", [Number(l.seq), l.prevGlobal, l.prevName], [v.link.seq, v.link.prev_global, null]);
const h = decodeHead(fromHex(v.head.borsh));
check("head fields", [Number(h.count), h.newest], [v.head.count, v.head.newest]);
const r = decodeRelease(fromHex(v.release.borsh));
check("release fields", [r.revision, r.at.toString(), r.buildHash], [v.release.revision, v.release.at, v.entry.build_hash]);
await throws("truncated entry refused", () => decodeEntry(fromHex(v.entry.borsh).slice(0, 100)));
await throws("trailing bytes refused", () => decodeHead(fromHex(v.head.borsh + "00")));

// A mocked sequencer holding one deployer entry with a matching header, and its IDL on Storage.
const program = toBase58(id(7));
const entryBytes = fromHex(v.entry.borsh);
const header = new Uint8Array(66); // tag 0 = LoaderEntry::Header
header.set(fromHex(v.entry.image_id), 1);
const idlBytes = new TextEncoder().encode('{"name":"token","version":"1.2.0","instructions":[]}');
const fakeFetch = async (url, init) => {
  if (init?.method === "POST") {
    const { method, params } = JSON.parse(init.body);
    if (method === "getAccount" && params[0] === program)
      return { json: async () => ({ result: { nonce: 0, data: { actor_states: { [v.registry]: [...entryBytes], [LOADER]: [...header] } } } }) };
    return { json: async () => ({ result: null }) };
  }
  return { ok: true, arrayBuffer: async () => idlBytes.buffer };
};
const registry = new Registry({ rpc: "http://127.0.0.1:3062", registry: v.registry, fetch: fakeFetch });
const res = await registry.resolve(program);
check("resolve: fresh deployer", [res.freshness, res.deployer.name], ["fresh", "token-é"]);
check("label", (await registry.programLabel(program)).text, "token-é 1.2.0");
check("unregistered label", (await registry.programLabel(toBase58(id(6)))).kind, "unregistered");
const goodRef = { ...e.idl, sha256: hex(await sha256(idlBytes)), size: idlBytes.length };
check("IDL ok", (await fetchIdl({ idl: goodRef }, { fetch: fakeFetch })).status, "ok");
check("IDL ok: SPEL v1, no calls", (({ kind, format, calls }) => [kind, format, calls])(await fetchIdl({ idl: goodRef }, { fetch: fakeFetch })), ["ok", "spel-v1", []]);
check("IDL hash mismatch refused", (await fetchIdl({ idl: e.idl }, { fetch: fakeFetch })).status.startsWith("hash mismatch"), true);

// IDL formats: every sample classified as the Rust SDK classifies it (status, format, calls).
const sample = (path) => new Uint8Array(readFileSync(new URL(`../../${path}`, import.meta.url)));
for (const [path, want] of Object.entries(v.idl_formats)) {
  const bytes = sample(path);
  check(`IDL sample sha256 ${path}`, hex(await sha256(bytes)), want.sha256);
  const r = parseIdl(bytes);
  const info = r.idl ? idlFormat(r.idl) : null;
  check(`IDL format ${path}`, [r.kind, info && { format: info.format, format_version: info.formatVersion, name: info.name, program_version: info.programVersion }], [want.status, want.info]);
  check(`IDL calls ${path}`, r.calls, want.calls);
}
check("IDL samples cover every kind", [...new Set(Object.values(v.idl_formats).map((x) => x.info?.format ?? x.status))].sort(), ["spel-v1", "spel-v2", "unknown", "unparseable"]);

// The fetch keeps exactly the bytes the chain names, in any format, and only then classifies.
for (const [path, kind, format] of [
  ["sdk/fixtures/idl/spel-v2-state-entry.json", "ok", "spel-v2"],
  ["sdk/fixtures/idl/unknown-object.json", "unknown-format", "unknown"],
  ["sdk/fixtures/idl/garbage.json", "unparseable", null],
]) {
  const bytes = sample(path);
  const serve = async () => ({ ok: true, arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length) });
  const ref = { ...e.idl, sha256: hex(await sha256(bytes)), size: bytes.length };
  const got = await fetchIdl({ idl: ref }, { fetch: serve });
  check(`fetch ${format ?? "garbage"}: kind, format`, [got.kind, got.format, got.idl !== null], [kind, format, kind !== "unparseable"]);
  const wrong = await fetchIdl({ idl: { ...ref, sha256: hex(await sha256(idlBytes)) } }, { fetch: serve });
  check(`fetch ${format ?? "garbage"}: other bytes refused before parsing`, [wrong.kind, wrong.idl], ["hash-mismatch", null]);
}

console.log(failed ? `${failed} FAILED` : "ALL PASS");
process.exit(failed ? 1 : 0);
