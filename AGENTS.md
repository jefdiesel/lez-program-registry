# AGENTS.md: LEZ Program Registry

For coding agents (and people in a hurry). Last checked: 2026-11-20.

The registry turns a LEZ program id into a name, version, author, interface file (IDL) and a
source pointer anyone can rebuild and compare. `lezreg` is its command line. Full docs:
`README.md`, `docs/sdk.md` (resolve an id from Rust, JavaScript or a Basecamp module),
`docs/APPS.md` (apps checked by LNS name), `docs/CYCLES.md` (what each operation costs).
Every `lezreg` command, in one table: README §"All commands".

## The live registry

| | |
|---|---|
| Network | LEZ 0.4 (v0.4.0-rc3), a test network run by this project. Not Logos's public testnet. It is reset now and then; a reset wipes every entry and changes the id below. |
| Sequencer | `https://lez.nameswallet.app/` |
| Registry id | `7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82` |
| Network file | embedded as `sdk/rust/network-rc3.json`: a lezreg built from this repo reaches it with `--network testnet` |

## Rules for agents

- **Never put a private key, seed phrase or wallet file in a prompt, a log, a commit or an issue.**
  Don't read `~/.lee/wallet` or any `storage.json` into your context.
- **Never send a transaction without `--dry-run` first**, and show the dry run's output to the
  person before sending. A failed transaction still pays its fee.
- **Never pass `--yes` on your own.** It allows writing through a remote sequencer; the person
  must decide that.
- Keys and coins need a human: you cannot create a funded account. On the live network coins
  are handed out by the chain's operator; there is no faucet.
- Reads (`show`, `list`, `search`, `doctor`) are safe: no account, no coins, nothing is signed.

## Read the live registry (no account needed)

```sh
# install lezreg (Linux x86_64/aarch64 with glibc 2.34+, macOS on Apple silicon; checks SHA-256; edits no shell files)
curl --proto '=https' --tlsv1.2 -fsSL https://github.com/jefdiesel/lez-program-registry/releases/latest/download/install.sh | sh
export PATH="$HOME/.lezreg/bin:$PATH"
lz() { lezreg --rpc https://lez.nameswallet.app/ --registry 7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82 "$@"; }
lz doctor                                 # sequencer, clock and registry should say ok
lz list                                   # every entry, newest first
lz show 7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82    # the registry's own entry
lz show <PROGRAM_ID> --json               # any program: entry, status, IDL pointer, source
```

The installer fetches the latest release, 0.1.0. It predates `--network testnet` (hence `lz`
above), the IDL status in `show`, `idl encode|decode`, `call --plan`, `image-id` and
`verify --offline`; for those, build from source.

No prebuilt build for your machine (Intel Mac, musl)? Build it: `git clone` this repository,
`cargo build --release -p lezreg` (Linux also needs libpcsclite-dev and pkg-config), and use
`target/release/lezreg`.

`show` prints the deployer entry (written with the program's own key), whether the program
still runs the code the entry describes (`FRESH` or `STALE`), and any community entries.
This address refuses grouped requests, so `list` and `search` read one call at a time there:
correct, just slower.

## Register a program (needs a person)

What the person must have: the program deployed on this network, the program's header key in
their LEZ wallet (`~/.lee/wallet`), a funded account, an IDL file, and a public git repository
at the deployed commit. Writes also need `r0vm` 3.0.5 for the local simulation and a Logos
Storage node for the IDL upload: add `-s -- --with-r0vm --with-storage` to the install line
(`... | sh -s -- --with-r0vm --with-storage`), then `lezreg storage up` starts the node. Before
any write, save the network yourself: `lezreg network set <name> --rpc <url> --registry <id>`.

```sh
cd <the program's repository>
lz init --detect                          # writes lezreg.toml from Cargo.toml, Cargo.lock and git; refuses a dirty tree
lz adopt <PROGRAM_ID> --dry-run           # simulate: what would be written, the gas and the fee; sends nothing
# show that output to the person; only if they agree, and they add the flags themselves:
#   lz adopt <PROGRAM_ID> --payer <funded account> --yes
```

`publish` does the same for a program that is not deployed yet (`--elf <program.bin>`), and
updates an entry that is already registered. Costs on rc3: registering a deployed program
spends about 1.5M and needs about 2.6M held; deploying and registering costs about 24 per byte
of program (`docs/CYCLES.md`).

## The Basecamp packages

`module/` is the `program_registry` core module and `registry_browser/` its app (QML). Since
0.3.1 / 0.1.1 the module reads the live registry above with no setup: its defaults come from the
built-in network profile `module/rust-lib/network-rc3.json` (`names-network` format). The host
moves it with `logosctl call program_registry config_set '{"network_file": "<abs path>"}'` or
single values (`sequencer`, `registry`, `lns`); `config_get` shows what is in use. Build:
`nix build .#lgx-portable` in `module/` and in `registry_browser/` (as `module.yml` does); install
and use: `docs/INTEGRATE.md`. Both manifests carry `"author": "lns:jefdiesel"`.
Since 0.1.7 Basecamp shows the browser as "Apps" (display name only; the ids `registry_browser` and
`program_registry` are unchanged). Since 0.1.3 it reads like a guidebook: an index grouped by section (from tags), each entry a
title, one sentence and its publisher, with a guide page beside it and everything technical folded
under "Technical details". Titles and sentences come from the entry (`registry_browser/qml/logic.mjs`,
`guideText` / `indexRows`); the look is in `registry_browser/qml/kit/` (no registry logic there:
shared with our other apps). Offscreen renders: `registry_browser/tools/render.py`, with answers from
`tools/record.py` (logosctl) or `tools/answers_from_lezreg.py` (read-only `lezreg ... --json` output).

## Work on this repository

- Tests: `cargo test --release -p lezreg-sdk -p lezreg`, `cargo test --release -p lezreg-engine-test`
  (needs the risc0 guest toolchain; `cycles_match_docs_cycles_md` checks `docs/CYCLES.md`), and
  `node --test` (JS SDK and the browser's logic). The module's tests run in `module.yml`. The full
  list is README §Tests.
- Offline IDL tools, no chain needed: `lezreg idl encode|decode --idl F`, `lezreg call --idl F
  --message M --args JSON --plan` (sends nothing), `lezreg image-id <elf|bin> --compare <hex>`.
- The whole walk an evaluator runs: `scripts/demo.sh` (starts its own sequencer and Storage
  nodes; `--verify` needs Docker). CI runs it in `.github/workflows/demo.yml`.
- The on-chain program is `program/`; its reproducible build is `artifacts/` (built in CI by
  `repro.yml`, never by hand). Don't change `program/` or `artifacts/` without saying so: the
  image id is the registry's identity.
- Keep this file current: when a command, the network or the registry id changes, change it here
  in the same commit.

## What was checked, and how

- The install line, then `list`, `doctor` and `show` against the live address: run 2026-11-13
  on macOS arm64 with the published lezreg 0.1.0; the installer also runs in CI on Debian,
  Ubuntu and Fedora.
- `init --detect`, `publish` and `adopt`: run on every push by `.github/workflows/e2e.yml`
  against a local sequencer. `--dry-run` is exercised by `scripts/demo.sh` (also in CI), for a deploy.
  `adopt --dry-run` against the live address with an outside account has not been run by the
  authors.
- `--network testnet network show --check-chain` and `show` with its IDL status line: run
  2026-11-20 against the live address from a source build (chain check ok; IDL status
  "unavailable" without a local Storage node, entry still shown).
- Building `lezreg` from a clean clone: CI (`ci.yml`, `demo.yml`) on Linux; macOS arm64 by hand.
