#!/usr/bin/env bash
# M1 smoke run on a LOCAL rc1 standalone sequencer (default 127.0.0.1:3062). Uses a COPY of a
# funded wallet (never the original). Output: smoke-out/smoke.log (gitignored).
#   WALLET_SRC=/path/to/funded/wallet FUNDER=<base58 account> scripts/smoke.sh
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
: "${WALLET_SRC:?set WALLET_SRC to a funded wallet directory (it is copied, not modified)}"
: "${FUNDER:?set FUNDER to a funded account in that wallet}"
RPC=${RPC:-http://127.0.0.1:3062}
T=${CARGO_TARGET_DIR:-$ROOT/target}
cargo build --release -p lezreg-methods -p lezreg-helper-methods -p lezreg-smoke
REG_ELF=$T/riscv-guest/lezreg-methods/lez_program_registry/riscv32im-risc0-zkvm-elf/release/lez_program_registry.bin
SAMPLE_ELF=$T/riscv-guest/lezreg-helper-methods/lezreg_forwarder/riscv32im-risc0-zkvm-elf/release/lezreg_forwarder.bin
mkdir -p smoke-out
rm -rf smoke-out/wallet
cp -R "$WALLET_SRC" smoke-out/wallet
export LEE_WALLET_HOME_DIR=$ROOT/smoke-out/wallet
"$T/release/lezreg-smoke" --rpc "$RPC" --reg-elf "$REG_ELF" --sample-elf "$SAMPLE_ELF" --funder "$FUNDER" "$@" 2>&1 | tee smoke-out/smoke.log
