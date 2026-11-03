// @lezreg/sdk: read the LEZ Program Registry from a browser or Node, with no dependencies
// (DESIGN §7.1, M7). Mirrors the Rust SDK's reads: addresses by SPEL's compute_pda rule under
// LEZ's for_public_pda, borsh decoders for every record, resolve / label / list / search, and
// the IDL fetch with its sha256 check. sdk/rust/tests/js_vectors.rs pins the two together.
//
//   import { Registry } from "./lezreg.js";
//   const reg = new Registry({ rpc: "http://127.0.0.1:3062", registry: "<registry id>" });
//   const label = await reg.programLabel("<program id>");   // { text, kind, stale, ... }
//
// It talks only to the sequencer (and Storage node) the caller names.

const SEEDS = {
  thirdParty: "lezreg/v1/tp",
  release: "lezreg/v1/rel",
  link: "lezreg/v1/link",
  head: "lezreg/v1/head",
  name: "lezreg/v1/name",
  author: "lezreg/v1/author",
  programHead: "lezreg/v1/tphead",
  authorNode: "lezreg/v1/anode",
};
const PDA_PREFIX = new TextEncoder().encode("/LEE/v0.2/AccountId/PDA/\0\0\0\0\0\0\0\0");
/** The loader program's id: program headers live under it. */
export const LOADER = "11111111111111111111111111111111";
/** IDLs above this are refused before parsing (DESIGN §5.2). */
export const MAX_IDL = 120 * 1024;
const FLAG_FROZEN = 1;
const FLAG_BORN_WITH_DEPLOY = 2;

// ── bytes ──────────────────────────────────────────────────────────────────────────────

export function hex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function fromHex(s) {
  const clean = s.startsWith("0x") ? s.slice(2) : s;
  if (clean.length % 2 || /[^0-9a-f]/i.test(clean)) throw new Error(`not hex: ${s}`);
  return Uint8Array.from(clean.match(/../g) ?? [], (h) => parseInt(h, 16));
}

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

export function toBase58(bytes) {
  let n = 0n;
  for (const b of bytes) n = (n << 8n) | BigInt(b);
  let out = "";
  while (n > 0n) {
    out = B58[Number(n % 58n)] + out;
    n /= 58n;
  }
  for (const b of bytes) {
    if (b !== 0) break;
    out = "1" + out;
  }
  return out;
}

export function fromBase58(s) {
  let n = 0n;
  for (const c of s) {
    const i = B58.indexOf(c);
    if (i < 0) throw new Error(`not base58: ${s}`);
    n = n * 58n + BigInt(i);
  }
  const out = [];
  while (n > 0n) {
    out.unshift(Number(n & 0xffn));
    n >>= 8n;
  }
  for (const c of s) {
    if (c !== "1") break;
    out.unshift(0);
  }
  return Uint8Array.from(out);
}

/** A 32-byte account id from base58; throws otherwise. */
export function accountId(s) {
  const b = fromBase58(String(s).trim());
  if (b.length !== 32) throw new Error(`'${s}' is not an account id`);
  return b;
}

export async function sha256(bytes) {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) throw new Error("no WebCrypto (crypto.subtle): needs a browser or Node 19+");
  return new Uint8Array(await subtle.digest("SHA-256", bytes));
}

