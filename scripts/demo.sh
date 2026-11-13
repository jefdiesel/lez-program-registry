#!/usr/bin/env bash
# The LP-0023 evaluator demo: one command from a clean clone, on Linux or macOS.
#
#   scripts/demo.sh                 # local: a fresh standalone LEZ sequencer + Logos Storage nodes
#   scripts/demo.sh --prepare-only  # only check prerequisites, build and download; run nothing
#   scripts/demo.sh --testnet --rpc <URL> --registry <ID> [--storage-api <URL>] [--program <ID>]
#
# Local mode starts its own chain (LEZ sequencer_service in standalone mode, built from the LEZ
# tag this repo pins) and three local Logos Storage nodes (the pinned release binary), then
# walks every Functionality, Usability and Reliability criterion of LP-0023 in the spec's order,
# printing a heading per criterion and PASS / FAIL / SKIP per step, and a summary table at the
# end. It exits nonzero if any step FAILs. SKIP means the step could not run here (for example
# the Docker rebuild with no Docker); it is never counted as a pass.
#
# --testnet runs the read-only walk (lookup, enumeration, search, IDL, SDK example, degradation,
# verify) against a public sequencer and a deployed registry. LEZ testnet 0.4 has not launched,
# so it needs --rpc and --registry (or LEZREG_TESTNET_RPC / LEZREG_TESTNET_REGISTRY) and stops
# at once without them.
#
# Nothing here needs a secret or a machine of ours. The payer is LEZ's own published local-test
# genesis account (the same one .github/workflows/e2e.yml uses); every account the demo makes is
# throwaway and lives under the output directory.
#
# Output: $DEMO_OUT (default demo-out/, gitignored): demo.log (this transcript), build.log,
# sequencer.log, storage logs, the throwaway wallets. A previous demo-out/ is moved aside.
#
# Optional environment:
#   SEQUENCER_BIN, SEQUENCER_CONFIG  a prebuilt standalone sequencer_service and its networks/local.json
#                                    (default: build it from the pinned LEZ tag, cached)
#   STORAGE_BIN                      a Logos Storage binary (default: download the pinned release)
#   LEZREG                           a prebuilt lezreg (default: cargo build --release -p lezreg)
#   LEZREG_DEMO_CACHE                cache for the sequencer build and Storage download
#                                    (default ${XDG_CACHE_HOME:-~/.cache}/lezreg-demo)
#   DEMO_RPC_PORT=3062 DEMO_METRICS_PORT=9162 DEMO_STORAGE_PORT=8090 DEMO_P2P_PORT=8070
#                                    (Storage nodes A, B, C use PORT, PORT+1, PORT+2)
#   DEMO_BUILD_WRAP                  a command prefix for the cargo builds (e.g. a lock or `nice`)
#   CARGO_TARGET_DIR, CARGO_BUILD_JOBS   as cargo reads them
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
E2E=$ROOT/.github/workflows/e2e.yml
MODE=local
VERIFY=auto
PREPARE_ONLY=0
KEEP=0
T_RPC=${LEZREG_TESTNET_RPC:-}
T_REG=${LEZREG_TESTNET_REGISTRY:-}
T_STORAGE=${LEZREG_TESTNET_STORAGE_API:-}
T_PROGRAM=""

usage() { sed -n '2,37p' "$0" | sed 's/^# \{0,1\}//'; cat <<'EOF'
Flags:
  --testnet              read-only walk against a public sequencer (needs --rpc and --registry)
  --rpc URL --registry ID [--storage-api URL] [--program ID]   (testnet mode)
  --verify               require the Docker rebuild (FAIL instead of SKIP without Docker)
  --no-verify            skip the Docker rebuild even if Docker is there
  --prepare-only         prerequisites, builds and downloads only
  --keep                 leave the local sequencer and Storage nodes running at the end
EOF
}
while [ $# -gt 0 ]; do
  case "$1" in
    --testnet) MODE=testnet; shift ;;
    --rpc) T_RPC=$2; shift 2 ;;
    --registry) T_REG=$2; shift 2 ;;
    --storage-api) T_STORAGE=$2; shift 2 ;;
    --program) T_PROGRAM=$2; shift 2 ;;
    --verify) VERIFY=on; shift ;;
    --no-verify) VERIFY=off; shift ;;
    --prepare-only) PREPARE_ONLY=1; shift ;;
    --keep) KEEP=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

if [ "$MODE" = testnet ] && { [ -z "$T_RPC" ] || [ -z "$T_REG" ]; }; then
  cat >&2 <<'EOF'
demo.sh --testnet needs the public sequencer and the deployed registry id:
  scripts/demo.sh --testnet --rpc <sequencer RPC URL> --registry <registry program id> [--storage-api <Storage REST API>]
  (or LEZREG_TESTNET_RPC / LEZREG_TESTNET_REGISTRY / LEZREG_TESTNET_STORAGE_API)
LEZ testnet 0.4 has not launched yet, so there is no public registry id to default to. Run the
local demo instead: scripts/demo.sh
EOF
  exit 2
fi

# ---- versions: read from where the repo pins them, never restated here ----------------------
yaml_env() { sed -n "s/^  $1: *\"\{0,1\}\([^\"# ]*\).*/\1/p" "$E2E" | head -1; }
LEZ_TAG=$(yaml_env LEZ_TAG)
LEZ_COMMIT=$(yaml_env LEZ_COMMIT)
CARGO_LEZ_TAG=$(sed -n 's/.*logos-execution-zone\.git", tag = "\([^"]*\)".*/\1/p' "$ROOT/Cargo.toml" | head -1)
STORAGE_ZIP_LINUX=$(yaml_env STORAGE_ZIP)
STORAGE_SHA_LINUX=$(yaml_env STORAGE_SHA256)
STORAGE_VERSION=$(printf '%s' "$STORAGE_ZIP_LINUX" | sed -n 's/.*-\(v[0-9][0-9.]*\)\.zip$/\1/p')
if [ -z "$LEZ_TAG" ] || [ -z "$LEZ_COMMIT" ] || [ -z "$STORAGE_VERSION" ]; then
  echo "cannot read LEZ_TAG / LEZ_COMMIT / STORAGE_ZIP from $E2E" >&2; exit 2
fi
if [ "$LEZ_TAG" != "$CARGO_LEZ_TAG" ]; then
  echo "the LEZ version is pinned twice and the pins disagree: e2e.yml LEZ_TAG=$LEZ_TAG, Cargo.toml tag=$CARGO_LEZ_TAG" >&2
  exit 2
fi

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) PLATFORM=darwin-arm64 ;;
  Linux-x86_64) PLATFORM=linux-amd64 ;;
  Linux-aarch64|Linux-arm64) PLATFORM=linux-arm64 ;;
  *) PLATFORM=unsupported ;;
esac
# Release asset digests of logos-storage-<platform>-v0.5.3.zip (GitHub's own sha256; linux-amd64
# is also pinned in e2e.yml, which wins).
storage_sha() {
  case "$1-$STORAGE_VERSION" in
    linux-amd64-*) printf '%s' "$STORAGE_SHA_LINUX" ;;
    darwin-arm64-v0.5.3) printf '7f32f62a14e555399ee003f11d1e29bba9b6ff842d973f38b5b280f018c2f6ab' ;;
    linux-arm64-v0.5.3) printf 'ecbc32536565096b75b2330dab4ff47a993714a1f3ce345099396968bc69796b' ;;
    *) printf '' ;;
  esac
}
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

# ---- output directory ----------------------------------------------------------------------
OUT=${DEMO_OUT:-$ROOT/demo-out}
if [ -e "$OUT" ]; then mv "$OUT" "$OUT.prev-$(date +%Y%m%d-%H%M%S)"; fi
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
LOG=$OUT/demo.log
BUILD_LOG=$OUT/build.log
CACHE=${LEZREG_DEMO_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/lezreg-demo}
T=${CARGO_TARGET_DIR:-$ROOT/target}
PAYER_KEY=10a26a9aec7d34b82364eeae45c5294dbb0a764b000b94eeb9b58511dc487c4d  # PRIVATE_KEY_PUB_ACC_A, lez/testnet_initial_state (public test key)
FUNDING_PK=0101010101010101010101010101010101010101010101010101010101010100
REPO_URL=https://github.com/jefdiesel/lez-program-registry
SAMPLE_BIN=$ROOT/artifacts/lez_program_registry.bin
REG_IDL=$ROOT/idl/lez_program_registry.idl.json
RPC_PORT=${DEMO_RPC_PORT:-3062}
METRICS_PORT=${DEMO_METRICS_PORT:-9162}
SP=${DEMO_STORAGE_PORT:-8090}
PP=${DEMO_P2P_PORT:-8070}
# Isolation: nothing the demo writes lands outside $OUT (and the build cache).
export LEE_WALLET_HOME_DIR=$OUT/wallet LEE_WALLET_PASSWORD=
export LEZREG_CONFIG_DIR=$OUT/config XDG_CACHE_HOME=$OUT/cache XDG_DATA_HOME=$OUT/data
unset LEZREG_NETWORK LEZREG_RPC LEZREG_REGISTRY LEZREG_STORAGE_API LEZREG_PIN_PEERS

