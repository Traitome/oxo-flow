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

snapshot_qstat() {  # $1 = output file, $2 = pid of the run being watched
  while kill -0 "$2" 2>/dev/null; do
    echo "=== $(date +%H:%M:%S) ===" >> "$1"
    qstat -f >> "$1" 2>&1
    sleep 2
  done
}

run_case() {  # $1 workflow file, $2 profile, $3 run dir, $4 log, $5 snap
  rm -rf "$3"
  mkdir -p "$3"
  cp "$WF/$(basename "$1")" "$3/"
  cp -r "$WF/profiles" "$3/"
  cd "$3"
  # Seed workflow inputs inside the run dir (paths are relative to it).
  mkdir -p input data
  echo "simulated reads for the matrix" > input/reads.txt
  for i in 01 02 03 04 05 06 07 08 09 10; do
    printf "sample-%s simulated reads\n" "$i" > "data/s$i.fq"
  done
  (
    "$OXO" run "$(basename "$1")" --profile "$2" > "$4" 2>&1
    echo "run_rc=$?" >> "$4"
  ) &
  local RUNPID=$!
  snapshot_qstat "$5" "$RUNPID" &
  local SNAP=$!
  wait "$RUNPID"
  local RC=$?
  kill "$SNAP" 2>/dev/null
  wait "$SNAP" 2>/dev/null
  return "$RC"
}

seed_data() {
  mkdir -p /work/runs/input /work/runs/data
  echo "simulated reads for the chain matrix" > /work/runs/input/reads.txt
  for i in 01 02 03 04 05 06 07 08 09 10; do
    printf "sample-%s simulated reads\n" "$i" > "/work/runs/data/s$i.fq"
  done
}

echo "== oxo-flow version ==" | tee "$EV/summary.txt"
"$OXO" --version 2>&1 | tee -a "$EV/summary.txt"

seed_data

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

{
  echo "=== chain jobs tree ==="
  find /work/runs/chain/jobs -type f 2>/dev/null | sort
  echo "=== chain status.json files ==="
  for f in /work/runs/chain/jobs/*/status.json; do
    echo "--- $f"
    cat "$f" 2>/dev/null
    echo
  done
  echo "=== chain index.json ==="
  cat /work/runs/chain/index.json 2>/dev/null
  echo "=== array index.json ==="
  cat /work/runs/array/index.json 2>/dev/null
} > "$EV/run-dir-contents.txt" 2>&1

{
  echo "=== chain events.jsonl ==="
  find /work/runs/chain -name events.jsonl -exec cat {} \;
  echo "=== array events.jsonl ==="
  find /work/runs/array -name events.jsonl -exec cat {} \;
} > "$EV/events-jsonl.txt" 2>&1

{
  echo "=== chain artifacts ==="
  for f in /work/runs/chain/work/*.host /work/runs/chain/final/report.host; do
    echo "--- $f"
    cat "$f" 2>/dev/null
    echo
  done
  echo "--- final/report.env (first 20)"
  head -20 /work/runs/chain/final/report.env 2>/dev/null
  echo "=== array element hosts ==="
  for f in /work/runs/array/chunks/*.host; do
    echo "--- $f"
    cat "$f" 2>/dev/null
  done
} > "$EV/artifacts.txt" 2>&1

echo "chain_rc=$CHAIN_RC array_rc=$ARRAY_RC" | tee -a "$EV/summary.txt"
