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
  sequencer you choose, and later to your Logos Storage node. There are no analytics.

## Status

| Milestone | State |
|---|---|
| M1: program, `spel04-shim`, guest, unit and engine tests, IDL | done (`b39edbe`) |
| **M2: Rust SDK, `lezreg` CLI, `chain/deploy.sh`** | **done.** The demo core passes on a local LEZ v0.4.0-rc1 sequencer ([`docs/demo-m2.log`](docs/demo-m2.log)) |
| M3: Logos Storage upload and fetch of IDLs and source archives | next. Until then you supply the IDL's CID, sha256 and size yourself |
| M4+: `verify`, Basecamp module, JS SDK, `publish`/`adopt`, GitHub Action | planned (DESIGN §11) |

LEZ testnet 0.4 has not launched. The `testnet` network is a placeholder: it has no RPC and no
registry id yet, and `lezreg` says so. The registry will be deployed there on launch day with
`chain/deploy.sh`.

## Install

Build from source. You need Rust; `rust-toolchain.toml` pins 1.98.1, the same as LEZ
v0.4.0-rc1. Building the CLI does not need the risc0 toolchain.

```sh
git clone https://github.com/jefdiesel/lez-program-registry
cd lez-program-registry
cargo build --release -p lezreg
install target/release/lezreg ~/.local/bin/    # or anywhere on PATH
lezreg --help
```

There are no release binaries yet. A CLI release workflow for macOS arm64, Linux amd64 and
arm64, and Windows is planned (DESIGN §7.4).

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
lezreg show my-token                     # every entry named exactly my-token
lezreg list --limit 20                   # newest first; prints the --after cursor for the next page
lezreg search name token                 # exact name first, then names containing "token"
lezreg search author <ACCOUNT>           # entries this account holds now (proven)
lezreg search label "Acme"               # free-text author label (UNPROVEN)
lezreg search tag defi
```

`Link` records are written once, so `lezreg` caches them in `~/.cache/lez-registry/`, one file
per registry and sequencer. A later listing reads only the list head and the entries.

### Wallet

Writes sign with the LEZ wallet, which is built in. Its keys live in `LEE_WALLET_HOME_DIR`
(default `~/.lee/wallet`).

```sh
lezreg wallet account new public          # prints Public/<id> and its pk
lezreg wallet account ls
lezreg wallet auth-transfer send --from Public/<funded> --to-pk <pk of a new account> --amount 1000000
```

### Writes

Every write runs in four steps:

1. It is **simulated** against current chain state, with the registry's bytecode read from the
   chain. A refusal therefore costs nothing.
2. It is sent with a **gas limit of 1.5 × the simulated cycles** (minimum 30,000). A failed
   transaction is charged its whole limit, so this also caps what any revert can cost.
3. It **waits** for inclusion and checks that the transaction did not revert.
4. It **reads the change back** from the chain.

Any write also takes these options:
- `--payer <ACCOUNT>`: who pays the fee.
- `--dry-run`: simulate only, send nothing.
- `--yes`: required before writing through a sequencer that is not on this machine.

The IDL and source pointers are a Logos Storage CID (`z…`, `b…` or `0x…`) plus the content's
sha256 and size. `--idl-file` hashes a local copy instead.

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
risc0-build 3.0.5). `lezreg init --detect` (M8) will fill them in from your project.

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
`R`'s key is the registry's upgrade key.

## SDK

`sdk/rust` is the `lezreg-sdk` crate. Reads need only `lezreg-types` and an HTTP client. Writes
sit behind the default feature `tx` (the LEZ wallet and engine).

```rust
use lezreg_sdk::{HttpRpc, Registry, Freshness};
let r = Registry::new(HttpRpc::new("http://127.0.0.1:3062"), registry_id);
let res = r.resolve(&program_id)?;                  // one getAccount
if let Some(e) = &res.deployer { println!("{} {} {:?}", e.name, e.version, res.freshness()); }
let page = r.list(None, 50)?;                       // then r.list(page.next.as_ref(), 50)
let mine = r.by_author(&author, 20)?;               // proven
let hits = r.search_name("token", 20, 1000)?;
let tps  = r.third_party(&program_id, 20)?;

// feature "tx"
let mut w = lezreg_sdk::tx::Writer::new(wallet_core, registry_lee_id);
w.register_deployer(program, author, &input, &SendOpts { payer: Some(p), ..Default::default() }).await?;
```

Errors are typed (`lezreg_sdk::Error`): `Transport`, `Rpc`, `BadRecord`, `MissingLink`, `NoEntry`,
`NotDeployed`, `Invalid`, `NoKey`, `NotAuthor`, `Frozen`, `Refused` (simulation), `Funds`,
`Rejected`, `NotIncluded`, `Reverted`, `ReadBack`, and others. With `--json`, the CLI reports the
same kinds as `{"error", "kind"}`.

## Tests

```sh
cargo test -p lezreg-types -p lezreg-program            # unit tests
cargo test --release -p lezreg-engine-test              # the real guest on the rc1 state machine
cargo test --release -p lezreg-sdk                      # SDK against a mocked sequencer
# end to end against a local standalone sequencer (writes need a COPY of a funded wallet):
LEZREG_E2E_RPC=http://127.0.0.1:3062 LEZREG_E2E_REGISTRY=<id> \
  [LEZREG_E2E_PAYER=<funded> LEE_WALLET_HOME_DIR=<wallet copy>] \
  cargo test --release -p lezreg-sdk --test e2e -- --nocapture --test-threads 1
# the M2 demo (deploys a fresh registry; copies the wallet; transcript in demo-out/):
WALLET_SRC=<funded wallet> FUNDER=<account> REG_ELF=<lez_program_registry.bin> SAMPLE_ELF=<program.bin> scripts/demo-m2.sh
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
