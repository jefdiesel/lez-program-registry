# LEZ Program Registry

An on-chain registry of programs on the Logos Execution Zone (LEZ 0.4), with a Rust SDK and the
`lezreg` command line. It is built for λPrize LP-0023. The full design is in
[`docs/DESIGN.md`](docs/DESIGN.md).

- A **deployer entry** for program `X` is stored on `X`'s own account. It can only be written
  with the authorization the loader requires to deploy or upgrade `X`. A client therefore
  recognises a deployer entry from its address alone, and **one `getAccount(X)` call** returns
  the entry and the live program header together, which shows whether the entry is **fresh or
  stale**.
- Anyone can publish a **third-party (community) entry** for a program. It sits at
  `T(program, publisher)` and can never take the deployer entry's place.
- An entry has a **proven author** (a co-signer). Only that author can update the entry.
  Transferring it needs both the old and the new author's signatures. A frozen entry can no
  longer change.
- Every entry joins **on-chain linked lists**: global, per name, per author, and per program
  for third-party entries. Clients list and search with plain sequencer reads, with no indexer.
- **No server of ours is ever on the path.** The SDK and `lezreg` talk directly to the LEZ
  sequencer you choose, and later to your Logos Storage node. There are no analytics. Reads
  refuse HTTP redirects and ignore `HTTP(S)_PROXY` unless you set `LEZREG_USE_PROXY=1`, and
  `lezreg wallet` never falls back to the LEZ wallet's own default sequencer.

## Status

| Milestone | State |
|---|---|
| M1: program, `spel04-shim`, guest, tests, IDL, **reproducible build** | **done.** CI builds the guest on two runners, byte-identical; the image (`5f42948a…4b55`) is in [`artifacts/`](artifacts/) |
| M2: Rust SDK, `lezreg` CLI, `chain/deploy.sh` | **done** ([`docs/demo-m2.log`](docs/demo-m2.log)) |
| M3: IDLs on Logos Storage | **done:** `idl push`, `--idl-cid push` inside register/update, `idl get` (hash-checked fetch ladder), `pin`, `pin-all`, `storage up` |
| M4: source verification | **done:** CI rebuilds the registry with `lezreg build` (its own Dockerfile, compile offline) and gets `5f42948a…` again. `lezreg verify` (Docker, compile offline, rebuilt vs claimed vs live), `lezreg build`, local marks (`verified`) |
| M5: end to end in CI | in progress: `e2e.yml`: rc1 sequencer, Storage nodes, deploy from `artifacts/`, IDL across nodes, `verify` → Match |
| M6: Basecamp core module `program_registry` | reads done and loading in a Logos host; writes, UI example and catalog next |
| M7: JS SDK | **done** ([`sdk/js`](sdk/js), pinned to Rust by vectors) |
| M8: adoption tools (`publish`, `adopt`, GitHub Action) | planned |

LEZ testnet 0.4 has not launched. The `testnet` network is a placeholder: it has no RPC and no
registry id yet, and `lezreg` says so. The registry will be deployed there on launch day with
`chain/deploy.sh`.

## Install

Build from source. You need Rust (`rust-toolchain.toml` pins 1.98.1, the same as LEZ
v0.4.0-rc1) and the RISC Zero toolchain, installed as `.github/workflows/ci.yml` does it:
`lezreg` links the registry program crate, which simulates transactions locally. The read-only
SDK (`lezreg-sdk` with `default-features = false`) needs neither LEZ nor risc0.

```sh
curl -L https://risczero.com/install | bash && rzup install cargo-risczero 3.0.5 && rzup install rust
git clone https://github.com/jefdiesel/lez-program-registry
cd lez-program-registry
cargo build --release -p lezreg
install target/release/lezreg ~/.local/bin/    # or anywhere on PATH
lezreg --help
```

