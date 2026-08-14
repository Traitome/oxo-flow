#!/usr/bin/env bash
# deploy-smoke.sh — deployment smoke test suite for oxo-flow-web.
#
# Covers the real deployment shapes, each as an isolated scenario:
#   1. source build, personal mode          — API + SPA
#   2. release-style binary, personal mode  — same assertions
#   3. sub-path mount (--base-path)         — API, SPA injection, assets
#   4. platform config file                 — port/base_path defaults, [[clusters]] seed
#   5. team mode + credentials              — login → session → auth/me
#   6. hpc mode                             — scheduler endpoint responds (available=false without a scheduler)
#   7. desktop app bundle (.app)            — self-contained SPA without a source checkout
#
# Usage:
#   scripts/deploy-smoke.sh                  # auto-detects target/debug binaries
#   OXO_BIN=/path/to/oxo-flow scripts/deploy-smoke.sh   # test any build
#   OXO_APP=/path/to/oxo-flow.app scripts/deploy-smoke.sh # add the .app scenario
#
# Exits non-zero if any assertion fails. Safe to re-run — every scenario
# runs in its own temp dir and kills its own server.

set -u
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Binary discovery: explicit override first, then repo-relative, then PATH.
# (The script may run from anywhere — on a deployed server there is no
# source checkout next to it.)
resolve_bin() { # $1=binary name, $2=repo-relative path
  if [ -x "$REPO_ROOT/$2" ]; then echo "$REPO_ROOT/$2"; return; fi
  command -v "$1" 2>/dev/null || true
}
BIN="${OXO_BIN:-$(resolve_bin oxo-flow target/debug/oxo-flow)}"
WEB="${OXO_WEB:-$(resolve_bin oxo-flow-web target/debug/oxo-flow-web)}"
FRONTEND="${OXO_FRONTEND_DIR:-$REPO_ROOT/crates/oxo-flow-web/static}"
APP_BUNDLE="${OXO_APP:-}"
WORK="$(mktemp -d /tmp/oxo-deploy-smoke.XXXXXX)"
PASS=0; FAIL=0
PORT=9090

ok()   { PASS=$((PASS+1)); printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { FAIL=$((FAIL+1)); printf '  \033[31m✗\033[0m %s\n' "$1"; }
check() { if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (got: $1, want: $2)"; fi; }

next_port() { PORT=$((PORT+1)); echo $PORT; }

start_server() { # $1=logfile, rest=args
  local log="$1"; shift
  "$BIN" serve "$@" > "$log" 2>&1 &
  SRV=$!
  for _ in $(seq 1 40); do
    curl -s -o /dev/null "$BASE/api/runs" && return 0
    sleep 0.25
  done
  return 1
}

cleanup() {
  [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null
  # Kill only servers this script started (by exact binary path) — broad
  # patterns would also match the caller's own ssh session command line.
  [ -n "${BIN:-}" ] && pkill -f "$BIN serve" 2>/dev/null
  [ -n "${WEB:-}" ] && pkill -f "$WEB" 2>/dev/null
}
trap cleanup EXIT

echo "== oxo-flow deployment smoke =="
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  echo "error: oxo-flow binary not found — build it or set OXO_BIN"
  exit 1
fi
echo "binary: $BIN"

# ── 1. Source build, personal mode ──────────────────────────────────────
echo "— 1. personal mode (source build)"
D="$WORK/personal"; mkdir -p "$D"; cd "$D"
P=$(next_port); BASE="http://127.0.0.1:$P"
start_server "$D/srv.log" -p "$P" || { bad "server start"; }
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs")" "200" "API responds"
check "$(curl -s "$BASE/api/health" | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')" "ok" "health ok"
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/")" "200" "SPA served"
cleanup

# ── 2. Release-style binary (web crate binary used standalone) ──────────
echo "— 2. standalone web binary (release-style)"
D="$WORK/release-style"; mkdir -p "$D"; cd "$D"
P=$(next_port); BASE="http://127.0.0.1:$P"
OXO_FLOW_PORT=$P "$WEB" > "$D/srv.log" 2>&1 &
SRV=$!
for _ in $(seq 1 40); do curl -s -o /dev/null "$BASE/api/runs" && break; sleep 0.25; done
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs")" "200" "web binary API responds"
cleanup

# ── 3. Sub-path mount ───────────────────────────────────────────────────
echo "— 3. sub-path mount (/oxoflow)"
D="$WORK/subpath"; mkdir -p "$D"; cd "$D"
P=$(next_port); BASE="http://127.0.0.1:$P/oxoflow"
start_server "$D/srv.log" -p "$P" --base-path /oxoflow
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs")" "200" "API under mount"
check "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$P/api/runs")" "404" "root is NOT mounted"
SPA_HTML=$(curl -s "$BASE/")
case "$SPA_HTML" in *"__OXO_BASE__"*) ok "SPA base injection";; *) bad "SPA base injection";; esac
ASSET=$(printf '%s' "$SPA_HTML" | grep -o 'src="[^"]*\.js"' | head -1 | sed 's/src="//;s/"//')
ASSET="${ASSET#./}"; ASSET="${ASSET#/}"
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/$ASSET")" "200" "asset under mount"
cleanup