# ---- results ---------------------------------------------------------------------------------
NR=0; NFAIL=0; NSKIP=0; NINFO=0
record() { # ID STATUS TEXT
  R_ID[$NR]=$1; R_ST[$NR]=$2; R_TXT[$NR]=$3; NR=$((NR + 1))
  [ "$2" = FAIL ] && NFAIL=$((NFAIL + 1))
  [ "$2" = SKIP ] && NSKIP=$((NSKIP + 1))
  [ "$2" = INFO ] && NINFO=$((NINFO + 1))
  printf '   ==> %-4s %s  %s\n' "$2" "$1" "$3"
}
criterion() { printf '\n\n######## %s\n' "$1"; [ $# -gt 1 ] && printf '# spec: %s\n' "$2"; return 0; }
note() { printf '   -- %s\n' "$*"; }
# Echo a lezreg command, then run it.
lz() { printf '$ lezreg'; printf ' %q' "$@"; printf '\n'; "$LEZREG" "$@"; }
# lzj FILE args...: echo the command, run `lezreg --json args` into FILE (stderr into FILE.err).
lzj() { local f=$1; shift; printf '$ lezreg --json'; printf ' %q' "$@"; printf '   > %s\n' "$(basename "$f")"; "$LEZREG" --json "$@" >"$f" 2>"$f.err" || { sed 's/^/   ! /' "$f.err" | tail -5; return 1; }; }
# ok ID TEXT cmd...: PASS when the command exits 0.
ok() { local id=$1 txt=$2 rc=0; shift 2; "$@" || rc=$?; if [ $rc -eq 0 ]; then record "$id" PASS "$txt"; else record "$id" FAIL "$txt (exit $rc)"; fi; }
# refused ID TEXT PATTERN lezreg-args...: PASS when lezreg FAILS and its output contains PATTERN.
refused() {
  local id=$1 txt=$2 want=$3 out rc=0; shift 3
  printf '$ lezreg'; printf ' %q' "$@"; printf '   # expected to be refused\n'
  out=$("$LEZREG" "$@" 2>&1) || rc=$?
  printf '%s\n' "$out" | sed 's/^/   | /'
  if [ $rc -eq 0 ]; then record "$id" FAIL "$txt: it was NOT refused"
  elif printf '%s' "$out" | grep -q -- "$want"; then record "$id" PASS "$txt (refused, exit $rc)"
  else record "$id" FAIL "$txt: refused, but without '$want' in the output"; fi
}
# Reads a dotted path from JSON on stdin ("" when missing).
jget() { python3 -c 'import json,sys
try: v=json.load(sys.stdin)
except Exception: print(""); sys.exit()
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) and k.isdigit() and int(k) < len(v) else (v.get(k) if isinstance(v, dict) else None)
print("" if v is None else (json.dumps(v) if isinstance(v,(dict,list)) else v))' "$1"; }
pyck() { python3 -c "$@"; }

summary() {
  printf '\n\n######## Summary (%s mode)\n' "$MODE"
  printf '%-6s %-5s %s\n' "step" "res" "what"
  local i=0
  while [ $i -lt $NR ]; do printf '%-6s %-5s %s\n' "${R_ID[$i]}" "${R_ST[$i]}" "${R_TXT[$i]}"; i=$((i + 1)); done
  printf '\n%d steps: %d PASS, %d FAIL, %d SKIP, %d INFO\n' "$NR" $((NR - NFAIL - NSKIP - NINFO)) "$NFAIL" "$NSKIP" "$NINFO"
  if [ "$NFAIL" -eq 0 ]; then echo "DEMO PASS"; else echo "DEMO FAIL"; fi
}

PIDS=""
cleanup() {
  if [ "$KEEP" = 1 ] && [ -n "$PIDS" ]; then echo "left running (--keep): pids $PIDS"; return; fi
  for p in $PIDS; do kill "$p" 2>/dev/null; done
}
fatal() { record "$1" FAIL "$2"; summary; exit 1; }

# ---- prerequisites ---------------------------------------------------------------------------
prereqs() {
  local miss=0 need_build=1 hint
  [ -n "${LEZREG:-}" ] && need_build=0
  echo "== prerequisites ($PLATFORM; LEZ $LEZ_TAG ($LEZ_COMMIT), Logos Storage $STORAGE_VERSION, from e2e.yml/Cargo.toml)"
  for c in git curl python3; do
    if command -v $c >/dev/null 2>&1; then echo "   ok   $c"; else echo "   MISSING $c (install it with your package manager)"; miss=1; fi
  done
  if command -v cargo >/dev/null 2>&1 && command -v rustup >/dev/null 2>&1; then echo "   ok   cargo/rustup ($(cargo --version 2>/dev/null))"
  else echo "   MISSING Rust: curl --proto '=https' -sSf https://sh.rustup.rs | sh   (rust-toolchain.toml selects $(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml"))"; miss=1; fi
  hint="curl -L https://risczero.com/install | bash && ~/.risc0/bin/rzup install cargo-risczero 3.0.5 && ~/.risc0/bin/rzup install rust"
  if [ $need_build = 1 ] || { [ "$MODE" = local ] && [ -z "${SEQUENCER_BIN:-}" ]; }; then
    if rustup toolchain list 2>/dev/null | grep -q '^risc0'; then echo "   ok   RISC Zero rust toolchain (builds the guests lezreg and LEZ embed)"
    else echo "   MISSING the RISC Zero toolchain: $hint"; miss=1; fi
  fi
  if [ "$MODE" = local ]; then
    if command -v r0vm >/dev/null 2>&1 || [ -x "${RISC0_SERVER_PATH:-}" ]; then echo "   ok   r0vm executor (every lezreg write is simulated with it first)"
    else echo "   MISSING r0vm (lezreg simulates every write with it): $hint ; then keep ~/.cargo/bin on PATH, or set RISC0_SERVER_PATH"; miss=1; fi
    if [ "$PLATFORM" != darwin-arm64 ] && [ "$(uname -s)" = Linux ]; then
      if pkg-config --exists libpcsclite 2>/dev/null; then echo "   ok   libpcsclite (the LEZ wallet's Keycard support)"
      else echo "   MISSING libpcsclite + pkg-config: sudo apt-get install -y libpcsclite-dev pkg-config"; miss=1; fi
    fi
    if [ -z "${STORAGE_BIN:-}" ]; then
      if [ -z "$(storage_sha "$PLATFORM")" ]; then echo "   MISSING Logos Storage: no pinned $STORAGE_VERSION release for $PLATFORM; set STORAGE_BIN=<logos-storage binary>"; miss=1
      elif ! command -v unzip >/dev/null 2>&1; then echo "   MISSING unzip (to unpack the Storage release)"; miss=1
      else echo "   ok   Logos Storage $STORAGE_VERSION release for $PLATFORM (to be downloaded and sha256-checked)"; fi
    else echo "   ok   STORAGE_BIN=$STORAGE_BIN"; fi
    local p
    for p in "$RPC_PORT" "$METRICS_PORT" "$SP" $((SP + 1)) $((SP + 2)) "$PP" $((PP + 1)) $((PP + 2)); do
      if ! python3 -c 'import socket,sys; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(("127.0.0.1", int(sys.argv[1])))' "$p" 2>/dev/null; then
        echo "   BUSY port 127.0.0.1:$p (set DEMO_RPC_PORT / DEMO_METRICS_PORT / DEMO_STORAGE_PORT / DEMO_P2P_PORT)"; miss=1
      fi
    done
  fi
  if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then HAVE_DOCKER=1; echo "   ok   docker (the source-verification rebuild)"
  else HAVE_DOCKER=0; echo "   --   no usable docker: the source-verification rebuild is SKIPPED$([ "$VERIFY" = on ] && echo ' and --verify makes that a FAIL')"; fi
  HAVE_NODE=0
  if command -v node >/dev/null 2>&1 && [ "$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null || echo 0)" -ge 19 ]; then HAVE_NODE=1; echo "   ok   node $(node --version) (the JS SDK step)"
  else echo "   --   no node >= 19: the JS SDK step is SKIPPED"; fi
  [ $miss = 0 ] || { echo "install what is missing above and run again"; return 1; }
}

