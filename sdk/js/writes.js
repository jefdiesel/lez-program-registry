// JS write encoders for the LEZ Program Registry, byte-for-byte identical to the Rust
// `module/rust-lib/src/writes.rs` (itself pinned to the real program by module_vectors.rs).
// Zero dependencies. Signing is delegated to an explicitly configured existing signer.
// SPDX-License-Identifier: MIT OR Apache-2.0

import { addr, accountId, fromBase58, fromHex, hex, toBase58 } from './lezreg.js';
import { encodePublicTransaction, hasSigner } from './signing.js';

export const TIME_WINDOW_MS = 10 * 60 * 1000;
const utf8 = new TextEncoder();

// ── minimal borsh writer ──────────────────────────────────────────────────────────────
class Writer {
  constructor() { this.parts = []; }
  raw(bytes) { this.parts.push(bytes); return this; }
  u8(value) { return this.raw(Uint8Array.of(value & 0xff)); }
  u16(value) { const b = new Uint8Array(2); new DataView(b.buffer).setUint16(0, value, true); return this.raw(b); }
  u32(value) { const b = new Uint8Array(4); new DataView(b.buffer).setUint32(0, value >>> 0, true); return this.raw(b); }
  u64(value) { const b = new Uint8Array(8); new DataView(b.buffer).setBigUint64(0, BigInt(value), true); return this.raw(b); }
  bytes(bytes) { return this.u32(bytes.length).raw(bytes); }
  string(text) { return this.bytes(utf8.encode(String(text))); }
  array32(bytes) {
    if (bytes.length !== 32) throw new Error('expected 32 bytes');
    return this.raw(bytes);
  }
  option(value, encode) { return value === null || value === undefined ? this.u8(0) : this.u8(1).raw(encode(value)); }
  vec(items, encode) {
    this.u32(items.length);
    for (const item of items) this.raw(encode(item));
    return this;
  }
  done() {
    const size = this.parts.reduce((n, part) => n + part.length, 0);
    const out = new Uint8Array(size);
    let at = 0;
    for (const part of this.parts) { out.set(part, at); at += part.length; }
    return out;
  }
}
const writer = () => new Writer();

const bytes32 = (value, what) => {
  const bytes = fromHex(value);
  if (bytes.length !== 32) throw new Error(`${what} must be 32 bytes of hex`);
  return bytes;
};
/** Storage CIDs in the module's JSON are multibase 'z' + base58 of the binary CID. */
const cidBytes = (value, what) => {
  const text = String(value ?? '');
  if (!text.startsWith('z')) throw new Error(`${what} must be a base58 CID with a 'z' prefix`);
  const bytes = fromBase58(text.slice(1));
  if (!bytes || !bytes.length) throw new Error(`${what} is not base58`);
  return bytes;
};

function encodeContentRef(value, w = writer()) {
  if (!value) throw new Error('idl is required');
  w.bytes(cidBytes(value.cid, 'idl.cid'));
  w.array32(bytes32(value.sha256, 'idl.sha256'));
  w.u32(value.size);
  return w.option(value.peerHint ?? value.peer_hint, (hint) => writer().bytes(fromHex(hint)).done()).done();
}

function encodeSourceRef(value, w = writer()) {
  if (!value) throw new Error('source is required');
  w.string(value.repo ?? '');
  w.bytes(fromHex(value.commit ?? ''));
  w.string(value.dockerRoot ?? value.docker_root ?? '');
  return w.option(value.archive, (archive) => encodeContentRef(archive)).done();
}

function encodeBuildConfig(value, w = writer()) {
  if (!value) throw new Error('build is required');
  w.u8(value.schema ?? 1).u8(value.builder ?? 1);
  w.array32(bytes32(value.imageDigest ?? value.image_digest, 'build.imageDigest'));
  w.string(value.imageTag ?? value.image_tag).string(value.risc0Build ?? value.risc0_build)
    .string(value.hostRisc0Rust ?? value.host_risc0_rust);
  w.string(value.manifestPath ?? value.manifest_path).string(value.binary);
  w.vec(value.features ?? [], (feature) => writer().string(feature).done());
  w.array32(bytes32(value.lockSha256 ?? value.lock_sha256, 'build.lockSha256'));
  w.vec(value.dockerEnv ?? value.docker_env ?? [], (entry) => writer().string(entry.key).string(entry.value).done());
  return w.done();
}

function fields(v) {
  return {
    name: v.name,
    version: v.version,
    authorLabel: v.author_label ?? '',
    description: v.description ?? '',
    tags: v.tags ?? [],
    idl: encodeContentRef(v.idl),
    source: encodeSourceRef(v.source),
    build: encodeBuildConfig(v.build),
  };
}

function writeFields(w, f) {
  w.string(f.name).string(f.version).string(f.authorLabel).string(f.description);
  w.vec(f.tags, (tag) => writer().string(tag).done());
  return w.raw(f.idl).raw(f.source).raw(f.build);
}

/** revision as 32 little-endian bytes (types/src/pda.rs revision_seed). */
export function revisionSeed(revision) {
  const out = new Uint8Array(32);
  new DataView(out.buffer).setUint32(0, revision >>> 0, true);
  return out;
}

const imageId = (v, liveImage) => {
  if (!v.image_id) {
    if (!liveImage) throw new Error('no program is deployed at that id: pass image_id');
    return typeof liveImage === 'string' ? fromHex(liveImage) : liveImage;
  }
  return bytes32(v.image_id, 'image_id');
};

