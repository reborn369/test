#!/usr/bin/env bash
set -Eeuo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "run as root: sudo $0 [vnc-password]" >&2
  exit 1
fi

readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly repo_root="$(cd -- "$script_dir/../.." && pwd)"
readonly built_binary="$repo_root/target/release/minter-desktop"
readonly vnc_password="${1:-}"

if [[ ! -x "$built_binary" ]]; then
  echo "release binary not found: $built_binary" >&2
  echo "build it first with: cargo build -p minter-desktop --release" >&2
  exit 1
fi
if [[ ${#vnc_password} -lt 8 ]]; then
  echo "provide a VNC password of at least 8 characters" >&2
  exit 1
fi

if ! id -u minter >/dev/null 2>&1; then
  useradd --system --home-dir /var/lib/minter --create-home --shell /usr/sbin/nologin minter
fi

install -d -m 0755 /opt/minter
install -d -o minter -g minter -m 0700 /var/lib/minter /var/lib/minter/.vnc
install -d -o minter -g minter -m 0700 /var/lib/minter/imports
install -m 0755 "$built_binary" /opt/minter/minter-desktop
install -m 0755 "$script_dir/start-minter-vnc.sh" /opt/minter/start-minter-vnc.sh
install -m 0644 "$script_dir/minter-vps.service" /etc/systemd/system/minter-vps.service

# Classic VNC authentication uses only the first eight characters. noVNC still
# supplies this as a second gate in addition to the reverse proxy/Tailscale.
x11vnc -storepasswd "${vnc_password:0:8}" /var/lib/minter/.vnc/passwd >/dev/null
chown minter:minter /var/lib/minter/.vnc/passwd
chmod 0600 /var/lib/minter/.vnc/passwd

systemctl daemon-reload
systemctl enable --now minter-vps.service

echo "MINTER installed. Local URL: http://127.0.0.1:3021/vnc.html?autoconnect=true&resize=scale"