**Writes need `r0vm` at run time, not only to build.** Every write is simulated locally with
the risc0 executor `r0vm` (rzup links it into `~/.cargo/bin`), so keep that directory on PATH
or set `RISC0_SERVER_PATH` to the binary. Without it a write stops with "simulation needs the
risc0 executor `r0vm`" before anything is sent. Reads never need it.

There are no release binaries yet. A CLI release workflow for macOS arm64, Linux amd64 and
arm64, and Windows is planned (DESIGN §7.4); those binaries will need `r0vm` too.

### A local sequencer and the registry ELF

The `local` network is a LEZ v0.4.0-rc1 sequencer in standalone mode on `127.0.0.1:3062`. In a
LEZ v0.4.0-rc1 checkout, run `just run-sequencer-standalone <home> --rpc-listen-address
127.0.0.1:3062` (LEZ README, "Standalone mode"), and fund a wallet account from it.

The registry guest is built by `cargo build --release -p lezreg-methods` and lands at
`target/riscv-guest/lezreg-methods/lez_program_registry/riscv32im-risc0-zkvm-elf/release/lez_program_registry.bin`.
That is the `--elf` for `chain/deploy.sh`. (The CI-reproduced `.bin`s go into `artifacts/` once
the M1 exit check has run; until then build them yourself.)

## Use

### Networks

```sh
lezreg network show                                  # what is in use
lezreg network set local --registry <REGISTRY_ID>    # chain/deploy.sh does this for you
lezreg --network local list                          # local = http://127.0.0.1:3062
lezreg --rpc http://host:port --registry <ID> list   # any sequencer, any registry
```

The settings live in `~/.config/lezreg/networks.json` (override with `LEZREG_CONFIG_DIR`). You
can also set `LEZREG_NETWORK`, `LEZREG_RPC` and `LEZREG_REGISTRY` in the environment. Every
command accepts `--json` and then prints exactly one JSON object on stdout; progress goes to
stderr.

### Reads (no wallet needed)

```sh
lezreg show <PROGRAM_ID>                 # deployer entry (fresh/stale) + community entries
lezreg show <PROGRAM_ID> --publisher Q   # Q's third-party entry for that program
lezreg show <PROGRAM_ID> --revision 2    # also the write-once release snapshot #2
lezreg show my-token                     # every entry named exactly my-token (case-insensitive)
lezreg list --limit 20                   # newest first, with fresh/stale/NO-PROGRAM; prints the --after cursor
lezreg search name token                 # exact name first, then names containing "token"
lezreg search author <ACCOUNT>           # entries this account holds now (proven)
lezreg search label "Acme"               # free-text author label (UNPROVEN)
lezreg search tag defi
```

**Check the status column.** Anyone can register an entry with any name, label and claimed
image for any account, including one with no program on it. `list`, `search` and `show` mark
such an entry **NO-PROGRAM** and rank it last among entries of the same name; `stale` means
the program now runs different code than the entry claims. Only a deployer entry (shown as
`deployer`) was written by the program's own key.

`Link` records are written once, so `lezreg` caches them in `~/.cache/lez-registry/`, one file
per registry and sequencer. A later listing reads only the list head and the entries (in
JSON-RPC batches of 100). If the chain was reset under the cache, the sequence numbers no longer
line up and the cache is dropped and read again. Searches by label, tag and substring examine
at most `--scan` entries (default 1000) and say when they stopped early.

### Wallet

Writes sign with the LEZ wallet, which is built in. Its keys live in `LEE_WALLET_HOME_DIR`
(default `~/.lee/wallet`; the wallet's own help says that variable "must be set", but lezreg
uses the default when it is not). `lezreg wallet` always talks to lezreg's network
(`--network`/`--rpc`): a network with no RPC, such as `testnet` before launch, is refused, and
so is `change-network`. Commands that send need `--yes` when the sequencer is not on this
machine.

```sh
lezreg wallet account new public          # prints Public/<id> and its pk
lezreg wallet account ls
lezreg wallet auth-transfer send --from Public/<funded> --to-pk <pk of a new account> --amount 5000000
lezreg wallet set-password                # encrypt the wallet
```

