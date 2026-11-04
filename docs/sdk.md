# SDK guide: resolve a program ID to its metadata and IDL

Most consumers (a wallet's approval prompt, an explorer, a developer tool) need one thing:
given a program ID, what is it, is the entry trustworthy, and what is its interface? This guide
walks that path three ways: Rust, JavaScript, and from a Basecamp app through the
`program_registry` module. Nothing here needs an indexer or a server of ours: reads go to the
sequencer you choose and, for the IDL, to a Logos Storage node.

You need a sequencer URL, the registry's program ID on that network, and the program ID to look
up. On a local chain, `chain/deploy.sh` prints the registry ID. On LEZ testnet 0.4 it will be in
the README once the testnet launches.

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
  IDL: 6 instructions
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

match fetch_idl(&entry.idl, &FetchConfig::default()) {
    IdlStatus::Ok(idl) => { /* SPEL IDL JSON: instructions, accounts, types */ }
    other => { /* show "IDL unavailable: {other}", keep showing the entry */ }
}
```

`fetch_idl` tries a local cache, then your Storage node with retries. Whatever arrives is
hashed and compared with the sha256 on chain before it is parsed. **It never fails the view:** an
unreachable node, a wrong hash or an unparsable IDL each come back as a status (`Unavailable`,
`HashMismatch`, `ParseError`), so the entry's metadata still displays.

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
  const { status, idl } = await fetchIdl(deployer, { storageApi: "http://127.0.0.1:8080/api/storage/v1" });
  console.log(status === "ok" ? idl.instructions.length : `IDL unavailable: ${status}`);
}

// One line for a prompt:
const label = await reg.programLabel("<program id>");                  // { text, kind, stale, ... }
```

## 3. From a Basecamp app: the `program_registry` module

Install `program_registry` from the catalog
(`https://raw.githubusercontent.com/jefdiesel/logos-modules/main/logos-repo.json`), point it at
the network once, then call it like any core module. Every method answers
`{success, value, error}`.

```qml
logos.callModule("program_registry", "config_set", [JSON.stringify({
    sequencer: "http://127.0.0.1:3062", registry: "<registry id>" })])

// A wallet prompt's one line:
var label = JSON.parse(logos.callModule("program_registry", "program_label", ["<program id>"]))
// label.value: { text: "token 1.2.0 ✓ verified", kind: "deployer", verified, stale, upgradeable, ... }

// Everything, and the IDL:
var resolved = JSON.parse(logos.callModule("program_registry", "resolve_program", ["<program id>"]))
var idl = JSON.parse(logos.callModule("program_registry", "get_idl", ["<program id>"]))
// idl.value: { status: "ok", idl: {...} } or { status: "unavailable: …", idl: null }
```

`program_label.verified` is true only when this machine rebuilt the running image from its
recorded source (`lezreg verify --mark`) and the program still runs that image. Names Wallet
uses exactly this on its approval sheet.

## 4. Apps, verified by publisher

The same module checks Basecamp **apps** against the LNS name that publishes them
([APPS.md](APPS.md)): `verify_app(module_name)` for an installed app, `verify_package(manifest)`
before installing, `publisher_apps(name)` to list a name's apps. Set the LNS program once with
`config_set({"lns": "<program id>"})`. The CLI equivalents are `lezreg app show`,
`lezreg app verify` and `lezreg app record`.

## 5. Search and listing

```rust
let page = reg.list(None, 50)?;                       // newest first; then reg.list(page.next.as_ref(), 50)
let hits = reg.search_name("token", 20, 1000)?;       // exact name first, then substrings
let mine = reg.by_author(&author, 20, 1000)?;         // proven deployers only
```

These walk on-chain links from the sequencer alone, with no indexer. `Link`s are write-once and
cached (`LinkCache::at`), so later walks cost only what is new.
