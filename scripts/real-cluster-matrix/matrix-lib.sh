#!/bin/bash
# Shared helpers for the real-scheduler matrix runners (run-sge.sh / run-pbs.sh).
# Sourced, never executed: it only defines functions. Runners run INSIDE the
# master container with /work mounted; evidence lands under /work/evidence/<backend>.

SNAPSHOT_INTERVAL=2

snapshot_qstat() {  # $1 = output file, $2 = pid of the run being watched
  while kill -0 "$2" 2>/dev/null; do
    echo "=== $(date +%H:%M:%S) ===" >> "$1"
    qstat -f >> "$1" 2>&1
    sleep "$SNAPSHOT_INTERVAL"
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

# Backend-agnostic run-dir evidence dump (both backends settle from the same
# artifacts: events timeline, per-rule status.json, index.json, .host files).
collect_run_dir_evidence() {  # $1 = evidence dir, $2 = run root (/work/runs)
  local EV=$1 RUNS=$2
  {
    echo "=== chain jobs tree ==="
    find "$RUNS/chain/jobs" -type f 2>/dev/null | sort
    echo "=== chain status.json files ==="
    for f in "$RUNS/chain/jobs"/*/status.json; do
      echo "--- $f"
      cat "$f" 2>/dev/null
      echo
    done
    echo "=== chain index.json ==="
    cat "$RUNS/chain/index.json" 2>/dev/null
    echo "=== array index.json ==="
    cat "$RUNS/array/index.json" 2>/dev/null
  } > "$EV/run-dir-contents.txt" 2>&1

  {
    echo "=== chain events.jsonl ==="
    find "$RUNS/chain" -name events.jsonl -exec cat {} \;
    echo "=== array events.jsonl ==="
    find "$RUNS/array" -name events.jsonl -exec cat {} \;
  } > "$EV/events-jsonl.txt" 2>&1

  {
    echo "=== chain artifacts ==="
    for f in "$RUNS/chain/work/"*.host "$RUNS/chain/final/report.host"; do
      echo "--- $f"
      cat "$f" 2>/dev/null
      echo
    done
    echo "--- final/report.env (first 20)"
    head -20 "$RUNS/chain/final/report.env" 2>/dev/null
    echo "=== array element hosts ==="
    for f in "$RUNS/array/chunks/"*.host; do
      echo "--- $f"
      cat "$f" 2>/dev/null
    done
  } > "$EV/artifacts.txt" 2>&1
}
