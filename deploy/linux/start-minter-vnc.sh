#!/usr/bin/env bash
set -Eeuo pipefail

readonly display_number="${MINTER_DISPLAY_NUMBER:-99}"
readonly display=":${display_number}"
readonly screen="${MINTER_SCREEN:-1440x900x24}"
readonly vnc_port="${MINTER_VNC_PORT:-5901}"
readonly bind="${MINTER_NOVNC_BIND:-127.0.0.1:3021}"
readonly binary="${MINTER_BINARY:-/opt/minter/minter-desktop}"
readonly password_file="${MINTER_VNC_PASSWORD_FILE:-/var/lib/minter/.vnc/passwd}"
readonly novnc_web="${MINTER_NOVNC_WEB:-/usr/share/novnc}"

export DISPLAY="$display"
export GDK_BACKEND=x11
export LIBGL_ALWAYS_SOFTWARE=1
export WEBKIT_DISABLE_DMABUF_RENDERER=1
export WEBKIT_DISABLE_COMPOSITING_MODE=1
export NO_AT_BRIDGE=1

children=()

cleanup() {
  trap - EXIT INT TERM
  if ((${#children[@]})); then
    kill "${children[@]}" 2>/dev/null || true
    wait "${children[@]}" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

for required in "$binary" "$password_file" "$novnc_web/vnc.html"; do
  if [[ ! -e "$required" ]]; then
    echo "required path is missing: $required" >&2
    exit 1
  fi
done

Xvfb "$display" -screen 0 "$screen" -nolisten tcp -dpi 96 -noreset &
children+=("$!")

for _ in {1..100}; do
  if xdpyinfo -display "$display" >/dev/null 2>&1; then
    break
  fi
  sleep 0.05
done
if ! xdpyinfo -display "$display" >/dev/null 2>&1; then
  echo "Xvfb did not become ready on $display" >&2
  exit 1
fi

openbox --sm-disable &
children+=("$!")

x11vnc \
  -display "$display" \
  -localhost \
  -rfbport "$vnc_port" \
  -rfbauth "$password_file" \
  -forever \
  -shared \
  -repeat \
  -noxdamage &
children+=("$!")

websockify --web "$novnc_web" "$bind" "127.0.0.1:${vnc_port}" &
children+=("$!")

"$binary" &
children+=("$!")

# Any component exiting makes systemd restart the complete, coherent session.
wait -n "${children[@]}"
