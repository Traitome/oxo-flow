#!/usr/bin/env bash
# Run the FROZEN governed cross-architecture corpus (#1397 / #1166) on ONE arm.
#
# Both arms run the same two workflows inside containers pulled from the SAME
# multi-arch manifest-list digest; only the resolved CPU architecture differs
# (selected by DOCKER_DEFAULT_PLATFORM, re-admitted into the rule shell through
# OXO_FLOW_ENV_PASSTHROUGH). The arm script records host identity, container
# identity, per-workflow checkpoints, `provenance verify --json` output, and an
# INDEPENDENT sha256 of every recorded declared output.
#
# Usage:
#   cross-arch-provenance-arm.sh <platform> <outdir> <repo-root> <bin> <workflow...>
#     <platform>   linux/amd64 | linux/arm64
#     <outdir>     absolute output directory (raw evidence is kept here)
#     <repo-root>  git checkout of the frozen engine commit
#     <bin>        oxo-flow binary built from that commit
#     <workflow>   one or more .oxoflow files from the frozen corpus
#
# Exit 0 only when every workflow runs, re-verifies, and every declared output
# is independently re-hashed; any missing output or mismatch is fatal.
set -euo pipefail

platform="${1:?usage: $0 <platform> <outdir> <repo-root> <bin> <workflow...>}"
outdir="${2:?}"
repo="${3:?}"
bin="${4:?}"
shift 4
[ "$#" -ge 1 ] || { echo "no workflows given" >&2; exit 2; }

mkdir -p "$outdir"
outdir="$(cd "$outdir" && pwd)"
repo="$(cd "$repo" && pwd)"
bin="$(cd "$(dirname "$bin")" && pwd)/$(basename "$bin")"
[ -x "$bin" ] || { echo "oxo-flow binary not executable: $bin" >&2; exit 2; }

digest=""
tag=""
image_ref=""
corpus="$repo/scripts/cross-arch-provenance-governed-corpus.json"
[ -f "$corpus" ] || { echo "missing corpus manifest: $corpus" >&2; exit 2; }
read -r image_ref digest tag <<< "$(python3 -I -c '
import json, sys
c = json.load(open(sys.argv[1]))["image"]
print(c["ref"], c["manifest_list_digest"], c["staged_tag"])
' "$corpus")"
[ -n "$image_ref" ] && [ -n "$digest" ] && [ -n "$tag" ] || {
  echo "corpus manifest is missing the image ref/digest/tag" >&2
  exit 2
}

if command -v sha256sum >/dev/null 2>&1; then
  sha256_of() { sha256sum "$1" | awk '{print $1}'; }
else
  sha256_of() { shasum -a 256 "$1" | awk '{print $1}'; }
fi

# The docker CLI is spawned by a rule shell whose environment is cleared to a
# fixed allowlist; DOCKER_DEFAULT_PLATFORM is not on it, so re-admit it through
# the documented operator escape hatch (issue #1108). The engine then selects
# the native container architecture for this arm.
export OXO_FLOW_ENV_PASSTHROUGH="DOCKER_DEFAULT_PLATFORM"
export DOCKER_DEFAULT_PLATFORM="$platform"

engine_commit="$(git -C "$repo" rev-parse HEAD)"
workflow_dirty="$(git -C "$repo" status --porcelain -- "$@" | wc -l | tr -d ' ')"
if [ "$workflow_dirty" != "0" ]; then
  echo "frozen corpus is not clean in $repo (git status -- workflow paths):" >&2
  git -C "$repo" status --porcelain -- "$@" >&2
  exit 2
fi

{
  echo "hostname:        $(hostname)"
  echo "host-uname:      $(uname -a)"
  echo "host-arch:       $(uname -m)"
  echo "nproc:           $(nproc 2>/dev/null || sysctl -n hw.ncpu)"
  cpu_model="$(lscpu 2>/dev/null | awk -F: '/Model name/ {gsub(/^ +/, \"\", $2); print $2; exit}')"
  [ -n "$cpu_model" ] || cpu_model="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)"
  echo "cpu-model:       $cpu_model"
  echo "os-release:      $(uname -s)"
  echo "kernel:          $(uname -r)"
  echo "docker-version:  $(docker --version 2>/dev/null || echo unknown)"
  echo "docker-server:   $(docker info --format '{{.ServerVersion}} {{.Architecture}} {{.OperatingSystem}}' 2>/dev/null || echo unknown)"
  echo "oxo-flow:        $("$bin" --version 2>/dev/null | head -1)"
  echo "binary-sha256:   $(sha256_of "$bin")"
  echo "engine-commit:   $engine_commit"
  echo "engine-dirty:    $(git -C "$repo" status --porcelain -- "$@" | wc -l | tr -d ' ')"
  echo "platform:        $platform"
  echo "passthru:        $OXO_FLOW_ENV_PASSTHROUGH"
  echo "date-utc:        $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$outdir/identity.txt"

# Stage this platform's image from the ONE shared manifest-list digest under a
# fixed local tag. Docker's classic snapshotter pins a digest-only reference to
# exactly ONE platform and refuses to re-resolve it to another ("cannot
# overwrite digest"), so a fresh host normally clears the prior reference first.
#
# CROSSARCH_STAGE=0 uses an image the operator has already pre-staged (pull +
# tag, no `docker rmi`) and verifies its architecture fail-closed. The
# operator path exists because Docker Desktop 4.94 crashed on `docker rmi` of
# the digest-only reference during the #1397 campaign; it does not change the
# environment identity, which is still the shared manifest-list digest.
stage="${CROSSARCH_STAGE:-1}"
if [ "$stage" = "1" ]; then
  docker rmi -f "$tag" >/dev/null 2>&1 || true
  docker rmi -f "$image_ref" >/dev/null 2>&1 || true
  docker pull --platform "$platform" "$image_ref" >/dev/null 2>&1
  docker tag "$image_ref" "$tag"
  staging="harness-pull"
else
  staging="pre-staged"
fi

case "$platform" in
  linux/amd64) expected_image_arch="amd64" ;;
  linux/arm64) expected_image_arch="arm64" ;;
  *) echo "unsupported platform: $platform" >&2; exit 2 ;;