A new wallet is created **unencrypted** unless the wallet password variable is set; lezreg says
so when it creates one. Encrypt it with `lezreg wallet set-password` before it holds a
program's header key. `account ls` on a new wallet also lists a public and a private account
the wallet derives for itself, with paths like `/0/0/0`; you can ignore them.

**How much to fund.** On the local rc1 sequencer a register reserves about 3–3.5M while it
runs (it costs about 1.4M), and deploying a 330 KB program plus registering it reserves up to
about 5.3M (it costs about 3.9M). Run the command with `--dry-run` first: it prints the reserve
for the payer you chose. A refusal for funds says how much more to send.

### Writes

Every write runs in four steps:

1. It is **simulated** against current chain state, with the registry's bytecode read from the
   chain. A refusal therefore costs nothing.
2. It is sent with a **gas limit of 1.5 × the simulated cycles** (minimum 30,000). A failed
   transaction is charged its whole limit, so this also caps what any revert can cost.
3. It **waits** for inclusion (at most `LEZREG_TX_TIMEOUT_SECS`, default 120) and checks that
   the transaction did not revert.
4. It **reads the change back** from the chain.

If inclusion cannot be confirmed in time, lezreg reads the chain back anyway: if the change is
there, the write succeeded ("confirmed by reading the chain back"). Otherwise it fails with
kind `not_included`, the transaction hash (`tx` in `--json`), and whether the signers' nonces
moved: unchanged means nothing applied, so a retry is safe; moved means it probably applied, so
check with `lezreg show` first. Two lezreg writes from the same wallet wait for each other (a
lock file next to the wallet), so they never race for a nonce.

Any write also takes these options:
- `--payer <ACCOUNT>`: who pays the fee. For `deploy` it defaults to `--author`, since a new
  header account holds nothing.
- `--dry-run`: run every check and the simulation, send nothing, and print the fee reserve.
  On `deploy` it also checks the payer can cover every segment write.
- `--yes`: required before writing through a sequencer that is not on this machine (the URL's
  host must be a loopback address; `http://127.0.0.1:3062@elsewhere/` is elsewhere).

The IDL and source pointers are a Logos Storage CID (`z…`, `b…` or `0x…`) plus the content's
sha256 and size. `--idl-file` hashes a local copy instead. Until Storage upload lands (M3),
`--idl-cid raw` uses the raw-codec sha2-256 CID of the bytes (`0x01551220<sha256>`), which anyone
can recompute; a raw CID that names other bytes is refused.

```sh
# Deploy a program and register it in the SAME transaction (DeployAndRegister). The entry is
# marked "born with deploy". The header key is a new wallet account unless --program is given.
lezreg deploy program.bin --author <AUTHOR> --payer <FUNDED> \
  --name my-token --version 1.0.0 --label "Acme Labs" --description "Fungible token" --tag token \
  --idl-cid z… --idl-file idl/my_token.json \
  --repo https://github.com/acme/my-token --commit <40-hex> \
  --manifest-path methods/guest/Cargo.toml --binary my_token_guest --lock-file Cargo.lock

# Register an already deployed program (its header key and the author sign).
lezreg register --program <PROGRAM_ID> --author <AUTHOR> --name … --version … --idl-cid … …

# A community entry for someone else's program (the publisher signs and is the author).
lezreg register --third-party --program <PROGRAM_ID> --publisher <Q> --name … …

lezreg update <PROGRAM_ID> --version 1.1.0 --idl-cid z… --idl-file …   # the author signs
lezreg update <PROGRAM_ID> --image-id <hex>                             # claim a new image after an upgrade
lezreg update <PROGRAM_ID> --publisher <Q> --description …              # Q's third-party entry
lezreg transfer <PROGRAM_ID> --to <NEW_AUTHOR>                          # both sign, in one transaction
lezreg freeze <PROGRAM_ID>                                              # irreversible
```

