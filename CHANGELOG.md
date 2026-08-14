# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-08-14

### Added

- Prebuilt releases for both audiences: a Windows zip to run locally and a
  Linux tarball consumed by the VPS installer. Neither needs a Rust toolchain.
- `deploy/linux/install.sh` — one-command install on a headless server.
  Detects apt/dnf/pacman (and derivatives via `ID_LIKE`), enforces the glibc
  and webkit2gtk 4.1 floors before touching anything, verifies the download
  against its published SHA256, and asks the loader which libraries are
  actually missing rather than trusting a hardcoded package list.
- [minter-connect](https://github.com/MaxBetov-pdd/minter-connect) — a Windows
  script that sets up the SSH key, opens the tunnel and launches the GUI.
- RPC transparency: the resolved endpoint order with per-node ping now reaches
  the mint log, and every broadcast names the endpoint that accepted it. Both
  were already computed and then discarded behind `QUIET=1`.
- Per-endpoint ping on the RPCs page, tagged by origin, so a paid provider is
  distinguishable from the public fallback appended to every chain.
- WL auto-load: the task modal reads back saved eligibility results and
  pre-selects the wallets eligible for the phase the task targets.
- Free-form wallet groups, replacing the fixed A/B/C set.

### Fixed

- A failed broadcast is no longer reported as a hard failure. Timeouts and lost
  responses are checked against the chain, the signed hash is kept, and a run
  ends with a reconciliation pass — a mint that landed can no longer be
  recorded as a loss with nothing to look up.
- A receipt lookup that errors is no longer treated as proof the transaction is
  absent, which could make a retry broadcast a second mint.
- Stop now interrupts queued OpenSea authentication instead of waiting for the
  whole queue to drain.
- The pre-auth OpenSea request goes through the wallet's proxy instead of
  leaving from the machine's own IP.
- Tuple and struct call parameters are no longer split on plain commas.

### Changed

- Cargo package metadata (license, repository, authors) on core and desktop
- SECURITY.md with concrete reporting channels
- README polished for public announcement
- Public-launch hygiene: CONTRIBUTING, Code of Conduct, issue/PR templates

## [0.1.0] - 2026-07-24

### Added

- **minter-core** — shared mint / vault / RPC engine (OpenSea SeaDrop + raw sniper)
- **minter-desktop** — Tauri 2 Windows GUI (tasks, Mission Control, wallets, proxies, raw mint)
- Encrypted vault (AES-256-GCM, PBKDF2 600k, Zeroizing)
- LIVE confirm gate, idle lock, dry-run defaults
- Multi-wallet sticky proxies (OpenSea auth path)
- Private Alchemy multi-chain RPC (user key); hedged reads / fan-out
- Results export (JSON/CSV), run history, full mint logs
- Dual license: MIT OR Apache-2.0
- CI: rustfmt, clippy `-D warnings`, `minter-core` tests

### Security

- Tauri CSP (non-null)
- Session / vault Debug redaction
- Wave A–D hardening (LIVE gate, fee caps, zero-address rejects, OpenSea value checks, etc.)

[Unreleased]: https://github.com/MaxBetov-pdd/Minter-rs-v2/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/MaxBetov-pdd/Minter-rs-v2/releases/tag/v0.2.0
[0.1.0]: https://github.com/MaxBetov-pdd/Minter-rs-v2/releases/tag/v0.1.0
