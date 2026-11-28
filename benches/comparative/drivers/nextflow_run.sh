#!/usr/bin/env bash
# Timed arm: Nextflow REAL execution of the N-step chain.
#
# Expects cwd = benchmark sandbox, run/nf reset by drivers/prepare.sh, and N as
# $1. `-ansi-log false` emits one "Submitted process" line per process, which
# is the execution marker counted by the guard. echo_step.nf publishes each
# declared output into ./results, so the produced artifacts are checked the
# same way as the other engines' outputs.
#
# Pinned by the caller's environment: NXF_VER (framework version) and
# NXF_OPTS (heap, e.g. -Xmx1g on small hosts).
set -euo pipefail
N="$1"

cd run/nf
nextflow run hello.nf --count "${N}" -work-dir work -ansi-log false > run.log 2>&1

submitted=$(grep -c "Submitted process" run.log || true)
outputs=$(ls results/step_*_output.txt 2>/dev/null | wc -l | tr -d ' ')

if [ "${submitted}" -ne "${N}" ]; then
    echo "non-vacuity: expected ${N} submitted processes, saw ${submitted}" >&2
    exit 3
fi
if [ "${outputs}" -ne "${N}" ]; then
    echo "non-vacuity: expected ${N} published outputs, saw ${outputs}" >&2
    exit 3
fi
