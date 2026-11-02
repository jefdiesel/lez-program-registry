#!/usr/bin/env bash
# Regression test for review findings robustness 1 and 2: a write whose inclusion cannot be
# confirmed (a proxy answers getTransaction with null) must hit its deadline and then confirm
# the change by reading the chain back, instead of failing with "not included" (and a retry
# paying twice) or hanging for hours.
#
#   LEE_WALLET_HOME_DIR=<wallet COPY> ENTRY=<entry its key holder authors> PAYER=<funded> \
#     [LEZREG=target/release/lezreg] [RPC=http://127.0.0.1:3062] scripts/test-not-included.sh
# Needs a local sequencer and LEZREG_REGISTRY/--network set as for any write.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
: "${ENTRY:?}" "${PAYER:?}" "${LEE_WALLET_HOME_DIR:?set it to a wallet COPY}"
LEZREG=${LEZREG:-$ROOT/target/release/lezreg}
PORT=${PORT:-3099}
LOG=$(mktemp)
UPSTREAM=${RPC:-http://127.0.0.1:3062} python3 "$ROOT/scripts/nullproxy.py" "$PORT" "$LOG" &
P=$!
trap 'kill $P 2>/dev/null; rm -f "$LOG"' EXIT
sleep 1
V="0.0.$(date +%s)"
START=$(date +%s)
OUT=$(LEZREG_TX_TIMEOUT_SECS=15 "$LEZREG" --rpc "http://127.0.0.1:$PORT" update --entry "$ENTRY" --version "$V" --payer "$PAYER" --json)
ELAPSED=$(( $(date +%s) - START ))
echo "$OUT"
python3 -c 'import json,sys; v=json.load(sys.stdin); assert v["confirmed_by_read_back"] is True and v["block"] is None and v["tx"], v' <<<"$OUT"
grep -q getTransaction "$LOG" || { echo "FAIL: the proxy saw no getTransaction"; exit 1; }
[ "$ELAPSED" -lt 120 ] || { echo "FAIL: took ${ELAPSED}s"; exit 1; }
echo "PASS: applied, confirmed by read-back after the ${ELAPSED}s deadline"
