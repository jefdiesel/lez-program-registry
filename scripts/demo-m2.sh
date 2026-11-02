#!/usr/bin/env bash
# M2 demo core (DESIGN §11), run with the `lezreg` binary only, against a LOCAL LEZ v0.4.0-rc1
# standalone sequencer:
#
#   deploy the registry (chain/deploy.sh) -> deploy + register a program (DeployAndRegister)
#   -> third-party entry -> update -> stale/fresh -> transfer (both sign) -> freeze -> show
#   (deployer vs third-party, fresh/stale) -> list -> search by name and author -> Mallory's
#   unauthorized deployer entry refused in simulation and reverted on chain at her cost.
#
#   WALLET_SRC=/path/to/funded/wallet FUNDER=<account> REG_ELF=… SAMPLE_ELF=… scripts/demo-m2.sh
#
# The funded wallet is COPIED, never modified. Output goes to demo-out/ (gitignored); the
# transcript is demo-out/demo-m2.log. Mallory's wallet is made outside the transcript so no
# recovery phrase lands in it.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
: "${WALLET_SRC:?set WALLET_SRC to a funded wallet directory (it is copied, not modified)}"
: "${FUNDER:?set FUNDER to a funded public account in that wallet}"
: "${REG_ELF:?set REG_ELF to lez_program_registry.bin}"
: "${SAMPLE_ELF:?set SAMPLE_ELF to a sample program .bin}"
RPC=${RPC:-http://127.0.0.1:3062}
T=${CARGO_TARGET_DIR:-$ROOT/target}
export LEZREG=${LEZREG:-$T/release/lezreg}
COMMIT=${COMMIT:-$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo 0000000000000000000000000000000000000000)}
case "$RPC" in http://127.0.0.1:*|http://localhost:*) ;; *) echo "refusing non-local RPC $RPC" >&2; exit 2 ;; esac

OUT=$ROOT/demo-out
rm -rf "$OUT"
mkdir -p "$OUT"
cp -R "$WALLET_SRC" "$OUT/wallet"
export LEE_WALLET_HOME_DIR=$OUT/wallet
export LEZREG_CONFIG_DIR=$OUT/config XDG_CACHE_HOME=$OUT/cache
export LEZREG_NETWORK=local LEZREG_RPC=$RPC STATE=$OUT/deploy-local.env
LOG=$OUT/demo-m2.log

