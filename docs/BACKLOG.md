# Minter Reactive backlog

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
