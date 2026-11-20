#!/usr/bin/env bash
# End-to-end test on a real (virtual) X11 desktop: Xvfb + openbox + real xterm windows.
# Usage: scripts/e2e-x11.sh [display-number]   (default :97)
set -euo pipefail
DISPLAY_NUM="${1:-97}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
for tool in Xvfb openbox xterm; do
  command -v "$tool" >/dev/null || { echo "missing $tool (apt install xvfb openbox xterm)"; exit 1; }
done
Xvfb ":$DISPLAY_NUM" -screen 0 1920x1080x24 >/dev/null 2>&1 &
XVFB_PID=$!
cleanup() { kill "${WM_PID:-}" "$XVFB_PID" 2>/dev/null || true; }
trap cleanup EXIT
sleep 1
DISPLAY=":$DISPLAY_NUM" openbox >/dev/null 2>&1 &
WM_PID=$!
sleep 1
cd "$ROOT/src-tauri"
DISPLAY=":$DISPLAY_NUM" REFRAME_E2E_X11=1 cargo test -p reframe-core --test e2e_x11 -- --nocapture --test-threads=1
