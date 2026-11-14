# AGENTS.md: LEZ Program Registry

For coding agents (and people in a hurry). Last checked: 2026-11-13.

The registry turns a LEZ program id into a name, version, author, interface file (IDL) and a
source pointer anyone can rebuild and compare. `lezreg` is its command line. Full docs:
`README.md`, `docs/sdk.md` (resolve an id from Rust, JavaScript or a Basecamp module),
`docs/APPS.md` (apps checked by LNS name), `docs/CYCLES.md` (what each operation costs).

## The live registry

| | |
|---|---|
| Network | LEZ 0.4 (v0.4.0-rc3), a test network run by this project. Not Logos's public testnet. It is reset now and then; a reset wipes every entry and changes the id below. |
| Sequencer | `https://lez.nameswallet.app/` |
| Registry id | `7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82` |

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
git clone https://github.com/jefdiesel/lez-program-registry
cd lez-program-registry
cargo build --release -p lezreg          # Rust toolchain from rust-toolchain.toml; Linux also needs libpcsclite-dev and pkg-config
# define lz in the checkout, so it keeps pointing at this build after you cd elsewhere (works in sh, bash and zsh):
lz() { "$PWD/target/release/lezreg" --rpc https://lez.nameswallet.app/ --registry 7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82 "$@"; }
lz doctor                                 # sequencer, clock and registry should say ok
lz show 7sGWX1JTYoD2GFs69sWdX9PqJzhpcdk8ouZBu3BYeY82    # the registry's own entry
lz show <PROGRAM_ID> --json               # any program: entry, status, IDL pointer, source
```

`show` prints the deployer entry (written with the program's own key), whether the program
still runs the code the entry describes (`FRESH` or `STALE`), and any community entries.
Through this address `list` and `search` fail today (it refuses grouped requests); they work on
a sequencer you run yourself.

## Register a program (needs a person)

What the person must have: the program deployed on this network, the program's header key in
their LEZ wallet (`~/.lee/wallet`), a funded account, an IDL file, and a public git repository
at the deployed commit. Writes also need `r0vm` 3.0.5 (RISC Zero, via `rzup`) for the local
simulation, and a Logos Storage node for the IDL upload (`lezreg storage up` starts one).

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

## Work on this repository

- Tests: `cargo test --release -p lezreg-sdk -p lezreg`, and `node sdk/js/lezreg.test.mjs`.
- The whole walk an evaluator runs: `scripts/demo.sh` (starts its own sequencer and Storage
  nodes; `--verify` needs Docker). CI runs it in `.github/workflows/demo.yml`.
- The on-chain program is `program/`; its reproducible build is `artifacts/` (built in CI by
  `repro.yml`, never by hand). Don't change `program/` or `artifacts/` without saying so: the
  image id is the registry's identity.
- Keep this file current: when a command, the network or the registry id changes, change it here
  in the same commit.

## What was checked, and how

- `doctor` and `show` against the live address: run 2026-11-13 with lezreg 0.1.0 (rc3).
- `init --detect`, `publish` and `adopt`: run on every push by `.github/workflows/e2e.yml`
  against a local sequencer. `--dry-run` is exercised by `scripts/demo.sh` (also in CI), for a deploy.
  `adopt --dry-run` against the live address with an outside account has not been run by the
  authors.
- Building `lezreg` from a clean clone: CI (`ci.yml`, `demo.yml`) on Linux; macOS arm64 by hand.
