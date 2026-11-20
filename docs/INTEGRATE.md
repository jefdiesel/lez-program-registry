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

Since 0.3.1 the module works out of the box on our LEZ v0.4.0-rc3 test chain: its defaults come
from a built-in `names-network` profile (`module/rust-lib/network-rc3.json`: sequencer
`https://lez.nameswallet.app/`, registry `7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82`, LNS
`6Yxj5k8AiuBSuMAYVQFGrYKjJGUSSaWWgoB9D9XXN3gA`). The host, never an app, can move it:
`logosctl call program_registry config_set '{"network_file": "/abs/path/<chain_id>.json"}'` (a
`names-network` file, format: [sdk.md](sdk.md), "Network file"; its `rpc`,
`programs.program_registry.id` and `programs.lns_registry.id` become the defaults), or single
values with `config_set '{"sequencer": "https://…", "registry": "<registry program id>"}'`, or
`~/.config/lez-registry/module.json`. Only values that differ from the profile are saved, so a
newer build's profile takes over by itself. `config_get` shows the profile in use (`network`).

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

Each change comes back with a random `change_id` and a `summary` for a confirm sheet, built by
the module from the instruction it encoded and from the chain, never from your input: `values`
(everything the change writes: name, version, description, tags, label, IDL, source and build
pointers, image id, author or new author), `current` (the entry as it is now), the full ids of
the entry and of every signer, the `payer`, `spends_real_funds` (when `allow_remote` is set),
`max_gas` and `max_fee` (`lezreg` refuses to send a change that would need more gas). Show it,
and put where it spends, whether it can be undone and what it can cost before the long values.
**`values` are raw chain strings and your input as encoded:** clean them before you show them
(drop controls, bidi and zero-width characters, and turn line or paragraph separators such as
U+2028 into a visible mark, as `registry_browser/qml/logic.mjs` `clean()` does), or a value can
draw fake lines on your card.

```qml
logos.callModuleAsync("program_registry", "prepare_freeze", [JSON.stringify({ program: id })], function (reply) {
    var r = JSON.parse(reply)
    if (!r.success) return show(r.error + " (" + r.code + ")")      // codes: frozen, not_author, no_entry, unknown_registry, …
    confirmSheet(r.value.summary, function () {                      // show the person what will be signed
        logos.callModuleAsync("program_registry", "submit", [JSON.stringify(r.value)], function (j) {
            var v = JSON.parse(j).value                              // {job} for the browser or the host, {request} otherwise
            follow(v.job || v.request)                               // submit_status(id) until applied/failed/denied/expired
        }, 10000)
    })
}, 20000)
```

`submit` takes only a change this same app prepared, exactly as returned, once: another app
can't submit it, a changed copy is refused (`code: "changed"`), and a second submit is
`unknown_change`. It answers at once; `submit_status(id)` reports `running`, `applied` (with the
transaction and block), `failed` (with a `code`: `missing_key`, `expired`, `refused`,
`reverted`, `insufficient_funds`, `over_gas_cap`, `unknown_registry`, `wallet_locked`,
`no_wallet`, `remote_sequencer`, …), or for a request `awaiting_approval`, `denied`, `expired`.
The `change_settled` event carries only the job and its state; each app reads its own details
with `submit_status`.

The `lezreg` that signs is found on `PATH`, `~/.local/bin` or `~/.cargo/bin`; only the host can
point the module at another one. Before signing, `lezreg module send`:

- decodes the change with the real program and refuses anything that is not exactly the
  registry transaction it claims to be;
- refuses a registry whose **live code** is not a registry image lezreg trusts: the committed
  reproducible build (`artifacts/lez_program_registry.imageid`), plus images you pin in lezreg
  itself (`lezreg network set <network> --registry-image <hex>` or `LEZREG_REGISTRY_IMAGE`). A
  registry id alone proves nothing: any program can answer with a self-entry named
  `lez_program_registry`, and it then receives changes your accounts signed (one attempt to
  move a signer's funds that way was refused by the rc3 engine, which is not a proof; see
  DESIGN §10). Every other `lezreg` write checks the same;
- signs only for a sequencer and registry you saved in lezreg yourself
  (`lezreg network set <name> --rpc <url> --registry <id>`; otherwise `unknown_network`): the
  image check reads the registry from that sequencer, so a sequencer you never chose could vouch
  for any code;
- refuses an encrypted wallet with no `LEE_WALLET_PASSWORD` (`wallet_locked`): it has no
  terminal to ask on. Putting the password in Basecamp's environment gives it to every module
  Basecamp starts; signing from a terminal is safer.