# ---- builds and downloads (output to build.log) -----------------------------------------------
build() { # build TEXT cmd...
  local txt=$1 t0 rc=0; shift
  t0=$(date +%s)
  echo "   building: $txt (log: build.log)"
  { echo "### $txt: $*"; ${DEMO_BUILD_WRAP:-} "$@"; } >>"$BUILD_LOG" 2>&1 || rc=$?
  echo "   built in $(( $(date +%s) - t0 ))s (exit $rc)"
  return $rc
}

prepare() {
  echo "== builds"
  if [ -z "${LEZREG:-}" ]; then
    (cd "$ROOT" && build "lezreg (cargo build --release -p lezreg; a cold build takes 15-40 min)" cargo build --release -p lezreg) || return 1
    LEZREG=$T/release/lezreg
  fi
  (cd "$ROOT" && build "the SDK worked example (read-only SDK)" cargo build --release -p lezreg-sdk --no-default-features --example resolve_program) || return 1
  EXAMPLE=$T/release/examples/resolve_program
  [ "$MODE" = testnet ] && return 0

  if [ -n "${SEQUENCER_BIN:-}" ]; then
    SEQ=$SEQUENCER_BIN
    SEQ_CONFIG=${SEQUENCER_CONFIG:-}
    [ -n "$SEQ_CONFIG" ] || { echo "SEQUENCER_BIN needs SEQUENCER_CONFIG=<LEZ checkout>/lez/sequencer/service/networks/local.json"; return 1; }
    echo "   sequencer: SEQUENCER_BIN=$SEQ (prebuilt; its LEZ version is NOT checked by this script)"
  else
    local d=$CACHE/lez-seq-$LEZ_COMMIT src
    if [ ! -x "$d/sequencer_service" ]; then
      src=$CACHE/lez-src-$LEZ_COMMIT
      rm -rf "$src"; mkdir -p "$CACHE"
      echo "   cloning logos-execution-zone $LEZ_TAG"
      git clone --quiet --depth 1 --branch "$LEZ_TAG" https://github.com/logos-blockchain/logos-execution-zone.git "$src" >>"$BUILD_LOG" 2>&1 || { echo "clone failed (build.log)"; return 1; }
      [ "$(git -C "$src" rev-parse --short=8 HEAD)" = "$LEZ_COMMIT" ] || { echo "the tag $LEZ_TAG is not commit $LEZ_COMMIT any more: refusing"; return 1; }
      (cd "$src" && unset CARGO_TARGET_DIR && build "LEZ sequencer_service --features standalone at $LEZ_TAG (about 40 min cold on a 4-core machine; cached afterwards)" cargo build --release --features standalone -p sequencer_service) || return 1
      mkdir -p "$d"
      cp "$src/target/release/sequencer_service" "$d/" && cp "$src/lez/sequencer/service/networks/local.json" "$d/" || return 1
      rm -rf "$src"
    fi
    SEQ=$d/sequencer_service; SEQ_CONFIG=$d/local.json
    echo "   sequencer: $SEQ (built from $LEZ_TAG = $LEZ_COMMIT, cached)"
  fi

  if [ -n "${STORAGE_BIN:-}" ]; then
    STORAGE=$STORAGE_BIN
    echo "   storage: STORAGE_BIN=$STORAGE ($("$STORAGE" --version 2>/dev/null | head -1))"
  else
    local zip=logos-storage-$PLATFORM-$STORAGE_VERSION.zip sd=$CACHE/storage-$STORAGE_VERSION-$PLATFORM
    if [ ! -x "$sd/logos-storage" ]; then
      mkdir -p "$sd"
      curl -fsSL -o "$sd/$zip" "https://github.com/logos-storage/logos-storage-nim/releases/download/$STORAGE_VERSION/$zip" || { echo "Storage download failed"; return 1; }
      [ "$(sha256 "$sd/$zip")" = "$(storage_sha "$PLATFORM")" ] || { echo "Storage zip sha256 mismatch: refusing"; rm -f "$sd/$zip"; return 1; }
      (cd "$sd" && unzip -q -o "$zip" && mv "logos-storage-$PLATFORM-$STORAGE_VERSION" logos-storage && chmod +x logos-storage && rm -f "$zip") || return 1
    fi
    STORAGE=$sd/logos-storage
    echo "   storage: $STORAGE (release $STORAGE_VERSION, sha256-checked zip)"
  fi
}

# ---- local chain and Storage nodes ---------------------------------------------------------------
RPC=http://127.0.0.1:$RPC_PORT
API_A=http://127.0.0.1:$SP/api/storage/v1
API_B=http://127.0.0.1:$((SP + 1))/api/storage/v1
API_C=http://127.0.0.1:$((SP + 2))/api/storage/v1
API_DEAD=http://127.0.0.1:9/api/storage/v1

start_sequencer() {
  echo "== a fresh standalone sequencer on $RPC"
  "$SEQ" setup "$OUT/seq" --config "$SEQ_CONFIG" --funding-public-key "$FUNDING_PK" --create-channel >"$OUT/sequencer-setup.log" 2>&1 || return 1
  RUST_LOG=warn nohup "$SEQ" start "$OUT/seq" --rpc-listen-address "127.0.0.1:$RPC_PORT" \
    --metrics-listen-address "127.0.0.1:$METRICS_PORT" >"$OUT/sequencer.log" 2>&1 &
  PIDS="$PIDS $!"
  local i
  for i in $(seq 1 90); do
    curl -sf -X POST "$RPC" -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getLastBlockId","params":[]}' >/dev/null 2>&1 && { echo "   sequencer up"; return 0; }
    sleep 2
  done
  echo "   the sequencer did not answer (sequencer.log):"; tail -20 "$OUT/sequencer.log"; return 1
}

storage_pid() { cat "$OUT/st-$1.pid" 2>/dev/null; }
start_storage() { # name api-port p2p-port [bootstrap spr]
  mkdir -p "$OUT/st-$1" && chmod 700 "$OUT/st-$1"
  local args=(--data-dir="$OUT/st-$1" --api-port="$2" --listen-ip=127.0.0.1 --listen-port="$3" --nat=extip:127.0.0.1 --block-ttl=86400)
  if [ -n "${4:-}" ]; then args+=(--bootstrap-node="$4"); else args+=(--no-bootstrap-node); fi
  nohup "$STORAGE" "${args[@]}" >"$OUT/st-$1.log" 2>&1 &
  echo $! >"$OUT/st-$1.pid"; PIDS="$PIDS $!"
  local i
  for i in $(seq 1 60); do curl -sf "127.0.0.1:$2/api/storage/v1/debug/info" >/dev/null 2>&1 && { echo "   Storage node $1 up on 127.0.0.1:$2"; return 0; }; sleep 1; done
  echo "   Storage node $1 did not start:"; tail -20 "$OUT/st-$1.log"; return 1
}
spr() { curl -s "127.0.0.1:$1/api/storage/v1/spr" | python3 -c 'import sys,json;print(json.load(sys.stdin)["spr"])'; }

# Prints "<id> <pk>" for a new public account in the current wallet (output kept out of the log).
new_account() {
  "$LEZREG" wallet account new public >"$OUT/new-account.txt" 2>&1
  python3 -c 'import sys,re; t=open(sys.argv[1]).read(); print(re.search(r"Public/(\w+)",t).group(1), re.search(r"With pk (\w+)",t).group(1))' "$OUT/new-account.txt"
}
cycles_of() { sed -n 's/.*simulated \([0-9][0-9]*\) cycles.*/\1/p' | head -1; }