# ── 4. Platform config file ─────────────────────────────────────────────
echo "— 4. platform config file (port/base_path defaults + cluster seed)"
D="$WORK/config"; mkdir -p "$D"; cd "$D"
cat > oxo-flow.web.toml << 'TOML'
[server]
port = 9121
base_path = "/oxo"

[[clusters]]
id = "smoke-seeded"
name = "Smoke seeded"
ssh_host = "example.invalid"
ssh_user = "nobody"
scheduler = "pbs"
TOML
"$BIN" serve > "$D/srv.log" 2>&1 &
SRV=$!
BASE="http://127.0.0.1:9121/oxo"
for _ in $(seq 1 40); do curl -s -o /dev/null "$BASE/api/runs" && break; sleep 0.25; done
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs")" "200" "config-file port + base_path applied"
check "$(curl -s "$BASE/api/clusters" | python3 -c 'import json,sys;print("smoke-seeded" in [c["id"] for c in json.load(sys.stdin)])')" "True" "cluster seeded from config file"
cleanup

# ── 5. Team mode + credentials ──────────────────────────────────────────
echo "— 5. team mode + credentials"
D="$WORK/team"; mkdir -p "$D"; cd "$D"
P=$(next_port); BASE="http://127.0.0.1:$P"
OXO_FLOW_ADMIN_PASSWORD="smoke-admin-pw" "$BIN" serve --mode team -p "$P" > "$D/srv.log" 2>&1 &
SRV=$!
for _ in $(seq 1 40); do curl -s -o /dev/null "$BASE/api/runs" && break; sleep 0.25; done
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs")" "401" "team mode requires auth"
TOKEN=$(curl -s -X POST "$BASE/api/auth/login" -H 'Content-Type: application/json' -d '{"username":"admin","password":"smoke-admin-pw"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])' 2>/dev/null)
if [ -n "$TOKEN" ] && [ "$TOKEN" != "None" ]; then
  ok "admin login with env credential"
  ME=$(curl -s "$BASE/api/auth/me" -H "Authorization: Bearer $TOKEN")
  check "$(printf '%s' "$ME" | python3 -c 'import json,sys;print(json.load(sys.stdin)["authenticated"])')" "True" "session authenticates"
  check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/runs" -H "Authorization: Bearer $TOKEN")" "200" "authenticated request allowed"
else
  bad "admin login with env credential"
fi
cleanup

# ── 6. HPC mode ─────────────────────────────────────────────────────────
echo "— 6. hpc mode (scheduler endpoint)"
D="$WORK/hpc"; mkdir -p "$D"; cd "$D"
P=$(next_port); BASE="http://127.0.0.1:$P"
start_server "$D/srv.log" --mode hpc -p "$P"
check "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/hpc")" "200" "hpc endpoint responds"
HPC=$(curl -s "$BASE/api/hpc")
case "$HPC" in *'"available"'*) ok "hpc status structured";; *) bad "hpc status structured: $HPC";; esac
cleanup

# ── 7. Desktop bundle (self-contained) ──────────────────────────────────
if [ -n "$APP_BUNDLE" ] && [ -x "$APP_BUNDLE/Contents/MacOS/oxo-flow" ]; then
  echo "— 7. desktop .app bundle"
  D="$WORK/app"; mkdir -p "$D"; cd "$D"
  P=$(next_port); BASE="http://127.0.0.1:$P"
  "$APP_BUNDLE/Contents/MacOS/oxo-flow" serve -p "$P" > "$D/srv.log" 2>&1 &
  SRV=$!
  for _ in $(seq 1 40); do curl -s -o /dev/null "$BASE/api/runs" && break; sleep 0.25; done
  SPA=$(curl -s -o /dev/null -w '%{http_code} %{size_download}' "$BASE/")
  case "$SPA" in "200 5"*|"200 6"*) ok "bundled SPA self-contained ($SPA)";; *) bad "bundled SPA self-contained ($SPA)";; esac
  cleanup
else
  echo "— 7. desktop .app bundle: skipped (set OXO_APP=/path/to/oxo-flow.app)"
fi

echo ""
echo "RESULT: $PASS passed, $FAIL failed"
rm -rf "$WORK"
[ "$FAIL" -eq 0 ]
