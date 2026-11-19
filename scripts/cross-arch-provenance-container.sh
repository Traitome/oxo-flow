#!/usr/bin/env bash
# Cross-architecture reproduction harness, CONTAINER arm (#1166).
# Runs one pinned workflow inside a container image pulled from a single
# multi-arch manifest-list digest with provenance capture, then re-audits the
# recorded checksums. Both arms share ONE manifest-list digest; only the
# resolved CPU architecture differs (DOCKER_DEFAULT_PLATFORM). This is the
# non-OS-confounded contrast the preregistration (PR-01 §C4) asks for.
#
# Docker's classic (non-containerd) snapshotter stores a digest-only reference
# for exactly ONE platform and refuses to re-resolve it to another
# ("cannot overwrite digest"), so each platform's image is staged under the
# fixed tag `crossarch1166:pin` immediately before that arm's run. Both staged
# images come from the same manifest-list digest.
#
# Usage: cross-arch-provenance-container.sh <workflow.oxoflow> <outdir> <platform> [oxo-flow-bin]
#   <platform> = linux/amd64 | linux/arm64
# Env: OXO_FLOW_BIN             oxo-flow binary
#      CROSSARCH_IMAGE_SRC      ref used to (re)stage the image, default the
#                               canonical manifest-list digest ref
#      CROSSARCH_STAGE          set to 0 to skip staging (image already present)
#
# Emits into <outdir>: identity.txt, checkpoint.json, verify.json,
# artifacts.sha256, rows.tsv, container.txt
set -euo pipefail

wf="${1:?usage: $0 <workflow> <outdir> <platform> [bin]}"
outdir="${2:?}"
platform="${3:?}"
bin="${4:-${OXO_FLOW_BIN:-}}"
if [ -z "$bin" ]; then
  repo="$(cd "$(dirname "$0")/.." && pwd)"
  if [ -x "$repo/target/debug/oxo-flow" ]; then bin="$repo/target/debug/oxo-flow"; else bin="oxo-flow"; fi
fi
mkdir -p "$outdir"
abs_wf="$(cd "$(dirname "$wf")" && pwd)/$(basename "$wf")"

DIGEST="sha256:5e2bdb1b0d542d5039cffb8181c65930093949194a65c365dfb1b0fe13a4fa5b"
TAG="crossarch1166:pin"
SRC="${CROSSARCH_IMAGE_SRC:-python@$DIGEST}"

# The docker CLI is spawned by a rule shell whose env is cleared to a fixed
# allowlist; DOCKER_DEFAULT_PLATFORM is not on it, so re-admit it via the
# operator escape hatch (issue #1108) so the *engine* selects the container
# architecture for each arm.
export OXO_FLOW_ENV_PASSTHROUGH="DOCKER_DEFAULT_PLATFORM"
export DOCKER_DEFAULT_PLATFORM="$platform"

{
  echo "hostname:   $(hostname)"
  echo "host-uname: $(uname -a)"
  echo "host-arch:  $(uname -m)"
  echo "nproc:      $(nproc 2>/dev/null || sysctl -n hw.ncpu)"
  echo "oxo-flow:   $("$bin" --version 2>/dev/null | head -1)"
  echo "commit:     ${CROSSARCH_COMMIT:-$(git -C "$(dirname "$abs_wf")/.." rev-parse HEAD 2>/dev/null || echo unknown)}"
  echo "platform:   $platform"
  echo "passthru:   $OXO_FLOW_ENV_PASSTHROUGH"
  echo "date-utc:   $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$outdir/identity.txt"

# Stage the per-platform image under the fixed tag from the shared digest.
# The classic snapshotter pins a digest-only ref to ONE platform and refuses to
# overwrite it, so drop any prior ref for this source before re-pulling.
if [ "${CROSSARCH_STAGE:-1}" = "1" ]; then
  docker rmi -f "$TAG" >/dev/null 2>&1 || true
  docker rmi -f "$SRC" >/dev/null 2>&1 || true
  docker pull --platform "$platform" "$SRC" >/dev/null 2>&1
  docker tag "$SRC" "$TAG"
fi

# Container identity: identical manifest-list digest both arms; the resolved
# per-platform image id and in-container toolchain versions differ per arm.
{
  echo "image-src:          $SRC"
  echo "manifest-list-digest: $DIGEST"
  echo "staged-tag:         $TAG"
  echo "platform:           $platform"
  echo "image-id:           $(docker image inspect "$TAG" --format '{{.Id}}' 2>/dev/null || echo unknown)"
  echo "image-arch:         $(docker image inspect "$TAG" --format '{{.Architecture}}' 2>/dev/null || echo unknown)"
  docker run --rm --pull=never "$TAG" sh -c 'echo "container-arch: $(uname -m)"; echo "python3: $(python3 --version)"; echo "sort: $(sort --version | head -1)"; echo "gzip: $(gzip -V | head -1)"; echo "sha256sum: $(sha256sum --version | head -1)"' 2>/dev/null
} > "$outdir/container.txt"

# Run with provenance capture.
rm -rf "$outdir/work"
"$bin" run "$abs_wf" -d "$outdir/work" --provenance --quiet
ckpt="$outdir/work/.oxo-flow/checkpoint.json"
cp "$ckpt" "$outdir/checkpoint.json"
"$bin" provenance verify "$ckpt" --json > "$outdir/verify.json" 2>/dev/null

# Independent raw artifact hashing.
if command -v sha256sum >/dev/null 2>&1; then
  ( cd "$outdir/work" && find . \( -name '*.txt' -o -name '*.gz' -o -name '*.bin' -o -name '*.sha256' \) \
      | sed 's|^\./||' | sort | xargs sha256sum ) > "$outdir/artifacts.sha256" 2>/dev/null || true
else
  ( cd "$outdir/work" && find . \( -name '*.txt' -o -name '*.gz' -o -name '*.bin' -o -name '*.sha256' \) \
      | sed 's|^\./||' | sort | xargs shasum -a 256 ) > "$outdir/artifacts.sha256" 2>/dev/null || true
fi

# Per-artifact rows from the make-recorded checksums.
python3 - "$outdir/verify.json" > "$outdir/rows.tsv" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))["verify"]
print("file\tstatus\texpected\tactual")
for e in d["entries"]:
    print("%s\t%s\t%s\t%s" % (e["file"], e["status"], e["expected"], e["actual"]))
PY

cat "$outdir/rows.tsv"
mism=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['verify']['mismatched'])" "$outdir/verify.json")
if [ "$mism" != "0" ]; then echo "PROVENANCE MISMATCH: $mism" >&2; exit 1; fi
echo "OK: all recorded checksums re-verified on $platform"
