#!/usr/bin/env bash
# Measures what registry transactions cost on a real LEZ sequencer (docs/CYCLES.md, "On chain"):
# a fresh standalone rc3 chain, the registry deployed from artifacts/, then each operation sent
# by lezreg, with the payer's balance read before and after. Writes one JSON line per step to
# $OUT/chain.jsonl and the full lezreg output to $OUT/chain.log.
#
#   SEQ=<sequencer_service built with --features standalone at v0.4.0-rc3> \
#   NETWORK_JSON=<lez>/lez/sequencer/service/networks/local.json \
#   LEZREG=<lezreg> B430=<lezreg_ballast_430k.bin> B1M=<lezreg_ballast_1m.bin> \
#   OUT=<dir> tools/cycle-bench/chain-costs.sh
#
# Local chain only: the payer is LEZ's published local-test genesis account (as e2e.yml).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
: "${SEQ:?}" "${NETWORK_JSON:?}" "${LEZREG:?}" "${B430:?}" "${B1M:?}" "${OUT:?}"
PORT=${PORT:-3462}
RPC=http://127.0.0.1:$PORT
mkdir -p "$OUT"
W=$OUT/run
rm -rf "$W" && mkdir -p "$W/wallet"
export LEE_WALLET_HOME_DIR=$W/wallet LEE_WALLET_PASSWORD= LEZREG_CONFIG_DIR=$W/config \
  XDG_CACHE_HOME=$W/cache XDG_DATA_HOME=$W/data
L=$LEZREG
LOG=$OUT/chain.log JL=$OUT/chain.jsonl
: > "$LOG"; : > "$JL"

rpc() { curl -sf -m 10 -X POST "$RPC" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":${2:-[]}}"; }
bal() { rpc getAccountBalance "[\"$1\"]" | python3 -c 'import sys,json;print(json.load(sys.stdin)["result"])'; }
fees() { rpc getFeeState | python3 -c 'import sys,json;r=json.load(sys.stdin)["result"];print(r["base_fee_exec"], r["base_fee_stor"])'; }

"$SEQ" setup "$W/seq" --config "$NETWORK_JSON" \
  --funding-public-key 0101010101010101010101010101010101010101010101010101010101010100 --create-channel >> "$LOG" 2>&1
RUST_LOG=warn nohup "$SEQ" start "$W/seq" --rpc-listen-address "127.0.0.1:$PORT" \
  --metrics-listen-address "127.0.0.1:$((PORT + 6000))" > "$W/seq.log" 2>&1 &
echo $! > "$W/seq.pid"
trap 'kill "$(cat "$W/seq.pid")" 2>/dev/null || true' EXIT
for _ in $(seq 1 60); do rpc getLastBlockId >/dev/null && break; sleep 2; done

"$L" wallet config set sequencer_addr "$RPC" >> "$LOG" 2>&1 || true
"$L" wallet account list >/dev/null 2>&1
# PRIVATE_KEY_PUB_ACC_A in lez/testnet_initial_state/src/lib.rs (public test key; local chains only).
out=$("$L" wallet account import public --private-key 10a26a9aec7d34b82364eeae45c5294dbb0a764b000b94eeb9b58511dc487c4d 2>&1)
P=$(echo "$out" | sed -n 's|.*Public/\([1-9A-HJ-NP-Za-km-z]*\).*|\1|p' | head -1)
"$L" network set cyc --rpc "$RPC" >> "$LOG" 2>&1
export LEZREG_NETWORK=cyc
newacc() { "$L" wallet account new public 2>&1 | sed -n 's|.*Public/\([1-9A-HJ-NP-Za-km-z]*\).*|\1|p' | head -1; }

# step <name> <command...>: runs it with --json, records the payer's balance delta and the
# receipt (or the deploy dry run's reserve).
step() {
  local name=$1; shift
  local before after rc=0 base
  base=$(fees)
  before=$(bal "$P")
  echo "== $name: $*" >> "$LOG"
  "$L" --json "$@" > "$W/last.json" 2>> "$LOG" || rc=$?
  cat "$W/last.json" >> "$LOG"
  after=$(bal "$P")
  python3 - "$name" "$before" "$after" "$rc" "$base" "$W/last.json" >> "$JL" <<'EOF'
import json, sys
name, before, after, rc, base, path = sys.argv[1:7]
try:
    r = json.load(open(path))
except Exception:
    r = None
print(json.dumps({"step": name, "payer_spent": int(before) - int(after), "exit": int(rc),
                  "base_fees_before": base, "receipt": r}))
EOF
}

