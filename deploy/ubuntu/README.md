# Ubuntu VPS deployment

This deployment runs the unchanged Tauri application inside an isolated Xvfb
desktop and exposes that desktop through noVNC on loopback port `3021`. The
mint engine and Tauri IPC stay identical to the Windows application.

Required Ubuntu packages:

```bash
sudo apt-get install build-essential pkg-config curl ca-certificates git file wget \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf \
  xvfb xauth x11-utils x11vnc openbox dbus-x11 novnc websockify \
  xdg-utils xdg-desktop-portal xdg-desktop-portal-gtk \
  pcmanfm fonts-dejavu-core
```

Build and install:

```bash
cargo build -p minter-desktop --release
sudo ./deploy/ubuntu/install-built.sh 'eight-or-more-characters'
```

The VPS needs **no desktop environment** — Xvfb provides a virtual display, so a
plain terminal-only Ubuntu server is the expected target.

## Reaching the GUI

The service listens on `127.0.0.1:3021` only. That is deliberate and must stay
that way: the noVNC layer is protected by a classic VNC password, and classic
VNC auth uses **only the first 8 characters** — see `x11vnc -storepasswd` in
`install-built.sh`. That is not a credential you can put on the open Internet in
front of a wallet GUI. Always reach it through a tunnel.

### Option 1 — SSH tunnel (nothing extra to install)

Windows 10/11, macOS and Linux all ship an SSH client. From your own machine:

```bash
ssh -f -N -L 3021:127.0.0.1:3021 -o ExitOnForwardFailure=yes user@YOUR_VPS_IP
```

Then open <http://127.0.0.1:3021/vnc.html?autoconnect=1&resize=scale>.

Nothing is exposed publicly: the port stays bound to loopback on both ends.
If the page does not load, check the tunnel first (`Get-NetTCPConnection
-LocalPort 3021` on Windows, `ss -ltn | grep 3021` elsewhere) before touching
the service.

### Option 2 — Tailscale (easiest for non-technical operators)

Install Tailscale on the VPS and on your own machine, log both into the same
account, then browse to `http://<vps-tailscale-name>:3021/vnc.html`. To have
Tailscale terminate TLS for you:

```bash
sudo tailscale serve --bg 3021
```

Still no public ports: the tailnet is private to your account.

### Do not

Publishing port 3021 with a reverse proxy and no real authentication puts an
unlocked-wallet GUI one weak password away from anyone scanning the Internet.
If you need browser access without a VPN client, put an authenticating proxy in
front of it (Cloudflare Access, oauth2-proxy, or similar) — never bare noVNC.

Runtime files are under `/var/lib/minter`: `keys.vault`, `config.json`, tasks,
auth cache, results, and logs. Files copied to `/var/lib/minter/imports` can be
selected from the application's native server-side file picker.

Operations:

```bash
sudo systemctl status minter-vps
sudo journalctl -u minter-vps -f
sudo systemctl restart minter-vps
```
