# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.2] - 2026-08-15

### Fixed

- Burner generation no longer aborts when one backup folder is refused.
  The plaintext recovery file — written before the encrypted vault, so a
  failed vault rewrite is always recoverable — went to `./imports`, resolved
  against the *working directory*, which on Windows is whatever launched the
  program rather than where the program lives. Three ordinary ways to hit it:
  a shortcut whose working directory is `C:\Windows\System32`; running from a
  still-zipped folder, which Explorer extracts read-only; and Controlled
  Folder Access, which refuses folder creation to unsigned programs in
  Downloads, Documents and the Desktop while still permitting the vault to be
  read — which is why the vault opens and only the backup fails.

  There is now a list of candidate directories and generation moves on when
  one cannot be created or written. The order keeps the existing location
  first, so a setup whose working directory already is the vault's directory
  — the Linux service, and double-clicking the executable in its own folder —
  writes exactly where it always has; the fallbacks (beside the executable,
  then the per-user data directory) engage only after a refusal. The path
  actually used is reported back. When every candidate is refused, the error
  names each one with its reason instead of reporting a single path with none,
  and keys are still never committed to the vault without a backup on disk.

## [0.2.1] - 2026-08-14

### Fixed

- Whitelist mints no longer starve themselves of OpenSea's request budget.
  Measured against the live endpoint: calldata is issued only once a stage is
  open — asking earlier returns `DropNotMintingError`, and that was on a public
  sale the wallet was eligible for, so it is timing alone. The budget is five
  mint-action requests per exit IP, refilling roughly one every four seconds,
  and a *refused* request costs a token exactly like a successful one. The
  pre-fetch loop retried every 500 ms through its whole window — about ten
  doomed requests per wallet in the five seconds before open — so wallets
  reached the fire with `x-ratelimit-remaining: 0` and only those that had
  refilled a token could mint. The pre-fetch for stages that are not open is
  removed rather than tuned; local `mintPublic` calldata is unaffected, being
  built offline at no request cost.
- The wait OpenSea asks for is honoured instead of discarded. `retry-after`
  arrives as fractional seconds (`2.5`) and, on the mint-action query, inside
  an HTTP 200 GraphQL error rather than a 429 header. It was parsed as an
  integer, failed, and was reported as "no wait"; the worker then slept a flat
  100 ms three times and gave up — 300 ms against a server asking for 2.5 s.
  Server-requested waits now survive as milliseconds and draw on their own
  budget, so honouring one no longer consumes the three attempts a genuine
  error gets.
- Wallets that reach T0 unarmed are spaced only against others sharing their
  exit IP, since the budget is per IP. A run now warns when more than five
  wallets share one proxy — the case where the surplus genuinely has to wait.
- Pre-fetch failures report why they failed instead of being swallowed.

## [0.2.0] - 2026-08-14

### Added

- Prebuilt releases for both audiences: a Windows zip to run locally and a
  Linux tarball consumed by the VPS installer. Neither needs a Rust toolchain.
- `deploy/linux/install.sh` — one-command install on a headless server.
  Detects apt/dnf/pacman (and derivatives via `ID_LIKE`), enforces the glibc
  and webkit2gtk 4.1 floors before touching anything, verifies the download
  against its published SHA256, and asks the loader which libraries are
  actually missing rather than trusting a hardcoded package list.
- Documented private SSH/Tailscale tunnelling without an external connector.
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

[Unreleased]: https://github.com/blcksquare7-png/Minter-Reactive-private/commits/reactive
