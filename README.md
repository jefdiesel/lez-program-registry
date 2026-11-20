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
- It is also an **app store for Basecamp apps**, with no list to curate: an app's identity is
  an LNS name, and the registry recomputes an app's package root from its files and checks it
  against the root that name vouches for, so a copy can't pass as the real app. Publishers can
  stay anonymous (a name can be held privately) and apps unlisted (a name's records are sealed
  and can't be read without the name; record addresses are visible on chain, though, and a
  guessable name can be found by trying it, so an app under a guessable name is public). Packages can be fetched from Logos Storage by CID, with no web
  host on the path. See [Apps](#apps-verified-by-publisher) and [`docs/APPS.md`](docs/APPS.md).
- **No server of ours is ever on the path.** The SDK and `lezreg` talk directly to the LEZ
  sequencer you choose, and later to your Logos Storage node. There are no analytics. Reads
  refuse HTTP redirects and ignore `HTTP(S)_PROXY` unless you set `LEZREG_USE_PROXY=1`, and
  `lezreg wallet` never falls back to the LEZ wallet's own default sequencer.

## Status

| Milestone | State |
|---|---|
| M1: program, `spel04-shim`, guest, tests, IDL, **reproducible build** | **done.** CI builds the guest on two runners, byte-identical; the image (`5ffbf8f5…6bdc` on LEZ rc3; `5f42948a…4b55` on rc1) is in [`artifacts/`](artifacts/) |
| M2: Rust SDK, `lezreg` CLI, `chain/deploy.sh` | **done** ([`docs/demo-m2.log`](docs/demo-m2.log)) |
| M3: IDLs on Logos Storage | **done:** `idl push`, `--idl-cid push` inside register/update, `idl get` (hash-checked fetch ladder), `pin`, `pin-all`, `storage up` |
| M4: source verification | **done:** CI rebuilds the registry with `lezreg build` (its own Dockerfile, compile offline) and gets the committed image (`5ffbf8f5…` on rc3) again. `lezreg verify` (Docker, compile offline, rebuilt vs claimed vs live), `lezreg build`, local marks (`verified`) |
| M5: end to end in CI | **done** (`e2e.yml` green):  rc1 sequencer, Storage nodes, deploy from `artifacts/`, IDL across nodes, `verify` → Match |
| M6: Basecamp core module `program_registry` + `registry_browser` | **done:** reads (`resolve_program`, `program_label`, `get_idl`, list and search, the app checks below) and writes (`prepare_register`, `prepare_register_third_party`, `prepare_update`, `prepare_transfer`, `prepare_freeze`, then `submit`, signed by your `lezreg` wallet through `lezreg module send`; `change_settled` event); built with `mkLogosModule`; reads published in the module catalog [`jefdiesel/logos-modules`](https://github.com/jefdiesel/logos-modules) (forked from `logos-modules-release-base`, released with `logos-modules-release-action`) for macOS and Linux, 0.3.0 with writes and `registry_browser` not released there yet. The [`registry_browser`](registry_browser/) UI module (list, search, entry detail, IDL status, verify mark, update/freeze) and the drop-in [`RegistryLabel.qml`](registry_browser/qml/RegistryLabel.qml); integration guide [`docs/INTEGRATE.md`](docs/INTEGRATE.md); `module.yml` CI. Names Wallet's approval sheet uses the module. Not yet: signing through the `lez_core` wallet module (no usable release) |
| M7: JS SDK | **done** ([`sdk/js`](sdk/js), pinned to Rust by vectors): resolve, list, search by name and by author, community entries, IDL status, and the write bytes for register, third-party register and update (signing is an injected signer; no keys in the SDK) |
| Apps by LNS name ([`docs/APPS.md`](docs/APPS.md)) | **done:** Basecamp apps checked against the LNS name that publishes them, so a copy can't pass as the real app; publishers can stay anonymous and apps unlisted. `lezreg app show/verify/record`, the module's `verify_app`, `verify_package`, `publisher_apps`. No change to the deployed program. Tested on a real chain (logos-names `wallet-e2e.sh` step 7) |
| M8: adoption tools | **done** (e2e green on 6f0dc66): `init --detect`, `publish` (deploy+register, register or update, idempotent), `publish --from-release`, `adopt`, `stats`, `doctor`, `release prepare`, and the GitHub Action ([`action/`](action/action.yml), `verify` and `prepare` modes). Run end to end on a local rc1 chain; `e2e.yml` now runs the same flow and the Action. Not yet: `idl gen`/`idl check`, source-archive upload, the Action's opt-in `publish` mode |

Logos LEZ testnet 0.4 has not launched. Until it does, `lezreg --network testnet` reads an
embedded `names-network/2` file that describes this project's own rc3 test network
(`https://lez.nameswallet.app/`, registry `7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82`; it is
reset now and then). On launch day the registry is deployed to testnet 0.4 with
`chain/deploy.sh` and a new network file replaces the embedded one.

The IDL tools for the prize spec rewrite (logos-co/lambda-prize#188) are in `lezreg idl
encode|decode`, `lezreg call --plan` and `lezreg image-id`; [`docs/PR188.md`](docs/PR188.md) maps
each requirement to what exists and what does not.

## Run the demo

One command, from a clean clone, on Linux or macOS:

```sh
git clone https://github.com/jefdiesel/lez-program-registry && cd lez-program-registry
scripts/demo.sh
```

It checks the prerequisites first and prints the exact install command for anything missing:
Rust, the RISC Zero toolchain and `r0vm`, git, curl, python3, and on Linux `libpcsclite-dev`.
Then it builds `lezreg` and builds LEZ's standalone `sequencer_service` from the tag this repo
pins. The sequencer build is cached; a cold build takes about 40 minutes. It also downloads the
pinned Logos Storage release and checks its sha256.

It then starts a fresh local chain and three Storage nodes and walks every Functionality,
Usability and Reliability criterion of LP-0023 in the spec's order. That includes the
unauthorised deployer-entry attempt being rejected (step `F3.c` onwards). Each criterion gets a
heading, each step prints PASS, FAIL or SKIP, and a summary table comes at the end. The exit code
is nonzero if any step fails.
- The source-verification rebuild needs Docker. Without Docker it is reported as SKIP, not PASS.
- On arm64 hosts the rebuild is also skipped unless you pass `--verify`, because the amd64
  builder image runs emulated there (slowly, and with about 8 GB of Docker memory).
- The JS SDK step needs node 19 or later.

CI runs the same command on a clean Linux runner, with Docker, via `--verify`
(`.github/workflows/demo.yml`; its transcript is the run's `demo-transcript` artifact). The
transcript of a macOS run is [`docs/demo.log`](docs/demo.log), and the criterion-by-criterion
self-assessment is [`docs/FURPS.md`](docs/FURPS.md).

Options:
- `--prepare-only` builds everything without running the demo.
- `--keep` leaves the chain running afterwards.
- To reuse binaries you already have, set `SEQUENCER_BIN`, `SEQUENCER_CONFIG`, `STORAGE_BIN` or
  `LEZREG`.
- To move off busy ports, set `DEMO_RPC_PORT` or `DEMO_STORAGE_PORT`.
- `scripts/demo.sh --testnet --rpc <URL> --registry <ID>` runs the read-only walk against a public
  sequencer. It refuses to run without those values, because testnet 0.4 has not launched.

## Install

One line, no sudo (it refuses to run as root unless you pass `--allow-root`). It downloads the
`lezreg` build for your machine from this repo's latest GitHub release over HTTPS, checks its
SHA-256 against the release's `SHA256SUMS`, and puts it in `~/.lezreg/bin` (or
`$LEZREG_HOME/bin`):

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/jefdiesel/lez-program-registry/releases/latest/download/install.sh | sh -s -- --with-r0vm --with-storage
```

- Builds exist for Linux x86_64 and aarch64 and macOS arm64 (macOS 12 or newer).
  - Linux needs glibc 2.34 or newer, and on aarch64 also libstdc++ from GCC 11 or newer. musl
    distributions (Alpine) are not supported; the installer says so.
    Nothing else: no libpcsclite, no Rust. Ubuntu 22.04, Debian 13 and Fedora 45 are tested
    in bare containers by the release workflow. Debian 12 and RHEL 9 meet the same minimum
    but are not tested.
  - Windows: use WSL.
  - Intel Macs are not supported, even from source: the LEZ crates link
    logos-blockchain-circuits, which ships no macOS x86_64 library.
- The installer does not edit your shell files. It prints the `export PATH=...` line to add.
- `--with-r0vm` also fetches RISC Zero's `r0vm` 3.0.5 from RISC Zero's own GitHub release,
  checked against a pinned SHA-256. **Every write needs it** (see below). Prebuilt `r0vm`
  3.0.5 exists only for macOS arm64 and Linux x86_64; elsewhere the installer prints how to
  build it (`cargo install --locked risc0-r0vm --version 3.0.5`).
- `--with-storage` also fetches the Logos Storage v0.5.3 node from its official release,
  checked the same way, as `logos-storage`. `lezreg storage up` starts it, to upload your IDL.
  Its Linux builds need glibc 2.38 (Ubuntu 24.04, Debian 13 or newer). On older Linux the
  installer skips it and says so; use a Storage node on another machine
  (`LEZREG_STORAGE_API`).
- Other options: `--version 0.1.0` for a given release, `--dry-run` to print what it would
  download and install, `--help`.
- It sends nothing about you. Like any download, GitHub and the hosts it redirects to see your
  IP address and your curl or wget user agent.
- **What the checksum proves.** The SHA-256 check catches a damaged or cut-off download. It is
  not a signature: `SHA256SUMS` comes from the same release as the files. Your trust in the
  one-liner rests on GitHub's TLS and on whoever can write to this repository. To check where a
  file was built, use the attestation (below).
- Run `lezreg doctor` afterwards to see what this machine still lacks.

**By hand, with your own checks.** Download `lezreg-<version>-<os>-<arch>.tar.gz` and
`SHA256SUMS` from the [releases page](https://github.com/jefdiesel/lez-program-registry/releases),
then:

```sh
sha256sum -c SHA256SUMS --ignore-missing      # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify lezreg-0.1.0-linux-x86_64.tar.gz -R jefdiesel/lez-program-registry \
  --signer-workflow jefdiesel/lez-program-registry/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0 --deny-self-hosted-runners
tar -xzf lezreg-0.1.0-linux-x86_64.tar.gz && install lezreg-0.1.0-linux-x86_64/lezreg ~/.local/bin/
```

Use the version you downloaded in the file name and in `--source-ref refs/tags/v<version>`.

**What the attestation proves.** `gh attestation verify` checks GitHub's signed build
provenance. With those flags it passes only if this exact file was built by this repo's
`release.yml`, from the tag `v<version>`, on a GitHub-hosted runner. The output names the
commit and the run. `install.sh` and `SHA256SUMS` are attested the same way. Releases are made
only from tags: a run started by hand builds and tests but attests nothing.

**What it does not prove:**
- that the source at that tag is benign;
- that the build inputs were clean: the GitHub runner image, the Rust toolchain, the crates in
  `Cargo.lock`, and the prebuilt logos-blockchain-circuits library (checked against a pinned
  SHA-256, but built by its own project);
- anything beyond this repository: the trust root is whoever has write access to
  `jefdiesel/lez-program-registry`. They can push a tag and the workflow will build and attest
  it.

**Writes need `r0vm` at run time.** Every write is simulated locally with the risc0 executor
`r0vm` 3.0.5 before it is sent. Without it a write stops with "simulation needs the risc0
executor `r0vm`" and nothing is sent. Reads (`show`, `list`, `search`, `idl get`) never need it.

`lezreg` looks for `r0vm` where risc0 does, in this order:
1. `RISC0_SERVER_PATH`, if set to the binary;
2. rzup's install directory, `~/.risc0/extensions/v3.0.5-cargo-risczero-<platform>/r0vm`
   (under `$RISC0_HOME` if that is set);
3. `PATH`. The installer's `--with-r0vm` puts it in `~/.lezreg/bin`.

RISC Zero's own installer works too:
`curl -L https://risczero.com/install | bash && ~/.risc0/bin/rzup install cargo-risczero 3.0.5`.
It puts `r0vm` only in the directory of step 2, which `lezreg` and `lezreg doctor` check.
- On Linux there is no `~/.cargo/bin/r0vm` link afterwards.
- On macOS a `~/.cargo/bin/r0vm` link exists only if rzup made one, e.g. after
  `rzup install r0vm`. Either way it is found.

**Registering needs a Logos Storage node** for the IDL upload: `lezreg storage up` (with
`logos-storage` on PATH, or `--bin`), or `LEZREG_STORAGE_API` pointing at a node you run.

**Linux and smart cards.** The Linux builds don't link `libpcsclite`. They carry a small
stand-in ([`scripts/release/pcsc-dlopen.c`](scripts/release/pcsc-dlopen.c)), so they start
on systems without it. The built-in wallet's Keycard commands load the system's
`libpcsclite.so.1` when they run. Install your distro's PC/SC package only if you use a
Keycard.

### Build from source

You need Rust (`rust-toolchain.toml` pins 1.98.1, the same as LEZ v0.4.0-rc3). On Linux also
the PC/SC headers, pkg-config and clang (`sudo apt install libpcsclite-dev pkg-config clang`).
Building `lezreg` needs no RISC Zero toolchain: the LEZ crates embed their programs from LEZ's
committed artifacts. The build does download logos-blockchain-circuits' prebuilt static
libraries from its GitHub release, or uses `LBC_ROOT_DIR` if set. Those exist only for Linux
x86_64 and aarch64, macOS arm64 and Windows. You need RISC Zero's toolchain only to build the
registry guest itself (`.github/workflows/ci.yml` installs it), and `r0vm` at run time as
above. The read-only SDK (`lezreg-sdk` with `default-features = false`) needs neither LEZ nor
risc0.

```sh
git clone https://github.com/jefdiesel/lez-program-registry
cd lez-program-registry
cargo build --release --locked -p lezreg
install target/release/lezreg ~/.local/bin/    # or anywhere on PATH
lezreg --help
```

### A local sequencer and the registry ELF

The `local` network is a LEZ v0.4.0-rc3 sequencer in standalone mode on `127.0.0.1:3062`. In a
LEZ v0.4.0-rc3 checkout, run `just run-sequencer-standalone <home> --rpc-listen-address
127.0.0.1:3062` (LEZ README, "Standalone mode"), and fund a wallet account from it.

The registry guest is built by `cargo build --release -p lezreg-methods` and lands at
`target/riscv-guest/lezreg-methods/lez_program_registry/riscv32im-risc0-zkvm-elf/release/lez_program_registry.bin`.
That is the `--elf` for `chain/deploy.sh`. (The CI-reproduced `.bin`s go into `artifacts/` once
the M1 exit check has run; until then build them yourself.)

## Use

### All commands

The one list of `lezreg` commands (`lezreg <command> --help` for the flags; every command takes
`--json`). AGENTS.md and docs/FURPS.md point here.

| Command | What it does | Needs |
|---|---|---|
| `show <program\|name>` | deployer entry (fresh/stale), community entries, and each entry's IDL status; `--publisher`, `--revision` | sequencer (Storage optional) |
| `list`, `search name\|author\|label\|tag` | every entry newest first; searches | sequencer |
| `stats` | the adoption table, from chain | sequencer |
| `idl get\|push` | fetch an entry's IDL and check its hash; upload one | Storage node |
| `idl encode\|decode` | message, state or event bytes from an IDL (v1, v2, `x-lez04`) | nothing |
| `call --plan` | message bytes, actors to declare, signers; sends nothing | nothing |
| `image-id <elf\|bin> [--compare]` | a program's image id as the loader computes it | nothing |
| `pin`, `pin-all [--every]` | keep IDLs on your Storage node | Storage node |
| `register`, `update`, `transfer`, `freeze` | writes (`--dry-run` first) | wallet, coins, r0vm |
| `deploy`, `publish`, `adopt`, `init --detect` | deploy and register; the author flow from `lezreg.toml` | wallet, coins, r0vm |
| `verify [--mark]`, `verified`, `build` | rebuild in Docker and compare image ids; `--offline` with `--image-tar`, `--vendor-tar`, `--archive-file`, `--no-forge` | Docker |
| `release prepare` | the unsigned payload for the GitHub Action | nothing |
| `app show\|verify\|record\|get` | Basecamp apps checked by LNS publisher ([docs/APPS.md](docs/APPS.md)) | sequencer (Storage for `get`) |
| `network show [--check-chain]`, `network set` | networks, network files, chain check | nothing (`--check-chain`: sequencer) |
| `doctor` | sequencer, registry, Storage, r0vm, wallet, Docker, clock | nothing |
| `storage up` | start a Logos Storage node | the `logos-storage` binary |
| `module send` | the signing bridge for the Basecamp module | wallet |
| `wallet …` | the LEZ wallet's own commands, always on lezreg's sequencer | wallet |

The prebuilt 0.1.0 release that the install line fetches predates `idl encode|decode`,
`call --plan`, `image-id`, the IDL status in `show`, network files and `verify --offline`; build
from source for those until the next release.

### Networks

```sh
lezreg network show                                  # what is in use
lezreg network set local --registry <REGISTRY_ID>    # chain/deploy.sh does this for you
lezreg --network local list                          # local = http://127.0.0.1:3062
lezreg --network testnet network show --check-chain  # the embedded network file; checks getChannelId
lezreg --rpc http://host:port --registry <ID> list   # any sequencer, any registry
lezreg network set mynet --network-file net.json     # one whole names-network/2 file
```

The settings live in `~/.config/lezreg/networks.json` (override with `LEZREG_CONFIG_DIR`). You
can also set `LEZREG_NETWORK`, `LEZREG_RPC`, `LEZREG_REGISTRY`, `LEZREG_LNS` and
`LEZREG_NETWORK_FILE` in the environment.

**Network files.** `testnet` comes from an embedded `names-network/2` file
(`sdk/rust/network-rc3.json`): RPC, chain id, registry id, LNS program id and the registry's
image. `LEZREG_NETWORK_FILE` or `network set --network-file` uses another file, always whole
(fields are never merged from two files). `network show --check-chain` asks the sequencer for
`getChannelId` and refuses a chain other than the file's; it runs only when you ask for it.

**Writes are signed only for registry code lezreg trusts.** Before any write, lezreg reads the
live image of the program the registry id names and refuses unless it is the committed
reproducible build (`artifacts/lez_program_registry.imageid`) or an image you pinned:
`lezreg network set <network> --registry-image <64 hex>` (or `LEZREG_REGISTRY_IMAGE`, comma
separated). A registry id alone proves nothing: any program can answer as a registry and would then
receive changes your accounts signed (DESIGN §10). Every
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
lezreg wallet auth-transfer send --from Public/<funded> --to-pk <pk of a new account> --amount 40000000
lezreg wallet set-password                # encrypt the wallet
```

A new wallet is created **unencrypted** unless the wallet password variable is set; lezreg says
so when it creates one. Encrypt it with `lezreg wallet set-password` before it holds a
program's header key. `account ls` on a new wallet also lists a public and a private account
the wallet derives for itself, with paths like `/0/0/0`; you can ignore them.

**How much to fund.** These figures were measured on a local rc3 sequencer at base fees 8/8
(docs/CYCLES.md). The payer must hold the reserve while the command runs, and spends less:

| | Must hold | Spends |
|---|---:|---:|
| (a) register a program that is already deployed | ~2.6M | ~1.5M |
| (b) deploy and register a ~430 KB program | ~17.4M | ~10.5M |
| (b) deploy and register a ~1 MB program | ~37.4M | ~24.7M |
| (c) update an entry | ~1.3M | ~0.75M |

Since rc3 the loader charges 1 gas per byte it reads. A deploy therefore costs about 24 per
byte of program: the segment bytes, plus the loader reading them. Registering and updating cost
the same whatever the program's size. Run the command with `--dry-run` first: it prints the
reserve for the payer you chose. A refusal for funds says how much more to send.

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
UNVERIFIED. Give at least `--binary`, `--manifest-path` and `--lock-file`, or use `lezreg
publish`, which takes them all from `lezreg.toml` (next section).

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

### Publish your program (one command)

```sh
cd my-program                       # a git checkout with a risc0 guest and its Cargo.lock
lezreg init --detect --author <YOUR_ACCOUNT>   # writes lezreg.toml; commit it
lezreg doctor                       # sequencer, registry, Storage node, r0vm, wallet, Docker, clock
lezreg publish --elf <program .bin or guest ELF> [--dry-run]
```

`init --detect` reads the guest (a methods crate's `[package.metadata.risc0] methods`, or a
`guest/Cargo.toml`; `methods/guest` wins when there are several), its bin target, the name,
version and description from `Cargo.toml`, the IDL under `idl/`, the repository from `git remote
origin` (as an https URL, any token in it dropped), and the canonical builder from the
`risc0-build` version in `Cargo.lock` (3.0.5 → `risczero/risc0-guest-builder:r0.1.91.1@sha256:fafb377a…`,
host Rust 1.91.1; an unknown version is refused, not guessed). It refuses a tree with
uncommitted changes (`--allow-dirty` overrides), and `--ci` also writes
`.github/workflows/lezreg.yml`.

`publish` then does whatever the chain still needs:

| The chain has | publish does |
|---|---|
| nothing at the program (or no `[deploy] program` yet) | uploads the IDL, writes the segments and sends `DeployAndRegister` (needs `--elf`); records the new program id in `lezreg.toml` |
| the program, but no entry | `RegisterDeployer` (the wallet must hold the header key) |
| an entry that differs from `lezreg.toml` or the tree | `Update` of just the fields that changed (version, label, description, tags, IDL, commit, build tuple, a new live image) |
| exactly what `lezreg.toml` says | nothing: "up to date" |

So the release routine is: bump `version` in `lezreg.toml`, commit, `lezreg publish`. The commit
and the `Cargo.lock` hash are read at publish time, and a commit that is on no remote branch gets
a warning (verifiers could not fetch it). With `--elf` on a deployed program, the binary's image
must equal the live one. `[deploy] author` (or `--author`) is the proven author; without it the
header key is the author, and `publish` suggests a separate author account, since one author
account across all your programs is what counts you as one author. The author pays the fees
unless `--payer` says otherwise. The IDL goes to `LEZREG_STORAGE_API` (only when it changed).

```sh
lezreg adopt <PROGRAM_ID> [--project dir] [--name n]   # a program you deployed earlier (header key in the wallet)
lezreg stats --by-month --by-author [--exclude-keys team.txt]
```

`stats` is the adoption table straight from the chain: entries whose program has a live header
are counted (others are listed as not counted), with distinct programs, proven authors (never
the label) and images, stale entries flagged, per month of registration and per author.
`--exclude-keys` drops entries whose author or registrant is listed (one account per line).

### From CI: the GitHub Action

```yaml
on: { pull_request: {}, push: { tags: ["v*"] } }
permissions: { contents: write }
jobs:
  registry:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: jefdiesel/lez-program-registry/action@main
        with:
          mode: ${{ startsWith(github.ref, 'refs/tags/') && 'prepare' || 'verify' }}
          # rpc: <a LEZ RPC>            # with a program id: compare with the live program
          # program-id: <PROGRAM_ID>    # default: lezreg.toml's [deploy] program
```

- **`verify`**: the canonical build of what `lezreg.toml` describes (`lezreg build`: the pinned
  builder, compile with the network off), its image id, and with `rpc` the live program's image;
  fails on a mismatch, and writes a job summary (image ids, builder digest, `network_denied`).
- **`prepare`**: verify, then `lezreg release prepare`: `program.bin`, the canonical IDL
  (`idl.json`) and an unsigned `lezreg-release.json` with every hash, kept as a workflow
  artifact and, on a tag, attached to the release. Then, on your machine:

```sh
lezreg publish --from-release https://github.com/<you>/<repo>/releases/tag/<tag>   # or a local directory
```

It downloads the three files (following the release host's redirect), checks their sizes and
sha256s against the payload, the binary's image id, that the IDL is canonical, and, when run in
a checkout of the payload's commit, the `Cargo.lock` hash; then it publishes exactly as above.
**Header and author keys never enter CI**: the Action has no key input and sends nothing. It
builds `lezreg` from its own source unless given `lezreg-bin` (a cold build takes about 30
minutes). The source tarball and the Action's opt-in `publish` mode (DESIGN §7.6) are not built
yet; `verify` rebuilds from the recorded repository and commit.

### Apps, verified by publisher

An app published by the LNS name `forumer` names it in its `metadata.json`
(`"author": "lns:forumer"`), and `forumer`'s profile vouches for each release with an
`app.<module>` entry ([`docs/APPS.md`](docs/APPS.md)). Pass `--lns <LNS program id>` (or set
`LEZREG_LNS`) to the commands that read names.

```sh
# Publisher: the entry for a release, with the package put on your Logos Storage node.
lezreg app record forumer_ui-1.2.0.lgx --upload \
    --catalog https://raw.githubusercontent.com/<you>/<catalog>/main/logos-repo.json \
    --program <program id the app calls> --keep forumer_ui-1.1.0.lgx
#   names set-text forumer app.forumer_ui '{"v":1,"root":"…","sha256":"…","manifest":"…","version":"1.2.0","cid":"zDvZ…",…}'
# Run the printed line: the name's holder signs the change, which is what vouches for it.

# Anyone who knows the name:
lezreg app show forumer                       # its apps, and their programs' registry status
lezreg app verify forumer_ui-1.2.0.lgx        # verified (exit 0) / files_verified (exit 3) / mismatch, root_mismatch, ... (exit 1)
lezreg app verify ~/Library/Application\ Support/Logos/LogosBasecamp/plugins/forumer_ui   # an installed copy
lezreg app get forumer forumer_ui -o app.lgx  # from Logos Storage by CID, written only if sha256 (required) and root match
```

`verify` recomputes the package root from the files (the `lgx` Merkle tree), so the genuine
`manifest.json` next to other files is refused (`root_mismatch`), and reads a `.lgx` strictly
enough that lgx can't read it differently (a polyglot is refused). The root does not cover
`manifest.json`: the record's `sha256` (a `.lgx`) and `manifest` (an installed copy) do, and
only then is the result `verified`; with neither it is `files_verified`, exit 3. `record`
vouches only for a package in lgx's own layout (as checked: tar headers, order, end marker,
canonical manifest lgx writes back unchanged). A differential test
(`sdk/rust/tests/fixtures/apps/differential.py`, run on a Mac with Basecamp's own `liblgx`)
installs each fixture and real package the way lgpm does (its validity gate, the host variant,
`lgx_extract`, the manifest from `lgx_get_manifest_json`) onto a case-insensitive volume, and
fails if lezreg reads a package whose installed bytes or manifest differ from what lezreg
hashed. It covers one liblgx build and the host's variant; it doesn't cover other variants,
other liblgx versions, or what Basecamp does after install. See
[`docs/APPS.md`](docs/APPS.md) §2–4. Names are normalised as
logos-names SPEC.md §2 says (`lns:Forumer` is `forumer`), and a name with a key set twice is
refused as ambiguous. A name that forwards (`nameswallet` → `names`, logos-names SPEC.md §7) is
checked where it points. In Basecamp, the `program_registry` module does the same check
(`verify_app`, `verify_package`, `publisher_apps`), and Names Wallet shows it on every approval
prompt: "forumer ✓ verified publisher", or a warning that the app may be a copy.

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

**Start with [`docs/sdk.md`](docs/sdk.md)**: resolving a program ID to its metadata and IDL, in
Rust, JavaScript and from a Basecamp app, with a runnable example CI runs on every push.

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
lezreg image-id prog.bin --compare <64 hex>         # a local ELF or .bin's image id, as the loader computes it; MATCH/MISMATCH
lezreg idl encode --idl my.idl.json --kind message --name transfer --value '{"to":"…","amount":"5"}'   # hex Borsh
lezreg idl decode --idl my.idl.json --kind message --data <hex>       # named fields (also --kind state|event)
lezreg call --idl my.idl.json --message transfer --args '{…}' --plan  # bytes, actors, signers; offline, sends nothing
```

`idl encode|decode` and `call --plan` read SPEL v1, SPEL v2 and v1 + `x-lez04` IDLs
(`lezreg_sdk::idl_codec`). `call --plan` is an offline plan, not a simulation: it does not
report apply/revert, cycles or the fee (a registry write's `--dry-run` does), and there is no
generic "send this message to any program" command.

**Third-party services used by `verify` and `build`.** Both disclose these in `--help`, on stderr
at every run, and in their JSON output:

| Service | Used for | Offline input |
| --- | --- | --- |
| Docker Hub (`risczero/risc0-guest-builder@sha256:…`) | the builder image | `--image-tar <docker save tar>` (loaded with `docker load`) |
| crates.io (`cargo fetch --locked`) | guest dependencies | `--vendor-tar <cargo vendor tar>` |
| The entry's git forge (`git clone`) | `verify` only, when no archive is used | `--archive-file <git archive tar>` with `--no-forge` |

`--offline` refuses any remaining network step apart from your configured sequencer and Storage
node, and fails before doing any work if an input is missing. The registry itself (register,
update, lookup, search, IDL) needs none of these; there is no analytics code.

**IDL formats.** The registry takes an IDL in any JSON format and tells clients which one it is:
SPEL v1 (`instructions[]`, what `spel generate-idl` v0.7.0 makes and what the registry's own IDL
uses), SPEL v2 (`"version": "2"` with `messages[]`, from SPEL's LEZ 0.4 rewrite), or "format not
recognised" for any other JSON object. An unrecognised format is still available, just shown
raw; only bytes that are not JSON count as unparseable. `idl get --json` reports `status`,
`format` and the list of calls (names, tags, argument names and types). See
[`docs/sdk.md`](docs/sdk.md#1-rust).

**JavaScript** (`sdk/js/lezreg.js`, no dependencies, browsers and Node 19+):

```js
import { Registry, fetchIdl } from "./lezreg.js";
const reg = new Registry({ rpc: "http://127.0.0.1:3062", registry: "<registry id>" });
const label = await reg.programLabel("<program id>");   // { text, kind, stale, upgradeable, ... }
const { deployer } = await reg.resolve("<program id>");
const { kind, statusKind, format, calls, idl } = await fetchIdl(deployer, { storageApi: "http://127.0.0.1:8080/api/storage/v1" });
// kind: "ok" | "unknown-format" | "unavailable" | "hash-mismatch" | "unparseable"; format: "spel-v1" | "spel-v2" | "unknown"
// statusKind: Rust's names ("Ok", "UnknownFormat", "Unavailable", "HashMismatch", "ParseError")
const mine = await reg.byAuthor("<account>", { limit: 20, scan: 1000 });  // { items, scanned, truncated }
const community = await reg.thirdParty("<program id>", 20);
```

Write bytes (register, third-party register, update) come from `sdk/js/writes.js`
(`buildChange`), checked byte for byte against the Rust builders; see
[`docs/sdk.md`](docs/sdk.md#discovery-and-writes-in-js).

**Basecamp apps:** depend on the `program_registry` core module and call
`program_label(program_id)` for a wallet prompt's one-line label, or drop in
[`RegistryLabel.qml`](registry_browser/qml/RegistryLabel.qml). **[`docs/INTEGRATE.md`](docs/INTEGRATE.md)**
is the short guide: QML, `logosctl`, JS, Rust, and writing entries through the module.

Errors are typed (`lezreg_sdk::Error`): `Transport`, `Rpc`, `BadRecord`, `MissingLink`, `NoEntry`,
`NotDeployed`, `Invalid`, `NoKey`, `NotAuthor`, `Frozen`, `Refused` (simulation), `Funds`,
`Rejected`, `NotIncluded { hash, detail }`, `Reverted`, `ReadBack`, and others. With `--json`,
the CLI reports the same kinds as `{"error", "kind"}`, plus `tx` when a transaction was sent and
`program` when a deploy failed after its header key was saved.

## Tests

```sh
cargo test -p lezreg-types -p lezreg-program            # unit tests
cargo test --release -p lezreg-engine-test              # the real guest on the rc3 state machine
cargo test --release -p lezreg-sdk -p lezreg            # SDK against a mocked sequencer; CLI unit tests
cargo build -p lezreg-sdk --no-default-features         # the read-only SDK still builds
cargo test -p lezreg-idl                                # IDL drift (scripts/idl.sh --check needs spel v0.7.0)
node --test                                             # JS SDK against the Rust vectors (sdk/js/lezreg.test.mjs) and the browser's logic
(cd module/rust-lib && cargo test --locked)             # the module; needs module/logos-rust-sdk-src (module.yml builds it with nix)
scripts/test-deploy-sh.sh                               # chain/deploy.sh resume/failure cases (stub lezreg)
# a write whose inclusion is never confirmed is confirmed by read-back (local sequencer, wallet copy):
LEE_WALLET_HOME_DIR=<copy> ENTRY=<entry> PAYER=<funded> scripts/test-not-included.sh
# end to end against a local standalone sequencer (writes need a COPY of a funded wallet):
LEZREG_E2E_RPC=http://127.0.0.1:3062 LEZREG_E2E_REGISTRY=<id> \
  [LEZREG_E2E_PAYER=<funded> LEE_WALLET_HOME_DIR=<wallet copy>] \
  cargo test --release -p lezreg-sdk --test e2e -- --nocapture --test-threads 1
# the evaluator demo (its own fresh chain and Storage nodes; transcript in demo-out/demo.log):
scripts/demo.sh
```

Network check (2026-11-02, macOS `sandbox-exec`): with outbound network denied
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
| `action/` | the GitHub Action (`verify`, `prepare`) |
| `chain/deploy.sh` | deploys the registry and its self-entry |
| `scripts/demo.sh` | the evaluator demo (local chain, or `--testnet`) |
| `idl/` | the registry's IDL: generated SPEL v1 + `x-lez04`, and a hand-written SPEL v2 twin checked against it |
| `tools/` | engine tests, IDL drift checks, the M1 smoke client |
| `docs/` | design notes and demo transcripts |

## Licence

MIT OR Apache-2.0.
