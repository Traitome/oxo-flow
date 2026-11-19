#!/usr/bin/env bash
# Range I/O and object-storage staging measurement for oxo-flow issue #1250
# (ROADMAP 12.1 row: "Range I/O and object-storage staging").
#
# Stands up an S3-compatible server (RustFS — MinIO's community server is no
# longer distributed; see crates/oxo-flow-cli/tests/remote_staging.rs), seeds
# one object, and runs four measurements against the REAL oxo-flow binary:
#
#   M1 end-to-end streaming   throughput + peak RSS, eager vs stream
#   M2 interrupt / resume     kill mid-eager-download and mid-stream-read
#   M3 backpressure           slow consumer; bounded producer memory/scratch?
#   M4 scratch footprint      eager vs stream peak scratch bytes
#
# Emits raw numbers. --self-test adds the FAIL-FIRST non-vacuity check
# (perturb stream -> eager and assert the scratch discriminator flips).
#
# Usage:
#   scripts/range-io-staging.sh [--bin <oxo-flow>] [--obj-mib N] [--self-test]
#                               [--no-serve] [--keep]
#
# The binary MUST be built with --features s3-storage:
#   cargo build -p oxo-flow-cli --features s3-storage
# OXO_FLOW_BIN overrides --bin; default resolves target/debug then PATH.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${OXO_FLOW_BIN:-}"
OBJ_MIB="${OXO_1250_OBJ_MIB:-256}"
SELF_TEST=0
NO_SERVE=0
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --bin) shift; BIN="$1" ;;
    --obj-mib) shift; OBJ_MIB="$1" ;;
    --self-test) SELF_TEST=1 ;;
    --no-serve) NO_SERVE=1 ;;
    --keep) KEEP=1 ;;
    -h|--help) sed -n '2,26p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ -z "$BIN" ]; then
  if [ -x "$ROOT/target/debug/oxo-flow" ]; then BIN="$ROOT/target/debug/oxo-flow"
  else BIN="$(command -v oxo-flow || true)"; fi
fi
[ -x "$BIN" ] || { echo "no oxo-flow binary (set OXO_FLOW_BIN / --bin)" >&2; exit 2; }

CONTAINER="${OXO_1250_CONTAINER:-oxo-rustfs}"
IMAGE="${OXO_1250_IMAGE:-quay.io/rustfs/rustfs:latest}"
export AWS_ENDPOINT_URL="${AWS_ENDPOINT_URL:-http://127.0.0.1:9000}"
export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-minioadmin}"
export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-minioadmin}"
export AWS_REGION="${AWS_REGION:-us-east-1}"
export OXO_S3_FORCE_PATH_STYLE=1

started=0
if [ "$NO_SERVE" = 0 ]; then
  if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
    echo "# starting $CONTAINER ($IMAGE)"
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    docker run -d --name "$CONTAINER" -p 9000:9000 -p 9001:9001 \
      -e RUSTFS_ACCESS_KEY="$AWS_ACCESS_KEY_ID" \
      -e RUSTFS_SECRET_KEY="$AWS_SECRET_ACCESS_KEY" \
      -e RUSTFS_CONSOLE_ENABLE=true "$IMAGE" >/dev/null
    started=1
  fi
  # Wait for the S3 API to answer (boto3 create_bucket is idempotent enough
  # for the probe; the driver tolerates an already-existing bucket).
  ready=0
  for _ in $(seq 1 60); do
    if python3 - "$AWS_ENDPOINT_URL" <<'PY' 2>/dev/null
import sys, boto3
from botocore.config import Config
ep = sys.argv[1]
s3 = boto3.client("s3", endpoint_url=ep, aws_access_key_id="minioadmin",
                  aws_secret_access_key="minioadmin", region_name="us-east-1",
                  config=Config(s3={"addressing_style": "path"}))
s3.list_buckets()
PY
    then ready=1; break; fi
    sleep 1
  done
  [ "$ready" = 1 ] || { echo "S3 endpoint not ready at $AWS_ENDPOINT_URL" >&2; exit 3; }
else
  echo "# --no-serve: using existing endpoint $AWS_ENDPOINT_URL"
fi

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/oxo1250.XXXXXX")"
echo "# scratch root: $STAGE"
echo "# binary: $BIN ($("$BIN" --version 2>/dev/null | head -1))"
echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ)"

ARGS=("$BIN" "$STAGE" --obj-mib "$OBJ_MIB")
[ "$SELF_TEST" = 1 ] && ARGS+=(--self-test)

# Capture the driver status without tripping `set -e` (a bare command that
# exits non-zero would abort before cleanup/config checks below).
rc=0
python3 "$ROOT/scripts/range_io_staging_run.py" "${ARGS[@]}" || rc=$?

if [ "$KEEP" = 0 ]; then rm -rf "$STAGE"; fi
if [ "$started" = 1 ] && [ "$KEEP" = 0 ]; then
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
fi
exit "$rc"
