#!/usr/bin/env bash
# Corpus-wide import-equivalence measurement for oxo-flow issue #1165.
#
# Runs the REAL oxo-flow binary over tests/fixtures/import/** and compares
# each emitted workflow against independently-derived declared-artifact truth
# (scripts/import_equivalence_expectations.py). Measures the QUIET failure
# (import succeeds but changes declared semantics), NOT execution-equivalence.
#
# Usage:
#   scripts/import-equivalence.sh [--strict] [--bin <oxo-flow>]
#
# Report-only by default: prints the confusion matrix, always exits 0.
# --strict: exits non-zero if any SUPPORTED fixture drifts from its expected
#           declared artifacts, or any REJECTED fixture unexpectedly imports.
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
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ -z "$BIN" ]; then
  if [ -x "$ROOT/target/debug/oxo-flow" ]; then BIN="$ROOT/target/debug/oxo-flow"
  else BIN="$(command -v oxo-flow || true)"; fi
fi
[ -x "$BIN" ] || { echo "no oxo-flow binary (set OXO_FLOW_BIN / --bin)" >&2; exit 2; }

echo "# import-equivalence corpus run"
echo "# binary: $BIN ($("$BIN" --version 2>/dev/null | head -1))"
echo "# fixture root: $ROOT/tests/fixtures/import"
echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 "$ROOT/scripts/import_equivalence_run.py" "$ROOT" "$BIN" "$tmp"
rc=$?
if [ "$STRICT" = 1 ] && [ "$rc" != 0 ]; then
  echo; echo "STRICT: drift detected (exit $rc)" >&2; exit "$rc"
fi
exit 0
