# MINTER frontend compatibility contract

The files in `demo/` are an autonomous visual prototype. They must **not** replace
`crates/minter-desktop/ui` as a bundle. Production integration is a component-by-component
visual port that retains the existing DOM IDs, Tauri module bridge, event listeners,
confirmation protocol, persisted state and i18n keys.

## Non-negotiable integration rules

1. Keep `window.__TAURI__.core.invoke` and every registered command payload unchanged.
2. Keep the production page nodes and stable IDs; restyle/reorder their containers instead
   of rendering route bodies with `innerHTML` as the demo does.
3. Keep live operations behind `begin_confirmation`. The typed word is only the operator
   gate; live calls also need the backend-issued `confirmationId` and current wallet context.
4. Report mint success only from a confirmed receipt. A submitted transaction is not success.
5. Keep `mint-event`, `mint-first-confirm`, `mint-reauth` and `batch-event` listeners mounted
   while their operations run, even when the visible page changes.
6. Keep vault data local, private keys masked, telemetry absent and burner acknowledgement
   ahead of unlock.
7. Keep EN/RU strings in `i18n.js`; the prototype's English copy is not a replacement catalog.
8. Namespace any new visual primitives during migration. Do not append the prototype's broad
   `.panel`, `.sidebar`, `.nav-item`, `.data-table` or `.toast` rules beside production rules.
9. Keep read-only preview traffic on the public RPC registry. Configured/custom RPCs remain
   reserved for forced funding verification and critical execution paths.

## Screen mapping

| Prototype route | Production page / contract | Required integration behavior |
| --- | --- | --- |
| `overview` | `home` plus persistent Mission Control | `get_status`, gas snapshot, wallet/proxy/RPC counts and run history replace every mock metric. |
| `tasks` | `tasks` and `task-modal` | Preserve phase discovery, cost quote with ready/insufficient/unknown wallet states, WL filtering, per-wallet quantity, auth warm-up, persistence, start/stop and streamed progress. |
| `raw` | `raw` | Preserve ABI discovery/probe, presets, Custom signature and params, scheduled sniper timing, wallet funding filter, gas policy, dry run, Flashbots, stop, results and log. |
| `wallets` | `wallets` | Preserve virtualized list, groups, proxy routes, public-RPC native balance refresh, generate/import/remove and bulk copy/sweep/delete. Wallet rows no longer expose aggregate NFT counts. |
| `disperse` | `disperse` | Preserve source/recipient selection, address-file import, public-RPC quote, dry run, server confirmation, result and log. A failed fee preview must clear totals instead of fabricating a fallback quote. |
| `sweep` | `sweep` | Preserve separate native-token and NFT flows, source selection, destination, contract, dry run, stop, results and logs. |
| `rpcs` | `rpcs` | Preserve network probe, RPC URL probe, warm/deep latency, proxy routing and chain selection. Probe health is evidence only and must not reorder configured execution RPCs. |
| `proxies` | `proxies` | Preserve masked editor, explicit reveal, file import, save and health probe. Never render credentials in tables or toasts. |
| `wl` | `wl` | Preserve OpenSea slug, selected wallets, concurrency, incremental `batch-event` rows, cancellation and raw detail. |
| `multicall` | `multicall` | Preserve dynamic calls, target/function/params or calldata/value/allow-fail, override address, dry run, confirmation and output. |
| `history` | `nfts` | Preserve disk-backed run history, per-wallet outcomes and results/log-folder actions. |
| `settings` | `settings` | Preserve all RPC, Flashbots, gas, retry, export, audio, dry-run, confirmation, quiet-log, idle-lock and fee-refresh settings. |
| vault profile | `unlock`, onboarding, idle lock | Preserve burner acknowledgement, password errors, local lock state and activity-based auto-lock. |

## Tauri command inventory

- Vault/status: `accept_burner`, `unlock`, `note_activity`, `get_status`, `security_status`, `app_version`,
  `begin_confirmation`, `should_warn_no_proxy`, `no_proxy_warn_message`.
- Wallets/files: `list_wallets`, `add_key`, `generate_burners`, `import_file`, `import_files`,
  `import_keys_text`, `remove_wallet`, `pick_file`, `pick_files`, `read_text_file`,
  `load_wallet_meta`, `save_wallet_meta`, `wallet_balances`.
- Settings/network/proxies: `get_settings`, `save_settings`, `list_proxies`, `probe_networks`,
  `probe_rpc`, `warm_rpc_latency`, `measure_latency`, `measure_fire_lag`, `network_fee_snapshot`,
  `apply_sniper`.
- Minting: `load_tasks`, `save_tasks`, `list_drop_phases`, `load_wl_for_slug`, `run_mint`,
  `mint_running`, `cancel_mint`, `mint_cost_quote`, `raw_mint`, `raw_sniper`, `probe_raw`,
  `discover_raw_functions`, `warm_auth`, `test_auth`, `clear_auth_cache`.
- Operations: `disperse_quote`, `disperse`, `multicall`, `sweep_eth`, `sweep_nfts`,
  `check_eligibility_wallets`, `cancel_batch`.
- History/system: `load_runs_history`, `save_runs_history`, `open_results_folder`,
  `open_logs_folder`, and Tauri Shell's `plugin:shell|open`.

## Migration sequence

1. Port shell, tokens, navigation and top bar while production pages remain intact.
2. Port unlock, Overview and read-only status surfaces using real backend state.
3. Port Wallets and Network pages without changing IDs or persistence.
4. Port Tasks and Raw Mint, then verify every event phase and cancellation path.
5. Port Disperse, Sweep and Multicall with server confirmation and dry-run coverage.
6. Port WL, Proxies, History and Settings; finish EN/RU and keyboard/accessibility checks.
7. Run the existing UI audit plus focused Rust tests and real Tauri smoke tests page by page.

This order deliberately puts visual shell work before operational rewiring and high-risk
fund-moving flows last. At no point should mock values or prototype handlers enter production.