The build fields default to LEZ's own builder pin (`risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a…`,
risc0-build 3.0.5), the binary to the entry name, and the Cargo.lock hash to zero. lezreg warns
when you give no build flags, and `show` marks build info without a Cargo.lock hash as
UNVERIFIED. Give at least `--binary`, `--manifest-path` and `--lock-file`. `lezreg init --detect`
(M8) will fill them in from your project.

**Upgradeable by default.** `deploy` makes the code upgradeable unless you pass `--immutable`:
the header key can later point the program at new code. The entry then shows `stale` until
its author runs `update --image-id`. A `transfer` hands over the *entry*, not the header key:
the original deployer can still upgrade the code. `deploy` is also checked end to end before
it spends anything: the input, that nothing is already deployed or registered at the program,
that the wallet holds every signer's key, a simulation of the final transaction over the
segment chain held in memory, and the payer's funds for every write.

`--force-send [--gas-limit N]` sends a transaction even when simulation refuses it, so you can
watch the chain refuse it too. The payer is charged the whole limit; the demo uses it for the
Mallory cases.

### Deploy the registry itself

```sh
chain/deploy.sh --elf lez_program_registry.bin --payer <FUNDED> [--rpc URL] [--network local] [--commit <hex>]
```

The script writes the guest's segments and sends the loader `CreateHeader` at a new header
account `R`. It then sends `RegisterDeployer` at `(R, R)`, signed by `R`'s header key, so the
registry is registered in itself. Finally it saves `R` as the network's registry. It can resume:
on a rerun it skips any step that is already on chain. Back up the wallet afterwards, because
`R`'s key is the registry's upgrade key. `R` is saved in `chain/.deploy-<network>.env` as soon
as its key is in the wallet, so a failure part-way is finished at the same `R` on the next run;
a read that fails stops the script instead of deploying a second registry.

## SDK

`sdk/rust` is the `lezreg-sdk` crate. Reads need only `lezreg-types` and an HTTP client. Writes
sit behind the default feature `tx` (the LEZ wallet and engine).

```rust
use lezreg_sdk::{HttpRpc, Registry, Freshness};
let r = Registry::new(HttpRpc::new("http://127.0.0.1:3062"), registry_id);
let res = r.resolve(&program_id)?;                  // one getAccount
if let Some(e) = &res.deployer { println!("{} {} {:?}", e.name, e.version, res.freshness()); }
let page = r.list(None, 50)?;                       // then r.list(page.next.as_ref(), 50)
let mine = r.by_author(&author, 20, 1000)?;         // proven; .items, .scanned, .truncated
let hits = r.search_name("token", 20, 1000)?;       // exact name first, then substrings
let tps  = r.third_party(&program_id, 20)?;         // r.third_party_count(&program_id)? for the total

// feature "tx"
let mut w = lezreg_sdk::tx::Writer::new(wallet_core, registry_lee_id);
w.register_deployer(program, author, &input, &SendOpts { payer: Some(p), ..Default::default() }).await?;
```

**IDLs, pinning, verification (CLI):**

```sh
lezreg idl push idl/my_program.json                 # canonical JSON to your Storage node; prints cid/sha256/size
lezreg register ... --idl-cid push --idl-file idl/my_program.json   # or upload inside the write
lezreg idl get <program|name> [-o file]             # fetched, checked against the chain's sha256
lezreg pin <program> ; lezreg pin-all --every 900   # keep IDLs available (a self-hostable pinner)
lezreg storage up --bin /path/to/logos-storage      # a Storage node for authors without one
lezreg verify <program> [--mark]                    # rebuild in Docker, compile offline: Match / MatchStale / Mismatch
lezreg build . --binary my_guest                    # the canonical image id before you deploy
```

**JavaScript** (`sdk/js/lezreg.js`, no dependencies, browsers and Node 19+):