FUNDER=${FUNDER#Public/}
step() { printf '\n######## %s\n' "$*"; }
# Runs lezreg, echoing the command line first.
lz() { printf '$ lezreg'; printf ' %q' "$@"; printf '\n'; "$LEZREG" "$@"; }
# Runs lezreg and requires it to FAIL with output containing $1.
lz_fails() {
  local want=$1; shift
  printf '$ lezreg'; printf ' %q' "$@"; printf '   # expected to fail\n'
  local out rc=0
  out=$("$LEZREG" "$@" 2>&1) || rc=$?
  printf '%s\n' "$out"
  [ "$rc" -ne 0 ] || { echo "DEMO FAIL: that should have failed"; exit 1; }
  grep -q -- "$want" <<<"$out" || { echo "DEMO FAIL: expected '$want' in the output"; exit 1; }
  echo "   -> refused as expected (exit $rc)"
}
jget() { python3 -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else (v.get(k) if isinstance(v, dict) else None)
print("" if v is None else v)' "$1"; }
new_account() { # prints "<id> <pk>" for a new public account in the current wallet
  "$LEZREG" wallet account new public 2>/dev/null | python3 -c 'import sys,re; t=sys.stdin.read(); print(re.search(r"Public/(\w+)",t).group(1), re.search(r"With pk (\w+)",t).group(1))'
}
IDL_CID=0x01551220$(printf 'sample idl' | shasum -a 256 | cut -d' ' -f1)
META=(--idl-cid "$IDL_CID" --idl-sha256 "$(printf 'sample idl' | shasum -a 256 | cut -d' ' -f1)" --idl-size 10
      --repo https://github.com/jefdiesel/lez-program-registry --commit "$COMMIT"
      --manifest-path tools/engine-test/helper-methods/guest/Cargo.toml --binary lezreg_forwarder)

# Mallory: her own wallet (new recovery phrase, kept out of the transcript), funded below.
mkdir -p "$OUT/mallory"
read -r M M_PK < <(LEE_WALLET_HOME_DIR=$OUT/mallory new_account)

{
echo "LEZ Program Registry: M2 demo core, $(date -u '+%Y-%m-%d %H:%M:%SZ')"
echo "sequencer $RPC (local LEZ v0.4.0-rc1 standalone); lezreg $("$LEZREG" --version)"
echo "registry ELF $REG_ELF"
echo "sample ELF   $SAMPLE_ELF"
echo "payer (funded, copied wallet) $FUNDER; Mallory $M (her own wallet)"

step "1. Deploy the registry and register it in itself (chain/deploy.sh)"
"$ROOT/chain/deploy.sh" --elf "$REG_ELF" --payer "$FUNDER" --rpc "$RPC" --commit "$COMMIT" 2>&1
lz network show
REG=$("$LEZREG" network show --json | jget registry)
lz show "$REG"

step "2. Accounts: author A, third-party publisher Q, new author B (main wallet)"
read -r A _ < <(new_account); read -r Q _ < <(new_account); read -r B _ < <(new_account)
echo "A=$A Q=$Q B=$B"

step "3. Deploy a program and register it in the same transaction (DeployAndRegister; header X and author A sign, FUNDER pays)"
DEP=$("$LEZREG" deploy "$SAMPLE_ELF" --author "$A" --payer "$FUNDER" --name smoke-forwarder --version 1.0.0 \
  --label "Smoke Labs" --description "sample program, deployed and registered in one transaction" --tag sample --tag demo \
  "${META[@]}" --json)
echo "$DEP"
X=$(echo "$DEP" | jget program)
echo "X=$X"
lz show "$X"

step "4. A third-party (community) entry for X, published by Q"
lz register --third-party --program "$X" --publisher "$Q" --payer "$FUNDER" --name smoke-forwarder --version 1.0.0 \
  --label "a community packager" --description "community metadata for X" --tag sample "${META[@]}"

step "5. Update by the author A (revision 2, release snapshot #2)"
lz update "$X" --version 1.1.0 --description "updated by its author" --payer "$FUNDER"

step "6. Stale and fresh: claim a different image (stale), then the live one again (fresh)"
LIVE=$("$LEZREG" show "$X" --json | jget header.image_id)
OTHER=$("$LEZREG" show "$REG" --json | jget header.image_id)
lz update "$X" --image-id "$OTHER" --version 1.2.0-wrong --payer "$FUNDER"
lz show "$X"
lz update "$X" --image-id "$LIVE" --version 1.2.0 --payer "$FUNDER"
lz show "$X" --revision 3

step "7. Transfer X's deployer entry from A to B (A and B both sign, in one transaction)"
lz transfer "$X" --to "$B" --payer "$FUNDER"

step "8. Freeze it (B, the new author, signs); a frozen entry refuses further changes"
lz freeze "$X" --payer "$FUNDER"
lz_fails "frozen" update "$X" --version 9.9.9 --payer "$FUNDER"

step "9. show: the deployer entry (proven by its address) vs the community entry"
lz show "$X"
lz show "$X" --publisher "$Q"

step "10. list (paged, newest first)"
lz list --limit 2
NEXT=$("$LEZREG" list --limit 2 --json | jget next)
lz list --limit 2 --after "$NEXT"

step "11. search by name (exact, then substring) and by proven author"
lz search name smoke-forwarder
lz search name forward
lz search author "$B"
lz search author "$A"
lz search author "$Q"
lz search label smoke
lz search tag demo

step "12. Mallory (her own wallet) tries to create the deployer entry of a program she does not control"
lz wallet auth-transfer send --from "Public/$FUNDER" --to-pk "$M_PK" --amount 20000000 2>&1 | grep -E "^Transaction (hash|is included)"
echo "-- target: a program X3 deployed by someone else (plain loader deploy, no entry)"
X3=$("$LEZREG" deploy "$SAMPLE_ELF" --no-register --payer "$FUNDER" --json | jget program)
echo "X3=$X3"
export MW=$OUT/mallory
echo "-- (a) RegisterDeployer at (X3, REG) signed only by Mallory: simulation refuses, nothing is sent"
LEE_WALLET_HOME_DIR=$MW lz_fails "must sign" register --program "$X3" --author "$M" --payer "$M" --name smoke-forwarder --version 6.6.6 \
  --label "totally the deployer" "${META[@]}"
echo "-- (a) forced on chain (--force-send): the registry reverts it and Mallory pays her whole gas limit"
LEE_WALLET_HOME_DIR=$MW lz_fails "REVERTED" register --program "$X3" --author "$M" --payer "$M" --name smoke-forwarder --version 6.6.6 \
  --label "totally the deployer" "${META[@]}" --force-send --gas-limit 400000
lz show "$X3"
"$LEZREG" show "$X3" --json | jget deployer_entry | grep -q . && { echo "DEMO FAIL: Mallory got an entry"; exit 1; }
echo "   -> (X3, REG) is still empty"
echo "-- (e) Mallory tries to update X, an entry she is not the author of"
LEE_WALLET_HOME_DIR=$MW lz_fails "only the entry's author" update "$X" --author "$M" --payer "$M" --version 6.6.6
LEE_WALLET_HOME_DIR=$MW lz_fails "REVERTED" update "$X" --author "$M" --payer "$M" --version 6.6.6 --force-send --gas-limit 200000
echo "-- (d) a second deployer entry for X, even by X's own key, is refused"
lz_fails "already has a deployer entry" register --program "$X" --payer "$FUNDER" --name smoke-forwarder --version 2.0.0 "${META[@]}"

step "13. Final state"
GOT=$("$LEZREG" show "$X" --json | jget deployer_entry.author)
echo "deployer entry author: $GOT (expected B=$B)"
[ "$GOT" = "$B" ] || { echo "DEMO FAIL: author"; exit 1; }
lz list --limit 10
echo
echo "REG=$REG X=$X A=$A B=$B Q=$Q X3=$X3 M=$M"
echo "DEMO PASS"
} 2>&1 | tee "$LOG"
[ "${PIPESTATUS[0]}" -eq 0 ]
