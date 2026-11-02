#!/usr/bin/env bash
# Deploys the LEZ Program Registry to a LEZ 0.4 sequencer and registers it in itself
# (DESIGN §7.5): writes the guest's segments, sends the loader CreateHeader at a new header
# account R, then RegisterDeployer at (R, R), signed by R's header key (author = R). Finally it
# saves R as the network's registry id (`lezreg network set`).
#
# DeployAndRegister cannot create the self-entry: its root (R, R) needs R's code to exist
# already, and such a transaction is dropped as "Unknown program" (spike d, N5). So the
# self-entry has no born_with_deploy; R's header-key signature is its proof.
#
# Resumable: the header id is kept in $STATE as soon as its key is saved in the wallet (even
# when the deploy fails after that). On a rerun, a header already at R skips the deploy, a
# saved R without a header is deployed at R again (never a second registry), and an entry
# already at (R, R) skips the registration. A read that fails stops the script: it never
# takes "could not read" for "not there".
#
#   chain/deploy.sh --elf lez_program_registry.bin --payer <funded account> \
#       [--rpc http://127.0.0.1:3062] [--network local] [--commit <hex>] [--idl-cid <cid>] [--yes]
#
# Needs: `lezreg` on PATH (or LEZREG=/path/to/lezreg), python3, and a wallet
# (LEE_WALLET_HOME_DIR) holding the payer's key. The new header key is created in that wallet:
# back it up, it is the registry's upgrade key.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
LEZREG=${LEZREG:-lezreg}
NETWORK=${LEZREG_NETWORK:-local}
RPC=""
ELF=""
PAYER=${LEZREG_PAYER:-}
COMMIT=""
IDL_CID=""
YES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --elf) ELF=$2; shift 2 ;;
    --rpc) RPC=$2; shift 2 ;;
    --network) NETWORK=$2; shift 2 ;;
    --payer) PAYER=$2; shift 2 ;;
    --commit) COMMIT=$2; shift 2 ;;
    --idl-cid) IDL_CID=$2; shift 2 ;;
    --yes) YES=(--yes); shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done
: "${ELF:?--elf <lez_program_registry.bin> is required}"
: "${PAYER:?--payer <funded account> is required}"
[ -f "$ELF" ] || { echo "no such file: $ELF" >&2; exit 2; }
NET=(--network "$NETWORK")
[ -n "$RPC" ] && NET+=(--rpc "$RPC")
STATE=${STATE:-$ROOT/chain/.deploy-$NETWORK.env}
IDL=$ROOT/idl/lez_program_registry.idl.json

if [ -z "$COMMIT" ]; then
  COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || true)
  [ -n "$COMMIT" ] || { echo "not a git checkout: pass --commit <hex>" >&2; exit 2; }
fi
if [ -z "$IDL_CID" ]; then
  # Logos Storage upload of the IDL is milestone M3. Until then the entry points at a CIDv1
  # (raw codec, sha2-256) of the exact IDL bytes, which any IPFS-style tool can recompute; the
  # sha256 and size on chain are the IDL's own either way.
  IDL_CID=$(python3 -c 'import hashlib,sys; d=hashlib.sha256(open(sys.argv[1],"rb").read()).digest(); print("0x"+(bytes([1,0x55,0x12,0x20])+d).hex())' "$IDL")
fi

json_get() { python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v.get(k) if isinstance(v, dict) else None
print("" if v is None else v)' "$1"; }

REG=""
[ -f "$STATE" ] && . "$STATE"

# `show R --json`, or stop: a transport error must not read as "nothing there".
show_reg() {
  local out
  out=$("$LEZREG" "${NET[@]}" --registry "$REG" show "$REG" --json) || { echo "cannot read $REG: $out" >&2; exit 1; }
  printf '%s' "$out"
}

# 1. Deploy (segments + CreateHeader), unless R already has a header.
HEADER=""
if [ -n "$REG" ]; then
  SHOWN=$(show_reg) || exit 1
  HEADER=$(printf '%s' "$SHOWN" | json_get header.image_id)
fi
if [ -n "$HEADER" ]; then
  echo "registry header already deployed at $REG (from $STATE); skipping the deploy"
else
  AT=()
  if [ -n "$REG" ]; then
    echo "== $REG (from $STATE) has no header yet: deploying the registry guest $ELF there"
    AT=(--program "$REG")
  else
    echo "== deploying the registry guest $ELF"
  fi
  RC=0
  OUT=$("$LEZREG" "${NET[@]}" deploy "$ELF" --no-register ${AT[@]+"${AT[@]}"} --payer "$PAYER" ${YES[@]+"${YES[@]}"} --json) || RC=$?
  # `program` is in the output, an error included, once its key is saved in the wallet.
  NEW=$(echo "$OUT" | json_get program)
  if [ -n "$NEW" ] && [ "$NEW" != "$REG" ]; then
    REG=$NEW
    echo "REG=$REG" > "$STATE"
  fi
  if [ "$RC" -ne 0 ]; then
    echo "deploy failed: $OUT" >&2
    [ -n "$REG" ] && echo "the header account $REG is saved in $STATE: rerun to finish the deploy there" >&2
    exit 1
  fi
  [ -n "$REG" ] || { echo "deploy printed no program id: $OUT" >&2; exit 1; }
  echo "deployed the registry at $REG (saved in $STATE)"
fi

# 2. Self-entry at (R, R), unless it is already there.
SHOWN=$(show_reg) || exit 1
if [ -n "$(printf '%s' "$SHOWN" | json_get deployer_entry.name)" ]; then
  echo "self-entry (REG, REG) already registered; skipping"
else
  echo "== registering the registry in itself at (REG, REG)"
  "$LEZREG" "${NET[@]}" --registry "$REG" register --program "$REG" \
    --name lez_program_registry --version "$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)" \
    --label "LEZ Program Registry" \
    --description "The LEZ Program Registry, registered in itself: on-chain registry of LEZ 0.4 programs with proven deployer entries" \
    --tag registry --tag infrastructure \
    --idl-cid "$IDL_CID" --idl-file "$IDL" \
    --repo https://github.com/jefdiesel/lez-program-registry --commit "$COMMIT" \
    --manifest-path methods/guest/Cargo.toml --binary lez_program_registry --lock-file "$ROOT/Cargo.lock" \
    --payer "$PAYER" ${YES[@]+"${YES[@]}"}
fi

# 3. Make it the network's registry (lezreg checks the self-entry first).
"$LEZREG" "${NET[@]}" network set "$NETWORK" --registry "$REG" ${RPC:+--rpc "$RPC"}
echo "REG=$REG"