```js
import { Registry, fetchIdl } from "./lezreg.js";
const reg = new Registry({ rpc: "http://127.0.0.1:3062", registry: "<registry id>" });
const label = await reg.programLabel("<program id>");   // { text, kind, stale, upgradeable, ... }
const { deployer } = await reg.resolve("<program id>");
const { status, idl } = await fetchIdl(deployer, { storageApi: "http://127.0.0.1:8080/api/storage/v1" });
```

**Basecamp apps:** depend on the `program_registry` core module and call
`program_label(program_id)` for a wallet prompt's one-line label (see [`module/`](module/)).

Errors are typed (`lezreg_sdk::Error`): `Transport`, `Rpc`, `BadRecord`, `MissingLink`, `NoEntry`,
`NotDeployed`, `Invalid`, `NoKey`, `NotAuthor`, `Frozen`, `Refused` (simulation), `Funds`,
`Rejected`, `NotIncluded { hash, detail }`, `Reverted`, `ReadBack`, and others. With `--json`,
the CLI reports the same kinds as `{"error", "kind"}`, plus `tx` when a transaction was sent and
`program` when a deploy failed after its header key was saved.

## Tests

```sh
cargo test -p lezreg-types -p lezreg-program            # unit tests
cargo test --release -p lezreg-engine-test              # the real guest on the rc1 state machine
cargo test --release -p lezreg-sdk -p lezreg            # SDK against a mocked sequencer; CLI unit tests
scripts/test-deploy-sh.sh                               # chain/deploy.sh resume/failure cases (stub lezreg)
# a write whose inclusion is never confirmed is confirmed by read-back (local sequencer, wallet copy):
LEE_WALLET_HOME_DIR=<copy> ENTRY=<entry> PAYER=<funded> scripts/test-not-included.sh
# end to end against a local standalone sequencer (writes need a COPY of a funded wallet):
LEZREG_E2E_RPC=http://127.0.0.1:3062 LEZREG_E2E_REGISTRY=<id> \
  [LEZREG_E2E_PAYER=<funded> LEE_WALLET_HOME_DIR=<wallet copy>] \
  cargo test --release -p lezreg-sdk --test e2e -- --nocapture --test-threads 1
# the M2 demo (deploys a fresh registry; copies the wallet; transcript in demo-out/; a
# previous demo-out/ is moved to demo-out.prev-<time>, since its wallet holds that registry's keys):
WALLET_SRC=<funded wallet> FUNDER=<account> REG_ELF=<lez_program_registry.bin> SAMPLE_ELF=<program.bin> scripts/demo-m2.sh
```

Network check (2026-11-02, macOS `sandbox-exec` on the Mini): with outbound network denied
except `localhost:3062`, `list` and `show` work; a write works with outbound limited to
loopback (the `r0vm` executor talks to lezreg over a local socket), and the same sandbox blocks
a request to any other host. lezreg links LEZ crates that contain other URLs (the LEZ wallet's
default testnet, rzup's download hosts), but no command reached them.

To build and test on another machine, sync the tree **without** the demo output, which holds
registry keys and exists only where the demo ran:

```sh
rsync -a --delete --exclude target --exclude .git --exclude _deps \
  --exclude 'demo-out*' --exclude 'smoke-out*' --exclude 'chain/.deploy-*.env' ./ host:path/
```

## Layout

| Path | What |
|---|---|
| `program/` | the on-chain program, in SPEL attribute syntax |
| `spel04-shim/` | compiles SPEL syntax for LEZ 0.4 actors |
| `methods/` | the guest build |
| `crates/types/` | records, PDA rule and validation, with no LEZ dependency |
| `sdk/rust/` | `lezreg-sdk`: reads, list cache and search; `tx` writes |
| `cli/` | `lezreg` |
| `chain/deploy.sh` | deploys the registry and its self-entry |
| `scripts/demo-m2.sh` | the M2 demo |
| `idl/` | the generated IDL |
| `tools/` | engine tests, IDL drift checks, the M1 smoke client |
| `docs/` | design notes and demo transcripts |

## Licence

MIT OR Apache-2.0.
