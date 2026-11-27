#!/usr/bin/env bash
# Two-host orchestration for the FROZEN governed cross-architecture campaign
# (#1397 / #1166): native linux/arm64 on the local host + native linux/amd64
# on a remote Linux host, then the exact Clopper-Pearson decision.
#
# The local host must be able to run its native container platform without
# emulation (Apple Silicon Docker Desktop's linux/arm64 VM, or a native Linux
# host). The remote host is the designated Linux x86_64 validation host.
#
# Usage:
#   cross-arch-provenance-governed.sh <outroot> [options]
#
# Options:
#   --repo-root DIR       local git checkout (default: this script's repo)
#   --bin PATH            local oxo-flow binary (default <repo>/target/debug/oxo-flow)
#   --remote-host HOST    ssh host (default: tx-ubuntu)
#   --remote-repo DIR     remote checkout at the same commit
#   --remote-bin PATH     remote binary (default <remote-repo>/target/debug/oxo-flow)
#   --remote-out DIR      remote raw-evidence dir (default /home/ubuntu/crossarch-1397-raw)
#   --expected-commit SHA  frozen engine commit (default: local HEAD)
#   --skip-build          do not run cargo build on either host
#
# Emits <outroot>/arm-local-<arch>/ and <outroot>/arm-remote-<arch>/ raw
# evidence, plus <outroot>/crossarch-results.json and <outroot>/campaign.log.
set -euo pipefail

outroot="${1:?usage: $0 <outroot> [options]}"
shift

repo="$(cd "$(dirname "$0")/.." && pwd)"
bin=""
remote_host="tx-ubuntu"
remote_repo=""
remote_bin=""
remote_out="/home/ubuntu/crossarch-1397-raw"
expected_commit=""
skip_build=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo-root) repo="$2"; shift 2 ;;
    --bin) bin="$2"; shift 2 ;;
    --remote-host) remote_host="$2"; shift 2 ;;
    --remote-repo) remote_repo="$2"; shift 2 ;;
    --remote-bin) remote_bin="$2"; shift 2 ;;
    --remote-out) remote_out="$2"; shift 2 ;;
    --expected-commit) expected_commit="$2"; shift 2 ;;
    --skip-build) skip_build=1; shift ;;
    -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done

repo="$(cd "$repo" && pwd)"
[ -n "$remote_repo" ] || { echo "--remote-repo is required (checkout at the frozen commit)" >&2; exit 2; }
local_commit="$(git -C "$repo" rev-parse HEAD)"
[ -n "$expected_commit" ] || expected_commit="$local_commit"
[ "$local_commit" = "$expected_commit" ] || {
  echo "local HEAD $local_commit != expected $expected_commit" >&2; exit 2;
}
remote_commit="$(ssh "$remote_host" "git -C $(printf %q "$remote_repo") rev-parse HEAD")"
[ "$remote_commit" = "$expected_commit" ] || {
  echo "remote HEAD $remote_commit != expected $expected_commit" >&2; exit 2;
}

corpus="$repo/scripts/cross-arch-provenance-governed-corpus.json"
workflow_files=()
while IFS= read -r wf; do workflow_files+=("$wf"); done < <(python3 -I -c '
import json, sys
c = json.load(open(sys.argv[1]))
for wf in c["workflows"]:
    print(wf["file"])
' "$corpus")
[ "${#workflow_files[@]}" -ge 1 ] || { echo "corpus lists no workflows" >&2; exit 2; }
workflow_paths=()
for wf in "${workflow_files[@]}"; do workflow_paths+=("$repo/$wf"); done

dirty="$(git -C "$repo" status --porcelain -- "${workflow_files[@]}" \
  "scripts/cross-arch-provenance-governed-corpus.json" \
  "scripts/cross-arch-provenance-arm.sh" \
  "scripts/cross-arch-provenance-governed-analyze.py" \
  "scripts/cross-arch-provenance-governed.sh" | wc -l | tr -d ' ')"
[ "$dirty" = "0" ] || {
  echo "frozen campaign files are dirty; commit them before the run:" >&2
  git -C "$repo" status --porcelain -- "${workflow_files[@]}" >&2
  exit 2
}

platform_of() {
  case "$(uname -s):$(uname -m)" in
    Linux:x86_64|Darwin:x86_64) echo "linux/amd64 amd64" ;;
    Linux:aarch64|Linux:arm64|Darwin:arm64) echo "linux/arm64 arm64" ;;
    *) echo "unsupported host: $(uname -s) $(uname -m)" >&2; exit 2 ;;
  esac
}
read -r local_platform local_label <<< "$(platform_of)"
remote_os_arch="$(ssh "$remote_host" "uname -s; uname -m" | paste -sd: -)"
case "$remote_os_arch" in
  Linux:x86_64) remote_platform="linux/amd64"; remote_label="amd64" ;;
  Linux:aarch64|Linux:arm64) remote_platform="linux/arm64"; remote_label="arm64" ;;
  *) echo "unsupported remote host: $remote_os_arch" >&2; exit 2 ;;
