#!/usr/bin/env bash
# Regenerates idl/ (DESIGN §6.3):
#   idl/lez_program_registry.spel.json  the released `spel generate-idl` v0.7.0 output, untouched
#   idl/lez_program_registry.idl.json   that plus `lezreg-idl-extend` (x-lez04, discriminator,
#                                       execution, variant, spec, metadata)
# With --check, fails if either committed file differs from a fresh run.
#
# The generator is SPEL's released v0.7.0 CLI, built from its tag with its own toolchain (1.94.0)
# into _deps/ (or SPEL_BIN).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
SPEL=${SPEL_BIN:-$ROOT/_deps/spel-root/bin/spel}
SPEL_REV=4107dcb14ec0eb64e9e80a9691a3df0b9acd3c2b  # v0.7.0
if [ ! -x "$SPEL" ]; then
  echo "installing spel v0.7.0 ($SPEL_REV) into _deps/spel-root" >&2
  cargo +1.94.0 install --locked --git https://github.com/logos-co/spel.git --rev "$SPEL_REV" spel --root "$ROOT/_deps/spel-root"
fi
# The generator must be v0.7.0 (its --version prints nothing useful; cargo's install record does).
if [ -z "${SPEL_BIN:-}" ]; then
  grep -q '"spel 0.7.0 (' "$ROOT/_deps/spel-root/.crates.toml" || { echo "_deps/spel-root is not spel 0.7.0" >&2; exit 1; }
  grep -q "$SPEL_REV" "$ROOT/_deps/spel-root/.crates.toml" || echo "warning: _deps/spel-root was not installed from commit $SPEL_REV (local path install?)" >&2
fi
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
"$SPEL" generate-idl program/src/lib.rs > "$TMP/spel.json"
cargo run -q -p lezreg-idl --bin lezreg-idl-extend -- "$TMP/spel.json" > "$TMP/idl.json"
if [ "${1:-}" = "--check" ]; then
  cmp "$TMP/spel.json" idl/lez_program_registry.spel.json
  cmp "$TMP/idl.json" idl/lez_program_registry.idl.json
  echo "idl/: up to date"
else
  mkdir -p idl
  cp "$TMP/spel.json" idl/lez_program_registry.spel.json
  cp "$TMP/idl.json" idl/lez_program_registry.idl.json
  sha256sum idl/*.json 2>/dev/null || shasum -a 256 idl/*.json
fi
