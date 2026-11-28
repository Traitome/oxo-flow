#!/usr/bin/env bash
# Timed arm: oxo-flow REAL execution of the N-step chain.
#
# Expects cwd = benchmark sandbox, run/oxo reset by drivers/prepare.sh, and
# N as $1. Non-vacuity guard: the run must report exactly N started rules and
# must leave exactly N outputs. A cached/no-op run (0 started rules) fails the
# iteration instead of being timed.
set -euo pipefail
N="$1"

./bin/oxo-flow run "defs/hello_${N}.oxoflow" -d run/oxo > run/oxo.log 2>&1

executed=$(grep -c '"event":"rule_started"' run/oxo.log || true)
outputs=$(ls run/oxo/step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')

if [ "${executed}" -ne "${N}" ]; then
    echo "non-vacuity: expected ${N} rule executions, saw ${executed}" >&2
    exit 3
fi
if [ "${outputs}" -ne "${N}" ]; then
    echo "non-vacuity: expected ${N} outputs, saw ${outputs}" >&2
    exit 3
fi