esac
[ "$local_platform" != "$remote_platform" ] || {
  echo "both arms resolve to $local_platform; a native cross-architecture contrast needs two platforms" >&2
  exit 2
}
if [ -z "$bin" ]; then bin="$repo/target/debug/oxo-flow"; fi
if [ -z "$remote_bin" ]; then remote_bin="$remote_repo/target/debug/oxo-flow"; fi

mkdir -p "$outroot"
outroot="$(cd "$outroot" && pwd)"
log="$outroot/campaign.log"
{
  echo "campaign: 1397"
  echo "started-utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "local-commit: $local_commit"
  echo "remote-commit: $remote_commit"
  echo "local-platform: $local_platform"
  echo "remote-platform: $remote_platform"
  echo "remote-host: $remote_host"
  echo "corpus: $corpus"
  echo "corpus-sha256: $(python3 -I -c 'import hashlib,sys;print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$corpus")"
} > "$log"

if [ "$skip_build" = "0" ]; then
  echo "building local oxo-flow from $local_commit" | tee -a "$log"
  ( cd "$repo" && cargo build -p oxo-flow-cli )
  echo "building remote oxo-flow from $remote_commit" | tee -a "$log"
  ssh "$remote_host" "cd $(printf %q "$remote_repo") && PATH=\"\$HOME/.cargo/bin:\$PATH\" cargo build -p oxo-flow-cli"
fi
[ -x "$bin" ] || { echo "local binary missing: $bin (build it or pass --bin)" >&2; exit 2; }

read -r image_ref image_digest image_tag <<< "$(python3 -I -c '
import json, sys
c = json.load(open(sys.argv[1]))["image"]
print(c["ref"], c["manifest_list_digest"], c["staged_tag"])
' "$corpus")"
echo "pre-staging local image ($image_ref) for $local_platform" | tee -a "$log"
docker pull --platform "$local_platform" "$image_ref" >/dev/null
docker tag "$image_ref" "$image_tag"
echo "pre-staging remote image ($image_ref) for $remote_platform" | tee -a "$log"
ssh "$remote_host" "docker pull --platform $(printf %q "$remote_platform") $(printf %q "$image_ref") >/dev/null && docker tag $(printf %q "$image_ref") $(printf %q "$image_tag")"
# The arm script verifies the pre-staged tag's architecture fail-closed and
# records `staging: pre-staged` in container.txt; the image identity remains
# the shared manifest-list digest.
export CROSSARCH_STAGE=0

local_arm="$outroot/arm-local-$local_label"
remote_arm="$outroot/arm-remote-$remote_label"
echo "running local arm ($local_platform) -> $local_arm" | tee -a "$log"
bash "$repo/scripts/cross-arch-provenance-arm.sh" \
  "$local_platform" "$local_arm" "$repo" "$bin" "${workflow_paths[@]}" | tee -a "$log"

echo "running remote arm ($remote_platform) on $remote_host -> $remote_out" | tee -a "$log"
remote_cmd="cd $(printf %q "$remote_repo") && CROSSARCH_STAGE=0 bash $(printf %q "$remote_repo/scripts/cross-arch-provenance-arm.sh") $(printf %q "$remote_platform") $(printf %q "$remote_out") $(printf %q "$remote_repo") $(printf %q "$remote_bin")"
for wf in "${workflow_files[@]}"; do
  remote_cmd+=" $(printf %q "$remote_repo/$wf")"
done
ssh "$remote_host" "$remote_cmd" | tee -a "$log"

rm -rf "$remote_arm"
mkdir -p "$remote_arm"
rsync -az "$remote_host:$remote_out/" "$remote_arm/"
echo "copied remote evidence to $remote_arm" | tee -a "$log"

python3 -I "$repo/scripts/cross-arch-provenance-governed-analyze.py" \
  --corpus "$corpus" \
  --arm-a "$local_arm" \
  --arm-b "$remote_arm" \
  --expected-commit "$expected_commit" \
  --out "$outroot/crossarch-results.json" | tee -a "$log"

echo "finished-utc: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$log"
echo "wrote $outroot/crossarch-results.json"
