#!/usr/bin/env bash
# Execution-equivalence measurement for oxo-flow issue #1304 (Bet-3 / Gate-3).
#
# Runs the SAME intent natively in three engines -- oxo-flow, Nextflow DSL2,
# Snakemake -- over the same inputs and the SAME pinned container digest, then
# compares the SHA-256 of the CONTENT of every declared output path
# (scripts/execution_equivalence_expectations.py defines the fixtures).
#
# This measures EXECUTION equivalence, the runtime sibling of the static
# declared-artifact check in scripts/import-equivalence.sh (#1165).
#
# Usage:
#   scripts/execution-equivalence.sh [--strict] [--bin <oxo-flow>]
#
# Report-only by default: prints the per-arm table, always exits 0.
# --strict: exits non-zero if any fixture DRIFTED. An engine that did not run
#           is SKIPPED, never MATCHED (honesty rule) -- and SKIPPED alone does
#           not fail --strict.
#
# Env knobs (absent engine => that arm is SKIPPED with the reason recorded):
#   EQ_NEXTFLOW_BIN, EQ_NEXTFLOW_PROFILE (-with-docker | -profile apptainer)
#   EQ_SNAKEMAKE_BIN, EQ_SNAKEMAKE_CONTAINER (a .sif path or docker://...)
#   EQ_SNAKEMAKE_EXTRA
#
# OXO_FLOW_BIN overrides --bin; default resolves target/debug then PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STRICT=0
BIN="${OXO_FLOW_BIN:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --strict) STRICT=1 ;;
    --bin) shift; BIN="$1" ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ -z "$BIN" ]; then
  if [ -x "$ROOT/target/debug/oxo-flow" ]; then BIN="$ROOT/target/debug/oxo-flow"
  else BIN="$(command -v oxo-flow || true)"; fi
fi
[ -x "$BIN" ] || { echo "no oxo-flow binary (set OXO_FLOW_BIN / --bin)" >&2; exit 2; }

echo "# execution-equivalence run"
echo "# binary: $BIN ($("$BIN" --version 2>/dev/null | head -1))"
echo "# fixture root: $ROOT/tests/fixtures/execution-equivalence"
echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
# Capture the runner's status without tripping `set -e`: a bare `python3 ...`
# that exits non-zero would abort the script before the STRICT check below,
# making the report-only (default) contract exit non-zero on drift.
rc=0
python3 "$ROOT/scripts/execution_equivalence_run.py" "$ROOT" "$BIN" "$tmp" || rc=$?
if [ "$STRICT" = 1 ] && [ "$rc" != 0 ]; then
  echo; echo "STRICT: drift detected (exit $rc)" >&2; exit "$rc"
fi
exit 0
