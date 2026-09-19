#!/usr/bin/env bash
# Restart the blake.bike dev app: stop any running dev session (Vite, `tauri dev`,
# and the debug binary), then start `pnpm tauri dev` fresh.
#
# Usage:
#   scripts/restart-dev.sh            # restart in the foreground (Ctrl-C to stop)
#   scripts/restart-dev.sh -b         # restart detached; logs go to .dev.log
#   scripts/restart-dev.sh --stop     # just stop the running dev session
#
# Note: `pnpm tauri dev` already hot-reloads the frontend and rebuilds Rust on
# save. Use this when the dev session is wedged, a port is stuck, or you changed
# something the watcher doesn't pick up (tauri.conf.json, Cargo.toml, env vars).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PORT=1420
LOG_FILE="$REPO_DIR/.dev.log"
BACKGROUND=0
STOP_ONLY=0

for arg in "$@"; do
  case "$arg" in
    -b|--background) BACKGROUND=1 ;;
    --stop) STOP_ONLY=1 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

kill_pids() {
  # $1 = label, $2 = whitespace-separated pid list (may be empty)
  local label="$1" pids="${2:-}"
  [ -z "$pids" ] && return 0
  echo "stopping $label (pids: $pids)"
  # shellcheck disable=SC2086
  kill $pids 2>/dev/null || true
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    # shellcheck disable=SC2086
    if ! kill -0 $pids 2>/dev/null; then return 0; fi
    sleep 0.3
  done
  echo "  still running, sending SIGKILL"
  # shellcheck disable=SC2086
  kill -9 $pids 2>/dev/null || true
}

stop_dev() {
  # Debug app binary produced by `tauri dev`.
  kill_pids "blake.bike debug binary" \
    "$(pgrep -f "$REPO_DIR/src-tauri/target/debug/blakebike" 2>/dev/null | tr '\n' ' ')"

  # The `tauri dev` / cargo watcher processes. Only match real pnpm/node/cargo
  # commands so a shell whose command line merely mentions "tauri dev" (like
  # the one running this script) is never killed.
  kill_pids "tauri dev" \
    "$(pgrep -f '^[^ ]*(pnpm|node|cargo|tauri)[^ ]* .*tauri dev' 2>/dev/null \
       | grep -vx -e "$$" -e "$PPID" | tr '\n' ' ')"

  # Whatever holds the Vite port (tauri requires strictPort on 1420).
  kill_pids "vite on :$PORT" \
    "$(lsof -tiTCP:"$PORT" -sTCP:LISTEN 2>/dev/null | tr '\n' ' ')"
}

cd "$REPO_DIR"

# Resolve pnpm: prefer one on PATH, otherwise go through Corepack (ships with Node).
if command -v pnpm >/dev/null 2>&1; then
  PNPM=(pnpm)
elif command -v corepack >/dev/null 2>&1; then
  PNPM=(corepack pnpm)
else
  cat >&2 <<'MSG'
error: pnpm not found on PATH (and no corepack either).
  Install it with one of:
    corepack enable && corepack prepare pnpm@12.4.2 --activate
    brew install pnpm
    npm install -g pnpm
  If `node` is also missing, this terminal isn't loading your Node install
  (nvm/fnm/Volta) -- check your shell profile.
MSG
  exit 127
fi

stop_dev

if [ "$STOP_ONLY" -eq 1 ]; then
  echo "dev session stopped."
  exit 0
fi

if [ "$BACKGROUND" -eq 1 ]; then
  : > "$LOG_FILE"
  nohup "${PNPM[@]}" tauri dev >>"$LOG_FILE" 2>&1 &
  echo "started pnpm tauri dev in the background (pid $!)"
  echo "logs: tail -f $LOG_FILE"
else
  echo "starting pnpm tauri dev (Ctrl-C to stop)"
  exec "${PNPM[@]}" tauri dev
fi
