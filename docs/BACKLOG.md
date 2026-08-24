# Minter Reactive backlog

This file contains verified follow-up work only. Historical failures from old
binaries are not treated as current defects until reproduced on the current
build.

## Priority 1: trustworthy hot-path metrics

Structured `metrics_*.json` must be populated from the same timers used by the
mint engine instead of reconstructing timings from text logs.

Required behaviour:

- record per-wallet `t_send_issued_ms` immediately before the first HTTP send;
- record first ACK, confirmation, endpoint, nonce, attempt count and fanout;
- record run-level `send_issue_spread_ms`, first/median/p90 ACK and confirmation;
- populate auth, nonce, balance, prefetch, pre-sign, wait and completion spans;
- distinguish initial sends, delayed hedges, later waves and RBF retries;
- keep metric collection non-blocking on the T0 path;
- add regression tests proving successful sends do not export zero attempts or
  null ACK/confirmation timings.

Acceptance test: a 10-wallet run can show whether the first network writes were
issued together without manually subtracting unrelated log durations.

## Priority 2: Ink broadcast A/B optimisation

Do not replace the current rate-aware broadcaster blindly. It was introduced
after immediate multi-wallet fanout triggered provider/IP rate limits.

After Priority 1 is complete:

- compare the current rotated lanes and delayed hedges against a denser first
  wave in controlled dry/safe tests;
- measure issue spread, ACK spread, accepted endpoint, 429/rate-limit errors,
  timeouts and first-block inclusion;
- investigate whether a small pre-warmed HTTP/1.1 pool or additional connection
  lanes improves issue time without opening `wallets x endpoints` connections;
- keep identical signed bytes for every hedge/retry so duplicate minting is
  impossible;
- retain a conservative fallback to the current policy;
- adopt a new policy only when repeated measurements beat the current one.

Acceptance test: lower p90 ACK/first-block results for 10 wallets with no higher
rate-limit or unresolved-send rate.

## RPC visibility and routing controls

The Settings/RPC screen must show the complete effective RPC plan, not only
operator-entered URLs. This includes endpoints generated from provider keys and
public fallbacks added by the application.

Required behaviour:

- show every effective endpoint with its origin: custom, settings, provider key,
  or public fallback;
- show the selected network, measured latency, health, and current role: lead,
  broadcast, delayed fallback, unused, or excluded;
- clearly indicate whether the paid Alchemy endpoint will participate in the
  next transaction broadcast;
- allow public fallbacks to be enabled or disabled per network;
- provide a safe speed-first default while retaining a paid provider as a
  delayed reliability hedge where supported;
- never display or log provider API keys;
- use exactly the same resolver and ordering logic as the mint engine so the UI
  cannot disagree with the actual T0 route.

Acceptance test: for Ink with an Alchemy key and no custom URLs, the screen must
show Alchemy plus all automatically added Ink endpoints and must match the roles
written to the mint log.

## Per-chain gas and ordering profiles

- model Robinhood as arrival-order/FCFS and avoid pointless priority-fee
  escalation there;
- provide an explicit, capped competitive priority-fee profile for Ink/OP Stack;
- keep automatic network fees as the safe default and show the estimated maximum
  cost before enabling an aggressive profile;
- never apply a global multiplier blindly to every network;
- verify ordering and fee behaviour against official chain documentation and
  live receipts before changing defaults.

## OpenSea official Drops API as a secondary calldata source

- add optional OpenSea API-key configuration with redaction at rest/log/UI
  boundaries;
- evaluate `POST /api/v2/drops/{slug}/mint` as an independent prefetched source;
- keep deterministic local SeaDrop public calldata as first priority;
- retain the existing authenticated GraphQL path when an exact selected phase or
  server-issued signed-stage data requires it;
- validate returned chain, target, calldata selector, quantity, recipient, phase
  terms and value before signing;
- never let the API's automatic phase selection silently override the operator's
  saved phase or accepted price;
- A/B availability and latency before making the REST route a default.

## Gas-mode semantics and logging cleanup

- remove the misleading `SKIP_PREFLIGHT ignored because GAS_LIMIT=0` warning for
  LIVE runs that are already pre-signed with the safe fixed/default limit;
- make UI `Auto` versus `Manual` gas semantics match the core representation;
- log separately whether estimation happened during PREP, whether the T0 send was
  pre-signed, and whether a late-start safety preflight ran;
- preserve the late-start (2s+) sold-out/closed-phase guard; do not move an
  `eth_estimateGas` call into the exact-T0 path.

## Sweep observability and stale-candidate handling

- write every ETH/NFT sweep to its own persistent timestamped file under `logs`;
- record selected source wallets, destination, chain, contract/filter, discovered
  assets, simulations, submitted hashes, receipts and final ownership;
- when a historical transfer candidate is no longer owned by the source, classify
  it as `SKIPPED_STALE`, not as a failed transfer;
- do not sign or spend gas for stale candidates;
- export a readable summary plus structured JSON/CSV results;
- add an Ink regression test where old incoming transfers coexist with newly
  minted NFTs and only currently owned tokens are swept.

## Explicitly not planned without new evidence

- no return to WebSocket transaction submission;
- no persistent paid-provider `newHeads` subscription;
- no unconditional 30-socket (`wallets x endpoints`) fanout;
- no removal of the final T-2 SeaDrop price/terms check;
- no attempt to reconstruct an allowlist Merkle proof from an on-chain root
  alone;
- no automatic enablement of experimental conditional submission on unsupported
  networks.
