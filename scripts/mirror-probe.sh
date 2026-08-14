#!/bin/bash
# China-network mirror reachability probe (issue #67 §5).
#
# Two-round curl check of the package/image mirrors oxo-flow users rely on:
# bioconda / conda-forge / PyPI channels, the Rust toolchain index, and
# Docker registry mirrors. Docker endpoints use the /v2/ challenge
# (401 = reachable, requires auth; 000 = unreachable; 403 = blocked).
#
# Interpretation caveat: results reflect THIS machine's real egress — a
# TUN-proxy (fake-IP) egress can make geo-gated mirrors (Aliyun) report 403
# even from a China network. Re-run on the target machine before writing
# mirror configs.
#
# Usage: bash scripts/mirror-probe.sh
# See also: docs/guide/src/reference/china-mirrors.md

set -u

probe() {
  local name="$1" url="$2"
  local out code time ip
  out=$(curl -s -o /dev/null -m 10 -w "%{http_code} %{time_total}s %{remote_ip}" "$url" 2>&1)
  code=${out%% *}
  time=$(echo "$out" | awk '{print $2}')
  ip=$(echo "$out" | awk '{print $3}')
  case "$code" in
    000) verdict="UNREACHABLE" ;;
    401) verdict="OK(auth-challenge)" ;;
    200|301|302|403|404) verdict="OK(http-$code)" ;;
    *) verdict="http-$code" ;;
  esac
  printf "%-42s %-20s %-8s %s\n" "$name" "$verdict" "$time" "$ip"
}

echo "=== bioconda channels (repodata.json) ==="
probe "TUNA bioconda"      "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/bioconda/noarch/repodata.json"
probe "USTC bioconda"      "https://mirrors.ustc.edu.cn/anaconda/cloud/bioconda/noarch/repodata.json"
probe "Aliyun bioconda"    "https://mirrors.aliyun.com/anaconda/cloud/bioconda/noarch/repodata.json"
probe "Tencent bioconda"   "https://mirrors.cloud.tencent.com/anaconda/cloud/bioconda/noarch/repodata.json"

echo "=== conda-forge (pixi [mirrors] targets) ==="
probe "TUNA conda-forge"   "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge/noarch/repodata.json"
probe "USTC conda-forge"   "https://mirrors.ustc.edu.cn/anaconda/cloud/conda-forge/noarch/repodata.json"

echo "=== pypi (pixi [pypi-config] targets) ==="
probe "TUNA pypi"          "https://pypi.tuna.tsinghua.edu.cn/simple/"
probe "USTC pypi"          "https://mirrors.ustc.edu.cn/pypi/web/simple/"
probe "Aliyun pypi"        "https://mirrors.aliyun.com/pypi/simple/"

echo "=== rust / crates ==="
probe "rsproxy.cn"         "https://rsproxy.cn/"
probe "rsproxy sparse idx" "https://rsproxy.cn/index/config.json"
probe "static.rust-lang"   "https://static.rust-lang.org/dist/channel-rust-stable.toml.sha256"
probe "crates.io API"      "https://crates.io/api/v1/crates/serde"

echo "=== docker registry mirrors (/v2/ challenge: 401=reachable) ==="
probe "USTC docker"        "https://docker.mirrors.ustc.edu.cn/v2/"
probe "Tencent docker"     "https://mirror.ccs.tencentyun.com/v2/"
probe "TUNA docker"        "https://docker.mirrors.tuna.tsinghua.edu.cn/v2/"
probe "1ms.run docker"     "https://docker.1ms.run/v2/"

echo "=== controls ==="
probe "github.com"         "https://github.com"
probe "docker hub"         "https://registry-1.docker.io/v2/"
