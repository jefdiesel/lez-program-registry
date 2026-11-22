#!/usr/bin/env bash
# Prints LEZ's published local-test key (PRIVATE_KEY_PUB_ACC_A), read from LEZ's own source at
# the pinned commit, so this repo holds no private key. Local test chains only.
# Offline: LEZ_INITIAL_STATE_RS=<path to lez/testnet_initial_state/src/lib.rs at that commit>.
# Never echo the output: assign it, then pass it to the wallet.
set -euo pipefail
COMMIT=bafd07be0701398f9bc435f73ab05e9a7936c301 # v0.4.0-rc3; move it with LEZ_COMMIT in e2e.yml
WANT=a89a780d7847621eac3e8cf7ac358c7d3272be81b3a96681a7ef0022998aec55 # sha256 of the hex text
URL=https://raw.githubusercontent.com/logos-blockchain/logos-execution-zone/$COMMIT/lez/testnet_initial_state/src/lib.rs
if [ -n "${LEZ_INITIAL_STATE_RS:-}" ]; then
  src=$(cat "$LEZ_INITIAL_STATE_RS")
else
  src=$(curl -fsS --retry 3 --max-time 30 "$URL")
fi
WANT=$WANT python3 -I -c '
import hashlib, os, re, sys
m = re.search(r"const PRIVATE_KEY_PUB_ACC_A: *\[u8; *32\] *= *\[([^\]]*)\]", sys.stdin.read())
if not m:
    sys.exit("lez-test-key: PRIVATE_KEY_PUB_ACC_A is not in that source")
key = bytes(int(x, 0) for x in re.findall(r"0x[0-9a-fA-F]+|\d+", m.group(1))).hex()
if hashlib.sha256(key.encode()).hexdigest() != os.environ["WANT"]:
    sys.exit("lez-test-key: not the expected published test key")
print(key)
' <<<"$src"