function concat(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

// ── addresses ─────────────────────────────────────────────────────────────────────────

/** A string seed component: UTF-8, zero-padded to 32 bytes. */
export function seedStr(s) {
  const b = new TextEncoder().encode(s);
  if (b.length > 32) throw new Error(`seed '${s}' is longer than 32 bytes`);
  const out = new Uint8Array(32);
  out.set(b);
  return out;
}

/** SPEL compute_pda: one component is the seed, several are sha256(c1 ‖ c2 ‖ …). */
export async function combine(parts) {
  return parts.length === 1 ? parts[0] : sha256(concat(...parts));
}

/** LEZ AccountId::for_public_pda. */
export async function forPublicPda(program, seed) {
  return sha256(concat(PDA_PREFIX, program, seed));
}

function revisionSeed(revision) {
  const s = new Uint8Array(32);
  new DataView(s.buffer).setUint32(0, revision, true);
  return s;
}

/** Every registry-owned address, as bytes. `reg` and ids are byte arrays. */
export const addr = {
  globalHead: async (reg) => forPublicPda(reg, await combine([seedStr(SEEDS.head)])),
  nameHead: async (reg, name) => forPublicPda(reg, await combine([seedStr(SEEDS.name), seedStr(name)])),
  authorHead: async (reg, author) => forPublicPda(reg, await combine([seedStr(SEEDS.author), author])),
  programHead: async (reg, program) => forPublicPda(reg, await combine([seedStr(SEEDS.programHead), program])),
  link: async (reg, entry) => forPublicPda(reg, await combine([seedStr(SEEDS.link), entry])),
  release: async (reg, entry, revision) => forPublicPda(reg, await combine([seedStr(SEEDS.release), entry, revisionSeed(revision)])),
  thirdParty: async (reg, program, publisher) => forPublicPda(reg, await combine([seedStr(SEEDS.thirdParty), program, publisher])),
  authorNode: async (reg, newAuthor, entry) => forPublicPda(reg, await combine([seedStr(SEEDS.authorNode), newAuthor, entry])),
};

// ── borsh ─────────────────────────────────────────────────────────────────────────────

class Reader {
  constructor(bytes) {
    this.b = bytes;
    this.at = 0;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  need(n) {
    if (this.at + n > this.b.length) throw new Error("record is truncated");
  }
  u8() {
    this.need(1);
    return this.b[this.at++];
  }
  u32() {
    this.need(4);
    const v = this.view.getUint32(this.at, true);
    this.at += 4;
    return v;
  }
  u64() {
    this.need(8);
    const v = this.view.getBigUint64(this.at, true);
    this.at += 8;
    return v;
  }
  fixed(n) {
    this.need(n);
    const v = this.b.slice(this.at, this.at + n);
    this.at += n;
    return v;
  }
  bytes(max = 1 << 20) {
    const n = this.u32();
    if (n > max) throw new Error(`byte field of ${n} is over its limit`);
    return this.fixed(n);
  }
  str(max = 4096) {
    return new TextDecoder("utf-8", { fatal: true }).decode(this.bytes(max));
  }
  vec(read, max = 1024) {
    const n = this.u32();
    if (n > max) throw new Error(`list of ${n} is over its limit`);
    return Array.from({ length: n }, () => read());
  }
  option(read) {
    const tag = this.u8();
    if (tag > 1) throw new Error("bad option tag");
    return tag === 1 ? read() : null;
  }
  id() {
    return toBase58(this.fixed(32));
  }
  end() {
    if (this.at !== this.b.length) throw new Error(`${this.b.length - this.at} trailing bytes`);
  }
}

function contentRef(r) {
  return { cid: r.bytes(64), sha256: hex(r.fixed(32)), size: r.u32(), peerHint: r.option(() => r.bytes(48)) };
}

function sourceRef(r) {
  return { repo: r.str(), commit: hex(r.bytes(32)), dockerRoot: r.str(), archive: r.option(() => contentRef(r)) };
}

function buildConfig(r) {
  return {
    schema: r.u8(),
    builder: r.u8(),
    imageDigest: hex(r.fixed(32)),
    imageTag: r.str(),
    risc0Build: r.str(),
    hostRisc0Rust: r.str(),
    manifestPath: r.str(),
    binary: r.str(),
    features: r.vec(() => r.str()),
    lockSha256: hex(r.fixed(32)),
    dockerEnv: r.vec(() => ({ key: r.str(), value: r.str() })),
  };
}

function finish(r, v) {
  r.end();
  return v;
}

export function decodeEntry(bytes) {
  const r = new Reader(bytes);
  const e = {
    schema: r.u8(),
    kind: r.u8(),
    flags: r.u8(),
    program: r.id(),
    author: r.id(),
    registrant: r.id(),
    name: r.str(32),
    version: r.str(32),
    authorLabel: r.str(64),
    description: r.str(512),
    tags: r.vec(() => r.str(24), 8),
    idl: contentRef(r),
    source: sourceRef(r),
    build: buildConfig(r),
    imageId: hex(r.fixed(32)),
    registeredAt: r.u64(),
    updatedAt: r.u64(),
    revision: r.u32(),
  };
  e.frozen = (e.flags & FLAG_FROZEN) !== 0;
  e.bornWithDeploy = (e.flags & FLAG_BORN_WITH_DEPLOY) !== 0;
  return finish(r, e);
}

export function decodeRelease(bytes) {
  const r = new Reader(bytes);
  return finish(r, {
    schema: r.u8(),
    entry: r.id(),
    revision: r.u32(),
    version: r.str(32),
    idl: contentRef(r),
    source: sourceRef(r),
    build: buildConfig(r),
    imageId: hex(r.fixed(32)),
    buildHash: hex(r.fixed(32)),
    at: r.u64(),
  });
}

export function decodeLink(bytes) {
  const r = new Reader(bytes);
  return finish(r, {
    schema: r.u8(),
    seq: r.u64(),
    kind: r.u8(),
    program: r.id(),
    name: r.str(32),
    author: r.id(),
    prevGlobal: r.option(() => r.id()),
    prevName: r.option(() => r.id()),
    prevAuthor: r.option(() => r.id()),
    prevProgram: r.option(() => r.id()),
  });
}

export function decodeHead(bytes) {
  if (bytes.length === 0) return { schema: 1, count: 0n, newest: null };
  const r = new Reader(bytes);
  return finish(r, { schema: r.u8(), count: r.u64(), newest: r.option(() => r.id()) });
}

/** A loader program header: 32-byte image id, first segment, immutable flag. */
export function decodeHeader(bytes) {
  if (bytes.length !== 65 || bytes[64] > 1) return null;
  return { imageId: hex(bytes.slice(0, 32)), firstSegment: toBase58(bytes.slice(32, 64)), immutable: bytes[64] === 1 };
}

// ── the registry ──────────────────────────────────────────────────────────────────────

/** `getAccount`'s reply: program id (base58) → state bytes. */
function actorStates(account) {
  const out = new Map();
  for (const [program, bytes] of Object.entries(account?.data?.actor_states ?? {})) {
    if (!Array.isArray(bytes) || bytes.some((b) => !Number.isInteger(b) || b < 0 || b > 255)) throw new Error("bad state bytes");
    out.set(program, Uint8Array.from(bytes));
  }
  return out;
}

export class Registry {
  /** `rpc`: the sequencer URL; `registry`: its program id (base58); `fetch`: optional. */
  constructor({ rpc, registry, fetch: f } = {}) {
    if (!rpc || !registry) throw new Error("Registry needs { rpc, registry }");
    this.rpc = rpc;
    this.reg = accountId(registry);
    this.regText = registry;
    this.fetch = f ?? globalThis.fetch.bind(globalThis);
  }

  async call(method, params) {
    const res = await this.fetch(this.rpc, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
      redirect: "error",
    });
    const reply = await res.json();
    if (reply.error) throw new Error(`${method}: ${JSON.stringify(reply.error)}`);
    return reply.result;
  }

  /** The registry's state on `address` (bytes; empty if none). */
  async state(address) {
    const result = await this.call("getAccountView", [{ account_id: address, program_account_id: this.regText }]);
    return actorStates(result).get(this.regText) ?? new Uint8Array();
  }

  /** `{program, deployer, header, freshness}` in one `getAccount`. */
  async resolve(programId) {
    accountId(programId);
    const account = await this.call("getAccount", [programId]);
    const states = actorStates(account);
    const entryBytes = states.get(this.regText);
    const headerBytes = states.get(LOADER);
    const deployer = entryBytes?.length ? decodeEntry(entryBytes) : null;
    const header = headerBytes ? decodeHeader(headerBytes) : null;
    const freshness = !deployer ? null : !header ? "no-program" : header.imageId === deployer.imageId ? "fresh" : "stale";
    return { program: programId, deployer, header, freshness };
  }

  /** A one-line label for a wallet prompt: `{text, kind, stale, upgradeable, name, version, author}`. */
  async programLabel(programId) {
    const r = await this.resolve(programId);
    const short = `${programId.slice(0, 6)}…${programId.slice(-4)}`;
    const upgradeable = !!r.header && !r.header.immutable;
    if (!r.deployer) return { text: `${short} (not in the registry)`, kind: "unregistered", stale: false, upgradeable, deployed: !!r.header };
    const stale = r.freshness !== "fresh";
    const e = r.deployer;
    return {
      text: `${e.name} ${e.version}${stale ? " (stale: the program runs other code than registered)" : ""}`,
      kind: "deployer",
      stale,
      upgradeable,
      name: e.name,
      version: e.version,
      author: e.author,
      authorLabelUnproven: e.authorLabel,
    };
  }

  /** Walks `Link`s from `start` along `prev`, up to `limit`, decoding each entry too. */
  async walk(start, limit, prev) {
    const items = [];
    let at = start;
    const seen = new Set();
    while (at && items.length < limit) {
      if (seen.has(at)) throw new Error(`the list loops at ${at}`);
      seen.add(at);
      const linkAt = toBase58(await addr.link(this.reg, accountId(at)));
      const linkBytes = await this.state(linkAt);
      if (!linkBytes.length) throw new Error(`entry ${at} has no link (private or unverifiable)`);
      const link = decodeLink(linkBytes);
      const entryBytes = await this.state(at);
      items.push({ address: at, link, entry: entryBytes.length ? decodeEntry(entryBytes) : null });
      at = link[prev];
    }
    return { items, next: at && items.length === limit ? items[items.length - 1].address : null };
  }

  /** The global list, newest first. Pass `next` back as `after`. */
  async list({ after = null, limit = 20 } = {}) {
    const head = decodeHead(await this.state(toBase58(await addr.globalHead(this.reg))));
    const start = after ? decodeLink(await this.state(toBase58(await addr.link(this.reg, accountId(after))))).prevGlobal : head.newest;
    const page = await this.walk(start, limit, "prevGlobal");
    return { ...page, total: head.count };
  }

  /** Every entry registered under exactly `name`, newest first. */
  async byName(name, limit = 50) {
    const head = decodeHead(await this.state(toBase58(await addr.nameHead(this.reg, name.toLowerCase()))));
    return (await this.walk(head.newest, limit, "prevName")).items;
  }
}

/**
 * Fetches IDL bytes from a Logos Storage node and keeps them only if they match the entry's
 * sha256 and size; then parses. Returns `{status, idl}`; never throws for the IDL itself.
 */
export async function fetchIdl(entry, { storageApi = "http://127.0.0.1:8080/api/storage/v1", fetch: f } = {}) {
  const get = f ?? globalThis.fetch.bind(globalThis);
  const ref = entry.idl;
  if (ref.size > MAX_IDL) return { status: "unavailable: too large", idl: null };
  const cid = "z" + toBase58(ref.cid);
  let bytes;
  try {
    const res = await get(`${storageApi}/data/${cid}/network/stream?advertise=false`, { redirect: "error" });
    if (!res.ok) return { status: `unavailable: HTTP ${res.status}`, idl: null };
    bytes = new Uint8Array(await res.arrayBuffer());
  } catch (e) {
    return { status: `unavailable: ${e.message}`, idl: null };
  }
  if (bytes.length > MAX_IDL) return { status: "unavailable: too large", idl: null };
  const got = hex(await sha256(bytes));
  if (got !== ref.sha256 || bytes.length !== ref.size) return { status: `hash mismatch: the chain names ${ref.sha256}, the bytes hash to ${got}`, idl: null };
  try {
    const idl = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
    if (!idl || typeof idl !== "object" || !Array.isArray(idl.instructions)) return { status: "not a usable IDL", idl: null };
    return { status: "ok", idl };
  } catch (e) {
    return { status: `not a usable IDL: ${e.message}`, idl: null };
  }
}
