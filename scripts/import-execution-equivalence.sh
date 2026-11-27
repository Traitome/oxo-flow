#!/usr/bin/env bash
# Execution-equivalence harness for the import corpus (#1300) -- the honest
# EXECUTION half of the #1165 transpiler ask. For each SUPPORTED fixture it
# runs the SOURCE workflow and the IMPORTED workflow on identical inputs and
# compares the produced output-artifact sets (checksums).
#
# HARD RULE: a fixture is MATCHED only when BOTH arms genuinely executed AND
# produced artifacts. Any missing runtime / container / conda backend makes
# its verdict SKIPPED with the reason; two arms that exit 0 while writing
# nothing make it NO_ARTIFACTS -- never MATCHED (issue #1349). Requires
# Python >= 3.11 (tomllib): an older interpreter exits 2 with an explicit
# message, and report-only mode warns instead of swallowing it silently.
# Report-only by default; --strict fails on drift for fixtures that actually
# executed.
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
# Arms run with cwd set to their staged directory, so a relative --bin would be
# resolved against that directory and fail with ENOENT (issue #1398 campaign
# finding). Anchor it to this invocation's working directory.
if [ "${BIN#/}" = "$BIN" ]; then
  BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
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

if [ "$rc" -ne 0 ]; then
  if [ "$STRICT" -eq 1 ]; then
    exit "$rc"
  fi
  # Report-only mode still never fails the build, but a runner failure
  # (drift, or an interpreter below the 3.11 floor) must not stay silent
  # behind exit 0 (issue #1349).
  echo "warning: runner exited rc=$rc in report-only mode -- see messages above; use --strict to fail" >&2
fi
exit 0