esac
docker image inspect "$tag" >/dev/null 2>&1 || {
  echo "image tag $tag is absent; pre-stage it or run with CROSSARCH_STAGE=1" >&2
  exit 2
}
actual_image_arch="$(docker image inspect "$tag" --format '{{.Architecture}}')"
[ "$actual_image_arch" = "$expected_image_arch" ] || {
  echo "image tag $tag resolves to $actual_image_arch, expected $expected_image_arch" >&2
  exit 2
}

{
  echo "image-src:            $image_ref"
  echo "manifest-list-digest: $digest"
  echo "staged-tag:           $tag"
  echo "platform:             $platform"
  echo "staging:              $staging"
  echo "image-id:             $(docker image inspect "$tag" --format '{{.Id}}' 2>/dev/null || echo unknown)"
  echo "image-arch:           $(docker image inspect "$tag" --format '{{.Architecture}}' 2>/dev/null || echo unknown)"
  docker run --rm --pull=never "$tag" sh -c '
    echo "container-arch: $(uname -m)"
    echo "python3: $(python3 --version 2>&1)"
    echo "sort: $(sort --version 2>&1 | head -1)"
    echo "wc: $(wc --version 2>&1 | head -1)"
    echo "gzip: $(gzip -V 2>&1 | head -1)"
    echo "sha256sum: $(sha256sum --version 2>&1 | head -1)"
    echo "libc: $(ldd --version 2>&1 | head -1)"
  '
} > "$outdir/container.txt"

for wf in "$@"; do
  # The frozen corpus keys artifacts by the workflow's declared name, not by
  # the file basename (the file names carry a `cross-arch-provenance-` prefix).
  stem="$(awk -F'"' '/^name = "/ {print $2; exit}' "$wf")"
  [ -n "$stem" ] || stem="$(basename "$wf" .oxoflow)"
  work="$outdir/work/$stem"
  rm -rf "$work"
  "$bin" run "$wf" -d "$work" --provenance --quiet
  ckpt="$work/.oxo-flow/checkpoint.json"
  cp "$ckpt" "$outdir/$stem.checkpoint.json"
  "$bin" provenance verify "$outdir/$stem.checkpoint.json" --json \
    > "$outdir/$stem.verify.json" 2>/dev/null
  # Independent hash of every recorded declared output. This is deliberately
  # NOT the checkpoint's own value: the comparison script requires the two to
  # agree, so a corrupted or hand-edited checkpoint cannot pass unnoticed.
  python3 - "$outdir/$stem.checkpoint.json" "$work" "$stem" \
    > "$outdir/$stem.artifacts.sha256" <<'PY'
import hashlib
import json
import os
import sys

ckpt_path, work, stem = sys.argv[1:4]
checksums = json.load(open(ckpt_path)).get("checksums", {})
if not checksums:
    sys.exit("no recorded checksums in %s" % ckpt_path)
for path in sorted(checksums):
    full = os.path.join(work, path)
    with open(full, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()
    print("%s  %s/%s" % (digest, stem, path))
PY
  python3 - "$outdir/$stem.verify.json" > "$outdir/$stem.rows.tsv" <<'PY'
import json
import sys

d = json.load(open(sys.argv[1]))["verify"]
print("file\tstatus\texpected\tactual")
for e in d["entries"]:
    print("%s\t%s\t%s\t%s" % (e["file"], e["status"], e["expected"], e["actual"]))
PY
  mismatched="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["verify"]["mismatched"])' "$outdir/$stem.verify.json")"
  missing="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["verify"]["missing"])' "$outdir/$stem.verify.json")"
  if [ "$mismatched" != "0" ] || [ "$missing" != "0" ]; then
    echo "PROVENANCE MISMATCH on $stem ($platform): mismatched=$mismatched missing=$missing" >&2
    exit 1
  fi
  echo "OK $stem on $platform: $(wc -l < "$outdir/$stem.artifacts.sha256" | tr -d ' ') declared outputs re-hashed"
done

echo "OK arm $platform: all workflows re-verified; evidence in $outdir"
