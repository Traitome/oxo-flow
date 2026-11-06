#!/bin/bash
# OpenPBS real-scheduler matrix runner. Executes INSIDE the pbs-master container:
#   docker exec pbs-master bash /work/run-pbs.sh
# Produces evidence under /work/evidence/pbs/.
set -uo pipefail

export PATH=/opt/pbs/bin:/opt/pbs/sbin:$PATH
export PBS_SERVER=${PBS_SERVER:-pbs-master}
OXO=/work/bin/oxo-flow
EV=/work/evidence/pbs
WF=/work/workflows

mkdir -p "$EV"

# Shared helpers (snapshot loop, run seeding, run-dir evidence dump) live in
# matrix-lib.sh copied to /work next to this runner — the SGE runner sources
# the same file so the two cannot drift apart.
source /work/matrix-lib.sh

echo "== oxo-flow version ==" | tee "$EV/summary.txt"
"$OXO" --version 2>&1 | tee -a "$EV/summary.txt"

echo "== chain matrix =="
run_case "$WF/wf-chain.oxoflow" pbs /work/runs/chain "$EV/chain-run.log" "$EV/qstat-f-chain.txt"
CHAIN_RC=$?

echo "== array matrix =="
run_case "$WF/wf-array.oxoflow" pbs-array /work/runs/array "$EV/array-run.log" "$EV/qstat-f-array.txt"
ARRAY_RC=$?

echo "== collect scheduler evidence =="
qstat -f > "$EV/qstat-f-final.txt" 2>&1
qstat -x > "$EV/qstat-x-final.json" 2>&1
{
  echo "=== chain job ids ==="
  find /work/runs/chain/jobs -name job.id -exec cat {} \;
} > "$EV/job-ids.txt" 2>&1
for id in $(cat /work/runs/chain/jobs/*/job.id 2>/dev/null); do
  echo "=== qstat -x $id ===" >> "$EV/qacct-chain.txt"
  qstat -x -f "$id" >> "$EV/qacct-chain.txt" 2>&1
done
for id in $(cat /work/runs/array/jobs/*/job.id 2>/dev/null); do
  echo "=== qstat -x $id ===" >> "$EV/qacct-array.txt"
  qstat -x -f "$id" >> "$EV/qacct-array.txt" 2>&1
done
echo "=== accounting files ===" >> "$EV/qacct-chain.txt"
ls -la /var/spool/pbs/server_priv/accounting/ >> "$EV/qacct-chain.txt" 2>&1
for f in /var/spool/pbs/server_priv/accounting/*; do
  echo "--- $f" >> "$EV/qacct-chain.txt"
  cat "$f" >> "$EV/qacct-chain.txt" 2>&1
done

collect_run_dir_evidence "$EV" /work/runs

echo "chain_rc=$CHAIN_RC array_rc=$ARRAY_RC" | tee -a "$EV/summary.txt"

# The evidence echo must not swallow the verdict: a CI or human caller has to
# be able to distinguish "both matrices passed" from "both failed" (H-2).
[[ $CHAIN_RC -eq 0 && $ARRAY_RC -eq 0 ]] || exit 1
