#!/usr/bin/env bash
# The M2 demo was folded into scripts/demo.sh, which covers every step it had (and needs no
# funded wallet: it starts its own chain). The M2 transcript is kept in docs/demo-m2.log.
exec "$(dirname "$0")/demo.sh" "$@"
