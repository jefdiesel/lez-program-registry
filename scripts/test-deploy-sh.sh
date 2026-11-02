#!/usr/bin/env bash
# Regression tests for chain/deploy.sh (review findings robustness 4, conformance 11), with a
# stub `lezreg` that records its calls. No sequencer and no wallet needed.
#
#   scripts/test-deploy-sh.sh
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
touch "$T/reg.bin"

# The stub: behaviour from $T/mode.<command>, every call logged to $T/calls.
cat > "$T/lezreg" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$STUB/calls"
cmd=""
for a in "$@"; do case "$a" in show|deploy|register|network) cmd=$a; break ;; esac; done
mode=$(cat "$STUB/mode.$cmd" 2>/dev/null || echo ok)
case "$cmd:$mode" in
  show:down) echo '{"error":"cannot reach the sequencer","kind":"transport"}'; exit 1 ;;
  show:empty) echo '{"header":null,"deployer_entry":null}' ;;
  show:deployed) echo '{"header":{"image_id":"ab"},"deployer_entry":null}' ;;
  show:*) echo '{"header":{"image_id":"ab"},"deployer_entry":{"name":"lez_program_registry"}}' ;;
  deploy:fail-after-save) echo '{"error":"transaction X was not confirmed","kind":"not_included","program":"NEWREG"}'; exit 1 ;;
  deploy:fail-before-save) echo '{"error":"payer holds 0","kind":"funds"}'; exit 1 ;;
  deploy:*) echo '{"program":"NEWREG","tx":"h"}' ;;
  *) echo '{}' ;;
esac
EOF
chmod +x "$T/lezreg"
export STUB=$T LEZREG=$T/lezreg STATE=$T/state.env
run() { "$ROOT/chain/deploy.sh" --elf "$T/reg.bin" --payer P --commit "$(printf '0%.0s' {1..40})" --idl-cid 0x01 >"$T/out" 2>&1; }
fail() { echo "FAIL: $*"; cat "$T/out"; echo "calls:"; cat "$T/calls" 2>/dev/null; exit 1; }
reset() { rm -f "$T"/mode.* "$T/calls" "$STATE"; }

# 1. A saved registry whose read fails: stop; never deploy a second registry or touch STATE.
reset; echo "REG=OLDREG" > "$STATE"; echo down > "$T/mode.show"
run && fail "a failed read must stop the script"
grep -q " deploy " "$T/calls" && fail "deployed after a failed read"
grep -qx "REG=OLDREG" "$STATE" || fail "STATE changed"
echo "ok 1: a failed read stops the script; no second registry"

# 2. A deploy that fails after the header key was saved: STATE keeps it ...
reset; echo fail-after-save > "$T/mode.deploy"
run && fail "the deploy failed"
grep -qx "REG=NEWREG" "$STATE" || fail "STATE does not hold the saved header account"
# ... and the rerun deploys at that same account.
rm -f "$T/calls"; echo empty > "$T/mode.show"; echo ok > "$T/mode.deploy"
run || fail "rerun"
grep -q "deploy $T/reg.bin --no-register --program NEWREG" "$T/calls" || fail "the rerun did not deploy at NEWREG"
echo "ok 2: a late deploy failure keeps the header account; the rerun finishes there"

# 3. A deploy refused before anything was saved leaves no STATE.
reset; echo fail-before-save > "$T/mode.deploy"
run && fail "the deploy failed"
[ -f "$STATE" ] && fail "STATE written for a refused deploy"
echo "ok 3: a refused deploy leaves no state"

# 4. Everything already there: no writes.
reset; echo "REG=OLDREG" > "$STATE"
run || fail "idempotent rerun"
grep -qE " (deploy|register) " "$T/calls" && fail "wrote although everything is there"
echo "ok 4: a rerun with everything in place writes nothing"
echo "PASS"
