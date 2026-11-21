#!/usr/bin/env bash
# Execution-equivalence harness for the import corpus (#1300) -- the honest
# EXECUTION half of the #1165 transpiler ask. For each SUPPORTED fixture it
# runs the SOURCE workflow and the IMPORTED workflow on identical inputs and
# compares the produced output-artifact sets (checksums).
#
# HARD RULE: a fixture is MATCHED only when BOTH arms genuinely executed. Any
# missing runtime / container / conda backend makes its verdict SKIPPED with
# the reason -- never MATCHED. Report-only by default; --strict fails on drift
# for fixtures that actually executed.
#
# Usage: scripts/import-execution-equivalence.sh [--strict] [--bin <path>]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STRICT=0
BIN=""
while [ $# -gt 0 ]; do
  case "$1" in
    --strict) STRICT=1; shift ;;
    --bin) BIN="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [ -z "$BIN" ]; then
  if [ -n "${OXO_FLOW_BIN:-}" ]; then
    BIN="$OXO_FLOW_BIN"
  elif [ -x "$ROOT/target/debug/oxo-flow" ]; then
    BIN="$ROOT/target/debug/oxo-flow"
  else
    BIN="$(command -v oxo-flow || true)"
  fi
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "no oxo-flow binary found (build it or set OXO_FLOW_BIN/--bin)" >&2
  exit 2
fi

echo "# import-execution-equivalence corpus run"
echo "# binary:       $BIN"
echo "# fixture root: $ROOT/tests/fixtures/import"
echo "# date:         $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "# (report-only; pass --strict to fail on drift for executed fixtures)"
echo

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

rc=0
python3 "$ROOT/scripts/import_execution_equivalence_run.py" "$ROOT" "$BIN" "$tmp" || rc=$?

if [ "$STRICT" -eq 1 ] && [ "$rc" -ne 0 ]; then
  exit "$rc"
fi
exit 0
