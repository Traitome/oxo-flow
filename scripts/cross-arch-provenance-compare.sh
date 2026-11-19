#!/usr/bin/env bash
# Cross-architecture provenance comparison (#1166): run the pinned container
# workflow on both arms, then emit a machine-readable results JSON with the
# per-artifact identical/differing verdict and a per-class verdict.
#
# Usage: cross-arch-provenance-compare.sh <workflow.oxoflow> <outroot> [bin]
# Emits <outroot>/crossarch-results.json
set -euo pipefail

wf="${1:?usage: $0 <workflow> <outroot> [bin]}"
outroot="${2:?}"
bin="${3:-${OXO_FLOW_BIN:-oxo-flow}}"
script_dir="$(cd "$(dirname "$0")" && pwd)"

export OXO_FLOW_BIN="$bin"
export CROSSARCH_IMAGE_SRC="${CROSSARCH_IMAGE_SRC:-python@sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b}"

mkdir -p "$outroot"
for plat in amd64 arm64; do
  d="$outroot/arm-$plat"
  rm -rf "$d"
  DOCKER_DEFAULT_PLATFORM="linux/$plat" bash "$script_dir/cross-arch-provenance-container.sh" \
    "$wf" "$d" "linux/$plat" "$bin"
done

python3 "$script_dir/cross-arch-provenance-compare.py" \
  "$outroot/arm-amd64" "$outroot/arm-arm64" > "$outroot/crossarch-results.json"
echo "wrote $outroot/crossarch-results.json"