**Who may sign.** The module knows which app calls it: Basecamp names each app's credential.
Only the Program Registry app (`registry_browser`) and the host (Basecamp itself, or
`logosctl`, which reaches the module as the host whatever client token it presents) may change
its settings (`config_set`; others get `code: "not_allowed"`), see other apps' requests, or have
a change signed straight away. `sequencer`, `registry`, `cli`, `basecamp_data` and
`registry_images` are the host's alone, not even the browser's. Any other app's `submit` signs nothing: it answers `{request, state:
"awaiting_approval"}`, and the person approves or refuses it in the Program Registry app before
the change expires (`pending_requests`, `approve_request`, `deny_request`); the card says the
app's name is the app's own, unverified. Reads and `prepare_*` stay open to every app.

**What that trust rests on.** The gate trusts a package *name*, and a package chooses its own
name. Installing a package that calls itself `registry_browser` with a higher version replaces
the real one (the package manager checks a package's files against the hashes in its own
manifest, not against a publisher; packages are unsigned today), and from then on it can sign without a request:
update, transfer or freeze any entry your lezreg wallet authors, and register entries with your
wallet's accounts, on the sequencer and registry the host set and lezreg has saved. It can't
point the module or lezreg at another sequencer or registry, or at registry code lezreg doesn't
trust. So install the Program Registry
app only from a catalog you trust, until the module checks `registry_browser`'s publisher
through its LNS app record (docs/APPS.md; the root-recomputing app checks are in progress).

**Registry upgrades.** Both the module and lezreg sign only for registry code they know. When
the real registry is upgraded to a new image, all signing stops (`unknown_registry`) until you pin
the new image (`lezreg network set <name> --registry-image <hex>`, and the host's
`config_set({"registry_images": [...]})`) or update lezreg and the module to a release that knows
it. That is the price of not trusting a registry id alone.

**Restarts.** Prepared changes, waiting requests and jobs live in the module's memory: a restart
of the module (or of Basecamp) forgets them. A `lezreg` already started keeps running, and its
transaction may still land; read the entry back from the chain to know.

**Not yet:** signing through the Logos wallet module `lez_core` (`"signer": "lez_core"` answers
`lez_core_unavailable`): its generic public send exists on its main branch since 2026-11-13 but
has no release for Basecamp 0.3.x and is pinned to LEZ rc1, whose wallet sends with a fixed gas
limit and no simulation (DESIGN §7.3).

## How this was tested

On 2026-11-13, on the Mac mini (macOS, arm64), everything from a scratch session, never the
Basecamp profile:

- A fresh LEZ v0.4.0-rc3 standalone sequencer (`bafd07be`, built with in-process execution) and
  a Logos Storage v0.5.3 node; the registry deployed from `artifacts/` by `chain/deploy.sh` with
  its IDL pushed (live image `5ffbf8f5…6bdc`, self-entry in block 69).
- `program_registry` 0.3.0 (SDK pinned to main `85a93ed`) and `registry_browser` 0.1.0 built with
  `nix build .#lgx-portable` (builder `de169fd`), installed with `logosctl install --dir`, `program_registry` loaded.
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
- The caller gate, from a real second module: [`tools/caller-probe`](../tools/caller-probe/)
  (`registry_probe`, which depends on `program_registry` and calls it through the builder's
  generated client) was installed and loaded beside it. Calls it made reached the module as
  "module registry_probe": `program_label` answered; `config_set`, `pending_requests` and
  `approve_request` answered `not_allowed`; its `prepare_update` worked and its `submit` (with
  `what` relabelled "Harmless note") came back `awaiting_approval`, while the host's
  `pending_requests` showed the module's own copy ("Update (revision 2)", fields `version`,
  the payer); an unprepared change was `unknown_change`. Approved by the host, it applied
  (block 776) and the probe's `submit_status` followed it to `applied`. A `logosctl` call reaches
  the module as the host whatever client token it presents.
- Unit tests: `module/rust-lib` 14 (every write vector rebuilt byte for byte, the caller gate,
  a relabelled change, an approved request, the signing bridge with a stand-in CLI), `lezreg-sdk` 31, `lezreg` 20,
  `registry_browser/logic.test.mjs`, `sdk/js/lezreg.test.mjs`.

Not run: `.github/workflows/module.yml` itself (it runs the same steps on Linux with the logosctl
AppImage), and the browser inside a real Basecamp.
