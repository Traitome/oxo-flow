#!/usr/bin/env bash
# Cohort-scale deterministic replay measurement for checkpoint re-entry +
# [[values]] expansion (issue #1245, ROADMAP Stage 12.2).
#
# Thin wrapper around scripts/reentry_cohort_replay.py — see that file for the
# fixture parameters and the proof. Report-only: always exits 0.
#
# Usage:
#   scripts/reentry-cohort-replay.sh [N] [M]
#   OXO_FLOW_BIN=<path> scripts/reentry-cohort-replay.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$ROOT/scripts/reentry_cohort_replay.py" "$@"
