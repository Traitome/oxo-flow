#!/usr/bin/env bash
# Timed arm: Snakemake REAL execution of the N-step chain.
#
# Expects cwd = benchmark sandbox, run/sm reset by drivers/prepare.sh, and N as
# $1. The fresh directory is what makes this a real execution: a rerun in an
# unchanged directory prints "Nothing to be done" and executes zero jobs (the
# original P0 bug measured those no-ops). The guard counts executed jobs in
# Snakemake's own narration and the produced outputs; either being short fails
# the iteration.
set -euo pipefail
N="$1"

cd run/sm
snakemake --cores 1 --config count="${N}" > run.log 2>&1

jobs=$(grep -c "Finished job" run.log || true)
outputs=$(ls step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')

if [ "${jobs}" -lt "${N}" ]; then
    echo "non-vacuity: expected >= ${N} finished jobs, saw ${jobs}" >&2
    exit 3
fi
if [ "${outputs}" -ne "${N}" ]; then
    echo "non-vacuity: expected ${N} outputs, saw ${outputs}" >&2
    exit 3
fi