# ======================================================================================================
local_demo() {
  local COMMIT REG X X3 A B Q M M_PK OUTX IDL_SHA TMP i
  COMMIT=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo 0000000000000000000000000000000000000000)
  start_sequencer || fatal S0 "start the local sequencer"
  echo "== three Logos Storage nodes (A, B bootstrapped from A; C is started later)"
  start_storage a "$SP" "$PP" || fatal S0 "start Storage node A"
  start_storage b $((SP + 1)) $((PP + 1)) "$(spr "$SP")" || fatal S0 "start Storage node B"

  "$LEZREG" network set demo --rpc "$RPC" >/dev/null || fatal S0 "lezreg network set demo"
  export LEZREG_NETWORK=demo LEZREG_STORAGE_API=$API_A
  echo "== wallet: LEZ's published local-test genesis account pays (wallet setup output in wallet-setup.txt)"
  {
    "$LEZREG" wallet config set sequencer_addr "$RPC" || true
    "$LEZREG" wallet account list
    "$LEZREG" wallet account import public --private-key "$PAYER_KEY"
  } >"$OUT/wallet-setup.txt" 2>&1
  PAYER=$(sed -n 's|.*Public/\([1-9A-HJ-NP-Za-km-z]*\).*|\1|p' "$OUT/wallet-setup.txt" | tail -1)
  [ -n "$PAYER" ] || fatal S0 "import the genesis payer account (wallet-setup.txt)"
  echo "   payer $PAYER"
  read -r A _ < <(new_account); read -r Q _ < <(new_account); read -r B _ < <(new_account)
  echo "   accounts in the demo wallet: author A=$A, community publisher Q=$Q, new author B=$B"
  mkdir -p "$OUT/mallory"
  read -r M M_PK < <(LEE_WALLET_HOME_DIR=$OUT/mallory new_account)
  echo "   Mallory M=$M, in her OWN wallet ($OUT/mallory): she holds none of the keys above"

  # ------------------------------------------------------------------------------------------
  criterion "F1  On-chain registry" "entry metadata stored on-chain as a LEZ program built with SPEL, with a published IDL; source and IDL off-chain on Logos Storage; on-chain only metadata and pointers"
  note "chain/deploy.sh deploys the committed reproducible build artifacts/lez_program_registry.bin, uploads"
  note "its SPEL-generated IDL (idl/) to Storage node A, and registers the registry in itself at (REG, REG)."
  printf '$ chain/deploy.sh --elf artifacts/lez_program_registry.bin --payer %s --rpc %s --network demo --commit %s --idl-cid push\n' "$PAYER" "$RPC" "$COMMIT"
  STATE=$OUT/deploy-demo.env LEZREG=$LEZREG "$ROOT/chain/deploy.sh" --elf "$SAMPLE_BIN" --payer "$PAYER" \
    --rpc "$RPC" --network demo --commit "$COMMIT" --idl-cid push >"$OUT/deploy.txt" 2>&1
  local rc=$?
  sed 's/^/   | /' "$OUT/deploy.txt"
  REG=$(sed -n 's/^REG=//p' "$OUT/deploy.txt" | tail -1)
  if [ $rc -ne 0 ] || [ -z "$REG" ]; then fatal F1.a "deploy the registry and register it in itself"; fi
  record F1.a PASS "registry deployed at REG=$REG and registered in itself"
  lz show "$REG"
  "$LEZREG" --json show "$REG" >"$OUT/reg.json"
  ok F1.b "live program image = artifacts/lez_program_registry.imageid (the CI-reproduced build)" \
    pyck 'import json,sys; s=json.load(open(sys.argv[1])); w=open(sys.argv[2]).read().strip(); print("   live", s["header"]["image_id"], "\n   artifacts", w); sys.exit(s["header"]["image_id"]!=w)' "$OUT/reg.json" "$ROOT/artifacts/lez_program_registry.imageid"
  ok F1.c "(REG, REG) holds a deployer entry named lez_program_registry whose IDL pointer is a Storage CID" \
    pyck 'import json,sys; e=json.load(open(sys.argv[1]))["deployer_entry"]; print("   idl", e["idl"]); sys.exit(not(e["kind"]=="deployer" and e["name"]=="lez_program_registry" and e["idl"]["cid"].startswith("z")))' "$OUT/reg.json"
  lzj "$OUT/reg-idl.json" idl get "$REG" --storage-api "$API_A"
  ok F1.d "the IDL on Storage, fetched by the pointer and hash-checked, is the committed SPEL IDL ($(basename "$REG_IDL"))" \
    pyck 'import json,sys; d=json.load(open(sys.argv[1])); c=json.load(open(sys.argv[2])); print("   %d instructions, generator %s" % (len(d["idl"]["instructions"]), d["idl"].get("metadata"))); sys.exit(d["idl"]!=c)' "$OUT/reg-idl.json" "$REG_IDL"
  note "the entry stores only pointers (CID + sha256 + size) for the IDL and the source; the IDL itself is $(wc -c <"$REG_IDL" | tr -d ' ') bytes on Storage."
  note "SPEL: the program is written in SPEL attribute syntax (program/src/lib.rs) and compiled for LEZ 0.4 by the"
  note "in-repo spel04-shim; idl/ is generated by the released spel v0.7.0 'generate-idl' (ci.yml job 'idl' checks it)."

  # ------------------------------------------------------------------------------------------
  criterion "F2  Register" "any account may register a program it deployed: name, version, author, description, IDL pointer, code pointer, registration timestamp; lookup without a centralised indexer"
  note "deploy the sample program X (the registry guest at a new header account) and register it in the SAME"
  note "transaction (DeployAndRegister); author A co-signs, the genesis account pays, the IDL goes to Storage."
  printf '$ lezreg deploy artifacts/lez_program_registry.bin --author %s --payer %s --name demo-sample --version 1.0.0 ... --idl-cid push --json\n' "$A" "$PAYER"
  OUTX=$("$LEZREG" --json deploy "$SAMPLE_BIN" --author "$A" --payer "$PAYER" --name demo-sample --version 1.0.0 \
    --label "Demo Labs" --description "sample program for the LP-0023 demo (the registry guest deployed again)" \
    --tag sample --tag demo --idl-cid push --idl-file "$REG_IDL" \
    --repo "$REPO_URL" --commit "$COMMIT" --manifest-path methods/guest/Cargo.toml --binary lez_program_registry \
    --lock-file "$ROOT/Cargo.lock" 2>"$OUT/deploy-x.err")
  printf '%s\n' "$OUTX" | python3 -m json.tool 2>/dev/null | sed 's/^/   | /' | head -30
  X=$(printf '%s' "$OUTX" | jget program)
  [ -n "$X" ] || { cat "$OUT/deploy-x.err"; fatal F2.a "deploy and register the sample program X"; }
  record F2.a PASS "X=$X deployed and registered in one transaction (DeployAndRegister)"
  lz show "$X"
  "$LEZREG" --json show "$X" >"$OUT/x.json"
  ok F2.b "X's entry carries name, version, author, description, IDL pointer, code pointer (repo+commit+build), registration time" \
    pyck 'import json,sys,time; e=json.load(open(sys.argv[1]))["deployer_entry"]; a=sys.argv[2]
f={"name":e["name"],"version":e["version"],"author":e["author"],"description":e["description"],"idl.cid":e["idl"]["cid"],"source.repo":e["source"]["repo"],"source.commit":e["source"]["commit"],"build.binary":e["build"]["binary"],"registered_at":e["registered_at"]}
[print("   %-14s %s" % (k, v)) for k, v in f.items()]
print("   registered    %s UTC" % time.strftime("%Y-%m-%d %H:%M:%S", time.gmtime(e["registered_at"]/1000)))
sys.exit(not(all(f.values()) and e["author"]==a and e["born_with_deploy"]))' "$OUT/x.json" "$A"
  note "every read above is plain sequencer RPC (getAccount): no indexer, no server; 'show' resolves X with one read."
  REG_CYCLES=""

  # ------------------------------------------------------------------------------------------
  criterion "F3  Deployer vs third-party entries" "deployer entry signed by the account that deployed the program; clients tell it apart without an off-chain service; the demo shows an unauthorised attempt to create a deployer entry being rejected"
  note "Q publishes a community (third-party) entry for X"
  OUTX=$(lz register --third-party --program "$X" --publisher "$Q" --payer "$PAYER" --name demo-sample --version 1.0.0 \
    --label "a community packager" --description "community metadata for X" --tag sample \
    --idl-cid push --idl-file "$REG_IDL" --repo "$REPO_URL" --commit "$COMMIT" \
    --manifest-path methods/guest/Cargo.toml --binary lez_program_registry --lock-file "$ROOT/Cargo.lock" 2>&1)
  rc=$?; printf '%s\n' "$OUTX"; REG_CYCLES=$(printf '%s' "$OUTX" | cycles_of)
  [ $rc -eq 0 ] && record F3.a PASS "Q's third-party entry for X registered" || record F3.a FAIL "Q's third-party entry (exit $rc)"
  lz show "$X" --publisher "$Q"
  "$LEZREG" --json show "$X" --publisher "$Q" >"$OUT/x-q.json"
  ok F3.b "a client tells them apart from chain data alone: the deployer entry lives AT X (only X's key can write there); Q's lives at T(X, Q)" \
    pyck 'import json,sys; d=json.load(open(sys.argv[1])); t=json.load(open(sys.argv[2])); x,q=sys.argv[3],sys.argv[4]
