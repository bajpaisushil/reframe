#!/usr/bin/env bash
# Runs reframe-platform's X11 integration tests (tests/x11_integration.rs) against a private,
# headless X server:
#   - Xvfb :91, 2560x1080, split into two RandR monitors with `xrandr --setmonitor`:
#     LEFT 1280x1080+0+0 (primary) and RIGHT 1280x1080+1280+0
#   - Xft.dpi 120 in the resource database (scale factor 1.25)
#   - openbox as the (EWMH) window manager
# Display :92 is handed to a test that starts, kills and restarts its own bare Xvfb there.
# Two tests are then re-run in simulated Wayland sessions (with and without XWayland).
#
# Usage: bash crates/reframe-platform/tests/run-x11-tests.sh [extra cargo test args]
# Override the displays with REFRAME_X11_DISPLAY / REFRAME_X11_SECONDARY (numbers only).
# The script only ever stops processes it started itself.
set -euo pipefail

display_num="${REFRAME_X11_DISPLAY:-91}"
secondary_num="${REFRAME_X11_SECONDARY:-92}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src_tauri="$(cd "$here/../../.." && pwd)"
log_dir="$(mktemp -d "${TMPDIR:-/tmp}/reframe-x11-tests.XXXXXX")"
pids=()

cleanup() {
  local status=$?
  local i
  # Stop in reverse start order (window manager before the X server).
  for ((i = ${#pids[@]} - 1; i >= 0; i--)); do
    kill -TERM "${pids[$i]}" 2>/dev/null || true
  done
  for ((i = ${#pids[@]} - 1; i >= 0; i--)); do
    wait "${pids[$i]}" 2>/dev/null || true
  done
  if [ "$status" -eq 0 ]; then
    rm -rf "$log_dir"
  else
    echo "X11 integration tests failed (exit $status); server logs kept in $log_dir" >&2
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

command -v cargo >/dev/null 2>&1 || export PATH="$HOME/.cargo/bin:$PATH"
for tool in cargo Xvfb xrandr xrdb openbox xterm xdpyinfo xprop xwininfo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "required tool not found: $tool" >&2
    exit 2
  fi
done

for n in "$display_num" "$secondary_num"; do
  if [ -e "/tmp/.X11-unix/X$n" ] || [ -e "/tmp/.X$n-lock" ]; then
    echo "display :$n is already in use; refusing to touch another X server" >&2
    exit 2
  fi
done

wait_for() {
  # wait_for <tries> <command...>: retry every 0.1 s.
  local tries=$1
  shift
  local attempt
  for ((attempt = 0; attempt < tries; attempt++)); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}

echo "starting Xvfb :$display_num"
Xvfb ":$display_num" -screen 0 2560x1080x24 -nolisten tcp -noreset >"$log_dir/xvfb.log" 2>&1 &
pids+=($!)
export DISPLAY=":$display_num"
if ! wait_for 100 xdpyinfo; then
  echo "Xvfb :$display_num did not start" >&2
  exit 1
fi

# Two logical monitors; LEFT takes over Xvfb's only output ("screen") so the automatic
# full-screen monitor disappears.
xrandr --setmonitor '*LEFT' 1280/338x1080/286+0+0 screen >/dev/null
xrandr --setmonitor RIGHT 1280/338x1080/286+1280+0 none >/dev/null
xrandr --listmonitors

echo "Xft.dpi: 120" | xrdb -nocpp -merge

echo "starting openbox"
openbox --sm-disable >"$log_dir/openbox.log" 2>&1 &
pids+=($!)
wm_ready() { xprop -root _NET_SUPPORTING_WM_CHECK | grep -q "window id"; }
if ! wait_for 100 wm_ready; then
  echo "openbox did not start" >&2
  exit 1
fi

# A plain X11 session, whatever session this script was started from.
unset WAYLAND_DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP DESKTOP_SESSION

cd "$src_tauri"
echo "== X11 session"
REFRAME_X11_TESTS=1 REFRAME_X11_SECONDARY_DISPLAY=":$secondary_num" \
  cargo test -p reframe-platform --test x11_integration -- --test-threads=1 "$@"

# Session detection reads the environment: simulate Wayland sessions around the same X server
# (WAYLAND_DISPLAY only has to be set; nothing connects to it).
echo "== simulated Wayland session with XWayland"
XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY=reframe-test-wayland REFRAME_X11_TESTS=1 \
  REFRAME_SESSION_MODE=xwayland \
  cargo test -p reframe-platform --test x11_integration -- --test-threads=1 session_xwayland "$@"

echo "== simulated Wayland session without X"
env -u DISPLAY XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY=reframe-test-wayland \
  REFRAME_X11_TESTS=1 REFRAME_SESSION_MODE=wayland \
  cargo test -p reframe-platform --test x11_integration -- --test-threads=1 session_wayland_only "$@"