IDL=$ROOT/idl/lez_program_registry.idl.json
TYPICAL=(--name token-program --version 1.2.3 --label "Example Labs"
  --description "A fungible token program with mint, burn and transfer, deployed on LEZ."
  --tag token --tag defi --idl-cid raw --idl-file "$IDL"
  --repo https://github.com/example-org/example-program.git --commit 0303030303030303030303030303030303030303
  --archive-cid raw --archive-file "$IDL"
  --manifest-path methods/guest/Cargo.toml --binary guest --lock-file "$ROOT/Cargo.lock")

# 0. The registry itself, from artifacts/ (segments, CreateHeader, RegisterDeployer at (R, R)).
before=$(bal "$P")
LEZREG=$L "$ROOT/chain/deploy.sh" --elf "$ROOT/artifacts/lez_program_registry.bin" --payer "$P" --rpc "$RPC" \
  --network cyc --commit "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo 0303030303030303030303030303030303030303)" \
  > "$W/deploy.log" 2>&1
cat "$W/deploy.log" >> "$LOG"
after=$(bal "$P")
REG=$(sed -n 's/^REG=//p' "$W/deploy.log" | tail -1)
echo "{\"step\": \"registry deploy (chain/deploy.sh)\", \"payer_spent\": $((before - after)), \"reg\": \"$REG\"}" >> "$JL"
export LEZREG_REGISTRY=$REG

# 1. Plain loader deploy of the ~430 KB program, then RegisterDeployer of the deployed program.
step "dry run: plain deploy 430k" deploy "$B430" --no-register --payer "$P" --dry-run
step "plain deploy 430k (segments + CreateHeader)" deploy "$B430" --no-register --payer "$P"
X1=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["program"])' "$W/last.json")
step "dry run: RegisterDeployer" register --program "$X1" --author "$P" --payer "$P" "${TYPICAL[@]}" --dry-run
step "RegisterDeployer (typical, author = payer)" register --program "$X1" --author "$P" --payer "$P" "${TYPICAL[@]}"

# 2. Deploy + register in one DeployAndRegister: ~430 KB, the registry's own 555 KB, ~1 MB.
for elf in "$B430" "$ROOT/artifacts/lez_program_registry.bin" "$B1M"; do
  n=$(basename "$elf" .bin)
  step "dry run: deploy+register $n" deploy "$elf" --author "$P" --payer "$P" "${TYPICAL[@]}" --dry-run
  step "deploy+register $n (segments + DeployAndRegister)" deploy "$elf" --author "$P" --payer "$P" "${TYPICAL[@]}"
done
X2=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["program"])' "$W/last.json")

# 3. Updates.
step "dry run: Update (version)" update "$X1" --version 1.2.4 --payer "$P" --dry-run
step "Update (version)" update "$X1" --version 1.2.4 --payer "$P"
step "Update (version, description, IDL)" update "$X1" --version 1.3.0 \
  --description "A fungible token program with mint, burn and transfer; now with permits." \
  --idl-cid raw --idl-file "$ROOT/artifacts/build.json" --payer "$P"

# 4. A second account: third-party entry, transfer, freeze.
Q=$(newacc)
"$L" wallet auth-transfer send --from "Public/$P" --to "Public/$Q" --amount 50000000 >> "$LOG" 2>&1
sleep 3
step "RegisterThirdParty (typical, publisher Q, fee paid by P)" register --third-party --program "$X2" --publisher "$Q" --payer "$P" "${TYPICAL[@]}"
step "TransferAuthor (P to Q, both sign)" transfer "$X1" --to "$Q" --payer "$P"
step "Freeze (author = payer)" freeze "$X2" --payer "$P"

# 5. A forced revert: a second RegisterDeployer for X1, sent anyway with a 300,000 gas limit.
step "forced revert: duplicate RegisterDeployer, --gas-limit 300000" register --program "$X1" --author "$Q" --payer "$P" "${TYPICAL[@]}" --force-send --gas-limit 300000

echo "done: $JL"