print("   deployer entry: address", d["program"], "kind", d["deployer_entry"]["kind"]); print("   community entry: address", t["address"], "kind", t["kind"])
sys.exit(not(d["program"]==x and d["deployer_entry"]["kind"]=="deployer" and t["kind"]=={"third_party":{"publisher":q}} and t["address"]!=x))' "$OUT/x.json" "$OUT/x-q.json" "$X" "$Q"

  note "Mallory gets funds, then tries to create the deployer entry of a program she did not deploy."
  lz wallet auth-transfer send --from "Public/$PAYER" --to-pk "$M_PK" --amount 20000000 2>&1 | grep -E "^Transaction (hash|is included)" | sed 's/^/   | /'
  printf '$ lezreg deploy artifacts/lez_program_registry.bin --no-register --payer %s --json   # X3: deployed, not registered\n' "$PAYER"
  X3=$("$LEZREG" --json deploy "$SAMPLE_BIN" --no-register --payer "$PAYER" 2>"$OUT/deploy-x3.err" | jget program)
  [ -n "$X3" ] || { cat "$OUT/deploy-x3.err"; fatal F3.c "deploy target program X3"; }
  note "X3=$X3 (its header key is in the demo wallet, not Mallory's)"
  LEE_WALLET_HOME_DIR=$OUT/mallory refused F3.c "UNAUTHORISED: Mallory's RegisterDeployer for X3 is refused before sending (simulation)" "must sign" \
    register --program "$X3" --author "$M" --payer "$M" --name demo-sample --version 6.6.6 --label "totally the deployer" \
    --idl-cid raw --idl-file "$REG_IDL" --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock"
  LEE_WALLET_HOME_DIR=$OUT/mallory refused F3.d "UNAUTHORISED, forced on chain (--force-send): the registry program reverts it; Mallory pays her gas limit" "REVERTED" \
    register --program "$X3" --author "$M" --payer "$M" --name demo-sample --version 6.6.6 --label "totally the deployer" \
    --idl-cid raw --idl-file "$REG_IDL" --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock" \
    --force-send --gas-limit 400000
  ok F3.e "after both attempts (X3, REG) is still empty" \
    sh -c '"$1" --json show "$2" | python3 -c "import json,sys; d=json.load(sys.stdin); print(\"   deployer_entry:\", d[\"deployer_entry\"]); sys.exit(d[\"deployer_entry\"] is not None)"' _ "$LEZREG" "$X3"
  note "the rightful header key of X3 registers it (RegisterDeployer, signed by X3's key)"
  ok F3.f "X3's own key registers its deployer entry" \
    lz register --program "$X3" --payer "$PAYER" --name demo-sample-3 --version 1.0.0 --description "X3, registered by its own header key" \
      --idl-cid push --idl-file "$REG_IDL" --repo "$REPO_URL" --commit "$COMMIT" --manifest-path methods/guest/Cargo.toml \
      --binary lez_program_registry --lock-file "$ROOT/Cargo.lock"
  refused F3.g "a SECOND deployer entry for X, even signed by X's own key, is refused by lezreg" "already has a deployer entry" \
    register --program "$X" --payer "$PAYER" --name demo-sample --version 2.0.0 --idl-cid raw --idl-file "$REG_IDL" \
    --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock"
  refused F3.h "... and, forced, by the chain (the registry's init check on (X, REG))" "REVERTED" \
    register --program "$X" --payer "$PAYER" --name demo-sample --version 2.0.0 --idl-cid raw --idl-file "$REG_IDL" \
    --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock" \
    --force-send --gas-limit 400000
  ok F3.i "X's deployer entry is unchanged (version 1.0.0, author A)" \
    sh -c '"$1" --json show "$2" | python3 -c "import json,sys; e=json.load(sys.stdin)[\"deployer_entry\"]; print(\"   version\", e[\"version\"], \"author\", e[\"author\"]); sys.exit(not(e[\"version\"]==\"1.0.0\" and e[\"author\"]==sys.argv[1]))" "$3"' _ "$LEZREG" "$X" "$A"

  # ------------------------------------------------------------------------------------------
  criterion "F4  Update" "only the original registering author may update an entry (version, IDL pointer, description); authorship transfer, if implemented, needs both signatures"
  OUTX=$(lz update "$X" --version 1.1.0 --description "updated by its author A" --payer "$PAYER" 2>&1); rc=$?
  printf '%s\n' "$OUTX"; UPD_CYCLES=$(printf '%s' "$OUTX" | cycles_of)
  [ $rc -eq 0 ] && record F4.a PASS "author A updates X to 1.1.0 (revision 2, a write-once release snapshot is kept)" || record F4.a FAIL "author A updates X (exit $rc)"
  LEE_WALLET_HOME_DIR=$OUT/mallory refused F4.b "a non-author (Mallory) cannot update X: refused in simulation" "only the entry's author" \
    update "$X" --author "$M" --payer "$M" --version 6.6.6
  LEE_WALLET_HOME_DIR=$OUT/mallory refused F4.c "... and, forced on chain, reverted" "REVERTED" \
    update "$X" --author "$M" --payer "$M" --version 6.6.6 --force-send --gas-limit 200000
  ok F4.d "transfer X's entry from A to B: A and B both sign the one transaction" lz transfer "$X" --to "$B" --payer "$PAYER"
  refused F4.e "the previous author A can no longer update" "only the entry's author" update "$X" --author "$A" --payer "$PAYER" --version 9.9.9
  ok F4.f "the new author B updates X (1.2.0)" lz update "$X" --version 1.2.0 --payer "$PAYER"
  ok F4.g "X's entry: author B, registrant A, version 1.2.0" \
    sh -c '"$1" --json show "$2" | python3 -c "import json,sys; e=json.load(sys.stdin)[\"deployer_entry\"]; print(\"   author\", e[\"author\"], \"registrant\", e[\"registrant\"], \"version\", e[\"version\"], \"revision\", e[\"revision\"]); sys.exit(not(e[\"author\"]==sys.argv[1] and e[\"registrant\"]==sys.argv[2] and e[\"version\"]==\"1.2.0\"))" "$3" "$4"' _ "$LEZREG" "$X" "$B" "$A"
  note "a transfer missing the new author's signature cannot be built with the CLI (it signs with both keys);"
  note "the program's refusal of it is pinned by engine test 5 (tools/engine-test, run by ci.yml)."

  # ------------------------------------------------------------------------------------------
  criterion "F5  IDL publishing" "the IDL is stored on Logos Storage"
  XDG_CACHE_HOME=$OUT/cache-b lzj "$OUT/idl-b.json" idl get "$REG" --storage-api "$API_B"
  ok F5.a "REG's IDL fetched through node B (uploaded to A), hash-checked against the chain" \
    pyck 'import json,sys; d=json.load(open(sys.argv[1])); print("   cid", d["cid"], "name", d["idl"]["name"]); sys.exit(d["idl"]["name"]!="lez_program_registry")' "$OUT/idl-b.json"
  ok F5.b "pin REG's IDL on node B (fetch + announce: B becomes a provider)" lz pin "$REG" --storage-api "$API_B"
  kill "$(storage_pid a)" 2>/dev/null; note "node A (the uploader) stopped"
  if start_storage c $((SP + 2)) $((PP + 2)) "$(spr $((SP + 1)))"; then
    XDG_CACHE_HOME=$OUT/cache-c lzj "$OUT/idl-c.json" idl get "$REG" --storage-api "$API_C"
    ok F5.c "a fresh node C, with the uploader gone, still resolves REG's IDL (from B)" \
      pyck 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(d["idl"]["name"]!="lez_program_registry")' "$OUT/idl-c.json"
  else record F5.c FAIL "start Storage node C"; fi
  export LEZREG_STORAGE_API=$API_B

  # ------------------------------------------------------------------------------------------
  criterion "F6  Discovery" "look up an entry by program ID, enumerate the full set of entries, search by name and author, using only the submitted software"
  ok F6.a "lookup by program id (one getAccount)" lz show "$X3"
  note "enumerate every entry, two per page, following the --after cursor"
  pyck 'import json,subprocess,sys
