# Integrating the registry: turn a program id into a label and an IDL

You have a program id (a wallet approval, a transaction list, an explorer row) and want to show
what it is. The LEZ Program Registry answers: its name and version, who deployed it (proven by
the chain, not by a label), whether it still runs the code that was registered, whether this
machine has rebuilt that code from source, and its IDL. Pick the path for where your code runs.

| You are | Use | Section |
|---|---|---|
| A Basecamp UI module (QML) | the `program_registry` core module, through `RegistryLabel.qml` or one call | [1](#1-a-basecamp-ui-module-qml) |
| Any Logos module or script | the same module through `logosctl` / the Logos Core IPC | [2](#2-logosctl-or-any-logos-core-client) |
| A web page or Node program | the JS SDK, `sdk/js/lezreg.js` (no dependencies) | [3](#3-javascript) |
| A Rust program | the Rust SDK, `lezreg-sdk` | [4](#4-rust) |
| Something that writes entries | the module's `prepare_*` + `submit`, or `lezreg` | [5](#5-writing-entries) |

Every path reads only the sequencer you choose and, for the IDL, a Logos Storage node. There is
no server of ours in between.

## 1. A Basecamp UI module (QML)

**Declare the dependency** in your `metadata.json` (and, as with any core module dependency of a
`ui_qml` package, the matching `program_registry` flake input):

```json
"dependencies": [{ "name": "program_registry", "version": ">=0.3.0" }]
```

**Drop in the label.** Copy [`registry_browser/qml/RegistryLabel.qml`](../registry_browser/qml/RegistryLabel.qml)
into your `qml/` folder. It is one file and needs only `QtQuick`:

```qml
RegistryLabel {
    programId: tx.program                 // base58 program id
    textColor: Theme.palette.text         // optional: bind its colours to your theme
    onResolved: (info) => console.log(info.kind, info.name, info.verified)
}
```

It shows `token v1.2.0 by 7Pq3…kZ9d ✓` for a deployer entry (✓ only when this machine rebuilt the
running code from its recorded source), `token v1.2.0 · community` when only others described the
program, `· stale` when the program now runs other code than registered, `· not deployed` when
nothing runs at that id yet, and the short id when the
program is not registered, or when the module is missing: it never breaks your screen. `info` holds
the whole answer.

**Or make the call yourself** (the same thing in ten lines):

```qml
function registryLabel(programId, done) {
    logos.callModuleAsync("program_registry", "program_label", [programId], function (reply) {
        var r = JSON.parse(reply)
        if (!r || !r.success) return done(programId.slice(0, 4) + "…" + programId.slice(-4))
        var v = r.value   // {text, kind, name, version, author, author_short, verified, stale, upgradeable, …}
        done(v.kind === "unregistered" ? programId.slice(0, 4) + "…" + programId.slice(-4)
             : v.name + " v" + v.version + (v.kind === "deployer" ? " by " + v.author_short : " · community") + (v.verified ? " ✓" : ""))
    }, 8000)
}
```

**The IDL:** `get_idl(programId)` answers `{status, reason, format, format_version, calls, idl}`.
`status` is `ok` (a format the SDK reads: `calls` lists `[{name, tag, args}]`), `unknown-format`
(the bytes match the chain but the format is not one the SDK knows: `idl` still holds it; show
"available, format not recognised"), or `unavailable` / `hash-mismatch` / `unparseable` with
`reason` in words. It never fails because of the IDL, so keep showing the entry.

**Everything at once:** `resolve_program(programId)` gives the deployer entry, the live program
header and `freshness` (`fresh`, `stale`, `no-program`), plus how many community entries exist.

The module needs its network once (a settings screen, or the registry browser's):
`config_set('{"sequencer": "https://…", "registry": "<registry program id>"}')`.

The registry browser ([`registry_browser/`](../registry_browser/)) is a complete example: list,
search, entry detail with the deployer/community badge, IDL status with its degradation, the local
verify mark, and the label component.

## 2. `logosctl` or any Logos Core client

```sh
logosctl install --dir <folder holding program_registry's .lgx> -y
logosctl module load program_registry
logosctl --json call program_registry config_set '{"sequencer":"http://127.0.0.1:3062","registry":"<REG>"}'
logosctl --json call program_registry program_label <program id>
logosctl --json call program_registry resolve_program <program id>
logosctl --json call program_registry get_idl <program id>
```

Run on 2026-11-13 (logosctl 0.3.2, macOS, a fresh LEZ v0.4.0-rc3 standalone sequencer with
the registry deployed by `chain/deploy.sh` as `GnoF…TDs7`), answers abridged:

```text
$ logosctl --json call program_registry program_label GnoF…TDs7
{"success": true, "value": {"kind": "deployer", "name": "lez_program_registry", "version": "0.1.0",
 "author_short": "GnoF…TDs7", "stale": false, "upgradeable": true, "verified": false, "deployed": true, …}}
$ logosctl --json call program_registry get_idl GnoF…TDs7
{"success": true, "value": {"status": "ok", "format": "spel-v1", "format_version": "0.1.0",
 "calls": [{"name": "register_deployer", …}, …6 calls], "idl": {…}}}
```

## 3. JavaScript

[`sdk/js/examples/label.mjs`](../sdk/js/examples/label.mjs), the whole path:

```js
import { Registry, fetchIdl } from "../lezreg.js";   // sdk/js/lezreg.js: no dependencies

const [rpc, registry, program, storageApi] = process.argv.slice(2);
const reg = new Registry({ rpc, registry });
const label = await reg.programLabel(program);           // { text, kind, stale, ... }
const { deployer } = await reg.resolve(program);
const { status, idl } = deployer ? await fetchIdl(deployer, { storageApi }) : {};
console.log(label.text, status === "ok" ? `${idl.instructions.length} instructions` : `IDL unavailable: ${status}`);
```

Run with Node 25 against the same chain and its Storage node, then with no Storage node:

```text
$ node sdk/js/examples/label.mjs http://127.0.0.1:3071 GnoF…TDs7 GnoF…TDs7 http://127.0.0.1:8095/api/storage/v1
lez_program_registry 0.1.0 6 instructions
$ node sdk/js/examples/label.mjs http://127.0.0.1:3071 GnoF…TDs7 GnoF…TDs7 http://127.0.0.1:8099/api/storage/v1
lez_program_registry 0.1.0 IDL unavailable: unavailable: fetch failed
```

## 4. Rust

```rust
use lezreg_sdk::{HttpRpc, Registry, idl::{fetch_idl, FetchConfig, IdlStatus}};

let reg = Registry::new(HttpRpc::new("http://127.0.0.1:3062"), registry_id);
let r = reg.resolve(&program_id)?;                       // one getAccount
if let Some(e) = &r.deployer {
    println!("{} {} by {} ({:?})", e.name, e.version, e.author.to_base58(), r.freshness());
    match fetch_idl(&e.idl, &FetchConfig::default()) {
        IdlStatus::Ok(idl) => println!("IDL: {} instructions", idl["instructions"].as_array().map_or(0, Vec::len)),
        other => println!("IDL unavailable: {other}"),
    }
}
```

`lezreg-sdk` with `default-features = false` has no LEZ dependency. The runnable version is
[`sdk/rust/examples/resolve_program.rs`](../sdk/rust/examples/resolve_program.rs) (see
[sdk.md](sdk.md)).

```text
$ cargo run -p lezreg-sdk --no-default-features --example resolve_program -- \
      http://127.0.0.1:3071 GnoF…TDs7 GnoF…TDs7 http://127.0.0.1:8095/api/storage/v1
lez_program_registry 0.1.0 by GnoFoiAs8xP31Tey8s92wYC7URsqrrDgenY5X6geTDs7
  The LEZ Program Registry, registered in itself: …
  fresh: the program runs the image the entry claims
  IDL: 6 instructions (spel-v1 0.1.0): register_deployer, register_third_party, update, transfer_author, freeze, deploy_and_register
```

## 5. Writing entries

The module builds registry transactions but holds no keys. Each `prepare_*` returns a *change*;
`submit` has it signed and sent by the person's own `lezreg` wallet.

| Method | Input (JSON text) | Signed by |
|---|---|---|
| `prepare_register` | `{program, author?, name, version, author_label?, description?, tags?, idl, source, build, image_id?}` | the program's header key and `author` |
| `prepare_register_third_party` | the same with `publisher` | the publisher |
| `prepare_update` | `{entry` or `program, version?, description?, tags?, author_label?, idl?, source?, build?, image_id?}` | the entry's author |
| `prepare_transfer` | `{entry` or `program, new_author}` | the author **and** the new author, in one transaction |
| `prepare_freeze` | `{entry` or `program}` | the entry's author; irreversible |

All take an optional `payer` (an account in the lezreg wallet that pays the fee) and
`allow_remote: true` (needed to spend through a sequencer that is not on this machine). `idl`,
`source` and `build` take the shapes `resolve_program` shows; `image_id` defaults to the live
program's. A change is valid for 10 minutes (the registry's time window).

```qml
logos.callModuleAsync("program_registry", "prepare_freeze", [JSON.stringify({ program: id })], function (reply) {
    var r = JSON.parse(reply)
    if (!r.success) return show(r.error + " (" + r.code + ")")      // codes: frozen, not_author, no_entry, …
    confirmSheet(r.value.summary, function () {                      // show the person what will be signed
        logos.callModuleAsync("program_registry", "submit", [JSON.stringify(r.value)], function (j) {
            var v = JSON.parse(j).value                              // {job} for a trusted app, {request} otherwise
            follow(v.job || v.request)                               // submit_status(id) until applied/failed/denied
        }, 10000)
    })
}, 20000)
```

`submit` answers at once with a job; `submit_status(job)` reports `running`, `applied` (with the
transaction and block) or `failed` (with a `code`: `missing_key`, `expired`, `refused`,
`reverted`, `insufficient_funds`, `no_wallet`, `remote_sequencer`, …), and the `change_settled`
event fires when it ends. The `lezreg` that signs is found on `PATH`, `~/.local/bin` or
`~/.cargo/bin`, or set with `config_set('{"cli": "/abs/path/to/lezreg"}')`; `signer_status()`
says which one. Before signing, `lezreg module send` decodes the change with the real program
and refuses anything that is not exactly the registry transaction it claims to be.

**Who may sign.** The module knows which app calls it (Basecamp names each app's credential).
Only the registry browser (`registry_browser`), the Names Wallet (`names_ui`) and the host
(Basecamp itself, or `logosctl`) may change its settings (`config_set`; others get `code: "not_allowed"`) or have a change signed
straight away. Any other app's `submit` signs nothing: it answers `{request, state:
"awaiting_approval"}`, and the person approves or refuses it in the Program Registry app
(`pending_requests`, `approve_request`, `deny_request`). Follow it with `submit_status(request)`,
which turns into the job once approved; an app only sees what it submitted itself. Reads
(`resolve_program`, `program_label`, `get_idl`, lists, searches, `prepare_*`) stay open to every
app. Still show your own summary before you submit. (`logosctl` acts as the host whatever
client token it presents, so it is trusted; the gate is about other apps inside Basecamp.)

**Not yet:** signing through the Logos wallet module `lez_core` (`"signer": "lez_core"` answers
`lez_core_unavailable`): its generic public send exists on its main branch since 2026-11-13 but
has no release for Basecamp 0.3.x and is pinned to LEZ rc1, whose wallet sends with a fixed gas limit and no simulation (DESIGN §7.3).

## How this was tested

On 2026-11-13, on the Mac mini (macOS, arm64), everything from a scratch session, never the
Basecamp profile:

- A fresh LEZ v0.4.0-rc3 standalone sequencer (`bafd07be`, built with in-process execution) and
  a Logos Storage v0.5.3 node; the registry deployed from `artifacts/` by `chain/deploy.sh` with
  its IDL pushed (live image `5ffbf8f5…6bdc`, self-entry in block 69).
- `program_registry` 0.3.0 and `registry_browser` 0.1.0 built with `nix build .#lgx-portable`
  (builder `de169fd`), installed with `logosctl install --dir`, `program_registry` loaded.
- Reads through `logosctl call`: `config_set`, `resolve_program` (fresh), `program_label`,
  `get_idl` (`ok`, `spel-v1 0.1.0`, 6 calls; an IDL no node holds answers `unavailable` in 6 s),
  `list_entries`, `third_party_entries`, `signer_status`.
- Writes through the module, signed by `lezreg module send`: `prepare_register` + `submit`
  (block 160), `prepare_update` (161, revision 2), `prepare_transfer` with both keys in the
  wallet (162), `prepare_freeze` by the new author (163), `prepare_register_third_party` (166).
  A transfer to an account the wallet holds no key for: refused in simulation, nothing sent,
  `code: "missing_key"`. Then `prepare_update` of the frozen entry: `code: "frozen"`;
  `prepare_register` of a registered id: `code: "exists"`; `"signer": "lez_core"`:
  `code: "lez_core_unavailable"`. `logosctl watch program_registry` printed each
  `change_settled` event.
- §3 and §4 above, as shown. The registry browser rendered offscreen (Qt 6.9 `qml`, the real
  `Logos.Theme`) with answers recorded from the module over logosctl
  (`registry_browser/tools/record.py`, `render.py`).
- Unit tests: `module/rust-lib` 13 (every write vector rebuilt byte for byte, the caller gate,
  the signing bridge with a stand-in CLI), `lezreg-sdk` 31, `lezreg` 20,
  `registry_browser/logic.test.mjs`, `sdk/js/lezreg.test.mjs`.

Not run: `.github/workflows/module.yml` itself (it runs the same steps on Linux with the logosctl
AppImage), and the browser inside a real Basecamp.
