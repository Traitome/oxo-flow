#!/usr/bin/env bash
# PostgreSQL run-ledger campaign for oxo-flow issue #1251.
#
# Produces the measurement evidence named by ROADMAP 12.1's exit criterion
# for "Unified run-ledger storage abstraction": N-way admission concurrency,
# crash recovery (writer killed mid-transaction), and horizontal-instance
# behavior on one live PostgreSQL.
#
# Usage:
#   scripts/postgres-ledger-campaign.sh [--strict] [--url <DATABASE_URL>]
#                                       [--concurrency <N>] [--keep] [--image <IMAGE>]
#
#   OXO_FLOW_TEST_POSTGRES_URL   reuse an existing disposable database instead
#                                of standing one up (skips the container step)
#   --concurrency N              N-way fan-out passed to the tests (default 16)
#   --keep                       leave the container running for inspection
#
# Report-only by default: prints the raw `measurement=` lines and the test
# tail, always exits 0. --strict exits non-zero if the test run fails (drift,
# a >1 winner, a ghost row, an error under contention).
#
# It cannot run vacuously: it resolves either a reachable DATABASE_URL or a
# working container runtime to create one, and fails hard if neither exists.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STRICT=0
KEEP=0
URL="${OXO_FLOW_TEST_POSTGRES_URL:-}"
CONCURRENCY="${OXO_FLOW_TEST_POSTGRES_CONCURRENCY:-16}"
IMAGE="${OXO_FLOW_TEST_POSTGRES_IMAGE:-postgres:16}"
CONTAINER="oxo-flow-ledger-$$"

while [ $# -gt 0 ]; do
  case "$1" in
    --strict) STRICT=1 ;;
    --keep) KEEP=1 ;;
    --url) shift; URL="$1" ;;
    --concurrency) shift; CONCURRENCY="$1" ;;
    --image) shift; IMAGE="$1" ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done

started_container=0
cleanup() {
  if [ "$started_container" = 1 ] && [ "$KEEP" = 0 ]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# ── Resolve a live PostgreSQL URL ──────────────────────────────────────
if [ -z "$URL" ]; then
  if ! command -v docker >/dev/null 2>&1; then
    echo "no OXO_FLOW_TEST_POSTGRES_URL and no docker on PATH — cannot run" >&2
    exit 2
  fi
  echo "# starting $IMAGE as $CONTAINER"
  docker run -d --name "$CONTAINER" \
    -e POSTGRES_USER=oxo -e POSTGRES_PASSWORD=oxo-test -e POSTGRES_DB=oxo_test \
    -p 127.0.0.1:0:5432 "$IMAGE" >/dev/null
  started_container=1
  port="$(docker port "$CONTAINER" 5432/tcp | head -1 | sed -E 's/.*:([0-9]+)$/\1/')"
  URL="postgres://oxo:oxo-test@127.0.0.1:${port}/oxo_test"
  echo "# waiting for PostgreSQL readiness on port ${port}"
  ready=0
  for _ in $(seq 1 60); do
    if docker exec "$CONTAINER" pg_isready -U oxo -d oxo_test >/dev/null 2>&1; then
      ready=1; break
    fi
    sleep 1
  done
  [ "$ready" = 1 ] || { echo "PostgreSQL did not become ready in time" >&2; exit 2; }
fi

echo "# postgres-ledger campaign"
echo "# url: ${URL%%@*}@…(redacted)"
echo "# concurrency: $CONCURRENCY"
echo "# host: $(uname -an)"
echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

# ── Run the measurements ───────────────────────────────────────────────
# One `cargo test` invocation; the whole file shares one database. The
# status is captured as `rc=0; … || rc=$?` so `set -e` cannot abort before
# the --strict check (mirrors scripts/import-equivalence.sh).
out="$(mktemp)"
rc=0
( cd "$ROOT" && \
  OXO_FLOW_TEST_POSTGRES_URL="$URL" \
  OXO_FLOW_TEST_POSTGRES_CONCURRENCY="$CONCURRENCY" \
  cargo test -p oxo-flow-web --features postgres \
    --test postgres_concurrency -- --nocapture --test-threads=1 ) \
  >"$out" 2>&1 || rc=$?

echo "## raw measurement lines"
grep -oE 'measurement=.*result=[A-Z]+' "$out" || echo "(none — see test tail)"
echo
echo "## test summary"
grep -E '^test result:|^test .* \.\.\. (ok|FAILED)' "$out" || true
echo
echo "## tail"
tail -n 20 "$out"

if [ "$rc" != 0 ]; then
  echo
  echo "## full log retained: $out"
fi
if [ "$STRICT" = 1 ] && [ "$rc" != 0 ]; then
  echo "STRICT: measurement run failed (exit $rc)" >&2
  exit "$rc"
fi
exit 0