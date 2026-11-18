#!/usr/bin/env bash
# Desktop shell lifecycle smoke (ledger UX-09, issue #1151).
#
# Proves the wry/tao shell reaches a live embedded server, refuses a second
# instance on the same data directory, and exits cleanly on SIGTERM. It runs
# against an isolated $HOME so the operator's real ~/.oxo-flow/desktop is
# never touched.
#
# Requirements:
#   - a built binary: (cd crates/oxo-flow-desktop && cargo build) or
#     OXO_FLOW_DESKTOP_BIN=/path/to/oxo-flow-desktop
#   - macOS: a logged-in WindowServer session (any local terminal)
#   - Linux: a display; when DISPLAY/WAYLAND_DISPLAY is unset the script
#     starts a private Xvfb (CI installs xvfb + WebKitGTK).
#
# Exit codes: 0 = pass, 1 = lifecycle regression, 77 = environment cannot
# run a GUI (missing display tooling) — a skip, never a silent pass.

set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
BIN=${OXO_FLOW_DESKTOP_BIN:-$ROOT/crates/oxo-flow-desktop/target/debug/oxo-flow-desktop}
START_TIMEOUT_SECS=${OXO_FLOW_DESKTOP_SMOKE_START_TIMEOUT:-60}
EXIT_TIMEOUT_SECS=${OXO_FLOW_DESKTOP_SMOKE_EXIT_TIMEOUT:-30}

fail() {
  echo "desktop-smoke: FAIL: $*" >&2
  if [[ -f "$WORK/desktop.log" ]]; then
    echo "--- desktop log (tail) ---" >&2
    tail -40 "$WORK/desktop.log" >&2 || true
  fi
  exit 1
}

if [[ ! -x "$BIN" ]]; then
  echo "desktop-smoke: $BIN is missing; build it with: (cd crates/oxo-flow-desktop && cargo build)" >&2
  exit 2
fi

WORK=$(mktemp -d "${TMPDIR:-/tmp}/oxo-flow-desktop-smoke.XXXXXX")
XVFB_PID=""
cleanup() {
  if [[ -n "${PID:-}" ]] && kill -0 "$PID" 2>/dev/null; then
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  if [[ -n "$XVFB_PID" ]] && kill -0 "$XVFB_PID" 2>/dev/null; then
    kill -KILL "$XVFB_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# tao/wry need a display. On headless Linux start a private Xvfb (not
# xvfb-run: the SIGTERM must reach the desktop process itself, and
# xvfb-run does not forward it to its child).
if [[ "$(uname -s)" == "Linux" && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  if ! command -v Xvfb >/dev/null 2>&1; then
    echo "desktop-smoke: SKIP: no DISPLAY/WAYLAND_DISPLAY and no Xvfb (install xvfb or run under a desktop session)" >&2
    exit 77
  fi
  DISPLAY_NUM=$((100 + RANDOM % 400))
  Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 >/dev/null 2>&1 &
  XVFB_PID=$!
  export DISPLAY=":$DISPLAY_NUM"
  # WebKitGTK without a GPU: software compositing avoids EGL init failures
  # under Xvfb.
  export WEBKIT_DISABLE_COMPOSITING_MODE=1
  export WEBKIT_DISABLE_DMABUF_RENDERER=1
  export LIBGL_ALWAYS_SOFTWARE=1
  for _ in $(seq 1 50); do
    [[ -S "/tmp/.X11-unix/X$DISPLAY_NUM" ]] && break
    kill -0 "$XVFB_PID" 2>/dev/null || fail "Xvfb failed to start on :$DISPLAY_NUM"
    sleep 0.1
  done
  [[ -S "/tmp/.X11-unix/X$DISPLAY_NUM" ]] || fail "Xvfb did not create a display socket within 5s"
fi

export HOME="$WORK/home"
mkdir -p "$HOME"
DATA_DIR="$HOME/.oxo-flow/desktop"
echo "desktop-smoke: launching $BIN with HOME=$HOME"

"$BIN" >"$WORK/desktop.log" 2>&1 &
PID=$!

# 1. The shell anchors its data directory and starts the embedded server
#    before it builds the window. The lock + SQLite database appearing is the
#    observable side effect of steps 2-3 of the documented lifecycle.
deadline=$((SECONDS + START_TIMEOUT_SECS))
while ((SECONDS < deadline)); do
  if [[ -f "$DATA_DIR/oxo-flow.desktop.lock" && -f "$DATA_DIR/oxo-flow.db" ]]; then
    break
  fi
  kill -0 "$PID" 2>/dev/null || fail "desktop exited before creating its data directory"
  sleep 0.5
done
[[ -f "$DATA_DIR/oxo-flow.desktop.lock" && -f "$DATA_DIR/oxo-flow.db" ]] \
  || fail "desktop did not create its lock + database within ${START_TIMEOUT_SECS}s"

# Give the blocking steps (server readiness wait, webview construction) time
# to fail if the GUI environment is broken — a healthy shell stays alive.
sleep 5
kill -0 "$PID" 2>/dev/null || fail "desktop exited while starting the window"

# 2. Single-instance lock: a second launch on the same data directory must
#    refuse instead of starting a second server against the same SQLite file.
set +e
HOME="$HOME" "$BIN" >"$WORK/second.log" 2>&1
SECOND_CODE=$?
set -e
[[ $SECOND_CODE -ne 0 ]] || fail "second instance exited 0 (should refuse the data-directory lock)"
grep -q "another oxo-flow desktop instance is already running" "$WORK/second.log" \
  || fail "second instance failed without the documented lock message"

# 3. SIGTERM is the documented shutdown path; the shell must exit 0 and leave
#    no orphaned embedded server.
kill -TERM "$PID"
deadline=$((SECONDS + EXIT_TIMEOUT_SECS))
while kill -0 "$PID" 2>/dev/null && ((SECONDS < deadline)); do
  sleep 0.25
done
if kill -0 "$PID" 2>/dev/null; then
  fail "desktop did not exit within ${EXIT_TIMEOUT_SECS}s of SIGTERM"
fi
set +e
wait "$PID"
EXIT_CODE=$?
set -e
PID=""
[[ $EXIT_CODE -eq 0 ]] || fail "desktop exited ${EXIT_CODE} after SIGTERM (expected 0)"

echo "desktop-smoke: PASS: launch (data dir + server), single-instance refusal, clean SIGTERM exit"