// ── instruction encoders (variant tag first, as borsh enum) ──────────────────────────

export function encodeRegister(v, { now, liveImage } = {}) {
  if (!v.program) throw new Error('program is required');
  const x = accountId(v.program);
  const author = v.author ? accountId(v.author) : x;
  const f = fields(v);
  const w = writer().u8(0);
  writeFields(w, f);
  w.array32(imageId(v, liveImage)).array32(author).u32(1).raw(revisionSeed(1));
  w.u64(now);
  w.option(null, () => writer().done()); // via_pda_seed
  return w.done();
}

export function encodeRegisterThirdParty(v, { now, liveImage } = {}) {
  const program = accountId(v.program);
  const publisher = accountId(v.publisher);
  if (hex(program) === hex(publisher)) throw new Error("a third-party entry cannot be for the publisher's own account");
  const f = fields(v);
  const w = writer().u8(1);
  w.array32(program);
  writeFields(w, f);
  w.array32(imageId(v, liveImage)).u32(1).raw(revisionSeed(1)).u64(now);
  return w.done();
}

export function encodeUpdate(v, { now, storedRevision } = {}) {
  const entry = accountId(v.entry ?? v.program);
  const revision = storedRevision + 1;
  const w = writer().u8(2);
  w.array32(entry).u32(revision).raw(revisionSeed(revision));
  w.option(v.version, (value) => writer().string(value).done());
  w.option(v.description, (value) => writer().string(value).done());
  w.option(v.tags, (values) => writer().vec(values, (tag) => writer().string(tag).done()).done());
  w.option(v.author_label, (value) => writer().string(value).done());
  w.option(v.idl, (value) => encodeContentRef(value));
  w.option(v.source, (value) => encodeSourceRef(value));
  w.option(v.build, (value) => encodeBuildConfig(value));
  w.option(v.image_id, (value) => writer().array32(bytes32(value, 'image_id')).done());
  w.u64(now);
  w.option(null, () => writer().done()); // via_pda_seed
  return w.done();
}

// ── change building: the same accounts/actors/root/signers as the module's `Built::change` ──

const unique = (items) => [...new Set(items)];
const b58 = (bytes) => typeof bytes === 'string' ? bytes : toBase58(bytes);

/** Build a signer-ready change JSON. No transaction is signed or sent here. */
export async function buildChange(op, v, { registry, now, liveImage, storedRevision } = {}) {
  if (!registry || !now) throw new Error('registry and now are required');
  const reg = accountId(registry);
  let message, root, rest, signers, entry;
  if (op === 'register') {
    const x = accountId(v.program);
    const author = v.author ? accountId(v.author) : x;
    entry = x; root = x; signers = unique([x, author]);
    message = encodeRegister(v, { now, liveImage });
    rest = [author, ...await Promise.all([
      addr.release(reg, x, 1), addr.globalHead(reg), addr.nameHead(reg, v.name),
      addr.authorHead(reg, author), addr.link(reg, x),
    ])];
  } else if (op === 'register_third_party') {
    entry = await addr.thirdParty(reg, accountId(v.program), accountId(v.publisher));
    root = accountId(v.publisher); signers = [root];
    message = encodeRegisterThirdParty(v, { now, liveImage });
    rest = [entry, ...await Promise.all([
      addr.release(reg, entry, 1), addr.globalHead(reg), addr.nameHead(reg, v.name),
      addr.authorHead(reg, root), addr.programHead(reg, accountId(v.program)), addr.link(reg, entry),
    ])];
  } else if (op === 'update') {
    entry = accountId(v.entry ?? v.program);
    root = v.author ? accountId(v.author) : null;
    if (!root) throw new Error('author is required for update');
    signers = [root];
    message = encodeUpdate(v, { now, storedRevision });
    rest = [entry, await addr.release(reg, entry, storedRevision + 1)];
  } else {
    throw new Error(`unsupported op ${op}`);
  }
  const accounts = [root, ...rest];
  return {
    format: 'lezreg-change-v1',
    what: op === 'register' ? 'RegisterDeployer' : op === 'register_third_party' ? 'RegisterThirdParty' : `Update (revision ${storedRevision + 1})`,
    sequencer: null, registry: b58(registry), root: 0,
    accounts: unique(accounts).map((account) => ({ account: b58(account), program: b58(registry), signs: signers.some((s) => hex(s) === hex(account)) })),
    signers: signers.map(b58),
    message: hex(message),
    entry: b58(entry), now, expires_at: now + TIME_WINDOW_MS,
  };
}

/**
 * Sign and submit a prepared change. Requires an explicitly configured signer from the existing
 * audited bundle (see signing.js); the zero-dependency core never holds a key.
 */
export async function signAndSend(change, { rpc, fetch: f } = {}) {
  if (!hasSigner()) throw new Error('no signer configured; call configureSigner() with the existing lez-tx.js bundle');
  if (!rpc) throw new Error('rpc (JSON-RPC sendTransaction client) is required');
  const witnesses = []; // supplied by the signer adapter, which knows the keys and nonces
  const bytes = encodePublicTransaction(change.message, witnesses);
  return rpc.sendTransaction(typeof bytes === 'string' ? bytes : hex(bytes));
}

export { writer as BorshWriter };