lz=sys.argv[1]; after=None; seen=[]
while True:
    cmd=[lz,"--json","list","--limit","2"]+(["--after",after] if after else [])
    print("$ lezreg list --limit 2" + (" --after "+after if after else ""))
    d=json.loads(subprocess.run(cmd,capture_output=True,text=True,check=True).stdout)
    for i in d["items"]:
        e=i["entry"] or {}; k=i["kind"]; k=k if isinstance(k,str) else "third-party"
        print("   %-46s %-12s %s %s" % (i["address"], k, e.get("name"), e.get("version"))); seen.append(i["address"])
    after=d["next"]
    if not after: break
print("   total %d, walked %d" % (d["total"], len(seen)))
open(sys.argv[2],"w").write(json.dumps({"total":d["total"],"seen":seen}))' "$LEZREG" "$OUT/walk.json"
  ok F6.b "enumeration walked all 4 entries (REG, X, Q's entry for X, X3), no duplicates" \
    pyck 'import json,sys; d=json.load(open(sys.argv[1])); s=d["seen"]; sys.exit(not(d["total"]==4 and len(s)==4 and len(set(s))==4 and sys.argv[2] in s and sys.argv[3] in s and sys.argv[4] in s))' "$OUT/walk.json" "$REG" "$X" "$X3"
  lz search name demo-sample; "$LEZREG" --json search name demo-sample >"$OUT/s-name.json"
  ok F6.c "search by name 'demo-sample': exact matches (X's deployer entry and Q's), then substring matches (demo-sample-3)" \
    pyck 'import json,sys; d=json.load(open(sys.argv[1])); n=[(i["entry"]["name"], i["entry"]["kind"]) for i in d["items"]]; print("  ", n); sys.exit(not(n[:2] and set(n[:2])=={("demo-sample","deployer"),("demo-sample","third-party")} and ("demo-sample-3","deployer") in n))' "$OUT/s-name.json"
  lz search author "$B"; "$LEZREG" --json search author "$B" >"$OUT/s-b.json"
  "$LEZREG" --json search author "$Q" >"$OUT/s-q.json"; "$LEZREG" --json search author "$A" >"$OUT/s-a.json"
  ok F6.d "search by proven author: B holds X (after the transfer), Q holds its community entry, A holds nothing now" \
    pyck 'import json,sys; c=lambda f:[i["entry"]["program"] for i in json.load(open(f))["items"]]; b,q,a=c(sys.argv[1]),c(sys.argv[2]),c(sys.argv[3]); print("   B:",b," Q:",q," A:",a); x=sys.argv[4]; sys.exit(not(b==[x] and q==[x] and a==[]))' "$OUT/s-b.json" "$OUT/s-q.json" "$OUT/s-a.json" "$X"

  # ------------------------------------------------------------------------------------------
  criterion "F7  Source verification" "an entry records a source pointer, a commit and a build configuration sufficient to reproduce the bytecode; a user may rebuild locally and, on a match, mark the program locally verified"
  ok F7.a "REG's entry records repo, commit and the full build tuple (builder image digest, risc0-build, manifest, binary, Cargo.lock sha256)" \
    pyck 'import json,sys; e=json.load(open(sys.argv[1]))["deployer_entry"]; s,b=e["source"],e["build"]
for k in ("repo","commit"): print("   source.%-12s %s" % (k, s[k]))
for k in ("image_tag","image_digest","risc0_build","host_risc0_rust","manifest_path","binary","lock_sha256","build_hash"): print("   build.%-13s %s" % (k, b[k]))
sys.exit(not(s["repo"] and len(s["commit"])==40 and b["image_digest"].strip("0") and b["lock_sha256"].strip("0") and b["binary"]))' "$OUT/reg.json"
  if [ "$HAVE_DOCKER" = 1 ] && [ "$VERIFY" != off ]; then
    note "rebuilding REG's recorded source at the recorded commit in Docker (compile with the network off); this takes a while"
    lzj "$OUT/verify.json" verify "$REG" --mark --work "$OUT/verify"; tail -5 "$OUT/verify.json.err" | sed 's/^/   | /'
    ok F7.b "lezreg verify REG: rebuilt image = claimed = live (Match), marked locally verified" \
      pyck 'import json,sys; d=json.load(open(sys.argv[1])); print("   verdict", d.get("verdict"), d.get("rebuilt")); sys.exit(d.get("verdict")!="Match")' "$OUT/verify.json"
    ok F7.c "lezreg verified lists the local mark" lz verified
  else
    local why="no usable Docker on this machine"; [ "$VERIFY" = off ] && why="--no-verify"
    if [ "$VERIFY" = on ]; then record F7.b FAIL "lezreg verify REG (Docker rebuild): $why, and --verify requires it"
    else record F7.b SKIP "lezreg verify REG (Docker rebuild): $why. NOT rebuilt here"; fi
    note "What was NOT done here: no rebuild of the source. What CI does on every push to main: repro.yml rebuilds the guest"
    note "in Docker on two runners (byte-identical, = artifacts/), verify.yml reproduces it with lezreg's own Dockerfile,"
    note "and e2e.yml runs 'lezreg verify REG --mark' against a fresh chain and asserts Match."
    ok F7.c "(not a rebuild) the live images of REG and X equal artifacts/*.imageid, which those CI rebuilds reproduce" \
      sh -c 'w=$(cat "$4"); for p in "$2" "$3"; do i=$("$1" --json show "$p" | python3 -c "import json,sys;print(json.load(sys.stdin)[\"header\"][\"image_id\"])"); echo "   $p live $i"; [ "$i" = "$w" ] || exit 1; done' _ "$LEZREG" "$REG" "$X" "$ROOT/artifacts/lez_program_registry.imageid"
  fi

  criterion "F8  Testnet 0.4" "the registry operates end-to-end against the Logos LEZ zone on testnet 0.4"
  record F8 SKIP "LEZ testnet 0.4 has not launched; this run is a local $LEZ_TAG chain. On launch: scripts/demo.sh --testnet --rpc ... --registry ..."

  # ------------------------------------------------------------------------------------------
  criterion "U1  Module/SDK" "a module/SDK usable by other Logos modules and third-party apps to register entries, resolve a program ID to its metadata and IDL, and run searches"
  note "docs/sdk.md's worked example (sdk/rust/examples/resolve_program.rs), run as written against X:"
  printf '$ resolve_program %s %s %s %s\n' "$RPC" "$REG" "$X" "$API_B"
  XDG_CACHE_HOME=$OUT/cache-u1 "$EXAMPLE" "$RPC" "$REG" "$X" "$API_B" >"$OUT/u1.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/u1.txt"
  ok U1.a "Rust SDK: program id -> name, version, author, freshness, IDL" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by $2" "$3" && grep -q "fresh: the program runs" "$3" && grep -qE "IDL: [0-9]+ instructions" "$3"' _ $rc "$B" "$OUT/u1.txt"
  if [ "$HAVE_NODE" = 1 ]; then
    cat >"$OUT/u1.mjs" <<EOF
