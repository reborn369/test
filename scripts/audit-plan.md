# Reactive audit / 2026-09-05

Implemented: fresh per-wallet eligibility, phase-switch selection, stale collection
response guards, explicit negative eligibility guard during existing preparation,
shared read-only preview caches (fees/balances 15s, currency prices 300s), coalesced
history lookup (600s) filtered by public/signed transaction selector and quantity.
No new RPC calls were inserted on the mint broadcast path.

UI regression suite: `node --test scripts/audit-ui.cjs` (mocked RPC/DOM, production
functions and actual phase-load callback). Core tests and clippy are required too.
GitHub Release now runs UI and core tests before packaging.

Arc: explicitly Arc Testnet, chain ID 5042002, native USDC, 18-decimal native RPC
amounts. This is NOT Arc mainnet and NOT a promise of OpenSea drop support.
Official reference: https://docs.arc.io/arc/references/connect-to-arc
Live read-only RPC smoke check returned HTTP/Cloudflare error 1009 from this
machine. No live Arc mint, transfer or sweep has been verified. A reachable RPC
and the intended collection/platform are required before treating Arc as ready.
Testnet balances/fees have no real-dollar spending value.

No VPS deployment. No live test transactions. Existing installed executable is
unchanged until a successful GitHub artifact is downloaded and verified.
