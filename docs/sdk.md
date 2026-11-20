# SDK guide: resolve a program ID to its metadata and IDL

Most consumers (a wallet's approval prompt, an explorer, a developer tool) need one thing:
given a program ID, what is it, is the entry trustworthy, and what is its interface? This guide
walks that path three ways: Rust, JavaScript, and from a Basecamp app through the
`program_registry` module. Nothing here needs an indexer or a server of ours: reads go to the
sequencer you choose and, for the IDL, to a Logos Storage node.

You need a sequencer URL, the registry's program ID on that network, and the program ID to look
up. On a local chain, `chain/deploy.sh` prints the registry ID. For the project's own rc3 test
network, the RPC and registry ID are in the embedded network file (`sdk/rust/network-rc3.json`,
section 1 "Network file"). On Logos LEZ testnet 0.4 it will be in the README once the testnet
launches.

## 1. Rust

[`sdk/rust/examples/resolve_program.rs`](../sdk/rust/examples/resolve_program.rs) is the whole
path, runnable as written. CI runs it against a fresh chain on every push (`e2e.yml`, "SDK worked
example"):

```sh
cargo run -p lezreg-sdk --no-default-features --example resolve_program -- \
    http://127.0.0.1:3062 <registry id> <program id> [storage api]
```

```text
lez_program_registry 0.1.0 by 7Pq…
  The LEZ Program Registry, registered in itself: …
  fresh: the program runs the image the entry claims
  IDL: 6 instructions (spel-v1 0.1.0): register_deployer, register_third_party, update, transfer_author, freeze, deploy_and_register
```

The steps it takes:

**Resolve: one `getAccount`.** `Registry::resolve` reads the deployer entry and the program's
live header together.

```rust
use lezreg_sdk::{Freshness, HttpRpc, Registry};

let reg = Registry::new(HttpRpc::new("http://127.0.0.1:3062"), registry_id);
let resolved = reg.resolve(&program_id)?;
let Some(entry) = &resolved.deployer else { /* not registered by its deployer */ };
println!("{} {} by {}", entry.name, entry.version, entry.author.to_base58());
```

`entry` has the name, version, author (the account proven to have deployed the program),
description, tags, the IDL pointer, the source pointer and the build configuration. A
**deployer** entry can only be written by the program's own header key, so a client can trust
"who registered this" without any off-chain service. Community entries for the same program are
separate (`reg.third_party(&program_id, 20)`) and never mistaken for it.

**Freshness: is the entry still about the running code?**

```rust
match resolved.freshness() {
    Some(Freshness::Fresh) => { /* the program runs the image the entry claims */ }
    Some(Freshness::Stale { live }) => { /* upgraded since: the entry describes other code */ }
    Some(Freshness::NoProgram) => { /* nothing deployed at this account */ }
    None => {}
}
```

Show a stale entry as stale. Its IDL may not match the program anymore.

**IDL: from Logos Storage, checked against the chain.**

```rust
use lezreg_sdk::idl::{FetchConfig, IdlStatus, fetch_idl};

let status = fetch_idl(&entry.idl, &FetchConfig::default());
match &status {
    IdlStatus::Ok(idl) => { /* a format we know: status.info(), status.calls() */ }
    IdlStatus::UnknownFormat(idl) => { /* the right bytes, a format we don't know: show it raw */ }
    other => { /* show "IDL unavailable: {other}", keep showing the entry */ }
}
for call in status.calls() {
    // e.g. "Transfer (tag 2): to: account_id, amount: u128"
    let args: Vec<String> = call.args.iter().map(|a| format!("{}: {}", a.name, a.label)).collect();
    println!("{} (tag {:?}): {}", call.name, call.tag, args.join(", "));
}
```

`fetch_idl` tries a local cache, then your Storage node with retries. Whatever arrives is
hashed and compared with the sha256 on chain before it is parsed. **It never fails the view:** an
unreachable node, a wrong hash or an unparsable IDL each come back as a status (`Unavailable`,
`HashMismatch`, `ParseError`), so the entry's metadata still displays.

**IDL formats.** Programs publish whatever IDL their tooling makes, and on LEZ 0.4 there is more
than one kind. The SDK reads the file and says which kind it is (`status.info()`):

| format | what it looks like | made by |
| --- | --- | --- |
| `spel-v1` | `instructions[]` with accounts and args | `spel generate-idl` up to v0.7.0; this registry's own IDL |
| `spel-v2` | `"version": "2"`, `messages[]`, `state`, `events`, `types` | SPEL's LEZ 0.4 rewrite (logos-co/spel#301) |
| `unknown` | any other JSON object | a format this SDK version doesn't know yet |

So an entry's IDL is in one of four states:

- **ok**: the bytes match the chain and the format is one we know. `status.calls()` lists the
  program's messages (v2) or instructions (v1) with their argument names and types, enough to
  label a call in a wallet.
- **unknown format**: the bytes match the chain and are JSON, but not a format we know. The IDL
  is still there (`status.idl()`); show it raw and say the format is not recognised. It is not
  "unavailable".
- **unavailable** or **hash mismatch**: no trustworthy bytes.
- **unparseable**: the right bytes, but not JSON (or too large or too deeply nested).

The hash check and the canonical form used for uploads (RFC 8785) work on bytes and don't depend
on the format.

**Encode and decode with an IDL.** `lezreg_sdk::idl_codec::Idl` turns typed JSON into Borsh bytes
and back, for messages, actor state and events, from any of the three shapes: SPEL v1, SPEL v2,
and v1 plus this registry's `x-lez04` extension (where state and event layouts live). It needs
no Rust types of the program, so a wallet or explorer can decode any registered program's calls.

```rust
use lezreg_sdk::idl_codec::Idl;

let json: serde_json::Value = serde_json::from_slice(&idl_bytes)?;
let idl = Idl::new(&json);
let bytes = idl.encode_message("freeze", &serde_json::json!({"entry_id": "11…", "now": 5, "via_pda_seed": null}))?;
let (name, fields) = idl.decode_message(&bytes)?;        // ("freeze", {...})
let state = idl.decode_named(idl.state_type().unwrap(), &account_data)?;
```

JSON follows the LEZ CLI conventions: integers up to u64 as numbers, u128/i128 as decimal
strings, account and program ids as base58 or hex, byte arrays as hex, enums as
`{"Variant": {…}}` or a bare `"Variant"` for a unit variant. The CLI has the same codec:
`lezreg idl encode|decode`, `lezreg call --plan` (message bytes, actors to declare and signers,
offline; nothing is signed or sent, and it is not a chain simulation) and
`lezreg image-id <elf|bin> [--compare HEX]`. The registry's own interface is published twice:
`idl/lez_program_registry.idl.json` (SPEL v1 + `x-lez04`, generated from source) and
`idl/lez_program_registry.v2.json`, a SPEL v2 twin written by hand and tested to describe the
same message bytes, state and event selectors (it is not generated from source).
[`PR188.md`](PR188.md) maps these tools to the IDL rows of the prize spec rewrite.

**Network file.** `lezreg_sdk::network_file` parses a network file, v1 or v2 (`"format":
"names-network/1"` with `"version": 2` while LEZ is 0.4; the same file Names Wallet publishes): chain id, RPC, and the program ids of the registry and of LNS.
`network_file::builtin()` is the embedded profile `lezreg --network testnet` uses, and
`chain_matches` compares a file's chain id with the sequencer's `getChannelId`. It is parse-only:
one whole file, no merging, no fetch.

**Source verification** is a local action (`lezreg verify <program> --mark`): it rebuilds the
recorded source with the recorded build configuration and marks the program verified on this
machine if the bytes match. The module's `program_label` reports that mark (section 3).

## 2. JavaScript

[`sdk/js/lezreg.js`](../sdk/js/lezreg.js) has no dependencies and runs in browsers and Node 19+.
It decodes the same records, checked against the Rust SDK by shared vectors
(`sdk/js/vectors.json`).

```js
import { Registry, fetchIdl } from "./lezreg.js";

const reg = new Registry({ rpc: "http://127.0.0.1:3062", registry: "<registry id>" });
const { deployer, freshness } = await reg.resolve("<program id>");
if (deployer) {
  console.log(deployer.name, deployer.version, freshness);           // "fresh", "stale" or "no-program"
  const { kind, status, format, calls } = await fetchIdl(deployer, { storageApi: "http://127.0.0.1:8080/api/storage/v1" });
  // kind: "ok" | "unknown-format" | "unavailable" | "hash-mismatch" | "unparseable"
  if (kind === "ok") console.log(format, calls.map((c) => c.name));      // "spel-v2", ["Transfer", …]
  else if (kind === "unknown-format") console.log("IDL format not recognised");
  else console.log(`IDL unavailable: ${status}`);
}

// One line for a prompt:
const label = await reg.programLabel("<program id>");                  // { text, kind, stale, ... }
```

### Discovery and writes in JS

The JS SDK covers the same discovery surface as `sdk/rust` and builds the same write bytes:

```js
import { Registry } from './lezreg.js';
import { buildChange, signAndSend } from './writes.js';
import { configureSigner } from './signing.js';

const reg = new Registry({ rpc, registry });
const thirdParty = await reg.thirdParty(programId, 20);      // Rust: Registry::third_party
const { items, scanned, truncated } = await reg.byAuthor(authorId, { limit: 20, scan: 1000 }); // Rust: by_author

// IDL statuses use the Rust enum names in `statusKind` as well as the legacy `kind` string:
// "Ok" | "UnknownFormat" | "Unavailable" | "HashMismatch" | "ParseError" | "Pending".

// Writes: build the same instruction bytes and actor list as module/rust-lib/src/writes.rs.
const change = await buildChange('register', input, {
  registry: registryId,
  now,
  liveImage: input.image_id,
});
// change.message is hex Borsh; change.accounts/signers/entry match the Rust `Built::change`.
configureSigner(lezTx);                 // the existing audited lez-tx.js bundle; no keys in this SDK
await signAndSend(change, { rpc });
```

`buildChange` supports `register`, `register_third_party` and `update`; `transfer` and `freeze`
remain module/CLI-only. Signing is deliberately absent from the zero-dependency core: an object
implementing `signHash`, `accountIdOfPrivateKey` and `encodePublicTransaction` must be supplied
explicitly (see `sdk/js/signing.js`). Nothing is sent without it, and a dry run is still the
caller's responsibility before sending. The signer path has not been run against a chain from JS;
the bytes are checked against Rust, the sending is not.

Every JS encoder is checked byte for byte against Rust: `sdk/js/vectors.json` embeds the
`module/rust-lib/testdata/write-vectors.json` cases produced by
`sdk/rust/tests/module_vectors.rs`, and `sdk/rust/tests/js_vectors.rs` regenerates/verifies the
combined file. Run both with `node sdk/js/lezreg.test.mjs` and
`cargo test -p lezreg-sdk --test js_vectors --test module_vectors`.

## 3. From a Basecamp app: the `program_registry` module

Install `program_registry` from the catalog
(`https://raw.githubusercontent.com/jefdiesel/logos-modules/main/logos-repo.json`), point it at
the network once, then call it like any core module. Every method answers
`{success, value, error}`. With no saved settings, a module built from this repository (0.3.0 and
later) starts from the embedded network file: the project's rc3 test network
(`https://lez.nameswallet.app/`), its registry, its LNS program and the trusted registry image.
Saved settings (`config_set`) always win, and a sequencer URL alone no longer selects an LNS
program.

```qml
logos.callModule("program_registry", "config_set", [JSON.stringify({
    sequencer: "http://127.0.0.1:3062", registry: "<registry id>" })])

// A wallet prompt's one line:
var label = JSON.parse(logos.callModule("program_registry", "program_label", ["<program id>"]))
// label.value: { text: "token 1.2.0 ✓ verified", kind: "deployer", verified, stale, upgradeable, ... }

// Everything, and the IDL:
var resolved = JSON.parse(logos.callModule("program_registry", "resolve_program", ["<program id>"]))
var idl = JSON.parse(logos.callModule("program_registry", "get_idl", ["<program id>"]))
// idl.value: { status: "ok" | "unknown-format" | "unavailable" | …, reason, format, calls, idl }
```

The module is pinned to an SDK release, so its `get_idl` may not report the format yet; read it
from the IDL itself (`version: "2"` with `messages` is SPEL v2, `instructions` is SPEL v1).

`program_label.verified` is true only when this machine rebuilt the running image from its
recorded source (`lezreg verify --mark`) and the program still runs that image. Names Wallet
uses exactly this on its approval sheet.

## 4. Apps, verified by publisher

The same module checks Basecamp **apps** against the LNS name that publishes them
([APPS.md](APPS.md)): `verify_app(module_name)` for an installed app, `verify_package(package_path)`
before installing, `publisher_apps(name)` to list a name's apps. Set the LNS program once with
`config_set({"lns": "<program id>"})`. The CLI equivalents are `lezreg app show`,
`lezreg app verify`, `lezreg app record` (with `--upload`, the package goes on Logos Storage and
its CID into the record) and `lezreg app get`, which installs a package by CID only if its
sha256 (required) and recomputed root match what the name vouches for. In Rust,
`lgx::Contents::from_lgx(bytes)` / `from_installed(dir)` recompute a package's root from its
files, `apps::check_following` (or `Names::verify`) checks them against the publisher's name,
and `apps::check_download` checks a download. Key on `Verdict::status()` (or `exit_code()`):
only `verified` proves the whole package; `files_verified` proves the files and not the
manifest. `lgx::check_canonical_layout` is the stricter rule `lezreg app record` applies before
vouching for a package (lgx's tar layout and a manifest lgx writes back unchanged). A check from a manifest alone is no longer
possible: the manifest's `hashes.root` is a claim, and only files prove it. The module now uses
these same SDK calls (`Contents::from_installed` / `from_lgx` and `Names::verify`), so its
`files_verified` and `root_mismatch` verdicts mean what the CLI's do.

A name's records can't be read without the name, but they are not secret: record addresses are
visible on chain and a guessable name can be found by trying it, so treat an app record under a
guessable name as public.

## 5. Search and listing

```rust
let page = reg.list(None, 50)?;                       // newest first; then reg.list(page.next.as_ref(), 50)
let hits = reg.search_name("token", 20, 1000)?;       // exact name first, then substrings
let mine = reg.by_author(&author, 20, 1000)?;         // proven deployers only
```

These walk on-chain links from the sequencer alone, with no indexer. `Link`s are write-once and
cached (`LinkCache::at`), so later walks cost only what is new.