import { Registry, fetchIdl } from "$ROOT/sdk/js/lezreg.js";
const reg = new Registry({ rpc: "$RPC", registry: "$REG" });
const { deployer, freshness } = await reg.resolve("$X");
const { status, idl } = await fetchIdl(deployer, { storageApi: "$API_B" });
console.log(deployer.name, deployer.version, freshness, status, idl ? idl.instructions.length + " instructions" : "");
if (deployer.name !== "demo-sample" || status !== "ok") process.exit(1);
EOF
    printf '$ node %s\n' "u1.mjs (sdk/js/lezreg.js: Registry.resolve + fetchIdl)"
    ok U1.b "JS SDK (sdk/js/lezreg.js): the same resolve and IDL fetch" node "$OUT/u1.mjs"
  else record U1.b SKIP "JS SDK step: no node >= 19 here (CI runs sdk/js/lezreg.test.mjs)"; fi
  note "writes through the SDK: every lezreg write above goes through lezreg_sdk::tx::Writer (sdk/rust/src/tx.rs)."
  note "the Basecamp core module program_registry (module/) exposes resolve_program, program_label, get_idl, list and"
  note "search; it is not loaded by this script (it needs Basecamp/logosctl), and it has no write methods."

  criterion "U2  CLI" "a CLI covering register, update, lookup, search and source verification"
  local u2=PASS id
  for id in F2.a F3.a F3.f F4.a F4.f F6.a F6.b F6.c F6.d; do
    i=0; while [ $i -lt $NR ]; do [ "${R_ID[$i]}" = "$id" ] && [ "${R_ST[$i]}" != PASS ] && u2=FAIL; i=$((i + 1)); done
  done
  record U2 "$u2" "lezreg deploy/register/update/transfer/show/list/search ran above (F2-F6); verify: see F7.b"

  # ------------------------------------------------------------------------------------------
  criterion "R1  Graceful degradation" "when an IDL identifier is unresolvable or the stored IDL fails to parse, the entry's on-chain metadata still displays, with the IDL surfaced as unavailable"
  note "(a) no Storage node reachable (the SDK example pointed at a dead port, empty cache)"
  printf '$ resolve_program %s %s %s %s\n' "$RPC" "$REG" "$X" "$API_DEAD"
  XDG_CACHE_HOME=$OUT/cache-r1a "$EXAMPLE" "$RPC" "$REG" "$X" "$API_DEAD" >"$OUT/r1a.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/r1a.txt"
  ok R1.a "no Storage node: metadata shown, 'IDL unavailable', exit 0" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by" "$2" && grep -q "IDL unavailable (unavailable: no Storage node" "$2"' _ $rc "$OUT/r1a.txt"

  note "(b) an IDL identifier nobody can resolve: B points X's IDL at a CID of bytes that were never uploaded"
  TMP=$OUT/never-uploaded.json; printf '{"never":"uploaded","at":"%s"}' "$(date +%s)" >"$TMP"
  ok R1.b0 "B updates X's IDL pointer to the unresolvable CID" lz update "$X" --idl-cid raw --idl-file "$TMP" --payer "$PAYER"
  printf '$ resolve_program %s %s %s %s\n' "$RPC" "$REG" "$X" "$API_B"
  XDG_CACHE_HOME=$OUT/cache-r1b "$EXAMPLE" "$RPC" "$REG" "$X" "$API_B" >"$OUT/r1b.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/r1b.txt"
  ok R1.b "unresolvable CID: metadata shown, 'IDL unavailable', exit 0" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by" "$2" && grep -q "IDL unavailable" "$2"' _ $rc "$OUT/r1b.txt"
  ok R1.b2 "the CLI's entry view (lezreg show) still displays X's metadata" lz show "$X"
  refused R1.b3 "lezreg idl get X reports the IDL as unavailable (and exits nonzero) instead of hanging or crashing" "IDL unavailable" \
    idl get "$X" --storage-api "$API_B" --tries 1

  # Uploads a file to node B's REST API and points X's IDL at it (CID, sha256 and size of the bytes).
  idl_to() { # ID TEXT FILE
    local cid
    printf '$ curl -X POST %s/data --data-binary @%s\n' "$API_B" "$(basename "$3")"
    cid=$(curl -s -X POST -H 'Content-Type: application/json' -H "Content-Disposition: attachment; filename=\"$(basename "$3")\"" --data-binary @"$3" "$API_B/data")
    note "cid $cid"
    ok "$1" "$2" lz update "$X" --idl-cid "$cid" --idl-file "$3" --payer "$PAYER"
  }
  note "(c) stored bytes that do not parse at all: not JSON"
  TMP=$OUT/not-json.idl; printf 'this is not JSON {' >"$TMP"
  idl_to R1.c0 "B points X's IDL at those bytes, uploaded to node B" "$TMP"
  printf '$ resolve_program %s %s %s %s\n' "$RPC" "$REG" "$X" "$API_B"
  XDG_CACHE_HOME=$OUT/cache-r1c "$EXAMPLE" "$RPC" "$REG" "$X" "$API_B" >"$OUT/r1c.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/r1c.txt"
  ok R1.c "unparseable IDL: metadata shown, 'IDL unavailable (not a usable IDL ...)', exit 0" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by" "$2" && grep -q "IDL unavailable (not a usable IDL" "$2"' _ $rc "$OUT/r1c.txt"

  note "(c2) valid JSON in no IDL format this client knows (not SPEL v1 or v2)"
  TMP=$OUT/unknown-format.json; printf '{"hello":"this is not an IDL"}' >"$TMP"
  idl_to R1.c2a "B points X's IDL at it, uploaded to node B" "$TMP"
  XDG_CACHE_HOME=$OUT/cache-r1c2 "$EXAMPLE" "$RPC" "$REG" "$X" "$API_B" >"$OUT/r1c2.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/r1c2.txt"
  ok R1.c2 "unknown IDL format: metadata shown, IDL 'available, format not recognised', exit 0" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by" "$2" && grep -q "format not recognised" "$2"' _ $rc "$OUT/r1c2.txt"

  note "(d) bytes that are not the content the chain names: the pointer's sha256 does not match what the CID serves"
  IDL_SHA=$(printf 'other bytes' | python3 -c 'import sys,hashlib;print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())')
  ok R1.d0 "B points X's IDL at the real IDL's CID with another sha256" \
    lz update "$X" --idl-cid "$("$LEZREG" --json show "$REG" | jget deployer_entry.idl.cid)" --idl-sha256 "$IDL_SHA" --idl-size "$(wc -c <"$REG_IDL" | tr -d ' ')" --payer "$PAYER"
  XDG_CACHE_HOME=$OUT/cache-r1d "$EXAMPLE" "$RPC" "$REG" "$X" "$API_B" >"$OUT/r1d.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/r1d.txt"
  ok R1.d "hash mismatch: the bytes are discarded unparsed, metadata shown, 'IDL unavailable (hash mismatch ...)'" \
    sh -c '[ "$1" -eq 0 ] && grep -q "^demo-sample 1.2.0 by" "$2" && grep -q "IDL unavailable (hash mismatch" "$2"' _ $rc "$OUT/r1d.txt"
  ok R1.e "B restores X's real IDL (uploaded again to node B)" lz update "$X" --idl-cid push --idl-file "$REG_IDL" --payer "$PAYER"

  criterion "R2  No mandatory centralised services" "beyond the user's sequencer, LEZ node and Logos Storage, no third-party web service is needed"
  note "every network address this run used: sequencer $RPC, Storage $API_A $API_B $API_C (all 127.0.0.1)."
  note "register, update, lookup, enumerate, search and IDL resolution above used nothing else. 'lezreg verify'"
  note "pulls the builder image (Docker Hub), crates (crates.io) and the source (the git forge); README and DESIGN §7.7."
  record R2 INFO "not machine-checked by this script; see docs/FURPS.md (Reliability) for the evidence"

  criterion "P1  Performance (information only)" "document cycle_bench cycle counts for register and update"
  note "simulated cycles from lezreg's own pre-send simulation in this run (NOT cycle_bench):"
  note "  register (third-party entry, F3.a): ${REG_CYCLES:-?} cycles; update (F4.a): ${UPD_CYCLES:-?} cycles"
  record P1 INFO "simulation cycles shown above; cycle_bench numbers are not done (docs/FURPS.md)"

  # ------------------------------------------------------------------------------------------
  criterion "X   Further checks (beyond the spec's list)"
  ok X.a "a deploy dry run checks keys, simulation and funds, and sends nothing" \
    lz deploy "$SAMPLE_BIN" --dry-run --author "$A" --payer "$PAYER" --name demo-dry --version 1.0.0 --idl-cid raw --idl-file "$REG_IDL" \
      --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock"
  refused X.b "a deploy whose payer cannot cover it is refused before the first paid write" "Nothing was written" \
    deploy "$SAMPLE_BIN" --author "$A" --name demo-poor --version 1.0.0 --idl-cid raw --idl-file "$REG_IDL" \
      --repo "$REPO_URL" --commit "$COMMIT" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml --lock-file "$ROOT/Cargo.lock"
  local LIVE OTHER
  LIVE=$("$LEZREG" --json show "$X" | jget header.image_id)
  OTHER=0000000000000000000000000000000000000000000000000000000000000001
  lz update "$X" --image-id "$OTHER" --version 1.3.0-wrong --payer "$PAYER" 2>&1 | tail -2
  ok X.c "an entry claiming another image than the live program shows STALE" \
    sh -c '"$1" --json show "$2" | python3 -c "import json,sys; d=json.load(sys.stdin); print(\"  \", d[\"freshness\"]); sys.exit(d[\"freshness\"][\"status\"]!=\"stale\")"' _ "$LEZREG" "$X"
  lz update "$X" --image-id "$LIVE" --version 1.3.0 --payer "$PAYER" 2>&1 | tail -2
  ok X.d "claiming the live image again shows fresh" \
    sh -c '"$1" --json show "$2" | python3 -c "import json,sys; d=json.load(sys.stdin); print(\"  \", d[\"freshness\"]); sys.exit(d[\"freshness\"][\"status\"]!=\"fresh\")"' _ "$LEZREG" "$X"
  ok X.e "a community entry claiming an account with no program is flagged NO-PROGRAM" \
    sh -c '"$1" register --third-party --program "$2" --publisher "$3" --payer "$4" --name demo-sample --version 9.9.9 --label "Demo Labs" --description "claims a program that does not exist" --image-id "$5" --idl-cid raw --idl-file "$6" --repo "$7" --commit "$8" --binary lez_program_registry --manifest-path methods/guest/Cargo.toml 2>&1 | tail -2 && "$1" list --limit 1 | tee /dev/stderr | grep -q NO-PROGRAM' _ "$LEZREG" "$M" "$Q" "$PAYER" "$LIVE" "$REG_IDL" "$REPO_URL" "$COMMIT"
  ok X.e2 "show <name> lists every entry of that name and ranks the NO-PROGRAM one last" \
    sh -c '"$1" show Demo-Sample | tee /dev/stderr | tail -1 | grep -q NO-PROGRAM' _ "$LEZREG"
  ok X.e3 "show --revision 2: the write-once release snapshot of X's revision 2" lz show "$X" --revision 2
  ok X.f "search by tag and by (unproven) label" sh -c '"$1" search tag demo && "$1" search label "Demo Labs"' _ "$LEZREG"
  ok X.g "freeze X (author B signs)" lz freeze "$X" --payer "$PAYER"
  refused X.h "a frozen entry refuses updates" "frozen" update "$X" --version 9.9.9 --payer "$PAYER"
  unset LEZREG_NETWORK
  refused X.i "the wallet refuses a network with no RPC instead of falling back to a default sequencer" "has not launched" --network testnet wallet check-health
  export LEZREG_NETWORK=demo
  refused X.j "writes through a sequencer that is not on this machine need --yes" "is not on this machine" \
    --rpc http://192.0.2.1:3062 wallet auth-transfer send --from "Public/$PAYER" --to "Public/$A" --amount 1
  refused X.j2 "the URL is parsed, not prefix-matched: 127.0.0.1:PORT@elsewhere is elsewhere" "is not on this machine" \
    --rpc "http://127.0.0.1:$RPC_PORT@example.invalid/" register --program "$X" --name x --version 1 --idl-cid raw --idl-file "$REG_IDL" --repo "$REPO_URL" --commit "$COMMIT"
  refused X.j3 "lezreg wallet refuses change-network (it never moves to another sequencer behind your back)" "change-network" \
    wallet change-network testnet
  ok X.k "stats: the adoption table read from the chain" lz stats --by-month --by-author
  echo; echo "REG=$REG X=$X X3=$X3 A=$A B=$B Q=$Q M=$M"
}

# ======================================================================================================
testnet_demo() {
  local REG=$T_REG P=${T_PROGRAM:-$T_REG} rc
  export LEZREG_RPC=$T_RPC LEZREG_REGISTRY=$T_REG
  criterion "T0  Public sequencer" ""
  ok T0 "the sequencer answers ($T_RPC)" sh -c 'curl -sf -m 15 -X POST "$1" -H "Content-Type: application/json" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getLastBlockId\",\"params\":[]}"' _ "$T_RPC"
  criterion "F1/F3  The registry registered in itself; a deployer entry told apart from chain data" ""
  ok T1 "show REG: (REG, REG) holds a deployer entry" sh -c '"$1" show "$2" && "$1" --json show "$2" | python3 -c "import json,sys; e=json.load(sys.stdin)[\"deployer_entry\"]; sys.exit(not e or e[\"kind\"]!=\"deployer\")"' _ "$LEZREG" "$REG"
  criterion "F6  Discovery" ""
  ok T2 "lookup by program id ($P)" lz show "$P"
  ok T3 "enumerate the full registry, page by page" pyck 'import json,subprocess,sys
after=None; n=0
while True:
    d=json.loads(subprocess.run([sys.argv[1],"--json","list","--limit","50"]+(["--after",after] if after else []),capture_output=True,text=True,check=True).stdout)
    n+=len(d["items"]); after=d["next"]
    if not after: break
print("   walked", n, "of", d["total"]); sys.exit(n!=d["total"])' "$LEZREG"
  ok T4 "search by name" lz search name lez_program_registry
  ok T5 "search by proven author (the registry's own header key)" lz search author "$REG"
  criterion "F5/U1  IDL on Storage, through the SDK worked example" ""
  if [ -n "$T_STORAGE" ]; then
    ok T6 "lezreg idl get REG (hash-checked)" sh -c '"$1" idl get "$2" --storage-api "$3" | head -5' _ "$LEZREG" "$REG" "$T_STORAGE"
    "$EXAMPLE" "$T_RPC" "$REG" "$P" "$T_STORAGE" >"$OUT/t7.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/t7.txt"
    ok T7 "SDK example: metadata, freshness and IDL" sh -c '[ "$1" -eq 0 ] && grep -qE "IDL: [0-9]+ instructions" "$2"' _ $rc "$OUT/t7.txt"
  else record T6 SKIP "no --storage-api given"; record T7 SKIP "no --storage-api given"; fi
  criterion "R1  Degradation" ""
  XDG_CACHE_HOME=$OUT/cache-t8 "$EXAMPLE" "$T_RPC" "$REG" "$P" "$API_DEAD" >"$OUT/t8.txt" 2>&1; rc=$?; sed 's/^/   | /' "$OUT/t8.txt"
  ok T8 "no Storage node: metadata shown, IDL unavailable" sh -c '[ "$1" -eq 0 ] && grep -q "IDL unavailable" "$2"' _ $rc "$OUT/t8.txt"
  criterion "F7  Source verification" ""
  if [ "$HAVE_DOCKER" = 1 ] && [ "$VERIFY" != off ]; then
    ok T9 "lezreg verify $P says Match" sh -c '"$1" --json verify "$2" --mark --work "$3" | tee /dev/stderr | python3 -c "import json,sys; sys.exit(json.load(sys.stdin).get(\"verdict\")!=\"Match\")"' _ "$LEZREG" "$P" "$OUT/verify"
  elif [ "$VERIFY" = on ]; then record T9 FAIL "verify needs Docker"
  else record T9 SKIP "verify: no usable Docker here"; fi
}

main() {
  echo "LEZ Program Registry: LP-0023 demo ($MODE mode), $(date -u '+%Y-%m-%d %H:%M:%SZ')"
  echo "repo $(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo '(not a git checkout)')$(git -C "$ROOT" status --porcelain 2>/dev/null | grep -q . && echo ' (with uncommitted changes)')"
  echo "host $(uname -srm); RISC0_DEV_MODE=${RISC0_DEV_MODE:-unset}; output $OUT"
  prereqs || exit 1
  prepare || { echo "a build or download failed: see $BUILD_LOG"; exit 1; }
  echo "lezreg $("$LEZREG" --version 2>/dev/null)"
  [ "$PREPARE_ONLY" = 1 ] && { echo "--prepare-only: everything is built; run scripts/demo.sh to run the demo"; exit 0; }
  if [ "$MODE" = local ]; then local_demo; else testnet_demo; fi
  summary
  [ "$NFAIL" -eq 0 ]
}

# The traps go on an explicit subshell: bash 3.2 (macOS) skips an EXIT trap set inside a
# function that runs as a pipeline element.
( trap cleanup EXIT; trap 'exit 130' INT TERM; main ) 2>&1 | tee "$LOG"
exit "${PIPESTATUS[0]}"
