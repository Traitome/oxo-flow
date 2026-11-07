#!/bin/bash
# SGE real-scheduler matrix runner. Executes INSIDE the sge-master container:
#   docker exec sge-master bash /work/run-sge.sh
# Produces evidence under /work/evidence/sge/.
set -uo pipefail

export SGE_ROOT=/opt/sge
export SGE_CELL=default
export SGE_ARCH=lx-amd64
export LD_LIBRARY_PATH=$SGE_ROOT/lib/$SGE_ARCH
export PATH=$SGE_ROOT/bin/$SGE_ARCH:$PATH
OXO=/work/bin/oxo-flow
EV=/work/evidence/sge
WF=/work/workflows

mkdir -p "$EV"
source "$SGE_ROOT/$SGE_CELL/common/settings.sh"

# Shared helpers (snapshot loop, run seeding, run-dir evidence dump) live in
# matrix-lib.sh copied to /work next to this runner — the PBS runner sources
# the same file so the two cannot drift apart.
source /work/matrix-lib.sh

echo "== oxo-flow version ==" | tee "$EV/summary.txt"
"$OXO" --version 2>&1 | tee -a "$EV/summary.txt"

echo "== chain matrix =="
run_case "$WF/wf-chain.oxoflow" sge /work/runs/chain "$EV/chain-run.log" "$EV/qstat-f-chain.txt"
CHAIN_RC=$?

echo "== array matrix =="
run_case "$WF/wf-array.oxoflow" sge-array /work/runs/array "$EV/array-run.log" "$EV/qstat-f-array.txt"
ARRAY_RC=$?

echo "== collect scheduler evidence =="
qstat -f > "$EV/qstat-f-final.txt" 2>&1
for rule in prep align_s1 align_s2 merge gpu_step report; do
  echo "=== qacct -j $rule ===" >> "$EV/qacct-chain.txt"
  qacct -j "$rule" >> "$EV/qacct-chain.txt" 2>&1
done
for rule in chunk gather; do
  echo "=== qacct -j $rule ===" >> "$EV/qacct-array.txt"
  qacct -j "$rule" >> "$EV/qacct-array.txt" 2>&1
done

collect_run_dir_evidence "$EV" /work/runs

echo "chain_rc=$CHAIN_RC array_rc=$ARRAY_RC" | tee -a "$EV/summary.txt"

# The evidence echo must not swallow the verdict: a CI or human caller has to
# be able to distinguish "both matrices passed" from "both failed" (H-2).
[[ $CHAIN_RC -eq 0 && $ARRAY_RC -eq 0 ]] || exit 1
