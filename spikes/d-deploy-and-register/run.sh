#!/bin/bash
# Spike (d): DeployAndRegister in one tx on the local rc1 sequencer (127.0.0.1:3062).
# Run on the Mini from this directory. Every step appends to out/run.log.
set -uo pipefail
S=$(cd "$(dirname "$0")" && pwd)
export LEE_WALLET_HOME_DIR=$S/wallet
C="$S/target/release/reg_client --rpc http://127.0.0.1:3062"
RPC=http://127.0.0.1:3062
FUNDER=26pkAnG15Y5QzdKBLpxzH9CLmWQCkQ91zQqEJnsrchNg
REG_ELF=$(ls $S/target/riscv-guest/reg_methods/lezreg_spike_reg/riscv32im-risc0-zkvm-elf/release/lezreg_spike_reg.bin)
TARGET_ELF=${TARGET_ELF:-/Volumes/exos162/logos/names-0.4/target/riscv-guest/names_methods/logos_names/riscv32im-risc0-zkvm-elf/release/logos_names.bin}
mkdir -p $S/out
SEQLOG=/Volumes/exos162/logos/lp0023-checks/sequencer.log
OFF=$(wc -c <$SEQLOG)
step() { echo; echo "===== $*"; }
acct() { $C new-accounts "$1"; }

getaccount() { # one getAccount(X) call; print actor_states keys and sizes
  curl -s -X POST -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$1\"]}" $RPC |
  python3 -c '
import sys, json
r = json.load(sys.stdin)["result"]
print("getAccount", sys.argv[1], "nonce", r.get("nonce"), "actor_states:")
for k, v in r["data"]["actor_states"].items():
    print("  ", k, len(v), "bytes", bytes(v[:8]).hex() + "...")
' "$1"
}

{
step "encoding check"; $C check-encoding
read RN RTOT RUSER RIMG <<<"$($C chunks $REG_ELF)"
read TN TTOT TUSER TIMG <<<"$($C chunks $TARGET_ELF)"
echo "REG    $REG_ELF total=$RTOT user_elf=$RUSER segments=$RN image_id=$RIMG"
echo "TARGET $TARGET_ELF total=$TTOT user_elf=$TUSER segments=$TN image_id=$TIMG"

step "accounts"
A=($(acct $((2 + 1 + RN + TN + 5))))
P=${A[0]}; M=${A[1]}; R=${A[2]}
RS=("${A[@]:3:$RN}"); TS=("${A[@]:$((3+RN)):$TN}")
o=$((3+RN+TN)); X=${A[$o]}; X3=${A[$((o+1))]}; X4=${A[$((o+2))]}; X5=${A[$((o+3))]}; R2=${A[$((o+4))]}
cat > $S/out/accounts.env <<EOF
P=$P M=$M R=$R X=$X X3=$X3 X4=$X4 X5=$X5 R2=$R2
RS=(${RS[*]})
TS=(${TS[*]})
EOF
cat $S/out/accounts.env

step "fund payer P (200M) and Mallory M (20M) from the lp0023 funder"
LEE_WALLET_HOME_DIR=$S/wallet $C fund --from $FUNDER --to $P --amount 200000000
$C fund --from $FUNDER --to $M --amount 20000000

step "deploy REG: segments then plain CreateHeader (also = standalone CreateHeader cost)"
$C write-segments --elf $REG_ELF --payer $P --segments ${RS[*]}
$C tx --variant create-header --header $R --first-segment ${RS[0]} --payer $P

step "write ~400 KB target segments (one chain, reused by every header below)"
$C write-segments --elf $TARGET_ELF --payer $P --segments ${TS[*]}

step "T1 POSITIVE: DeployAndRegister for X in ONE tx (X signs, P pays)"
$C tx --variant dar --reg $R --header $X --first-segment ${TS[0]} --payer $P --record "x-born-with-deploy"
step "T1 read-back: ONE getAccount(X)"; getaccount $X

step "T2/T3 two-tx fallback on X3: CreateHeader, then Register"
$C tx --variant create-header --header $X3 --first-segment ${TS[0]} --payer $P
$C tx --variant register --reg $R --header $X3 --payer $P --record "x3-two-tx"
getaccount $X3

step "N1 NEGATIVE: Mallory registers (X4, REG) for X4 she doesn't control (X4 has a header, no entry: the two-tx squat window)"
$C tx --variant create-header --header $X4 --first-segment ${TS[0]} --payer $P
$C tx --variant register --reg $R --header $X4 --header-no-sign --payer $M --record "mallory" || true
$C tx --variant register --reg $R --header $X4 --header-no-sign --payer $M --record "mallory" --force-gas 100000
getaccount $X4

step "N2 NEGATIVE: Mallory DeployAndRegister on fresh, non-existent X5 (X5 does not sign)"
$C tx --variant dar --reg $R --header $X5 --first-segment ${TS[0]} --header-no-sign --payer $M --record "mallory" --force-gas 100000
step "N2b: same after X5 exists (funded 1 unit), so admission is not the reason"
$C fund --from $P --to $X5 --amount 1
$C tx --variant dar --reg $R --header $X5 --first-segment ${TS[0]} --header-no-sign --payer $M --record "mallory" --force-gas 100000
step "N3 NEGATIVE: REG check removed (dar-nocheck): the loader's own is_authorized must refuse"
$C tx --variant dar-nocheck --reg $R --header $X5 --first-segment ${TS[0]} --header-no-sign --payer $M --record "mallory" --force-gas 100000
getaccount $X5

step "N4: honest DeployAndRegister again on X (already deployed + registered) must revert"
$C tx --variant dar --reg $R --header $X --first-segment ${TS[0]} --payer $P --record "again" --force-gas 100000
getaccount $X

step "N5: self-entry (R2, R2) by DeployAndRegister: root program not deployed yet"
$C tx --variant dar --reg $R2 --header $R2 --first-segment ${RS[0]} --payer $P --record "self" --force-gas 100000

step "sequencer log: panics from this run"
} 2>&1 | tee -a $S/out/run.log
tail -c +$((OFF+1)) $SEQLOG | grep -aoE "REG: [^\"]*|CreateHeader target must be an authorized account|header target already deployed|UnknownProgram[^\"]{0,80}|Unadmitted[^\"]{0,80}" | sort | uniq -c | tee -a $S/out/run.log
