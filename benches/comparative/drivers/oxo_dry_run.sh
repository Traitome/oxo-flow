#!/usr/bin/env bash
# Timed arm: oxo-flow dry-run (parse + DAG build + scheduling plan only).
#
# This arm does NOT spawn rule processes; it is a scheduling-overhead arm and
# must never be read against the real-execution arms. Guard: the plan must
# report all N rules as runnable, so a parse that silently drops rules fails
# the iteration.
set -euo pipefail
N="$1"

./bin/oxo-flow dry-run "defs/hello_${N}.oxoflow" > run/oxo-dry.log 2>&1

if ! grep -Eq "would run: ${N}([^0-9]|$)" run/oxo-dry.log; then
    echo "non-vacuity: dry-run did not plan ${N} rules" >&2
    exit 3
fi
