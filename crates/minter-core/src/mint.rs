//! OpenSea mint orchestration (auth, phase, calldata, send, RBF).
//! Shared by CLI and desktop via `MintReporter` + `MintOptions`.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use anyhow::{Context, Result, bail};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::api::{MintOptions, collect_rpc_urls_for_chain, parse_collection_slug};
use crate::auth_cache;
use crate::export;
use crate::flashbots::{self, BundleTx, FlashbotsClient, FlashbotsConfig, MAINNET_CHAIN_ID};
use crate::gas;
use crate::opensea;
use crate::progress::{FileTeeReporter, MintEvent, MintReporter};
use crate::proxy::ProxyManager;
use crate::rpc;
use crate::sign;
use crate::types::*;

fn report_msg(reporter: &dyn MintReporter, quiet: bool, msg: impl Into<String>) {
    if !quiet {
        reporter.report(MintEvent::message(msg));
    }
}

fn log_always(reporter: &dyn MintReporter, msg: impl Into<String>) {
    reporter.report(MintEvent::message(msg));
}

fn report_phase(reporter: &dyn MintReporter, phase: &str, label: impl Into<String>) {
    let label = label.into();
    reporter.report(MintEvent::phase(phase, label));
}

fn report_wallet(
    reporter: &dyn MintReporter,
    address: &Address,
    status: Option<WalletStatus>,
    detail: Option<String>,
    tx_hash: Option<B256>,
    error: Option<String>,
) {
    reporter.report(MintEvent::wallet(*address, status, detail, tx_hash, error));
}

fn mint_log(reporter: &dyn MintReporter, quiet: bool, msg: impl AsRef<str>) {
    report_msg(reporter, quiet, msg.as_ref().to_string());
}

/// Summary returned after a mint run (for UI / export).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintRunSummary {
    pub slug: String,
    pub chain: String,
    pub phase: String,
    pub dry_run: bool,
    pub elapsed_ms: u64,
    #[serde(skip)]
    pub results: Vec<MintResult>,
    pub confirmed: usize,
    pub failed: usize,
    pub export_json: Option<String>,
    pub export_csv: Option<String>,
    /// Per-wallet rows for UI table (address, status, tx, error).
    pub wallets: Vec<crate::api::SweepResultRow>,
}

fn maybe_beep(beep: bool, first_confirm: &AtomicBool) {
    if !beep {
        return;
    }
    if first_confirm
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        // CLI/terminal only. Desktop plays a real system chime in TauriMintReporter.
        print!("\x07");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

// Error classification lives in one place (`crate::errors`) so retry / RBF
// decisions stay consistent across providers and are unit-tested there.
pub(crate) use crate::errors::classify_mint_error;
use crate::errors::{is_already_known, is_nonce_too_low, is_underpriced};

/// SeaDrop `NotActive(uint256 current, uint256 start, uint256 end)` — selector `0x13da22f2`.
pub(crate) const NOT_ACTIVE_SELECTOR: &str = "13da22f2";

/// Seconds after wall-clock phase open where chain `block.timestamp` often still
/// lags (~1 L1 block). Used for auto skip-estimate and NotActive → fixed gas.
pub(crate) const PHASE_OPEN_LAG_WINDOW_SECS: i64 = 25;

/// Max chain lag (seconds) decoded from NotActive still treated as "about to open".
pub(crate) const NOT_ACTIVE_CHAIN_WAIT_MAX_SECS: u64 = 20;

/// A reverted transaction may be retried with a fresh nonce only when its
/// mined block itself proves that SeaDrop had not opened yet.  Keep this
/// separate from the general retry count: a bad contract call must not turn
/// `MAX_RETRIES=20` into twenty paid reverts.
pub(crate) const MAX_PROVEN_EARLY_REVERT_RECOVERIES: u32 = 1;

/// Mutable public SeaDrop configuration is finalized well before the precision
/// window. Server-signed stages need no blockchain reads in this window.
pub(crate) const PREOPEN_PUBLIC_FINALIZE_LEAD_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NotActiveInfo {
    pub chain_ts: u64,
    pub start_ts: u64,
    pub end_ts: u64,
}

pub(crate) fn is_proven_pre_open_revert(stage_start_ts: Option<i64>, mined_block_ts: u64) -> bool {
    let Some(start) = stage_start_ts.filter(|start| *start > 0) else {
        return false;
    };
    let start = start as u64;
    mined_block_ts < start && start.saturating_sub(mined_block_ts) <= NOT_ACTIVE_CHAIN_WAIT_MAX_SECS
}

pub(crate) fn validate_wallet_subset_counts(
    requested_entries: usize,
    requested_unique: usize,
    matched_vault_wallets: usize,
) -> Result<()> {
    if requested_entries != requested_unique {
        bail!(
            "Selected wallet set has {requested_entries} entries but {requested_unique} unique addresses; refusing partial/duplicate mint"
        );
    }
    if matched_vault_wallets != requested_unique {
        bail!(
            "Selected wallet set mismatch: requested {requested_unique}, found {matched_vault_wallets} in unlocked vault; refusing partial mint"
        );
    }
    Ok(())
}

fn format_unix_hms(ts: u64) -> String {
    chrono::DateTime::from_timestamp(ts as i64, 0)
        .map(|d| d.format("%H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

fn parse_abi_u64_word(word64_hex: &str) -> Option<u64> {
    let w = word64_hex.trim();
    if w.len() != 64 {
        return None;
    }
    // Timestamps fit in u64 — take low 16 hex chars (8 bytes).
    u64::from_str_radix(&w[w.len() - 16..], 16).ok()
}

/// Parse SeaDrop `NotActive` from an RPC / estimate error string (hex `data` blob).
pub(crate) fn parse_not_active(err: &str) -> Option<NotActiveInfo> {
    let lower = err.to_ascii_lowercase();
    let idx = lower.find(NOT_ACTIVE_SELECTOR)?;
    // Collect hex digits after the selector (ABI: 3 × 32-byte words = 192 nibbles).
    // JSON may interleave quotes/spaces — strip non-hex.
    let hex: String = lower[idx + NOT_ACTIVE_SELECTOR.len()..]
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(192)
        .collect();
    if hex.len() < 192 {
        return None;
    }
    let chain_ts = parse_abi_u64_word(&hex[0..64])?;
    let start_ts = parse_abi_u64_word(&hex[64..128])?;
    let end_ts = parse_abi_u64_word(&hex[128..192])?;
    Some(NotActiveInfo {
        chain_ts,
        start_ts,
        end_ts,
    })
}

pub(crate) fn format_not_active(info: &NotActiveInfo) -> String {
    let wait = info.start_ts.saturating_sub(info.chain_ts);
    format!(
        "NotActive: chain_ts={} ({}) start={} ({}) end={} ({}) (wait ~{}s)",
        info.chain_ts,
        format_unix_hms(info.chain_ts),
        info.start_ts,
        format_unix_hms(info.start_ts),
        info.end_ts,
        format_unix_hms(info.end_ts),
        wait
    )
}

/// Prefer human-readable NotActive decode; keep a short raw tail for debugging.
pub(crate) fn enrich_mint_rpc_error(err: &str) -> String {
    if let Some(info) = parse_not_active(err) {
        let decoded = format_not_active(&info);
        // Keep message compact — full multi-RPC dump is huge.
        let raw = err.trim();
        let short = if raw.len() > 160 {
            format!("{}…", raw.chars().take(160).collect::<String>())
        } else {
            raw.to_string()
        };
        format!("{decoded} | {short}")
    } else if let Some(decoded) = decode_common_seadrop_revert(err) {
        decoded
    } else {
        err.to_string()
    }
}

fn decode_common_seadrop_revert(err: &str) -> Option<String> {
    let lower = err.to_ascii_lowercase();
    if lower.contains("f477d26f") {
        return Some(
            "FeeRecipientNotAllowed: fee recipient is not approved by this collection".into(),
        );
    }
    let selector = "e12d2314";
    let index = lower.find(selector)?;
    let args: String = lower[index + selector.len()..]
        .chars()
        .filter(char::is_ascii_hexdigit)
        .take(128)
        .collect();
    if args.len() < 128 {
        return Some("MintQuantityExceedsMaxSupply".into());
    }
    let total = U256::from_str_radix(&args[..64], 16).ok()?;
    let max_supply = U256::from_str_radix(&args[64..128], 16).ok()?;
    Some(format!(
        "MintQuantityExceedsMaxSupply: requested total {total}, collection max supply {max_supply}"
    ))
}

/// Wall clock is at/after phase start and still inside typical L1 timestamp lag.
pub(crate) fn in_phase_open_lag_window(stage_start_ts: Option<i64>, now_wall: i64) -> bool {
    let Some(start) = stage_start_ts.filter(|s| *s > 0) else {
        return false;
    };
    let elapsed = now_wall - start;
    // Allow 1s clock skew early; window covers ~2 eth blocks of lag.
    (-1..=PHASE_OPEN_LAG_WINDOW_SECS).contains(&elapsed)
}

/// Policy after eth_estimateGas failure: always restore calldata; maybe force fixed gas.
///
/// Returns `(enriched_error, force_fixed_gas, wait_ms_override)`.
/// `wait_ms_override = 0` → caller uses normal burst delays.
///
/// Force fixed-gas send only when the stage is **not yet open on chain**
/// (`chain_ts < start`) with a small wait, or wall clock is still inside the
/// post-open lag window — never when `chain_ts` is already past `end`.
pub(crate) fn estimate_fail_policy(
    err: &str,
    stage_start_ts: Option<i64>,
    now_wall: i64,
) -> (String, bool, u64) {
    let enriched = enrich_mint_rpc_error(err);
    let info = parse_not_active(err);
    let force = if let Some(info) = info {
        let not_yet_open = info.chain_ts < info.start_ts;
        let still_before_end = info.end_ts == 0 || info.chain_ts < info.end_ts;
        let wait_s = info.start_ts.saturating_sub(info.chain_ts);
        still_before_end
            && ((not_yet_open && wait_s <= NOT_ACTIVE_CHAIN_WAIT_MAX_SECS)
                || in_phase_open_lag_window(stage_start_ts, now_wall))
    } else {
        let lower = err.to_ascii_lowercase();
        lower.contains("notactive") && in_phase_open_lag_window(stage_start_ts, now_wall)
    };
    let wait_ms = if force {
        if let Some(info) = info {
            let wait_s = info.start_ts.saturating_sub(info.chain_ts);
            (wait_s.saturating_mul(250)).clamp(150, 2_000)
        } else {
            300
        }
    } else {
        0
    };
    (enriched, force, wait_ms)
}

fn parse_hex_u256(value: &str) -> Option<U256> {
    if let Some(hex) = value.strip_prefix("0x") {
        U256::from_str_radix(hex, 16).ok()
    } else {
        U256::from_str_radix(value, 10).ok()
    }
}

struct WalletAuth {
    address: alloy_primitives::Address,
    signer: Signer,
    session: Option<opensea::AuthSession>,
    auth_ok: bool,
    /// Cold SIWE duration. `None` means the encrypted auth cache was used or
    /// the request could not start.
    auth_elapsed_ms: Option<u64>,
    auth_cached: bool,
    nonce: u64,
    prefetched_tx: Option<PrefetchedMintTx>,
    /// Fully signed whitelist transaction prepared before the scheduled fire.
    /// When present, the worker bypasses GQL/build/sign/log work and broadcasts
    /// this raw transaction as its very first operation after T0.
    pre_signed_tx: Option<PreSignedMintTx>,
    /// Hash accepted by experimental conditional submission before T0. The
    /// normal T0 broadcast still sends the identical raw transaction.
    conditional_hash: Option<B256>,
    /// Proxy assigned at auth time (signer index). Must not be re-derived from
    /// `wallets` order — cache vs join reorders the vec.
    proxy_url: Option<String>,
}

fn assigned_proxy_routes(
    signers: &[Signer],
    proxies: &ProxyManager,
    direct_addresses: &std::collections::HashSet<String>,
) -> Vec<Option<String>> {
    signers
        .iter()
        .enumerate()
        .map(|(i, signer)| {
            let address = crate::api::normalize_address(&format!("{:?}", signer.address()));
            if direct_addresses.contains(&address) {
                None
            } else {
                proxies.get(i).map(str::to_string)
            }
        })
        .collect()
}

/// A failed early balance read must never silently remove a selected wallet.
/// Only a balance proven to be exactly zero is safe to discard before we know
/// the selected phase price and exact gas requirement.
fn keep_before_opensea_auth(balance: Option<U256>) -> bool {
    balance.map(|value| value != U256::ZERO).unwrap_or(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduledPreopenPlan {
    CachedOnly,
    ValidatePublicState,
}

fn scheduled_preopen_plan(stage_type: &str, use_gql: bool) -> ScheduledPreopenPlan {
    if stage_type == "PUBLIC_SALE" && !use_gql {
        ScheduledPreopenPlan::ValidatePublicState
    } else {
        ScheduledPreopenPlan::CachedOnly
    }
}

fn required_mint_balance(mint_value: U256, gas_limit: u64, max_fee: U256) -> U256 {
    mint_value.saturating_add(U256::from(gas_limit).saturating_mul(max_fee))
}

fn initial_force_fixed_gas(
    dry_run: bool,
    skip_preflight: bool,
    skip_estimate_on_open: bool,
    auto_skip_estimate: bool,
) -> bool {
    !dry_run || skip_preflight || skip_estimate_on_open || auto_skip_estimate
}

fn route_auth_summary(label: &str, wallets: &[WalletAuth], via_proxy: bool) -> String {
    let group: Vec<&WalletAuth> = wallets
        .iter()
        .filter(|w| w.proxy_url.is_some() == via_proxy)
        .collect();
    let total = group.len();
    let ok = group.iter().filter(|w| w.auth_ok).count();
    let cached = group.iter().filter(|w| w.auth_ok && w.auth_cached).count();
    let mut cold_ok: Vec<u64> = group
        .iter()
        .filter(|w| w.auth_ok && !w.auth_cached)
        .filter_map(|w| w.auth_elapsed_ms)
        .collect();
    cold_ok.sort_unstable();
    let latency = if cold_ok.is_empty() {
        "cold_ok=0".to_string()
    } else {
        let median = cold_ok[(cold_ok.len() - 1) / 2];
        let p90_idx = cold_ok
            .len()
            .saturating_mul(9)
            .div_ceil(10)
            .saturating_sub(1);
        format!(
            "cold_ok={} median={}ms p90={}ms",
            cold_ok.len(),
            median,
            cold_ok[p90_idx]
        )
    };
    format!(
        "A/B auth {label}: ok={ok}/{total} failed={} cached={cached} {latency}",
        total.saturating_sub(ok)
    )
}

type PrefetchedMintTx = (alloy_primitives::Address, U256, Bytes);

#[derive(Clone)]
struct PreSignedMintTx {
    tx: sign::BuiltTx,
    raw: Bytes,
    hash: B256,
    gas_limit: u64,
}

/// Absorb only already-finished prefetch jobs. This function never waits, so a
/// slow OpenSea request cannot push ready wallets past the fire timestamp.
fn collect_ready_prefetches(
    jobs: &mut tokio::task::JoinSet<(alloy_primitives::Address, Option<PrefetchedMintTx>)>,
    wallets: &mut [WalletAuth],
) -> usize {
    let mut collected = 0usize;
    while let Some(joined) = jobs.try_join_next() {
        if let Ok((addr, Some(tx_data))) = joined {
            if let Some(wallet) = wallets.iter_mut().find(|w| w.address == addr) {
                wallet.prefetched_tx = Some(tx_data);
                collected += 1;
            }
        }
    }
    collected
}

/// Sign every wallet whose personalized OpenSea calldata is already available.
/// Signing happens before the fire deadline; failures safely fall back to the
/// existing worker path, which can rebuild/sign after T0 without blocking other
/// wallets that are already armed.
fn pre_sign_ready_wallets(
    wallets: &mut [WalletAuth],
    chain_id: u64,
    gas_limit: u64,
    max_fee: U256,
    max_priority_fee: U256,
) -> usize {
    let mut signed = 0usize;
    for wallet in wallets.iter_mut() {
        if !wallet.auth_ok || wallet.pre_signed_tx.is_some() {
            continue;
        }
        let Some((to, value, calldata)) = wallet.prefetched_tx.as_ref() else {
            continue;
        };
        let tx = sign::BuiltTx {
            chain_id,
            nonce: wallet.nonce,
            to: *to,
            value: *value,
            data: calldata.clone(),
            gas_limit,
            max_fee,
            max_priority_fee,
        };
        if let Ok((raw, hash)) = sign::sign_transaction(&wallet.signer, &tx) {
            wallet.pre_signed_tx = Some(PreSignedMintTx {
                tx,
                raw,
                hash,
                gas_limit,
            });
            signed += 1;
        }
    }
    signed
}

const DEFAULT_SEADROP_ADDRESS: &str = "0x00005EA00Ac477B1030CE78506496e8C2dE24bf5";
const DEFAULT_FEE_RECIPIENT: &str = "0x0000a26b00c1F0DF003000390027140000fAa719";

/// Absolute ceiling (0.05 ETH) on the tx value OpenSea may request when the
/// resolved phase price is zero — i.e. a free mint, or a priced phase whose
/// price we failed to parse.
///
/// The relative "4x phase price" guard degenerates to 0 in that case, leaving
/// the response free to specify any value. This keeps a bad/tampered response
/// from draining wallets while staying well above real gas-inclusive overhead.
const UNPRICED_VALUE_CAP_WEI: u64 = 50_000_000_000_000_000;

/// Decode OpenSea / local mint `data` hex. Fail-fast on empty or invalid.
pub(crate) fn parse_tx_calldata_hex(data_hex: &str) -> anyhow::Result<Bytes> {
    let raw = data_hex.trim();
    if raw.is_empty() || raw == "0x" || raw == "0X" {
        bail!("empty calldata");
    }
    let hex_body = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    if hex_body.is_empty() {
        bail!("empty calldata");
    }
    let bytes = hex::decode(hex_body).context("invalid calldata hex")?;
    if bytes.is_empty() {
        bail!("empty calldata");
    }
    Ok(Bytes::from(bytes))
}

fn calldata_word(data: &[u8], index: usize) -> Result<&[u8]> {
    let start = 4usize
        .checked_add(index.checked_mul(32).context("calldata word overflow")?)
        .context("calldata offset overflow")?;
    let end = start.checked_add(32).context("calldata offset overflow")?;
    data.get(start..end)
        .with_context(|| format!("calldata is missing ABI word {index}"))
}

fn calldata_address(data: &[u8], index: usize) -> Result<Address> {
    let word = calldata_word(data, index)?;
    if word[..12].iter().any(|byte| *byte != 0) {
        bail!("ABI address word {index} has non-zero padding");
    }
    Ok(Address::from_slice(&word[12..]))
}

fn calldata_u64(data: &[u8], index: usize) -> Result<u64> {
    let word = calldata_word(data, index)?;
    if word[..24].iter().any(|byte| *byte != 0) {
        bail!("ABI integer word {index} does not fit u64");
    }
    Ok(u64::from_be_bytes(
        word[24..].try_into().expect("8-byte slice"),
    ))
}

/// Validate the wallet-specific SeaDrop call returned by OpenSea before it is
/// signed. Chain id and target/value are checked separately by the caller; this
/// verifies the critical ABI fields that a compromised response could swap.
fn validate_seadrop_calldata(
    data: &[u8],
    nft_contract: Address,
    minter: Address,
    quantity: u32,
    expected_stage_index: Option<i64>,
) -> Result<()> {
    if data.len() < 4 {
        bail!("SeaDrop calldata has no selector");
    }
    let public_selector = &keccak256("mintPublic(address,address,address,uint256)".as_bytes())[..4];
    let signed_selector = &keccak256("mintSigned(address,address,address,uint256,(uint256,uint256,uint256,uint256,uint256,uint256,uint256,bool),uint256,bytes)".as_bytes())[..4];

    let kind = if &data[..4] == public_selector {
        "mintPublic"
    } else if &data[..4] == signed_selector {
        "mintSigned"
    } else {
        bail!(
            "OpenSea returned an unsupported SeaDrop function selector 0x{}",
            hex::encode(&data[..4])
        );
    };

    let actual_nft = calldata_address(data, 0)?;
    let actual_minter = calldata_address(data, 2)?;
    let actual_quantity = calldata_u64(data, 3)?;
    if actual_nft != nft_contract {
        bail!("{kind} NFT contract mismatch: got {actual_nft:?}, expected {nft_contract:?}");
    }
    // SeaDrop permits zero here to mean msg.sender; since this transaction is
    // signed by `minter`, both encodings mint to the same wallet.
    if actual_minter != minter && actual_minter != Address::ZERO {
        bail!("{kind} minter mismatch: got {actual_minter:?}, expected {minter:?}");
    }
    if actual_quantity != u64::from(quantity) {
        bail!("{kind} quantity mismatch: got {actual_quantity}, expected {quantity}");
    }

    if kind == "mintSigned" {
        // MintParams: price, wallet max, start, end, stage index, stage supply,
        // fee bps, restrictFeeRecipients.
        let stage_index = calldata_u64(data, 8)?;
        if let Some(expected) = expected_stage_index.filter(|value| *value >= 0)
            && stage_index != expected as u64
        {
            bail!("mintSigned stage mismatch: got {stage_index}, expected {expected}");
        }
        if calldata_u64(data, 11)? != 1 {
            bail!("mintSigned does not restrict fee recipients");
        }
        // `bytes signature` is word 13; ensure its offset and declared length
        // both point inside this call rather than trusting malformed ABI.
        let signature_offset = usize::try_from(calldata_u64(data, 13)?)
            .context("mintSigned signature offset does not fit usize")?;
        let length_word_start = 4usize
            .checked_add(signature_offset)
            .context("mintSigned signature offset overflow")?;
        let length_word_end = length_word_start
            .checked_add(32)
            .context("mintSigned signature length offset overflow")?;
        let length_word = data
            .get(length_word_start..length_word_end)
            .context("mintSigned signature offset is outside calldata")?;
        if length_word[..24].iter().any(|byte| *byte != 0) {
            bail!("mintSigned signature length does not fit u64");
        }
        let signature_len = usize::try_from(u64::from_be_bytes(
            length_word[24..].try_into().expect("8-byte slice"),
        ))
        .context("mintSigned signature length does not fit usize")?;
        let signature_end = length_word_end
            .checked_add(signature_len)
            .context("mintSigned signature length overflow")?;
        if signature_len == 0 || signature_end > data.len() {
            bail!("mintSigned signature bytes are missing or truncated");
        }
    }
    Ok(())
}

/// Build wallet-specific PUBLIC_SALE calldata without an OpenSea GraphQL call.
/// This is pure local work, so it can be completed and signed before T0.
fn build_local_public_mint(
    nft_contract: &str,
    quantity: u32,
    unit_price: U256,
    seadrop_address: Option<&str>,
    fee_recipient: Option<&str>,
    address: Address,
) -> Result<PrefetchedMintTx> {
    let address_text = format!("{address:?}");
    let fallback_value = unit_price.saturating_mul(U256::from(quantity.max(1)));
    let tx_data = opensea::build_public_mint_tx(
        nft_contract,
        quantity,
        unit_price,
        seadrop_address,
        fee_recipient,
        Some(&address_text),
    )?;
    let to: Address = tx_data
        .get("to")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_SEADROP_ADDRESS)
        .parse()
        .context("invalid local PUBLIC_SALE target")?;
    let value = tx_data
        .get("value")
        .and_then(|v| v.as_str())
        .and_then(parse_hex_u256)
        .unwrap_or(fallback_value);
    let calldata = parse_tx_calldata_hex(
        tx_data
            .get("data")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
    )?;
    Ok((to, value, calldata))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SeaDropPublicState {
    mint_price: U256,
    start_time: u64,
    end_time: u64,
    max_total_mintable_by_wallet: u64,
    restrict_fee_recipients: bool,
    allowed_fee_recipients: Vec<Address>,
}

fn abi_word_u64(bytes: &[u8]) -> Result<u64> {
    if bytes.len() != 32 || bytes[..24].iter().any(|byte| *byte != 0) {
        bail!("SeaDrop ABI word does not fit u64");
    }
    Ok(u64::from_be_bytes(bytes[24..32].try_into()?))
}

/// Read the authoritative public-drop configuration once per collection.
/// This is intentionally an RPC view call, not one request per wallet.
async fn read_seadrop_public_state(
    rpc: &rpc::RpcClient,
    seadrop_address: Option<&str>,
    nft_contract: &str,
) -> Result<SeaDropPublicState> {
    let seadrop: Address = seadrop_address
        .unwrap_or(DEFAULT_SEADROP_ADDRESS)
        .parse()
        .context("invalid SeaDrop address")?;
    let nft: Address = nft_contract.parse().context("invalid NFT contract")?;
    let address_call = |signature: &str| {
        let selector = keccak256(signature.as_bytes());
        let mut calldata = Vec::with_capacity(36);
        calldata.extend_from_slice(&selector[..4]);
        calldata.extend_from_slice(&[0u8; 12]);
        calldata.extend_from_slice(nft.as_slice());
        Bytes::from(calldata)
    };
    // Run both views concurrently. This keeps the final safety gate to one RPC
    // round trip even on high-latency L2 endpoints.
    let public_call = address_call("getPublicDrop(address)");
    let recipients_call = address_call("getAllowedFeeRecipients(address)");
    let (public_result, recipients_result) = tokio::join!(
        rpc.eth_call(&Address::ZERO, &seadrop, &public_call),
        rpc.eth_call(&Address::ZERO, &seadrop, &recipients_call),
    );
    let raw = public_result.context("SeaDrop getPublicDrop failed")?;
    if raw.len() < 6 * 32 {
        bail!(
            "SeaDrop getPublicDrop returned {} bytes, expected 192",
            raw.len()
        );
    }
    let restrict_fee_recipients = abi_word_u64(&raw[160..192])? != 0;
    let allowed_fee_recipients = if restrict_fee_recipients {
        let encoded = recipients_result.context("SeaDrop getAllowedFeeRecipients failed")?;
        decode_abi_address_array(&encoded)
            .context("SeaDrop returned malformed allowed fee recipients")?
    } else {
        Vec::new()
    };
    Ok(SeaDropPublicState {
        mint_price: U256::from_be_slice(&raw[0..32]),
        start_time: abi_word_u64(&raw[32..64])?,
        end_time: abi_word_u64(&raw[64..96])?,
        max_total_mintable_by_wallet: abi_word_u64(&raw[96..128])?,
        restrict_fee_recipients,
        allowed_fee_recipients,
    })
}

fn decode_abi_address_array(raw: &[u8]) -> Result<Vec<Address>> {
    let offset = usize::try_from(abi_word_u64(
        raw.get(0..32).context("missing array offset")?,
    )?)
    .context("array offset does not fit usize")?;
    let count_end = offset.checked_add(32).context("array offset overflow")?;
    let count = usize::try_from(abi_word_u64(
        raw.get(offset..count_end)
            .context("array offset is outside result")?,
    )?)
    .context("array length does not fit usize")?;
    let available_words = raw.len().saturating_sub(count_end) / 32;
    if count > available_words {
        bail!("address array length {count} exceeds encoded result");
    }
    let mut recipients = Vec::with_capacity(count);
    for index in 0..count {
        let start = count_end
            .checked_add(index.checked_mul(32).context("array index overflow")?)
            .context("array address offset overflow")?;
        let end = start
            .checked_add(32)
            .context("array address end overflow")?;
        let word = raw.get(start..end).context("truncated address array")?;
        if word[..12].iter().any(|byte| *byte != 0) {
            bail!("address array contains non-zero ABI padding");
        }
        recipients.push(Address::from_slice(&word[12..]));
    }
    Ok(recipients)
}

fn resolve_public_fee_recipient(
    state: &SeaDropPublicState,
    configured: Option<&str>,
) -> Result<Option<String>> {
    if !state.restrict_fee_recipients {
        return Ok(configured.map(str::to_owned));
    }
    if state.allowed_fee_recipients.is_empty() {
        bail!("PUBLIC_SALE restricts fee recipients but SeaDrop has no allowed recipient");
    }
    if let Some(configured) = configured {
        let address: Address = configured.parse().context("invalid FEE_RECIPIENT")?;
        if !state.allowed_fee_recipients.contains(&address) {
            bail!("configured FEE_RECIPIENT {address:?} is not allowed by this SeaDrop collection");
        }
        return Ok(Some(format!("{address:?}")));
    }
    let default: Address = DEFAULT_FEE_RECIPIENT
        .parse()
        .expect("valid built-in fee recipient");
    if state.allowed_fee_recipients.contains(&default) {
        Ok(None)
    } else {
        // Every address in this enumeration was explicitly approved by the
        // collection owner. Selecting the first mirrors SeaDrop's canonical
        // configuration order and keeps the mint fully local at T0.
        Ok(Some(format!("{:?}", state.allowed_fee_recipients[0])))
    }
}

fn validate_seadrop_public_state(
    state: &SeaDropPublicState,
    expected_unit_price: U256,
    quantity: u32,
    fire_time: i64,
) -> Result<U256> {
    if state.mint_price != expected_unit_price {
        bail!(
            "PUBLIC_SALE price changed after the task was saved: expected {} wei, on-chain {} wei; refusing changed mint terms",
            expected_unit_price,
            state.mint_price
        );
    }
    if state.max_total_mintable_by_wallet > 0
        && u64::from(quantity) > state.max_total_mintable_by_wallet
    {
        bail!(
            "PUBLIC_SALE quantity {} exceeds on-chain wallet limit {}",
            quantity,
            state.max_total_mintable_by_wallet
        );
    }
    let fire_time = fire_time.max(0) as u64;
    if state.start_time > 0 && fire_time < state.start_time {
        bail!(
            "PUBLIC_SALE opens on chain at unix {}, but task fires at {}",
            state.start_time,
            fire_time
        );
    }
    if state.end_time > 0 && fire_time >= state.end_time {
        bail!(
            "PUBLIC_SALE is closed on chain (ended at unix {})",
            state.end_time
        );
    }
    Ok(state.mint_price)
}

fn rebuild_local_public_wallets(
    wallets: &mut [WalletAuth],
    wallet_quantities: &HashMap<Address, u32>,
    default_quantity: u32,
    nft_contract: &str,
    unit_price: U256,
    seadrop_address: Option<&str>,
    fee_recipient: Option<&str>,
) -> Result<usize> {
    let mut built = 0usize;
    for wallet in wallets.iter_mut().filter(|wallet| wallet.auth_ok) {
        let quantity = wallet_quantities
            .get(&wallet.address)
            .copied()
            .unwrap_or(default_quantity);
        wallet.prefetched_tx = Some(build_local_public_mint(
            nft_contract,
            quantity,
            unit_price,
            seadrop_address,
            fee_recipient,
            wallet.address,
        )?);
        // A price/config refresh invalidates any signature made from the old
        // value. The caller signs the rebuilt transactions before T0.
        wallet.pre_signed_tx = None;
        built += 1;
    }
    Ok(built)
}

/// Wall-clock fire lag in ms: `now_ms − start_ts*1000`, floored at 0.
pub(crate) fn fire_lag_ms_from_clock(start_ts: i64, now_ms: i64) -> u64 {
    let open_ms = start_ts.saturating_mul(1000);
    now_ms.saturating_sub(open_ms).max(0) as u64
}

/// Human-readable RPC selection plan for the mint log / UI.
///
/// Answers the two questions the operator actually has at T0: which endpoint
/// leads (nonce, fees, hedged reads) and which endpoints a broadcast reaches.
/// Endpoints the probe dropped are listed as EXCLUDED, and endpoints beyond the
/// fan-out width as `unused`, so a configured-but-idle node is never silent —
/// notably the public fallback that is appended to every chain automatically.
pub(crate) fn format_rpc_plan(
    chain: &str,
    probes: &[rpc::RpcNodeProbe],
    fanout_width: usize,
) -> Vec<String> {
    let usable = probes.iter().filter(|p| p.ok).count();
    let reach = fanout_width.min(usable);
    let mut out = vec![format!(
        "RPC plan for {chain}: {usable} usable endpoint(s), broadcast reaches {reach}"
    )];
    let mut rank = 0usize;
    for p in probes {
        let ping = p
            .latency_ms
            .map(|ms| format!("{ms}ms"))
            .unwrap_or_else(|| "not measured".to_string());
        if p.ok {
            rank += 1;
            let role = if rank == 1 {
                "LEAD"
            } else if rank <= reach {
                "broadcast"
            } else {
                "unused"
            };
            out.push(format!("  [{rank}] {role} — {} ping={ping}", p.url_short));
        } else {
            out.push(format!(
                "  [x] EXCLUDED — {} (probe failed, {ping})",
                p.url_short
            ));
        }
    }
    out
}

/// Resolve gas limit for OpenSea mint: estimate path uses L2 floors; fixed path
/// clamps up on elevated chains when operator fixed is below floor.
///
/// When `is_fixed` is true, `estimated_or_fixed` is a hard fixed limit (still L2-clamped).
pub(crate) fn resolve_mint_gas_limit(
    estimated_or_fixed: u64,
    gas_multiplier: f64,
    chain_id: u64,
    is_fixed: bool,
) -> u64 {
    if is_fixed {
        let mut limit = estimated_or_fixed.max(21_000);
        if gas::chain_needs_elevated_gas(chain_id) {
            const L2_FLOOR: u64 = 150_000;
            if limit < L2_FLOOR {
                limit = L2_FLOOR;
            }
        }
        limit.min(15_000_000)
    } else {
        gas::apply_gas_limit(estimated_or_fixed, gas_multiplier, chain_id, 21_000)
    }
}

/// Return the first already-mined hash among `hashes`, if any.
///
/// A send is fanned out to several RPC endpoints, so a `nonce too low` or
/// `underpriced` rejection is ambiguous: it can equally mean "a previous
/// attempt was already mined by another node". Re-broadcasting in that case
/// mints (and pays) twice, so every retry path must check here first.
///
/// Returns `Landed` / `NotFound` / `Unknown` — the caller MUST distinguish the
/// last two. An errored lookup used to be folded into "not found", so an RPC
/// outage during a `nonce too low` retry looked identical to "nothing landed"
/// and the worker broadcast a replacement for a transaction that was quietly
/// mining. Only `NotFound` is safe to re-send on.
async fn first_landed_hash(
    rpc: &crate::rpc::RpcClient,
    hashes: &[B256],
) -> crate::rpc::ReceiptLookup {
    rpc.find_landed(hashes).await
}

/// Build the worker result for a tx that is already on chain.
fn receipt_to_result(
    addr: alloy_primitives::Address,
    hash: B256,
    info: &crate::rpc::ReceiptInfo,
) -> MintResult {
    MintResult {
        address: addr,
        tx_hash: Some(hash),
        status: if info.success {
            WalletStatus::Confirmed
        } else {
            WalletStatus::Failed
        },
        gas_used: Some(info.gas_used),
        block_number: Some(info.block_number),
        error: if info.success {
            None
        } else {
            Some("transaction reverted on chain".to_string())
        },
    }
}

type AutoSweepJobResult = (
    Address,
    std::result::Result<crate::sweep::MintAutoSweepReport, String>,
);

fn spawn_auto_sweep_for_confirmed(
    jobs: &mut tokio::task::JoinSet<AutoSweepJobResult>,
    started: &mut HashSet<Address>,
    result: &MintResult,
    signers: &HashMap<Address, Signer>,
    rpc: &rpc::RpcClient,
    contract: Address,
    destination: Option<Address>,
    gas_params: &GasParams,
) -> bool {
    let Some(destination) = destination else {
        return false;
    };
    if result.status != WalletStatus::Confirmed || !started.insert(result.address) {
        return false;
    }
    let address = result.address;
    if address == destination {
        jobs.spawn(async move {
            (
                address,
                Ok(crate::sweep::MintAutoSweepReport {
                    discovered: 0,
                    swept: 0,
                    failed: 0,
                    skipped_destination: true,
                    errors: Vec::new(),
                }),
            )
        });
        return true;
    }
    let Some(signer) = signers.get(&address).cloned() else {
        jobs.spawn(async move { (address, Err("signer missing after mint".to_string())) });
        return true;
    };
    let Some(tx_hash) = result.tx_hash else {
        jobs.spawn(async move { (address, Err("confirmed mint has no tx hash".to_string())) });
        return true;
    };
    let rpc = rpc.clone();
    let gas_params = gas_params.clone();
    jobs.spawn(async move {
        let outcome = async {
            let receipt = rpc
                .transaction_receipt(&tx_hash)
                .await
                .map_err(|error| format!("mint receipt lookup: {error}"))?
                .ok_or_else(|| "mint receipt disappeared before auto-sweep".to_string())?;
            crate::sweep::sweep_mint_receipt_nfts(
                &signer,
                &rpc,
                &receipt,
                contract,
                destination,
                &gas_params,
            )
            .await
            .map_err(|error| error.to_string())
        }
        .await;
        (address, outcome)
    });
    true
}

async fn fetch_and_parse_gql(
    reporter: &dyn MintReporter,
    session: &opensea::AuthSession,
    slug: &str,
    addr: &alloy_primitives::Address,
    nft_contract: &str,
    chain: &str,
    stage_token_id: &str,
    quantity: u32,
    expected_stage_index: Option<i64>,
    payment_asset: &serde_json::Value,
    calldata_value: &U256,
    mint_started_at: &std::time::Instant,
    attempt: u32,
    quiet: bool,
) -> anyhow::Result<(alloy_primitives::Address, U256, Bytes)> {
    let gql_start = std::time::Instant::now();
    let fetch = opensea::fetch_mint_calldata(
        session,
        slug,
        addr,
        nft_contract,
        chain,
        stage_token_id,
        quantity,
        payment_asset,
    )
    .await?;

    let gql_ms = gql_start.elapsed().as_millis();
    match fetch.route {
        opensea::MintActionRoute::Short => log_always(
            reporter,
            format!(
                "[{}] OpenSea SHORT OK {}ms hash={}вЂ¦{}",
                sign::shorten_address(addr),
                fetch.short_ms.unwrap_or(gql_ms),
                &opensea::MINT_ACTION_TIMELINE_HASH[..8],
                &opensea::MINT_ACTION_TIMELINE_HASH[60..]
            ),
        ),
        opensea::MintActionRoute::FullFallbackExpired => log_always(
            reporter,
            format!(
                "[{}] OpenSea SHORT HASH EXPIRED {}ms -> FULL FALLBACK OK {}ms (total={}ms)",
                sign::shorten_address(addr),
                fetch.short_ms.unwrap_or_default(),
                fetch.full_ms.unwrap_or_default(),
                gql_ms
            ),
        ),
        opensea::MintActionRoute::FullHashDisabled => log_always(
            reporter,
            format!(
                "[{}] OpenSea SHORT DISABLED (expired precheck) -> FULL FALLBACK OK {}ms",
                sign::shorten_address(addr),
                fetch.full_ms.unwrap_or(gql_ms)
            ),
        ),
    }
    let resp = fetch.data;
    if std::env::var("DEBUG").ok().as_deref() == Some("1") {
        let _ = std::fs::create_dir_all("logs");
        let debug_file = format!(
            "logs/debug_gql_{}_{}.json",
            sign::shorten_address(addr),
            attempt
        );
        let _ = std::fs::write(
            &debug_file,
            serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()),
        );
        mint_log(
            reporter,
            quiet,
            format!(
                "[{}] GQL fetch OK {}ms (saved {})",
                sign::shorten_address(addr),
                gql_ms,
                debug_file
            ),
        );
    } else {
        mint_log(
            reporter,
            quiet,
            format!(
                "[{}] GQL fetch OK {}ms",
                sign::shorten_address(addr),
                gql_ms,
            ),
        );
    }

    let tx_data = opensea::extract_opensea_action_tx(&resp)?;
    let to_addr: alloy_primitives::Address = tx_data
        .get("to")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_SEADROP_ADDRESS)
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid tx to"))?;
    let tx_value = tx_data
        .get("value")
        .and_then(|v| v.as_str())
        .and_then(parse_hex_u256)
        .unwrap_or(*calldata_value);
    // Trust boundary (audit M3): OpenSea returns `to`/`value` for the mint tx.
    // Reject a zero `to`, warn if `to` isn't the collection contract or the known
    // SeaDrop, and hard-cap `value` so an anomalous response can't cause a gross
    // overpay. Normal fee overhead (value >= phase price) is still allowed.
    if to_addr == alloy_primitives::Address::ZERO {
        anyhow::bail!(
            "[{}] OpenSea returned a zero `to` address for the mint tx",
            sign::shorten_address(addr)
        );
    }
    let expected_seadrop = DEFAULT_SEADROP_ADDRESS
        .parse::<alloy_primitives::Address>()
        .ok();
    let expected_nft = nft_contract
        .trim()
        .parse::<alloy_primitives::Address>()
        .ok();
    if Some(to_addr) != expected_seadrop && expected_nft.is_some() && Some(to_addr) != expected_nft
    {
        mint_log(
            reporter,
            quiet,
            format!(
                "[{}] WARN OpenSea tx `to`={:?} is neither the collection contract nor the known SeaDrop",
                sign::shorten_address(addr),
                to_addr
            ),
        );
    }
    if calldata_value.is_zero() {
        // Free / unpriced phase: the relative 4x cap can't apply here, so bound
        // the absolute value instead. Otherwise a free mint — or any phase whose
        // price failed to parse — forwards whatever value the response asks for,
        // up to the wallet's entire balance, on every wallet.
        if tx_value > U256::from(UNPRICED_VALUE_CAP_WEI) {
            anyhow::bail!(
                "[{}] OpenSea tx value {} on a free/unpriced phase exceeds the {} wei safety cap — refusing (possible bad response)",
                sign::shorten_address(addr),
                tx_value,
                UNPRICED_VALUE_CAP_WEI
            );
        }
    } else if tx_value > calldata_value.saturating_mul(U256::from(4u64)) {
        anyhow::bail!(
            "[{}] OpenSea tx value {} exceeds 4x expected phase price {} — refusing (possible bad response)",
            sign::shorten_address(addr),
            tx_value,
            calldata_value
        );
    }
    if tx_value != *calldata_value {
        mint_log(
            reporter,
            quiet,
            format!(
                "[{}] WARN OpenSea tx value differs from selected phase price: gql_value={} parsed_phase_value={}",
                sign::shorten_address(addr),
                tx_value,
                calldata_value
            ),
        );
    }
    let data_hex = tx_data.get("data").and_then(|v| v.as_str()).unwrap_or("");
    let cd = parse_tx_calldata_hex(data_hex)
        .with_context(|| format!("[{}] OpenSea GQL tx data", sign::shorten_address(addr)))?;
    let expected_nft = nft_contract
        .parse::<Address>()
        .context("invalid expected NFT contract")?;
    validate_seadrop_calldata(
        cd.as_ref(),
        expected_nft,
        *addr,
        quantity,
        expected_stage_index,
    )
    .with_context(|| {
        format!(
            "[{}] unsafe OpenSea calldata rejected",
            sign::shorten_address(addr)
        )
    })?;

    mint_log(
        reporter,
        quiet,
        format!(
            "[{}] PREPARED t+{}ms (gql={}ms) to={:?} value={} data={} bytes",
            sign::shorten_address(addr),
            mint_started_at.elapsed().as_millis(),
            gql_ms,
            to_addr,
            tx_value,
            cd.len()
        ),
    );
    Ok((to_addr, tx_value, cd))
}

/// GQL fetch with one SIWE re-auth retry on 401 / auth errors.
async fn fetch_calldata_reauth(
    reporter: &dyn MintReporter,
    session: &mut Option<opensea::AuthSession>,
    signer: &Signer,
    chain_id: u64,
    proxy_url: Option<&str>,
    slug: &str,
    addr: &alloy_primitives::Address,
    nft_contract: &str,
    chain: &str,
    stage_token_id: &str,
    quantity: u32,
    expected_stage_index: Option<i64>,
    payment_asset: &serde_json::Value,
    calldata_value: &U256,
    mint_started_at: &std::time::Instant,
    attempt: u32,
    quiet: bool,
) -> anyhow::Result<(alloy_primitives::Address, U256, Bytes)> {
    let sess = session
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no auth session"))?;
    match fetch_and_parse_gql(
        reporter,
        sess,
        slug,
        addr,
        nft_contract,
        chain,
        stage_token_id,
        quantity,
        expected_stage_index,
        payment_asset,
        calldata_value,
        mint_started_at,
        attempt,
        quiet,
    )
    .await
    {
        Ok(r) => Ok(r),
        Err(e) => {
            let err_str = format!("{}", e);
            if !is_auth_error(&err_str) {
                return Err(e);
            }
            mint_log(
                reporter,
                quiet,
                format!(
                    "[{}] {}: {}",
                    sign::shorten_address(addr),
                    crate::mint_ops::reauth_required_message(),
                    err_str
                ),
            );
            report_wallet(
                reporter,
                addr,
                Some(WalletStatus::Auth),
                Some(crate::mint_ops::reauth_required_message().into()),
                None,
                None,
            );
            let new_sess = opensea::siwe_auth(addr, signer, chain_id, proxy_url)
                .await
                .map_err(|ae| anyhow::anyhow!("re-auth failed: {}", ae))?;
            *session = Some(new_sess);
            let sess = session
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("internal: session missing after re-auth"))?;
            fetch_and_parse_gql(
                reporter,
                sess,
                slug,
                addr,
                nft_contract,
                chain,
                stage_token_id,
                quantity,
                expected_stage_index,
                payment_asset,
                calldata_value,
                mint_started_at,
                attempt,
                quiet,
            )
            .await
        }
    }
}

fn cancelled(cancel: &Option<Arc<AtomicBool>>) -> bool {
    cancel
        .as_ref()
        .map(|c| c.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Mint-action requests OpenSea allows before it starts refusing.
///
/// Measured against the live endpoint: the fifth request reports
/// `x-ratelimit-remaining: 0` and the sixth is refused. A refused request costs
/// a token just like a successful one, which is why the old pre-fetch loop was
/// so expensive.
pub const OPENSEA_MINT_ACTION_BUDGET: usize = 5;

/// Roughly how long one token takes to come back, measured: refused at +1s,
/// +2s and +3s after exhaustion, allowed again at +4.3s.
pub const OPENSEA_MINT_ACTION_REFILL_MS: u64 = 4_300;

/// Spacing between the T0 calldata requests of wallets sharing one exit IP.
const GQL_STAGGER_STEP_MS: u64 = 25;

/// Ceiling on the spread within a single IP's group.
const GQL_STAGGER_MAX_SPREAD_MS: u64 = 1_500;

/// Warm the per-wallet OpenSea transport while the countdown still has enough
/// slack for large proxy pools. Our clients keep idle sockets for ten minutes,
/// so a 30-second lead cannot be reaped before T0.
const CONNECTION_WARM_LEAD_MS: i64 = 30_000;

// Warming earlier than the HTTP pool retains an idle connection would pay the
// proxy/TLS handshake twice and still put one on T0. Keep that impossible at
// compile time when either constant is changed later.
const _: () = assert!(
    CONNECTION_WARM_LEAD_MS < opensea::HTTP_POOL_IDLE_TIMEOUT_MS,
    "OpenSea connections warmed this early are reaped before the fire"
);

fn connection_warm_budget_ms(wallets: usize) -> u64 {
    const CAP_MS: u64 = (CONNECTION_WARM_LEAD_MS as u64).saturating_sub(8_000);
    (2_000 + 200 * wallets as u64).min(CAP_MS)
}

/// Gap to leave between the calldata requests of wallets on the same proxy.
///
/// The budget is per exit IP, so wallets on *different* proxies do not compete
/// and must not be delayed for each other — an earlier version spread every
/// wallet in the run, which taxed a well-proxied setup for nothing. Only a
/// shared IP needs spacing, and even then only to avoid arriving in the same
/// millisecond; genuine overflow past the five-token budget is handled by
/// honouring `retry-after` rather than by waiting here.
fn gql_stagger_step_ms(wallets_on_one_proxy: usize) -> u64 {
    let gaps = wallets_on_one_proxy.saturating_sub(1) as u64;
    if gaps == 0 {
        return 0;
    }
    GQL_STAGGER_STEP_MS.min(GQL_STAGGER_MAX_SPREAD_MS / gaps)
}

/// Total time one wallet may spend in waits OpenSea asked for.
///
/// Bounded because the server decides the length: without a ceiling a wallet
/// could be parked past the end of the drop by a limit that keeps renewing.
const RATE_LIMIT_WAIT_BUDGET_MS: u64 = 12_000;

/// A successful HTTP/GQL response without a transaction is a normal short-lived
/// state at the exact edge of a signed stage: collection metadata can show the
/// stage as open before OpenSea's mint-action resolver starts issuing signed
/// calldata.  It is safe to retry because no transaction has been signed or
/// broadcast yet.  This must consume the operator's configured attempt budget,
/// not the much smaller generic-error allowance.
fn is_gql_action_not_ready(err: &str, stage_start_ts: Option<i64>, now_wall: i64) -> bool {
    if !err.contains("OpenSea mint action response has no transactionSubmissionData") {
        return false;
    }
    // An empty action/error set at the exact opening edge can be propagation
    // lag. Once OpenSea names an action error it is terminal by default. The
    // sole exception we have observed as time-dependent is DropNotMinting at
    // T0, and even that is retried only inside the bounded opening window.
    if !err.contains("action errors:") {
        return true;
    }
    err.contains("DropNotMintingError") && in_phase_open_lag_window(stage_start_ts, now_wall)
}

fn is_terminal_gql_action_error(err: &str, stage_start_ts: Option<i64>, now_wall: i64) -> bool {
    err.contains("OpenSea mint action response has no transactionSubmissionData")
        && err.contains("action errors:")
        && !is_gql_action_not_ready(err, stage_start_ts, now_wall)
}

/// Keep the first edge retries tight, then ease off enough to avoid needlessly
/// hammering OpenSea while its stage state propagates.  HTTP request latency is
/// additional to this delay; explicit 429/retry-after responses use the
/// separate server-directed rate-limit budget below.
fn gql_action_not_ready_delay(attempt: u32) -> std::time::Duration {
    let millis = match attempt {
        0 | 1 => 50,
        2 => 75,
        3 => 100,
        4 => 150,
        5..=8 => 250,
        _ => 500,
    };
    std::time::Duration::from_millis(millis)
}

/// The wait OpenSea asked for, when this error is a rate limit and the wallet
/// can still afford to honour it.
///
/// A rate limit is not a failure — it is the server saying "come back in N".
/// The old path ignored N, slept a flat 100 ms, and spent one of only three GQL
/// attempts doing it, so a 2.5 s hold was exhausted in 300 ms and the wallet
/// was reported failed while the mint was still open. These waits therefore
/// draw on their own budget: a genuine error should still give up after three
/// tries.
fn rate_limit_backoff(
    err: &str,
    addr: &Address,
    budget_ms: &mut u64,
) -> Option<std::time::Duration> {
    let asked = match opensea::parse_retry_after_ms(err) {
        Some(ms) => ms,
        // No explicit instruction, so require unmistakable wording: a bare
        // "429" also occurs inside wei values and tx hashes.
        None => {
            let lower = err.to_lowercase();
            if lower.contains("too many requests") || lower.contains("rate limit") {
                1000
            } else {
                return None;
            }
        }
    };
    if *budget_ms == 0 {
        return None;
    }
    let wait = asked.min(*budget_ms);
    *budget_ms -= wait;
    // Wallets limited together must not return together — arriving in lockstep
    // is what produced the limit. Spread them deterministically by address so
    // the same wallet behaves the same way on every run.
    let jitter = (addr.as_slice()[19] as u64 % 250) + 25;
    Some(std::time::Duration::from_millis(wait + jitter))
}

/// Sleep that notices cancellation.
///
/// An honoured rate-limit wait can run to seconds, and the operator pressing
/// stop must not have to wait it out.
async fn sleep_cancellable(dur: std::time::Duration, cancel: &Option<Arc<AtomicBool>>) {
    let deadline = std::time::Instant::now() + dur;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline || cancelled(cancel) {
            return;
        }
        tokio::time::sleep((deadline - now).min(std::time::Duration::from_millis(100))).await;
    }
}

/// OpenSea mint orchestration.
///
/// `cancel`: when set to true (best-effort), countdown aborts and workers stop
/// between attempts (in-flight RPC may still finish).
///
pub async fn run_opensea_mint(
    signers: &[Signer],
    env: &HashMap<String, String>,
    proxies: &ProxyManager,
    opts: &MintOptions,
    vault_password: Option<&str>,
    reporter: Arc<dyn MintReporter>,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<MintRunSummary> {
    if signers.is_empty() {
        bail!("No wallets loaded. Add keys first.");
    }

    let slug = parse_collection_slug(&opts.slug);
    if slug.is_empty() {
        bail!("No collection specified");
    }

    // Full verbose trail on disk + UI reporter
    let reporter: Arc<dyn MintReporter> = match FileTeeReporter::create(reporter.clone(), &slug) {
        Ok(tee) => {
            let p = tee.path.display().to_string();
            log_always(&tee, format!("Full log file: {p}"));
            Arc::new(tee) as Arc<dyn MintReporter>
        }
        Err(e) => {
            log_always(
                reporter.as_ref(),
                format!("WARN: could not open mint log file: {e}"),
            );
            reporter
        }
    };
    if let (Some(task_id), Some(launch_id)) = (&opts.task_id, &opts.launch_id) {
        log_always(
            reporter.as_ref(),
            format!(
                "Task launch: task_id={task_id} launch_id={launch_id} source={}",
                opts.launch_source.as_deref().unwrap_or("unknown")
            ),
        );
    }

    let result = run_opensea_mint_inner(
        signers,
        env,
        proxies,
        opts,
        vault_password,
        reporter.clone(),
        cancel,
        &slug,
    )
    .await;
    if let Err(error) = &result {
        let message = format!("Mint stopped before completion: {error}");
        report_phase(reporter.as_ref(), "error", message.clone());
        log_always(reporter.as_ref(), format!("FATAL ERROR | {message}"));
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_opensea_mint_inner(
    signers: &[Signer],
    env: &HashMap<String, String>,
    proxies: &ProxyManager,
    opts: &MintOptions,
    vault_password: Option<&str>,
    reporter: Arc<dyn MintReporter>,
    cancel: Option<Arc<AtomicBool>>,
    slug: &str,
) -> Result<MintRunSummary> {
    let slug = slug.to_string();

    // Optional wallet subset: keep original vault indices for proxy mapping.
    // proxy_overrides: address → proxy list index (manual wallet→proxy map).
    // direct_wallet_addresses is deliberately separate: a missing override has
    // always meant automatic round-robin and must keep that meaning.
    let direct_addresses: std::collections::HashSet<String> = opts
        .direct_wallet_addresses
        .as_ref()
        .into_iter()
        .flatten()
        .map(|a| crate::api::normalize_address(a))
        .collect();
    let override_by_vault: std::collections::HashMap<usize, usize> = {
        let mut m = std::collections::HashMap::new();
        if let Some(ref ov) = opts.proxy_overrides {
            let by_addr: std::collections::HashMap<String, usize> = ov
                .iter()
                .map(|(a, idx)| (crate::api::normalize_address(a), *idx as usize))
                .collect();
            for (i, s) in signers.iter().enumerate() {
                let a = crate::api::normalize_address(&format!("{:?}", s.address()));
                if let Some(&pidx) = by_addr.get(&a) {
                    m.insert(i, pidx);
                }
            }
        }
        m
    };
    let (signers_owned, proxies_owned): (Vec<Signer>, ProxyManager) =
        if let Some(ref addrs) = opts.wallet_addresses {
            if addrs.is_empty() {
                bail!("No wallets selected for this task");
            }
            let want: std::collections::HashSet<String> = addrs
                .iter()
                .map(|a| crate::api::normalize_address(a))
                .collect();
            let mut selected = Vec::new();
            let mut orig_idx = Vec::new();
            for (i, s) in signers.iter().enumerate() {
                let a = crate::api::normalize_address(&format!("{:?}", s.address()));
                if want.contains(&a) {
                    selected.push(s.clone());
                    orig_idx.push(i);
                }
            }
            validate_wallet_subset_counts(addrs.len(), want.len(), selected.len())?;
            log_always(
                reporter.as_ref(),
                format!(
                    "Task wallets: {}/{} selected",
                    selected.len(),
                    signers.len()
                ),
            );
            (
                selected,
                proxies.remap_for_indices_with_overrides(&orig_idx, &override_by_vault),
            )
        } else if override_by_vault.is_empty() {
            (signers.to_vec(), proxies.clone())
        } else {
            let all_idx: Vec<usize> = (0..signers.len()).collect();
            (
                signers.to_vec(),
                proxies.remap_for_indices_with_overrides(&all_idx, &override_by_vault),
            )
        };
    let signers: &[Signer] = &signers_owned;
    let proxies: &ProxyManager = &proxies_owned;
    let wallet_proxy_routes = assigned_proxy_routes(signers, proxies, &direct_addresses);
    let direct_route_count = wallet_proxy_routes.iter().filter(|p| p.is_none()).count();
    let proxied_route_count = wallet_proxy_routes.len().saturating_sub(direct_route_count);
    let unique_proxy_count = wallet_proxy_routes
        .iter()
        .filter_map(|p| p.as_deref())
        .collect::<std::collections::HashSet<_>>()
        .len();
    log_always(
        reporter.as_ref(),
        format!(
            "OpenSea routes: direct={direct_route_count}, proxy={proxied_route_count}, unique_proxy_endpoints={unique_proxy_count}"
        ),
    );

    let mut quantity = opts.quantity.max(1);
    let dry_run = opts.dry_run;
    let at_time = opts.at_time.clone();
    let auto_mode = true; // GUI/core always non-interactive phase pick

    let primary_addr = signers[0].address();
    // Route the pre-auth collection lookup through the primary wallet's own
    // proxy. It used to go out direct even on a fully proxied run, which leaked
    // the operator IP and let OpenSea rate-limit (or geo-block) the run before
    // a single wallet had authenticated. `wallet_proxy_routes` is used rather
    // than `proxies.get(0)` so a wallet the operator marked direct stays direct.
    let primary_proxy: Option<String> = wallet_proxy_routes.first().cloned().flatten();
    let dummy_session = opensea::unauthenticated_session(&primary_addr, primary_proxy.as_deref());

    report_phase(
        reporter.as_ref(),
        "prep",
        format!("Preparing mint for «{slug}»…"),
    );
    log_always(
        reporter.as_ref(),
        format!("Fetching collection info for '{}'...", slug),
    );
    let info = match opensea::collection_drop_info(&dummy_session, &slug, &primary_addr).await {
        Ok(i) => i,
        Err(_) => {
            log_always(
                reporter.as_ref(),
                "Collection info requires auth. Authenticating primary wallet...",
            );
            let mut any_urls = collect_rpc_urls_for_chain(env, Some("ethereum"), &[]);
            if any_urls.is_empty() {
                any_urls = collect_rpc_urls_for_chain(env, None, &[]);
            }
            if any_urls.is_empty() {
                bail!("No RPC URLs for auth. Configure Alchemy or RPC in Settings.");
            }
            let mut any_rpc = rpc::RpcClient::new(any_urls);
            if let Err(e) = any_rpc.sort_by_fastest_provider().await {
                log_always(reporter.as_ref(), format!("RPC probe failed: {}", e));
            }
            let any_chain_id = any_rpc.chain_id().await.unwrap_or(1);
            // Same reason as the unauthenticated session above: this fallback
            // SIWE must not bypass the primary wallet's proxy.
            match opensea::siwe_auth(
                &primary_addr,
                &signers[0],
                any_chain_id,
                primary_proxy.as_deref(),
            )
            .await
            {
                Ok(session) => {
                    match opensea::collection_drop_info(&session, &slug, &primary_addr).await {
                        Ok(i) => i,
                        Err(e) => bail!("Failed collection info: {}", e),
                    }
                }
                Err(e) => bail!("Auth failed: {}", e),
            }
        }
    };

    let chain_for_rpc = opts
        .chain_override
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(info.chain.as_str());
    if chain_for_rpc != info.chain {
        log_always(
            reporter.as_ref(),
            format!(
                "Chain override: RPC uses '{}' (collection reports '{}')",
                chain_for_rpc, info.chain
            ),
        );
    }

    let urls = collect_rpc_urls_for_chain(env, Some(chain_for_rpc), &[]);
    if urls.is_empty() {
        bail!(
            "No RPC URLs for chain '{}'. Set RPC in Settings.",
            chain_for_rpc
        );
    }
    log_always(
        reporter.as_ref(),
        format!("Using {} RPC URL(s) for {}", urls.len(), chain_for_rpc),
    );
    let mut rpc = rpc::RpcClient::new(urls.clone());
    // The resolved endpoint order decides which node serves nonce/fee reads and
    // which nodes a broadcast fans out to. It used to be visible only through
    // `rlog!`, which the desktop silences with QUIET=1 — so the operator could
    // not tell a paid endpoint from the public fallback that is always appended.
    // Report it through the reporter instead, where the UI and log file see it.
    match rpc.sort_by_fastest_provider_report().await {
        Ok(probes) => {
            for line in format_rpc_plan(chain_for_rpc, &probes, rpc.fanout_width()) {
                log_always(reporter.as_ref(), line);
            }
        }
        Err(e) => log_always(reporter.as_ref(), format!("RPC probe failed: {}", e)),
    }

    let actual_chain_id = rpc
        .chain_id()
        .await
        .context("Failed to get chain ID from RPC")?;
    if actual_chain_id == 4663 {
        // Robinhood's official direct sequencer accepts raw transactions but
        // deliberately does not expose normal read RPC. Add it only after a
        // read-capable node has verified chainId=4663.
        rpc.add_send_only_url("https://sequencer.mainnet.chain.robinhood.com");
        log_always(
            reporter.as_ref(),
            "Robinhood broadcast: direct sequencer armed as send-only parallel ingress".to_string(),
        );
    }

    let use_flashbots = opts.use_flashbots.unwrap_or(false);
    let conditional_requested = opts.conditional_submit_enabled.unwrap_or(false);
    let conditional_submit_enabled = conditional_requested && !dry_run;
    let conditional_lead_ms = opts.conditional_lead_ms.unwrap_or(1_000).clamp(100, 10_000);
    if use_flashbots && actual_chain_id != MAINNET_CHAIN_ID {
        bail!(
            "Flashbots bundle only on Ethereum mainnet (chainId 1); RPC chainId is {}",
            actual_chain_id
        );
    }
    if use_flashbots {
        log_always(
            reporter.as_ref(),
            "Broadcast mode: Flashbots bundle (private, multi-wallet)".to_string(),
        );
    }
    if conditional_submit_enabled && use_flashbots {
        bail!("Conditional submit and Flashbots cannot be enabled together");
    }
    if conditional_submit_enabled && actual_chain_id != 4663 {
        bail!(
            "Experimental conditional submit is currently limited to Robinhood (chainId 4663); RPC chainId is {}",
            actual_chain_id
        );
    }
    if conditional_requested && dry_run {
        log_always(
            reporter.as_ref(),
            "Conditional submit is disabled in dry-run mode".to_string(),
        );
    } else if conditional_submit_enabled {
        log_always(
            reporter.as_ref(),
            format!(
                "Broadcast mode: experimental conditional at T-{}ms + identical public fallback at T0",
                conditional_lead_ms
            ),
        );
    }

    let chain_map = chain_id_map();
    let expected_chain_id = chain_map
        .get(chain_for_rpc.to_lowercase().as_str())
        .copied()
        .or_else(|| chain_map.get(info.chain.to_lowercase().as_str()).copied());
    if let Some(expected) = expected_chain_id {
        if actual_chain_id != expected {
            bail!(
                "Chain mismatch: expected {} (chainId {}), but RPC returned chainId {}. Fix RPC or chain override.",
                chain_for_rpc,
                expected,
                actual_chain_id
            );
        }
    }

    // Resolve the mint network first, then cheaply discard wallets that are
    // proven to have no native token at all. OpenSea SIWE plus per-wallet phase
    // discovery is orders of magnitude slower than eth_getBalance; doing it for
    // empty wallets made a 50-selected / 10-funded run miss a short public sale.
    //
    // This is deliberately only a zero-balance gate. A non-zero balance may
    // still be too small for price plus gas, but the exact requirement is known
    // later and remains enforced by the existing balance gate. RPC failures are
    // fail-open here so transient node trouble cannot recreate silent wallet
    // loss.
    report_phase(
        reporter.as_ref(),
        "funds",
        format!(
            "Checking {} selected wallet balance(s) on {} before OpenSea auth...",
            signers.len(),
            chain_for_rpc
        ),
    );
    let selected_before_early_gate = signers.len();
    let mut balance_handles = Vec::with_capacity(signers.len());
    for (index, signer) in signers.iter().enumerate() {
        let rpc = rpc.clone();
        let address = signer.address();
        balance_handles.push(tokio::spawn(async move {
            (index, address, rpc.balance(&address).await)
        }));
    }

    let mut keep_selected = vec![true; signers.len()];
    let mut zero_balance_count = 0usize;
    let mut balance_read_failures = 0usize;
    for handle in balance_handles {
        match handle.await {
            Ok((index, address, Ok(balance))) => {
                if !keep_before_opensea_auth(Some(balance)) {
                    keep_selected[index] = false;
                    zero_balance_count += 1;
                    report_wallet(
                        reporter.as_ref(),
                        &address,
                        Some(WalletStatus::Failed),
                        Some("skipped before OpenSea auth".to_string()),
                        None,
                        Some(format!(
                            "zero balance on resolved mint network {}",
                            chain_for_rpc
                        )),
                    );
                }
            }
            Ok((_, address, Err(error))) => {
                balance_read_failures += 1;
                log_always(
                    reporter.as_ref(),
                    format!(
                        "  [{}] early balance read failed; wallet retained: {}",
                        sign::shorten_address(&address),
                        error
                    ),
                );
            }
            Err(error) => {
                balance_read_failures += 1;
                log_always(
                    reporter.as_ref(),
                    format!("  early balance worker failed; wallet retained: {}", error),
                );
            }
        }
    }

    let early_signers_owned: Vec<Signer> = signers
        .iter()
        .zip(keep_selected.iter())
        .filter(|(_, keep)| **keep)
        .map(|(signer, _)| signer.clone())
        .collect();
    let early_wallet_proxy_routes: Vec<Option<String>> = wallet_proxy_routes
        .iter()
        .zip(keep_selected.iter())
        .filter(|(_, keep)| **keep)
        .map(|(route, _)| route.clone())
        .collect();
    if early_signers_owned.is_empty() {
        bail!(
            "All {} selected wallets have zero balance on resolved mint network '{}'",
            selected_before_early_gate,
            chain_for_rpc
        );
    }
    log_always(
        reporter.as_ref(),
        format!(
            "Early balance gate: {}/{} wallet(s) retained for OpenSea auth; {} zero-balance skipped; {} RPC read failure(s) retained",
            early_signers_owned.len(),
            selected_before_early_gate,
            zero_balance_count,
            balance_read_failures
        ),
    );

    // Shadow the selected task set only after the chain-aware gate. Routes stay
    // paired by original selected-wallet index, including manual/direct routes.
    let signers: &[Signer] = &early_signers_owned;
    let wallet_proxy_routes = early_wallet_proxy_routes;
    let direct_route_count = wallet_proxy_routes.iter().filter(|p| p.is_none()).count();
    let proxied_route_count = wallet_proxy_routes.len().saturating_sub(direct_route_count);
    let unique_proxy_count = wallet_proxy_routes
        .iter()
        .filter_map(|p| p.as_deref())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Limit parallel SIWE calls. OpenSea returns 429 when all wallets auth at once.
    // Mixed A/B runs get a shared cap plus a direct-only cap of 2, so adding
    // proxy wallets cannot raise concurrency against the VPS public IP.
    let auth_override: Option<usize> = env
        .get("AUTH_CONCURRENCY")
        .and_then(|v| v.trim().parse().ok())
        .filter(|&n| n > 0);
    let default_auth_concurrency = (if direct_route_count > 0 { 2 } else { 0 })
        + if proxied_route_count > 0 {
            crate::safety_policy::default_auth_concurrency(unique_proxy_count)
        } else {
            0
        };
    let auth_concurrency = auth_override
        .unwrap_or(default_auth_concurrency)
        .clamp(1, signers.len().max(1));
    let direct_auth_concurrency = auth_override.unwrap_or(2).clamp(1, 2);
    report_phase(
        reporter.as_ref(),
        "auth",
        format!(
            "Authenticating {} wallet(s) on OpenSea (concurrency={})…",
            signers.len(),
            auth_concurrency
        ),
    );
    log_always(
        reporter.as_ref(),
        format!(
            "Authenticating all wallets (chainId={}, concurrency={})...",
            actual_chain_id, auth_concurrency
        ),
    );
    if crate::safety_policy::should_warn_no_proxy(direct_route_count, 0) {
        log_always(
            reporter.as_ref(),
            crate::safety_policy::no_proxy_multi_wallet_message(direct_route_count),
        );
    } else if direct_route_count > 1 {
        log_always(
            reporter.as_ref(),
            format!(
                "Direct A/B group: {direct_route_count} wallet(s), auth concurrency capped at {direct_auth_concurrency}"
            ),
        );
    }

    let mut auth_cache = auth_cache::AuthCache::load(vault_password);
    let mut wallets: Vec<WalletAuth> = Vec::new();
    let mut auth_handles = Vec::new();
    let auth_sem = Arc::new(tokio::sync::Semaphore::new(auth_concurrency));
    let direct_auth_sem = Arc::new(tokio::sync::Semaphore::new(direct_auth_concurrency));
    // After first 429: serialize remaining auth (don't keep N-way hammering one IP).
    let force_serial = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let serial_mutex = Arc::new(tokio::sync::Mutex::new(()));

    for (i, signer) in signers.iter().enumerate() {
        let addr = signer.address();
        let addr_str = format!("{:?}", addr);
        let signer = signer.clone();
        let chain_id = actual_chain_id;
        let proxy_url = wallet_proxy_routes.get(i).cloned().flatten();
        let proxy_short = proxy_url
            .as_deref()
            .map(crate::proxy::short_proxy)
            .unwrap_or_else(|| "direct".to_string());

        if let Some(cached_token) = auth_cache.get(&addr_str, chain_id) {
            let cookie_jar = std::sync::Arc::new(reqwest::cookie::Jar::default());
            match opensea::build_client_with_cookie_jar_and_proxy(
                cookie_jar.clone(),
                proxy_url.as_deref(),
            ) {
                Ok(client) => {
                    log_always(
                        reporter.as_ref(),
                        format!("[{}] {:?} ... CACHED OK", i + 1, addr),
                    );
                    let session = opensea::AuthSession {
                        access_token: cached_token.to_string(),
                        address: addr_str,
                        client,
                        cookie_jar,
                    };
                    wallets.push(WalletAuth {
                        address: addr,
                        signer,
                        session: Some(session),
                        auth_ok: true,
                        auth_elapsed_ms: None,
                        auth_cached: true,
                        nonce: 0,
                        prefetched_tx: None,
                        pre_signed_tx: None,
                        conditional_hash: None,
                        proxy_url,
                    });
                    continue;
                }
                Err(e) => {
                    log_always(
                        reporter.as_ref(),
                        format!(
                            "[{}] {:?} ... CACHED token but client build failed: {} — re-auth",
                            i + 1,
                            addr,
                            e
                        ),
                    );
                }
            }
        }

        let sem = auth_sem.clone();
        let direct_sem = direct_auth_sem.clone();
        let force_serial = force_serial.clone();
        let serial_mutex = serial_mutex.clone();
        let stagger_ms = (i as u64 % auth_concurrency as u64) * 200;
        let rep = reporter.clone();
        let proxy_url_task = proxy_url.clone();
        let cancel_auth = cancel.clone();
        auth_handles.push(tokio::spawn(async move {
            let proxy_url = proxy_url_task;
            // Stop must not wait for hundreds of queued wallets to acquire a
            // permit. A plain `sem.acquire().await` parks the task until its
            // turn comes, so on a 100-wallet run the desktop's single-flight
            // mint lock stayed held for minutes after the operator had already
            // cancelled. Poll the semaphore instead and bail out on cancel.
            let _permit = loop {
                if cancelled(&cancel_auth) {
                    return (addr, signer, None, false, None, proxy_url, None);
                }
                tokio::select! {
                    permit = sem.acquire() => match permit {
                        Ok(p) => break p,
                        Err(_) => return (addr, signer, None, false, None, proxy_url, None),
                    },
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                }
            };
            if cancelled(&cancel_auth) {
                return (addr, signer, None, false, None, proxy_url, None);
            }
            // The direct-group cap is a second queue with the same problem.
            let _direct_permit = if proxy_url.is_none() {
                let permit = loop {
                    if cancelled(&cancel_auth) {
                        return (addr, signer, None, false, None, proxy_url, None);
                    }
                    tokio::select! {
                        permit = direct_sem.acquire() => match permit {
                            Ok(p) => break p,
                            Err(_) => return (addr, signer, None, false, None, proxy_url, None),
                        },
                        _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {}
                    }
                };
                Some(permit)
            } else {
                None
            };
            if cancelled(&cancel_auth) {
                return (addr, signer, None, false, None, proxy_url, None);
            }
            // After any 429, remaining auths go one-at-a-time.
            let _serial_guard = if force_serial.load(std::sync::atomic::Ordering::SeqCst) {
                Some(serial_mutex.lock().await)
            } else {
                None
            };
            if cancelled(&cancel_auth) {
                return (addr, signer, None, false, None, proxy_url, None);
            }
            // Stagger start within the concurrency window.
            if stagger_ms > 0 && !force_serial.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(stagger_ms)).await;
            }
            if cancelled(&cancel_auth) {
                return (addr, signer, None, false, None, proxy_url, None);
            }

            let start = std::time::Instant::now();
            let result = opensea::siwe_auth(&addr, &signer, chain_id, proxy_url.as_deref()).await;
            let elapsed = start.elapsed().as_millis();
            match result {
                Ok(session) => {
                    log_always(
                        rep.as_ref(),
                        format!(
                            "[{}] {:?} OK ({}ms) via {}",
                            i + 1,
                            addr,
                            elapsed,
                            proxy_short
                        ),
                    );
                    (
                        addr,
                        signer,
                        Some(session),
                        true,
                        None,
                        proxy_url,
                        Some(elapsed as u64),
                    )
                }
                Err(e) => {
                    let err_s = format!("{e}");
                    if crate::safety_policy::is_rate_limit_error(&err_s) {
                        force_serial.store(true, std::sync::atomic::Ordering::SeqCst);
                        log_always(
                            rep.as_ref(),
                            format!(
                                "[{}] rate limit — {}",
                                sign::shorten_address(&addr),
                                crate::safety_policy::rate_limit_actionable_message()
                            ),
                        );
                    }
                    let msg = format!(
                        "[{}] {:?} FAILED ({}ms) via {}: {}",
                        i + 1,
                        addr,
                        elapsed,
                        proxy_short,
                        e
                    );
                    log_always(rep.as_ref(), msg.clone());
                    (
                        addr,
                        signer,
                        None,
                        false,
                        Some(msg),
                        proxy_url,
                        Some(elapsed as u64),
                    )
                }
            }
        }));
    }

    for handle in auth_handles {
        match handle.await {
            Ok((addr, signer, session, auth_ok, _err, proxy_url, auth_elapsed_ms)) => {
                if auth_ok {
                    if let Some(ref sess) = session {
                        let addr_str = format!("{:?}", addr);
                        auth_cache.save(&addr_str, actual_chain_id, &sess.access_token);
                    }
                }
                wallets.push(WalletAuth {
                    address: addr,
                    signer,
                    session,
                    auth_ok,
                    auth_elapsed_ms,
                    auth_cached: false,
                    nonce: 0,
                    prefetched_tx: None,
                    pre_signed_tx: None,
                    conditional_hash: None,
                    proxy_url,
                });
            }
            Err(e) => log_always(reporter.as_ref(), format!("Auth task failed: {}", e)),
        }
    }

    // One disk encrypt (PBKDF2) for the whole auth batch — not per wallet.
    if let Err(e) = auth_cache.flush() {
        log_always(
            reporter.as_ref(),
            format!("WARN: auth cache flush failed: {e}"),
        );
    }

    // A SIWE request already in flight cannot be interrupted safely, but the
    // queued ones now exit promptly — so stop here instead of continuing into
    // phase / availability work. Returning releases the desktop's busy guard,
    // which lets the operator start the next task right away.
    if cancelled(&cancel) {
        bail!("Mint cancelled during wallet authentication");
    }

    let auth_ok_count = wallets.iter().filter(|w| w.auth_ok).count();
    if auth_ok_count == 0 {
        bail!("All wallets failed authentication");
    }
    log_always(
        reporter.as_ref(),
        format!("Auth: {}/{} wallets OK", auth_ok_count, wallets.len()),
    );
    log_always(
        reporter.as_ref(),
        route_auth_summary("direct", &wallets, false),
    );
    log_always(
        reporter.as_ref(),
        route_auth_summary("proxy", &wallets, true),
    );

    log_always(
        reporter.as_ref(),
        format!("\nRe-fetching collection info with auth for eligibility..."),
    );
    let primary = wallets
        .iter()
        .find(|w| w.auth_ok)
        .ok_or_else(|| anyhow::anyhow!("internal: auth_ok_count>0 but no auth_ok wallet"))?;
    let primary_session = primary
        .session
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("internal: auth_ok wallet missing OpenSea session"))?;
    let info = match opensea::collection_drop_info(primary_session, &slug, &primary.address).await {
        Ok(i) => i,
        Err(e) => {
            log_always(
                reporter.as_ref(),
                format!("Failed to re-fetch collection info: {}", e),
            );
            log_always(reporter.as_ref(), format!("Using unauthenticated data..."));
            info
        }
    };

    log_always(
        reporter.as_ref(),
        format!("\nCollection: {} ({})", info.name, info.slug),
    );
    log_always(
        reporter.as_ref(),
        format!("Chain: {} (chainId {})", info.chain, actual_chain_id),
    );
    if let Some(ref dt) = info.drop_type {
        log_always(reporter.as_ref(), format!("Drop type: {}", dt));
    }
    if !info.contracts.is_empty() {
        log_always(
            reporter.as_ref(),
            format!("NFT contract: {}", info.contracts[0]),
        );
    }

    let stages = &info.stages;
    if stages.is_empty() {
        bail!("No drop stages found");
    }

    log_always(reporter.as_ref(), format!("\nPhases:"));
    let mut phase_labels = Vec::new();
    for (i, stage) in stages.iter().enumerate() {
        let price = stage
            .price_eth
            .map(|p| format!("{} ETH", p))
            .unwrap_or_else(|| "-".to_string());
        let eligible = opensea::stage_eligibility_label(stage);
        let label = opensea::stage_label(stage);
        let available = opensea::available_mint_quantity(&info, stage)
            .map(|q| format!("available={}", q))
            .unwrap_or_else(|| "available=?".to_string());
        let start = stage
            .start_time
            .map(|t| format!("start={:.0}", t))
            .unwrap_or_default();
        let end = stage
            .end_time
            .map(|t| format!("end={:.0}", t))
            .unwrap_or_default();
        phase_labels.push(format!(
            "#{} {:30} {:12} {:10} {:12} {} {} {}",
            i + 1,
            label,
            stage.stage_type,
            eligible,
            available,
            price,
            start,
            end
        ));
        log_always(
            reporter.as_ref(),
            format!(
                "  {} | {} | {} | {} | {} | {} | {} | {}",
                i + 1,
                label,
                stage.stage_type,
                eligible,
                available,
                price,
                start,
                end
            ),
        );
    }
    if stages
        .iter()
        .any(|stage| stage.stage_type == "SIGNED_PRESALE" && stage.is_eligible.is_none())
    {
        log_always(
            reporter.as_ref(),
            format!(
                "  Note: signed phase eligibility is unknown from OpenSea phase list; selected wallet availability is checked after phase selection."
            ),
        );
    }

    let now = crate::timing::true_now_secs();
    let default_pick = stages
        .iter()
        .enumerate()
        .filter(|(_, s)| opensea::stage_is_selectable_at(s, now))
        .filter(|(_, s)| opensea::available_mint_quantity(&info, s).unwrap_or(0) > 0)
        .min_by_key(|(_, s)| {
            let is_public = s.stage_type == "PUBLIC_SALE";
            // Started = start_time <= wall clock now (missing start_time counts as started).
            // Comparing against 0 marked every real (past) timestamp as "not started".
            let has_started = s.start_time.map(|t| t as i64 <= now).unwrap_or(true);
            (
                is_public as usize,
                !has_started as usize,
                s.stage_index.unwrap_or(0),
            )
        })
        .map(|(i, _)| i)
        .or_else(|| {
            stages
                .iter()
                .position(|stage| !opensea::stage_is_expired_at(stage, now))
        })
        .context("No open or upcoming eligible drop stages found")?;
    if default_pick > 0 || stages.len() > 1 {
        let rec_stage = &stages[default_pick];
        let rec_available = opensea::available_mint_quantity(&info, rec_stage).unwrap_or(0);
        log_always(
            reporter.as_ref(),
            format!(
                "  Recommended: #{} {} (available={}, fastest={})",
                default_pick + 1,
                opensea::stage_label(rec_stage),
                rec_available,
                if rec_stage.stage_type == "PUBLIC_SALE" {
                    "local build"
                } else {
                    "GQL"
                }
            ),
        );
    }
    let pick = if let Some(idx) = opts.phase_index {
        if idx >= stages.len() {
            bail!("phase_index {} out of range (0..{})", idx, stages.len());
        }
        idx
    } else {
        log_always(
            reporter.as_ref(),
            format!("Auto mode: using recommended phase #{}", default_pick + 1),
        );
        default_pick
    };
    if opts.phase_index.is_none() {
        let now = crate::timing::true_now_secs();
        let selected_has_started = stages[pick]
            .start_time
            .map(|timestamp| timestamp as i64 <= now)
            .unwrap_or(true);
        if selected_has_started
            && let Some((future_index, future_stage)) = stages
                .iter()
                .enumerate()
                .filter(|(_, candidate)| opensea::stage_effective_eligible(candidate))
                .filter(|(_, candidate)| {
                    opensea::available_mint_quantity(&info, candidate).unwrap_or(0) > 0
                })
                .filter(|(_, candidate)| {
                    candidate
                        .start_time
                        .map(|timestamp| timestamp as i64 > now)
                        .unwrap_or(false)
                })
                .min_by_key(|(_, candidate)| candidate.start_time.map(|value| value as i64))
        {
            log_always(
                reporter.as_ref(),
                format!(
                    "WARN: Auto selected already-open phase #{} {}; future eligible phase #{} {} opens at unix {:.0}. Select the phase explicitly for a scheduled snipe.",
                    pick + 1,
                    opensea::stage_label(&stages[pick]),
                    future_index + 1,
                    opensea::stage_label(future_stage),
                    future_stage.start_time.unwrap_or_default()
                ),
            );
        }
    }
    let _ = (auto_mode, &phase_labels);
    let stage = &stages[pick];
    if opensea::stage_is_expired_at(stage, crate::timing::true_now_secs()) {
        bail!(
            "Selected phase {} is closed (ended at unix {:.0}); reload phases and choose an open/upcoming phase",
            opensea::stage_label(stage),
            stage.end_time.unwrap_or_default()
        );
    }
    let expected_unit_price_wei = match opts.expected_unit_price_wei.as_deref() {
        Some(value) => {
            parse_hex_u256(value).with_context(|| format!("Invalid saved phase price '{value}'"))?
        }
        None if dry_run => stage.price_wei.unwrap_or(U256::ZERO),
        None => bail!(
            "This task has no saved phase-price snapshot. Reload phases and save the task again before LIVE mint"
        ),
    };
    let metadata_price_wei = stage.price_wei.context(
        "OpenSea phase has no exact price; refusing LIVE mint without a verifiable price",
    )?;
    if metadata_price_wei != expected_unit_price_wei {
        bail!(
            "Phase price changed after the task was saved: expected {} wei, OpenSea now reports {} wei; reload phases and review the new terms",
            expected_unit_price_wei,
            metadata_price_wei
        );
    }
    log_always(
        reporter.as_ref(),
        format!("Selected: {}", opensea::stage_label(stage)),
    );
    if let Some(available) = opensea::available_mint_quantity(&info, stage) {
        if available == 0 {
            bail!("Selected phase has no NFTs available for this wallet");
        }
        if quantity > available {
            log_always(
                reporter.as_ref(),
                format!(
                    "Requested quantity {} exceeds available {}; using {}",
                    quantity, available, available
                ),
            );
            quantity = available;
        } else {
            log_always(
                reporter.as_ref(),
                format!("Available for this wallet in selected phase: {}", available),
            );
        }
        quantity = quantity.min(available).max(1);
    } else {
        log_always(
            reporter.as_ref(),
            format!(
                "Available quantity for selected phase is unknown; using requested {}",
                quantity
            ),
        );
        quantity = quantity.max(1);
    }
    log_always(reporter.as_ref(), format!("Mint quantity: {}", quantity));

    let selected_stage_type = stage.stage_type.clone();
    let selected_stage_index = stage.stage_index;
    let mut wallet_quantities: std::collections::HashMap<alloy_primitives::Address, u32> =
        std::collections::HashMap::new();
    // Seed requested qty per wallet (task per-wallet map or default quantity).
    {
        let addrs: Vec<String> = wallets
            .iter()
            .filter(|w| w.auth_ok)
            .map(|w| format!("{:?}", w.address))
            .collect();
        let expanded = crate::mint_ops::expand_wallet_quantities(
            quantity,
            &addrs,
            opts.wallet_quantities.as_ref(),
        );
        for w in &mut wallets {
            if !w.auth_ok {
                continue;
            }
            let k = crate::mint_ops::normalize_addr_key(&format!("{:?}", w.address));
            match expanded.get(&k).copied() {
                Some(q) => {
                    wallet_quantities.insert(w.address, q.max(1));
                }
                None => {
                    // `expand_wallet_quantities` drops zero entries, and every
                    // auth-ok address was passed in, so a missing key can only
                    // mean an explicit qty of 0 — i.e. "exclude this wallet".
                    // `unwrap_or(quantity)` used to resurrect it at the full
                    // default and mint (and pay) with a wallet the caller had
                    // deliberately excluded.
                    w.auth_ok = false;
                }
            }
        }
    }
    report_phase(
        reporter.as_ref(),
        "prep",
        format!(
            "Checking phase availability ({} wallets, parallel)…",
            wallets.iter().filter(|w| w.auth_ok).count()
        ),
    );
    log_always(
        reporter.as_ref(),
        format!("\nChecking selected phase availability per wallet (parallel)…"),
    );
    // Parallel OpenSea availability — bounded concurrency to limit 429s.
    {
        let avail_conc = env
            .get("AVAIL_CONCURRENCY")
            .and_then(|v| v.trim().parse().ok())
            .filter(|&n: &usize| n > 0)
            .unwrap_or_else(|| {
                // Safer defaults: fewer parallel OS drop calls (less 429).
                (if direct_route_count > 0 { 2 } else { 0 })
                    + if proxied_route_count > 0 {
                        crate::safety_policy::default_auth_concurrency(unique_proxy_count)
                    } else {
                        0
                    }
            });
        let sem = Arc::new(tokio::sync::Semaphore::new(avail_conc));
        let direct_sem = Arc::new(tokio::sync::Semaphore::new(2));
        let mut handles = Vec::new();
        for w in &wallets {
            if !w.auth_ok {
                continue;
            }
            let Some(session) = w.session.clone() else {
                continue;
            };
            let addr = w.address;
            let requested = wallet_quantities.get(&addr).copied().unwrap_or(quantity);
            let slug = slug.clone();
            let selected_stage_type = selected_stage_type.clone();
            let selected_stage_index = selected_stage_index;
            let sem = sem.clone();
            let direct_sem = direct_sem.clone();
            let is_direct = w.proxy_url.is_none();
            let rep = reporter.clone();
            handles.push(tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => {
                        return (addr, requested, Ok::<u32, ()>(requested), None);
                    }
                };
                let _direct_permit = if is_direct {
                    match direct_sem.acquire().await {
                        Ok(p) => Some(p),
                        Err(_) => {
                            return (addr, requested, Ok::<u32, ()>(requested), None);
                        }
                    }
                } else {
                    None
                };
                match opensea::collection_drop_info(&session, &slug, &addr).await {
                    Ok(wallet_info) => {
                        let wallet_stage = wallet_info.stages.iter().find(|s| {
                            s.stage_type == selected_stage_type
                                && s.stage_index == selected_stage_index
                        });
                        let available = wallet_stage
                            .and_then(|s| opensea::available_mint_quantity(&wallet_info, s))
                            .unwrap_or(requested);
                        let wallet_quantity = requested.min(available);
                        let msg = if wallet_quantity == 0 {
                            Some(format!(
                                "[{}] selected phase available=0, skipping wallet",
                                sign::shorten_address(&addr)
                            ))
                        } else if wallet_quantity < requested {
                            Some(format!(
                                "[{}] requested {} but available {}; using {}",
                                sign::shorten_address(&addr),
                                requested,
                                available,
                                wallet_quantity
                            ))
                        } else {
                            Some(format!(
                                "[{}] available={} quantity={}",
                                sign::shorten_address(&addr),
                                available,
                                wallet_quantity
                            ))
                        };
                        if let Some(ref m) = msg {
                            log_always(rep.as_ref(), m.clone());
                        }
                        (addr, requested, Ok(wallet_quantity), msg)
                    }
                    Err(e) => {
                        let m = format!(
                            "[{}] failed to check wallet availability: {}; using requested {}",
                            sign::shorten_address(&addr),
                            e,
                            requested
                        );
                        log_always(rep.as_ref(), m.clone());
                        (addr, requested, Ok(requested), Some(m))
                    }
                }
            }));
        }
        for h in handles {
            if let Ok((addr, _req, res, _msg)) = h.await {
                match res {
                    Ok(0) => {
                        if let Some(w) = wallets.iter_mut().find(|w| w.address == addr) {
                            w.auth_ok = false;
                        }
                        wallet_quantities.remove(&addr);
                    }
                    Ok(q) => {
                        wallet_quantities.insert(addr, q);
                    }
                    Err(_) => {}
                }
            }
        }
    }
    if wallets.iter().filter(|w| w.auth_ok).count() == 0 {
        bail!("No wallets have available mints in selected phase");
    }

    // Settings / .env → gas + retries + sniper flags.
    let mut gas_params = GasParams::from_env(env);
    if let Some(multiplier) = opts
        .base_fee_multiplier
        .filter(|value| *value >= 1.0 && *value <= 5.0)
    {
        gas_params.base_fee_multiplier = multiplier;
    }
    let max_attempts = max_retries_from_env(env);
    let quiet = opts.quiet.unwrap_or_else(|| quiet_from_env(env));
    let skip_preflight_flag = opts
        .skip_preflight
        .unwrap_or_else(|| skip_preflight_from_env(env));
    let beep = beep_from_env(env);
    let do_export = export_results_from_env(env);
    let first_confirm = Arc::new(AtomicBool::new(false));
    let priority_input = opts
        .priority_fee_gwei
        .clone()
        .unwrap_or_default()
        .trim()
        .to_string();
    if !priority_input.is_empty() {
        match priority_input.parse::<f64>() {
            Ok(pg) if pg > 0.0 => {
                gas_params = gas_params.with_priority_gwei(pg);
            }
            _ => {
                log_always(
                    reporter.as_ref(),
                    format!(
                        "Invalid priority fee '{}', keeping env/settings gas params",
                        priority_input
                    ),
                );
            }
        }
    }
    // Gas limit: MintOptions override → env/settings GAS_LIMIT. 0 = estimate (auto).
    let fixed_gas_limit: Option<u64> = {
        let gl = opts.gas_limit.unwrap_or_else(|| {
            env.get("GAS_LIMIT")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(250_000)
        });
        if gl == 0 { None } else { Some(gl) }
    };
    let skip_preflight = skip_preflight_flag && fixed_gas_limit.is_some();
    if skip_preflight_flag && fixed_gas_limit.is_none() {
        log_always(
            reporter.as_ref(),
            format!("WARN: SKIP_PREFLIGHT ignored because GAS_LIMIT=0 (estimate mode)"),
        );
    }
    log_always(
        reporter.as_ref(),
        format!(
            "Gas: mode={:?} base_mult={} gas_mult={} priority={} max_retries={} quiet={} skip_preflight={}",
            gas_params.mode,
            gas_params.base_fee_multiplier,
            gas_params.gas_multiplier,
            gas_params
                .priority_fee
                .map(|p| format!("{} gwei", p / U256::from(1_000_000_000u64)))
                .unwrap_or_else(|| "network".to_string()),
            max_attempts,
            quiet,
            skip_preflight
        ),
    );

    let nft_contract = info
        .contracts
        .first()
        .map(|c| c.as_str())
        .unwrap_or("0x0000000000000000000000000000000000000000");
    let payment_asset = opensea::stage_payment_asset(&info, stage);
    // Use the operator-approved snapshot, never a silently refreshed price.
    let mut price_wei = expected_unit_price_wei;
    let stage_type_owned = stage.stage_type.clone();
    let stage_token_id = opensea::stage_token_id(stage);
    let seadrop_address = env.get("SEADROP_ADDRESS").cloned();
    let mut fee_recipient = env.get("FEE_RECIPIENT").cloned();
    let mut public_drop_final_verified = false;

    // Explicit schedule must parse cleanly — never silently fall back to phase start.
    let stage_start_ts: Option<i64> = if let Some(ref at_str) = at_time {
        match crate::mint_ops::parse_at_time_unix(at_str) {
            Ok(Some(ts)) => {
                log_always(
                    reporter.as_ref(),
                    format!("Scheduled mint at unix {ts} (from at_time={at_str})"),
                );
                Some(ts)
            }
            Ok(None) => {
                // Empty string after trim — treat as unset.
                stage.start_time.map(|t| t as i64)
            }
            Err(e) => {
                bail!("{e}. Use unix seconds/ms or ISO 8601 / RFC3339.");
            }
        }
    } else {
        stage.start_time.map(|t| t as i64)
    };
    if conditional_submit_enabled {
        let start = stage_start_ts
            .context("Conditional submit needs a phase start or explicit future At time")?;
        if start.saturating_mul(1000) <= crate::timing::true_now_ms() {
            bail!(
                "Conditional submit needs a future fire time; set At time or select a future phase"
            );
        }
    }

    let gas_info = {
        let prio = gas_params
            .priority_fee
            .map(|p| format!("{}gwei", p / U256::from(1_000_000_000u64)))
            .unwrap_or_else(|| "auto".to_string());
        let gl = fixed_gas_limit
            .map(|g| format!("fixed={}", g))
            .unwrap_or_else(|| "est".into());
        let mut s = format!(
            "{} prio={} base*{} gas*{} retries={}",
            gl, prio, gas_params.base_fee_multiplier, gas_params.gas_multiplier, max_attempts
        );
        if dry_run {
            s = format!("[DRY-RUN] {}", s);
        }
        s
    };
    for w in wallets.iter() {
        let px = w
            .proxy_url
            .as_deref()
            .map(crate::proxy::short_proxy)
            .unwrap_or_else(|| "direct".to_string());
        if w.auth_ok {
            report_wallet(
                reporter.as_ref(),
                &w.address,
                Some(WalletStatus::Wait),
                Some(format!("proxy={}", px)),
                None,
                None,
            );
        } else {
            report_wallet(
                reporter.as_ref(),
                &w.address,
                Some(WalletStatus::Failed),
                None,
                None,
                Some("auth failed".into()),
            );
        }
    }
    log_always(
        reporter.as_ref(),
        format!(
            "{} wallet(s) auth OK, qty={}, retries={}, gas={}",
            auth_ok_count, quantity, max_attempts, gas_info
        ),
    );

    let chain_id = actual_chain_id;

    log_always(
        reporter.as_ref(),
        format!("\nRefreshing nonces for all wallets..."),
    );
    let mut nonce_handles = Vec::new();
    for w in &wallets {
        if !w.auth_ok {
            continue;
        }
        let rpc = rpc.clone();
        let addr = w.address;
        nonce_handles.push(tokio::spawn(async move {
            let result = rpc.nonce(&addr).await;
            (addr, result)
        }));
    }
    for handle in nonce_handles {
        if let Ok((addr, result)) = handle.await {
            if let Some(w) = wallets.iter_mut().find(|w| w.address == addr) {
                match result {
                    Ok(n) => {
                        w.nonce = n;
                        report_wallet(
                            reporter.as_ref(),
                            &w.address,
                            Some(WalletStatus::Wait),
                            Some(format!("nonce={}", n)),
                            None,
                            None,
                        );
                    }
                    Err(e) => {
                        w.auth_ok = false;
                        report_wallet(
                            reporter.as_ref(),
                            &w.address,
                            Some(WalletStatus::Failed),
                            None,
                            None,
                            Some(format!("nonce: {}", e)),
                        );
                    }
                }
            }
        }
    }

    log_always(reporter.as_ref(), format!("\nChecking balances..."));
    // This is the one fee snapshot used by both the balance gate and signing.
    // Keeping those values identical prevents a wallet from passing preparation
    // with a cheaper fee than the transaction eventually carries.
    let fee_snapshot = rpc
        .fee_history()
        .await
        .unwrap_or((U256::from(1_000_000_000u64), U256::from(1_000_000_000u64)));
    let (mut max_fee, mut max_priority_fee) =
        gas::calculate_fees(&gas_params, fee_snapshot.0, fee_snapshot.1).unwrap_or((
            fee_snapshot.0 * U256::from(2u64) + fee_snapshot.1,
            fee_snapshot.1,
        ));
    // Resolve the same fixed live limit that workers sign with. A gas limit is
    // only a ceiling (unused gas is not charged), but the account must be able
    // to cover that ceiling for an RPC to accept the transaction.
    let pre_sign_gas_limit = resolve_mint_gas_limit(
        fixed_gas_limit.unwrap_or(250_000),
        gas_params.gas_multiplier,
        chain_id,
        true,
    );
    {
        let before_balance = wallets.iter().filter(|w| w.auth_ok).count();
        let mut bal_handles = Vec::new();
        for w in &wallets {
            if !w.auth_ok {
                continue;
            }
            let rpc = rpc.clone();
            let addr = w.address;
            let qty = wallet_quantities.get(&addr).copied().unwrap_or(quantity);
            let val = price_wei * U256::from(qty);
            bal_handles.push(tokio::spawn(async move {
                let bal = rpc.balance(&addr).await;
                (addr, bal, val)
            }));
        }
        for handle in bal_handles {
            if let Ok((addr, bal_result, mint_value)) = handle.await {
                if let Some(w) = wallets.iter_mut().find(|w| w.address == addr) {
                    match bal_result {
                        Ok(bal) => {
                            let needed =
                                required_mint_balance(mint_value, pre_sign_gas_limit, max_fee);
                            let bal_eth = format!(
                                "{:.6}",
                                (bal / U256::from(1e12 as u64)).to::<u128>() as f64 / 1e6
                            );
                            let need_eth = format!(
                                "{:.6}",
                                (needed / U256::from(1e12 as u64)).to::<u128>() as f64 / 1e6
                            );
                            if bal < needed {
                                let deficit = needed - bal;
                                let def_eth = format!(
                                    "{:.6}",
                                    (deficit / U256::from(1e12 as u64)).to::<u128>() as f64 / 1e6
                                );
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "  [{}] LOW BALANCE: {} ETH (need {} ETH, deficit {} ETH)",
                                        sign::shorten_address(&addr),
                                        bal_eth,
                                        need_eth,
                                        def_eth
                                    ),
                                );
                                // Both live and dry-run: do not count as OK without funds.
                                // Tx would not succeed (and estimateGas fails with OutOfFunds).
                                w.auth_ok = false;
                                report_wallet(
                                    reporter.as_ref(),
                                    &w.address,
                                    Some(WalletStatus::Failed),
                                    None,
                                    None,
                                    Some(format!(
                                        "insufficient balance: have {} ETH, need {} ETH",
                                        bal_eth, need_eth
                                    )),
                                );
                            } else {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "  [{}] balance={} ETH (need {} ETH) OK",
                                        sign::shorten_address(&addr),
                                        bal_eth,
                                        need_eth
                                    ),
                                );
                            }
                        }
                        Err(e) => {
                            log_always(
                                reporter.as_ref(),
                                format!(
                                    "  [{}] balance check failed: {}",
                                    sign::shorten_address(&addr),
                                    e
                                ),
                            );
                        }
                    }
                }
            }
        }
        let funded = wallets.iter().filter(|w| w.auth_ok).count();
        if funded == 0 {
            bail!("No wallets with sufficient balance to mint");
        }
        if funded < before_balance {
            log_always(
                reporter.as_ref(),
                format!(
                    "Balance gate: {}/{} wallets remaining after low-balance skip",
                    funded, before_balance
                ),
            );
        }
    }

    // Proxy probe skipped on mint hot path (use Proxies page). Soft checklist only.
    let proxy_slots = if proxied_route_count == 0 {
        0
    } else {
        proxied_route_count
    };
    log_always(
        reporter.as_ref(),
        format!(
            "\nOpenSea routes: direct={}, proxy={} ({} unique endpoints) — probe skipped on mint path",
            direct_route_count, proxied_route_count, unique_proxy_count
        ),
    );

    let ready_wallets = wallets.iter().filter(|w| w.auth_ok).count();
    let mut checklist = vec![
        format!("✓ Collection   {}", info.slug),
        format!("✓ Phase        {}", opensea::stage_label(stage)),
        format!("✓ Chain        {} (id={})", info.chain, actual_chain_id),
        format!(
            "✓ Wallets      {} ready / {} total (qty={})",
            ready_wallets,
            wallets.len(),
            quantity
        ),
        format!(
            "✓ Routes       {} direct / {} proxy ({} unique)",
            direct_route_count, proxied_route_count, unique_proxy_count
        ),
        format!(
            "✓ Gas          {} · skip_preflight={} · quiet={}",
            gas_info, skip_preflight, quiet
        ),
        format!("✓ Price        {} wei × qty", price_wei),
    ];
    let _ = proxy_slots;
    if let Some(ts) = stage_start_ts {
        checklist.push(format!("✓ Start        unix {}", ts));
    }
    let mut can_start = ready_wallets > 0;
    if ready_wallets == 0 {
        checklist.push("! No wallets ready to mint".into());
        can_start = false;
    }

    log_always(reporter.as_ref(), "--- Checklist ---");
    for l in &checklist {
        log_always(reporter.as_ref(), l.clone());
    }
    if !can_start {
        bail!("Cannot start mint — no wallets ready");
    }

    let mut scheduled_fire_lag_ms: Option<u64> = None;
    let mut scheduled_pre_signed = 0usize;
    let mut scheduled_direct_pre_signed = 0usize;
    let mut scheduled_proxy_pre_signed = 0usize;
    let mut scheduled_direct_ready = 0usize;
    let mut scheduled_proxy_ready = 0usize;
    if let Some(start_ts) = stage_start_ts {
        // OpenSea stage.start_time is wall-clock (unix). Waiting on eth block.timestamp
        // lags ~1 block (~12s on L1) — that is why logs showed "opens in ~1s" then
        // "Phase is open!" only ~12s later. Fire on wall clock.
        let target_ms = start_ts.saturating_mul(1000);
        // Correct the clock before any late/early decision. The bounded NTP
        // query is skipped when T0 is already close, so it cannot delay fire.
        if target_ms.saturating_sub(chrono::Utc::now().timestamp_millis())
            > crate::timing::CLOCK_SYNC_MIN_LEAD_MS
        {
            log_always(reporter.as_ref(), crate::timing::sync_clock().await);
        }
        let before_wait_ms = crate::timing::true_now_ms();
        if before_wait_ms >= target_ms {
            log_always(
                reporter.as_ref(),
                format!(
                    "WARN: LATE START by {}ms; pre-open preparation and readiness reporting are unavailable, firing immediately",
                    before_wait_ms.saturating_sub(target_ms)
                ),
            );
        }
        let open_at = chrono::DateTime::from_timestamp(start_ts, 0)
            .map(|d| d.format("%H:%M:%S UTC").to_string())
            .unwrap_or_else(|| start_ts.to_string());
        report_phase(
            reporter.as_ref(),
            "wait",
            format!("Waiting for phase open at {open_at}…"),
        );
        log_always(
            reporter.as_ref(),
            format!("\nWaiting for phase open (wall clock) at {open_at} (unix={start_ts})"),
        );
        let mut prefetched = false;
        let mut prep_frozen = false;
        let mut conditional_started = false;
        let mut short_probe_started = false;
        let mut gql_warm_started = false;
        let mut fee_refreshed = false;
        let mut last_printed = -1i64;
        let prefetch_lead_ms = if conditional_submit_enabled {
            5_000u64.max(conditional_lead_ms.saturating_add(3_000))
        } else {
            5_000
        } as i64;
        let preopen_finalize_lead_ms = if conditional_submit_enabled {
            PREOPEN_PUBLIC_FINALIZE_LEAD_MS.max(conditional_lead_ms.saturating_add(5_000))
        } else {
            PREOPEN_PUBLIC_FINALIZE_LEAD_MS
        } as i64;

        let configured_use_gql = opts.use_gql.unwrap_or_else(|| {
            env.get("USE_GQL")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false)
        });
        // PUBLIC_SALE has deterministic SeaDrop calldata and must always use
        // the local builder. A stale global USE_GQL setting must not reinsert
        // network work into its pre-fire hot path.
        let use_gql = configured_use_gql && stage_type_owned != "PUBLIC_SALE";
        if configured_use_gql && stage_type_owned == "PUBLIC_SALE" {
            log_always(
                reporter.as_ref(),
                "PUBLIC_SALE: ignoring Use GraphQL; using deterministic local calldata".to_string(),
            );
        }
        let preopen_plan = scheduled_preopen_plan(&stage_type_owned, use_gql);
        let local_public_prefetch = preopen_plan == ScheduledPreopenPlan::ValidatePublicState;
        // Non-public stages use the nonce and fee snapshot already collected
        // during preparation. They must do no blockchain reads near T0. Public
        // stages additionally validate their mutable on-chain configuration,
        // but do so with a wide safety margin rather than in the final seconds.
        let mut preopen_finalized = !local_public_prefetch;
        // PUBLIC_SALE can be built locally before T0. Signed phases deliberately
        // reserve their limited OpenSea mint-action requests for T0 because the
        // service does not issue transactionSubmissionData before the stage opens.
        let should_prefetch = true;
        let pre_sign_enabled = should_prefetch && !dry_run && !use_flashbots;

        let mut prefetch_handles: tokio::task::JoinSet<(
            alloy_primitives::Address,
            Option<PrefetchedMintTx>,
        )> = tokio::task::JoinSet::new();

        let mut timer_guard = None;

        loop {
            if cancelled(&cancel) {
                bail!("Mint cancelled while waiting for phase open");
            }
            let wall_ms = crate::timing::true_now_ms();
            let remaining_ms = target_ms.saturating_sub(wall_ms);
            if remaining_ms <= 0 {
                break;
            }
            if timer_guard.is_none() && remaining_ms <= 5_000 {
                timer_guard = Some(crate::timer_resolution::TimerResolutionGuard::activate());
            }

            // The task may have been armed minutes or hours ago. Refresh the
            // fee ceiling before the final signing pass, not at T0. This keeps
            // the prepared 1.30x L2 cap current without putting an RPC read in
            // front of the actual broadcast.
            let fee_refresh_lead_ms = 5_000u64.max(
                conditional_submit_enabled
                    .then_some(conditional_lead_ms.saturating_add(1_500))
                    .unwrap_or_default(),
            ) as i64;
            if !fee_refreshed && remaining_ms <= fee_refresh_lead_ms {
                fee_refreshed = true;
                match tokio::time::timeout(std::time::Duration::from_millis(900), rpc.fee_history())
                    .await
                {
                    Ok(Ok((base_fee, network_priority))) => {
                        if let Ok((fresh_max, fresh_priority)) =
                            gas::calculate_fees(&gas_params, base_fee, network_priority)
                        {
                            max_fee = fresh_max;
                            max_priority_fee = fresh_priority;
                            for wallet in wallets.iter_mut() {
                                wallet.pre_signed_tx = None;
                            }
                            log_always(
                                reporter.as_ref(),
                                format!(
                                    "  Gas refreshed before fire: max={} gwei, priority={} gwei",
                                    max_fee / U256::from(1_000_000_000u64),
                                    max_priority_fee / U256::from(1_000_000_000u64)
                                ),
                            );
                        }
                    }
                    Ok(Err(error)) => log_always(
                        reporter.as_ref(),
                        format!(
                            "WARN: pre-fire gas refresh failed; keeping prepared fees: {error}"
                        ),
                    ),
                    Err(_) => log_always(
                        reporter.as_ref(),
                        "WARN: pre-fire gas refresh timed out; keeping prepared fees".to_string(),
                    ),
                }
            }

            let left = remaining_ms.saturating_add(999) / 1000;

            if left != last_printed {
                // Keep logs useful: minute/30s checkpoints, then 5s, then the
                // final ten-second precision window. The old one-line-per-second
                // countdown buried the actual failure hundreds of lines down.
                let should_report =
                    left <= 10 || (left <= 30 && left % 5 == 0) || (left > 30 && left % 30 == 0);
                if should_report {
                    let m = left / 60;
                    let s = left % 60;
                    report_phase(
                        reporter.as_ref(),
                        "wait",
                        if m > 0 {
                            format!("Phase opens in ~{m}m {s:02}s")
                        } else {
                            format!("Phase opens in ~{s}s")
                        },
                    );
                }
                last_printed = left;
            }

            // Validate OpenSea's persisted-query id a full minute before T0.
            // This is separate from the 30s transport warm so even a slow
            // precheck has ample time to refill its one request-budget token.
            if !short_probe_started && !local_public_prefetch && remaining_ms <= 60_000 {
                short_probe_started = true;
                if remaining_ms >= 15_000 {
                    if let Some(wallet) = wallets
                        .iter()
                        .find(|wallet| wallet.auth_ok && wallet.session.is_some())
                    {
                        let session = wallet.session.clone().expect("checked above");
                        let address = wallet.address;
                        let probe_quantity =
                            wallet_quantities.get(&address).copied().unwrap_or(quantity);
                        let probe = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            opensea::probe_mint_action_short(
                                &session,
                                &address,
                                nft_contract,
                                &info.chain,
                                &stage_token_id,
                                probe_quantity,
                                &payment_asset,
                            ),
                        )
                        .await;
                        match probe {
                            Ok(Ok(opensea::MintActionShortProbe::Available { elapsed_ms })) => {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "OpenSea SHORT PRECHECK OK {elapsed_ms}ms hash={}…{}; T0 short path armed",
                                        &opensea::MINT_ACTION_TIMELINE_HASH[..8],
                                        &opensea::MINT_ACTION_TIMELINE_HASH[60..]
                                    ),
                                );
                            }
                            Ok(Ok(opensea::MintActionShortProbe::Expired { elapsed_ms })) => {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "WARN OpenSea SHORT HASH EXPIRED in precheck after {elapsed_ms}ms; FULL FALLBACK armed for all wallets"
                                    ),
                                );
                            }
                            Ok(Err(error)) => log_always(
                                reporter.as_ref(),
                                format!(
                                    "WARN OpenSea SHORT PRECHECK INCONCLUSIVE: {error}; T0 will try SHORT once"
                                ),
                            ),
                            Err(_) => log_always(
                                reporter.as_ref(),
                                "WARN OpenSea SHORT PRECHECK TIMEOUT 5000ms; T0 will try SHORT once"
                                    .to_string(),
                            ),
                        }
                    }
                } else {
                    log_always(
                        reporter.as_ref(),
                        "OpenSea SHORT PRECHECK skipped: less than 15s to T0; T0 will try SHORT once"
                            .to_string(),
                    );
                }
            }

            // Auth/eligibility often finishes minutes before a scheduled mint.
            // Warm each wallet's exact proxy -> gql.opensea.io connection outside
            // the hot path.  Never start this close enough to threaten T0.
            if !gql_warm_started
                && !local_public_prefetch
                && remaining_ms <= CONNECTION_WARM_LEAD_MS
            {
                gql_warm_started = true;
                if remaining_ms >= 8_000 {
                    let mut warm_jobs = tokio::task::JoinSet::new();
                    for wallet in wallets.iter().filter(|wallet| wallet.auth_ok) {
                        if let Some(session) = wallet.session.clone() {
                            let address = wallet.address;
                            warm_jobs.spawn(async move {
                                let started = std::time::Instant::now();
                                let result = tokio::time::timeout(
                                    std::time::Duration::from_secs(20),
                                    opensea::warm_gql_connection(&session),
                                )
                                .await;
                                (address, started.elapsed().as_millis() as u64, result)
                            });
                        }
                    }
                    let total = warm_jobs.len();
                    let budget_ms = connection_warm_budget_ms(total)
                        .min(remaining_ms.saturating_sub(8_000) as u64);
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
                    let mut ready = 0usize;
                    let mut latencies = Vec::new();
                    while !warm_jobs.is_empty() {
                        let budget = deadline.saturating_duration_since(std::time::Instant::now());
                        if budget.is_zero() {
                            break;
                        }
                        match tokio::time::timeout(budget, warm_jobs.join_next()).await {
                            Ok(Some(Ok((_, elapsed, Ok(Ok(_status)))))) => {
                                ready += 1;
                                latencies.push(elapsed);
                            }
                            Ok(Some(_)) => {}
                            Ok(None) | Err(_) => break,
                        }
                    }
                    warm_jobs.abort_all();
                    latencies.sort_unstable();
                    let latency = latencies
                        .get((latencies.len().saturating_sub(1)) / 2)
                        .map(|ms| format!(" median={ms}ms"))
                        .unwrap_or_default();
                    log_always(
                        reporter.as_ref(),
                        format!(
                            "OpenSea GQL transport warm: {ready}/{total} ready{latency}; budget={budget_ms}ms; mint-action budget untouched"
                        ),
                    );
                    if ready < total {
                        log_always(
                            reporter.as_ref(),
                            format!(
                                "WARN: {} wallet(s) did not warm before the safety cutoff and may pay a TLS handshake at T0",
                                total - ready
                            ),
                        );
                    }
                } else {
                    log_always(
                        reporter.as_ref(),
                        "OpenSea GQL transport warm skipped: less than 8s to T0".to_string(),
                    );
                }
            }

            if !prefetched && should_prefetch && remaining_ms <= prefetch_lead_ms {
                log_always(
                    reporter.as_ref(),
                    format!("\n  Preparing calldata ({left}s before open)..."),
                );
                prefetched = true;
                if local_public_prefetch {
                    let built = rebuild_local_public_wallets(
                        &mut wallets,
                        &wallet_quantities,
                        quantity,
                        nft_contract,
                        price_wei,
                        seadrop_address.as_deref(),
                        fee_recipient.as_deref(),
                    )?;
                    log_always(
                        reporter.as_ref(),
                        format!("  Local PUBLIC_SALE calldata ready for {built} wallet(s)"),
                    );
                } else {
                    // Deliberately no pre-fetch here. Measured against live
                    // OpenSea: asking for calldata before a stage opens is
                    // answered `DropNotMintingError` with an empty action list,
                    // for a PUBLIC_SALE the wallet was eligible for — so timing
                    // alone, not eligibility, and it can never succeed early.
                    //
                    // It is not merely useless, it is what lost the wallets.
                    // The mint-action budget is five requests, refilling about
                    // one every four seconds. The old loop retried every 500ms
                    // for the whole window, roughly ten doomed requests per
                    // wallet, so each wallet reached T0 with an empty budget and
                    // only the one or two that had refilled a token could mint.
                    // Staying silent leaves all five tokens for the shot that
                    // counts.
                    log_always(
                        reporter.as_ref(),
                        format!(
                            "  {stage_type_owned}: calldata is issued only once the stage opens                              — keeping OpenSea's request budget for T0"
                        ),
                    );
                }
            }

            if !preopen_finalized && remaining_ms <= preopen_finalize_lead_ms {
                log_always(
                    reporter.as_ref(),
                    "\n  Finalizing public SeaDrop state outside the T0 hot path...".to_string(),
                );
                if chain_id == 57073 {
                    // Ink submit ingress is especially sensitive to a cold TLS
                    // burst. Warm every endpoint that can receive a transaction
                    // in parallel with the existing nonce/fee work, never on the
                    // critical path and never by submitting a dummy transaction.
                    let warm_rpc = rpc.clone();
                    let warm_reporter = reporter.clone();
                    tokio::spawn(async move {
                        let (ready, total) = warm_rpc.warm_broadcast_endpoints().await;
                        log_always(
                            warm_reporter.as_ref(),
                            format!("  Ink RPC transports warm: {ready}/{total} ready"),
                        );
                    });
                }
                let state = tokio::time::timeout(
                    std::time::Duration::from_millis(1_200),
                    read_seadrop_public_state(&rpc, seadrop_address.as_deref(), nft_contract),
                )
                .await
                .context("SeaDrop state check timed out outside the T0 hot path")??;
                let verified_price = validate_seadrop_public_state(
                    &state,
                    expected_unit_price_wei,
                    quantity,
                    start_ts,
                )?;
                let resolved_fee_recipient =
                    resolve_public_fee_recipient(&state, fee_recipient.as_deref())?;
                if resolved_fee_recipient != fee_recipient {
                    if let Some(ref recipient) = resolved_fee_recipient {
                        log_always(
                            reporter.as_ref(),
                            format!("  SeaDrop fee recipient resolved on chain: {recipient}"),
                        );
                    }
                    fee_recipient = resolved_fee_recipient;
                }
                price_wei = verified_price;
                let rebuilt = rebuild_local_public_wallets(
                    &mut wallets,
                    &wallet_quantities,
                    quantity,
                    nft_contract,
                    price_wei,
                    seadrop_address.as_deref(),
                    fee_recipient.as_deref(),
                )?;
                public_drop_final_verified = true;
                preopen_finalized = true;
                log_always(
                    reporter.as_ref(),
                    format!(
                        "  SeaDrop FINAL OK with >=30s safety margin: price={} wei, rebuilt={rebuilt}",
                        price_wei
                    ),
                );

                collect_ready_prefetches(&mut prefetch_handles, &mut wallets);
                if pre_sign_enabled {
                    let signed = pre_sign_ready_wallets(
                        &mut wallets,
                        chain_id,
                        pre_sign_gas_limit,
                        max_fee,
                        max_priority_fee,
                    );
                    if signed > 0 {
                        log_always(
                            reporter.as_ref(),
                            format!("  Pre-signed {signed} ready mint transaction(s)"),
                        );
                    }
                }
            }

            if !prep_frozen {
                collect_ready_prefetches(&mut prefetch_handles, &mut wallets);
                if preopen_finalized && pre_sign_enabled {
                    pre_sign_ready_wallets(
                        &mut wallets,
                        chain_id,
                        pre_sign_gas_limit,
                        max_fee,
                        max_priority_fee,
                    );
                }
            }

            if conditional_submit_enabled
                && !conditional_started
                && preopen_finalized
                && remaining_ms <= conditional_lead_ms as i64
            {
                conditional_started = true;
                // Make one last local signing pass, then submit every armed
                // wallet concurrently. Never let a slow provider cross T0.
                collect_ready_prefetches(&mut prefetch_handles, &mut wallets);
                pre_sign_ready_wallets(
                    &mut wallets,
                    chain_id,
                    pre_sign_gas_limit,
                    max_fee,
                    max_priority_fee,
                );
                let prepared: Vec<(Address, Bytes, B256)> = wallets
                    .iter()
                    .filter_map(|wallet| {
                        wallet
                            .pre_signed_tx
                            .as_ref()
                            .map(|tx| (wallet.address, tx.raw.clone(), tx.hash))
                    })
                    .collect();
                log_always(
                    reporter.as_ref(),
                    format!(
                        "  Conditional submit T-{}ms: {}/{} transaction(s) armed",
                        remaining_ms,
                        prepared.len(),
                        wallets.iter().filter(|w| w.auth_ok).count()
                    ),
                );
                let mut conditional_jobs = tokio::task::JoinSet::new();
                for (address, raw, expected_hash) in prepared {
                    let rpc = rpc.clone();
                    conditional_jobs.spawn(async move {
                        let result = rpc
                            .send_raw_transaction_conditional(&raw, start_ts.max(0) as u64)
                            .await;
                        (address, expected_hash, result)
                    });
                }
                let budget_ms = remaining_ms.saturating_sub(25).max(1) as u64;
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_millis(budget_ms);
                while !conditional_jobs.is_empty() {
                    let budget = deadline.saturating_duration_since(std::time::Instant::now());
                    if budget.is_zero() {
                        break;
                    }
                    match tokio::time::timeout(budget, conditional_jobs.join_next()).await {
                        Ok(Some(Ok((address, expected_hash, Ok(hash))))) => {
                            if let Some(wallet) =
                                wallets.iter_mut().find(|wallet| wallet.address == address)
                            {
                                wallet.conditional_hash = Some(hash);
                            }
                            log_always(
                                reporter.as_ref(),
                                format!(
                                    "[{}] CONDITIONAL OK hash={} expected={}",
                                    sign::shorten_address(&address),
                                    sign::shorten_hash(&hash),
                                    hash == expected_hash
                                ),
                            );
                        }
                        Ok(Some(Ok((address, _, Err(error))))) => log_always(
                            reporter.as_ref(),
                            format!(
                                "[{}] CONDITIONAL unavailable: {error} — T0 fallback remains armed",
                                sign::shorten_address(&address)
                            ),
                        ),
                        Ok(Some(Err(error))) => log_always(
                            reporter.as_ref(),
                            format!("Conditional task failed: {error}"),
                        ),
                        Ok(None) | Err(_) => break,
                    }
                }
                conditional_jobs.abort_all();
            }

            // Freeze preparation before the final precision window. Ready
            // wallets are never held behind a late OpenSea request.
            if !prep_frozen && remaining_ms <= 50 {
                prefetch_handles.abort_all();
                prep_frozen = true;
            }

            let sleep_ms = if remaining_ms > 10_000 {
                200
            } else if remaining_ms > 2_000 {
                50
            } else if remaining_ms > 100 {
                5
            } else if remaining_ms > 20 {
                1
            } else {
                tokio::task::yield_now().await;
                continue;
            };
            tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await;
        }
        drop(timer_guard);

        if !prep_frozen {
            // We reached T0 before the 50ms freeze branch could run. Never do
            // late collection/signing here: ready wallets must blast now.
            prefetch_handles.abort_all();
        }
        scheduled_pre_signed = wallets.iter().filter(|w| w.pre_signed_tx.is_some()).count();
        scheduled_direct_ready = wallets
            .iter()
            .filter(|w| w.auth_ok && w.proxy_url.is_none())
            .count();
        scheduled_proxy_ready = wallets
            .iter()
            .filter(|w| w.auth_ok && w.proxy_url.is_some())
            .count();
        scheduled_direct_pre_signed = wallets
            .iter()
            .filter(|w| w.proxy_url.is_none() && w.pre_signed_tx.is_some())
            .count();
        scheduled_proxy_pre_signed = wallets
            .iter()
            .filter(|w| w.proxy_url.is_some() && w.pre_signed_tx.is_some())
            .count();
        scheduled_fire_lag_ms = Some(fire_lag_ms_from_clock(
            start_ts,
            crate::timing::true_now_ms(),
        ));
    }

    // Late/manual launches do not pass through the scheduled T-2 preparation
    // window. Verify once before spawning workers; this costs latency only on a
    // launch that is already late and prevents stale-price paid reverts.
    if stage_type_owned == "PUBLIC_SALE" && !public_drop_final_verified {
        let state = tokio::time::timeout(
            std::time::Duration::from_millis(1_500),
            read_seadrop_public_state(&rpc, seadrop_address.as_deref(), nft_contract),
        )
        .await
        .context("Final SeaDrop state check timed out; mint not broadcast")??;
        price_wei = validate_seadrop_public_state(
            &state,
            expected_unit_price_wei,
            quantity,
            crate::timing::true_now_secs(),
        )?;
        let resolved_fee_recipient =
            resolve_public_fee_recipient(&state, fee_recipient.as_deref())?;
        if resolved_fee_recipient != fee_recipient {
            if let Some(ref recipient) = resolved_fee_recipient {
                log_always(
                    reporter.as_ref(),
                    format!("SeaDrop fee recipient resolved on chain: {recipient}"),
                );
            }
            fee_recipient = resolved_fee_recipient;
        }
        rebuild_local_public_wallets(
            &mut wallets,
            &wallet_quantities,
            quantity,
            nft_contract,
            price_wei,
            seadrop_address.as_deref(),
            fee_recipient.as_deref(),
        )?;
        log_always(
            reporter.as_ref(),
            format!("SeaDrop LIVE CHECK OK: price={} wei", price_wei),
        );
    }

    if cancelled(&cancel) {
        bail!("Mint cancelled before workers started");
    }

    let auto_sweep_destination = if dry_run {
        None
    } else {
        opts.auto_sweep_destination
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| {
                let destination: Address = value
                    .parse()
                    .context("invalid auto-sweep destination address")?;
                if destination == Address::ZERO {
                    bail!("auto-sweep destination is the zero address");
                }
                Ok(destination)
            })
            .transpose()?
    };
    let auto_sweep_contract: Address = nft_contract
        .parse()
        .context("invalid NFT contract for auto-sweep")?;
    if let Some(destination) = auto_sweep_destination {
        log_always(
            reporter.as_ref(),
            format!("Auto-sweep armed: confirmed mint NFTs → {destination:?}"),
        );
    }
    let auto_sweep_signers: HashMap<Address, Signer> = wallets
        .iter()
        .map(|wallet| (wallet.address, wallet.signer.clone()))
        .collect();
    let mut auto_sweep_jobs: tokio::task::JoinSet<AutoSweepJobResult> = tokio::task::JoinSet::new();
    let mut auto_sweep_started = HashSet::new();

    let rpc_clone = rpc.clone();
    let mint_started_at = std::time::Instant::now();
    let fb_pieces: Arc<std::sync::Mutex<Vec<BundleTx>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));

    let mut handles = tokio::task::JoinSet::new();

    // Wallets that reach T0 unarmed ask OpenSea for calldata the instant the
    // stage opens. The request budget is per exit IP, so group them by proxy
    // and space only within a group: two wallets behind different proxies do
    // not compete and neither should wait for the other.
    let gql_stagger_slots: HashMap<Address, u64> = {
        let mut by_route: HashMap<Option<String>, Vec<Address>> = HashMap::new();
        for w in wallets
            .iter()
            .filter(|w| w.auth_ok && w.pre_signed_tx.is_none() && w.prefetched_tx.is_none())
        {
            by_route
                .entry(w.proxy_url.clone())
                .or_default()
                .push(w.address);
        }
        let busiest = by_route.values().map(Vec::len).max().unwrap_or(0);
        let unarmed: usize = by_route.values().map(Vec::len).sum();
        if busiest > OPENSEA_MINT_ACTION_BUDGET {
            // Past the budget the extra wallets cannot be helped by spacing —
            // they must wait for a refill. Say so rather than letting it look
            // like an unexplained stall.
            log_always(
                reporter.as_ref(),
                format!(
                    "  WARNING: {busiest} wallet(s) share one exit IP but OpenSea allows \
                     {OPENSEA_MINT_ACTION_BUDGET} calldata requests per IP (~{}s per refill). \
                     Add proxies — aim for at most {OPENSEA_MINT_ACTION_BUDGET} wallets each, \
                     or the surplus waits.",
                    OPENSEA_MINT_ACTION_REFILL_MS / 1000
                ),
            );
        }
        let step = gql_stagger_step_ms(busiest);
        if step > 0 {
            log_always(
                reporter.as_ref(),
                format!(
                    "  {unarmed} wallet(s) unarmed at open across {} route(s); \
                     spacing same-IP requests {step}ms apart",
                    by_route.len()
                ),
            );
        }
        by_route
            .into_values()
            .flat_map(|group| {
                group
                    .into_iter()
                    .enumerate()
                    .map(move |(i, a)| (a, i as u64 * step))
            })
            .collect()
    };

    for (wallet_lane, w) in wallets.iter().enumerate() {
        if !w.auth_ok {
            continue;
        }

        let rpc = rpc_clone.clone();
        let signer = w.signer.clone();
        let mut session = w.session.clone();
        let addr = w.address;
        let auth_chain_id = chain_id;
        let mut nonce = w.nonce;
        let gas_multiplier = gas_params.gas_multiplier;
        let nft_contract_owned = nft_contract.to_string();
        let chain_owned = info.chain.clone();
        let slug_owned = slug.clone();
        let payment_asset_owned = payment_asset.clone();
        let quantity_owned = wallet_quantities.get(&addr).copied().unwrap_or(quantity);
        let calldata_value = price_wei * U256::from(quantity_owned);
        let stage_type_owned = stage_type_owned.clone();
        let expected_stage_index_owned = selected_stage_index;
        let stage_token_id_owned = stage_token_id.clone();
        let seadrop_address_owned = seadrop_address.clone();
        let fee_recipient_owned = fee_recipient.clone();
        let mint_started_at = mint_started_at;
        let fixed_gas_limit_owned = fixed_gas_limit;
        let max_fee = max_fee;
        let max_priority_fee = max_priority_fee;
        let use_gql_owned = stage_type_owned != "PUBLIC_SALE"
            && opts.use_gql.unwrap_or_else(|| {
                env.get("USE_GQL")
                    .map(|v| v == "1" || v == "true")
                    .unwrap_or(false)
            });
        let initial_cached_tx = w.prefetched_tx.clone();
        let initial_pre_signed = w.pre_signed_tx.clone();
        let stagger_ms = gql_stagger_slots.get(&addr).copied().unwrap_or(0);
        let initial_conditional_hash = w.conditional_hash;
        let max_attempts = max_attempts;
        let reporter = reporter.clone();
        let quiet_w = quiet;
        let skip_preflight_w = skip_preflight;
        // Default true: OS mint should not require a per-task "skip estimate" checkbox.
        let skip_estimate_on_open = opts.skip_estimate_on_open.unwrap_or(true);
        let stage_start_ts_w = stage_start_ts;
        let beep_w = beep;
        let first_confirm_w = first_confirm.clone();
        let cancel_w = cancel.clone();
        let use_flashbots_w = use_flashbots;
        let fb_pieces_w = fb_pieces.clone();
        let dry_run_w = dry_run;
        // Bound at auth time (signer index) — never re-derive from wallets order.
        let proxy_url_w = w.proxy_url.clone();
        let ink_broadcast = chain_id == 57073;

        handles.spawn(async move {
            let mut attempt = 0u32;
            let burst_delays = [0u64, 50, 100, 200, 500, 1000, 2000, 3000];
            let mut burst_idx = 0usize;
            // Prefetch / retry calldata. Use take()+restore so every write is later read.
            let mut cached_tx: Option<(alloy_primitives::Address, U256, Bytes)> = initial_cached_tx;
            let mut pre_signed_tx = initial_pre_signed;
            // Every tx hash this worker has signed and broadcast. A send is fanned
            // out to several RPCs, so a `nonce too low` / `underpriced` rejection
            // can mean "another node already mined the previous attempt", not
            // "nothing landed". Re-sending without checking these first is a
            // double mint / double spend.
            let mut sent_hashes: Vec<B256> = initial_conditional_hash.into_iter().collect();
            // Last failure message for exhaust path only (updated in place via helper).
            let mut last_error = String::new();
            // Drawn down by every wait OpenSea asks for; see rate_limit_backoff.
            let mut rate_limit_budget_ms = RATE_LIMIT_WAIT_BUDGET_MS;
            let mut max_fee = max_fee;
            let mut max_priority_fee = max_priority_fee;
            // Cap underpriced/RBF fee escalation: never exceed 4× fee at worker start.
            let fee_ceiling = max_fee.saturating_mul(U256::from(4u64)).max(max_fee);
            // Product rule: LIVE OpenSea mint is always fixed-gas / fast.
            // No separate "sniper mode". Dry-run still estimates unless skip flags.
            let wall_at_spawn = crate::timing::true_now_secs();
            let auto_skip_estimate = dry_run_w
                && !skip_preflight_w
                && !skip_estimate_on_open
                && in_phase_open_lag_window(stage_start_ts_w, wall_at_spawn);
            let mut force_fixed_gas = initial_force_fixed_gas(
                dry_run_w,
                skip_preflight_w,
                skip_estimate_on_open,
                auto_skip_estimate,
            );
            let mut logged_auto_skip = false;
            let mut logged_reuse = false;
            let mut proven_early_recoveries = 0u32;

            // Only wallets with no calldata and no signed transaction get a
            // slot here, so nothing that is ready to broadcast is delayed.
            if stagger_ms > 0 {
                sleep_cancellable(std::time::Duration::from_millis(stagger_ms), &cancel_w).await;
            }

            loop {
                if cancelled(&cancel_w) {
                    report_wallet(
                        reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::Failed),
                        Some("cancelled".into()),
                        None,
                        Some("cancelled by user".into()),
                    );
                    break (
                        addr,
                        MintResult {
                            address: addr,
                            tx_hash: None,
                            status: WalletStatus::Failed,
                            gas_used: None,
                            block_number: None,
                            error: Some("cancelled by user".into()),
                        },
                    );
                }
                attempt += 1;
                if attempt > max_attempts {
                    let err = if last_error.is_empty() {
                        "mint retries exhausted".to_string()
                    } else {
                        std::mem::take(&mut last_error)
                    };
                    // Retries can run out *after* a broadcast. Check every hash
                    // this worker put on the wire before reporting a loss —
                    // otherwise a transaction that mined on the last attempt is
                    // recorded as failed with no hash to verify.
                    if !sent_hashes.is_empty() {
                        if let crate::rpc::ReceiptLookup::Landed(landed, receipt) =
                            first_landed_hash(&rpc, &sent_hashes).await
                        {
                            let info = crate::rpc::parse_receipt(&receipt);
                            log_always(reporter.as_ref(), format!(
                                "[{}] retries exhausted but {} is on chain",
                                sign::shorten_address(&addr),
                                sign::shorten_hash(&landed)));
                            break (addr, receipt_to_result(addr, landed, &info));
                        }
                    }
                    let last_hash = sent_hashes.last().copied();
                    report_wallet(reporter.as_ref(),
                        &addr,
                        Some(if last_hash.is_some() {
                            WalletStatus::Sent
                        } else {
                            WalletStatus::Failed
                        }),
                        Some(format!("attempt {}/{}", attempt - 1, max_attempts)),
                        last_hash,
                        Some(err.clone()),
                    );
                    break (
                        addr,
                        MintResult {
                            address: addr,
                            // A broadcast happened: keep the hash so the run can
                            // still be reconciled against the chain later.
                            tx_hash: last_hash,
                            status: if last_hash.is_some() {
                                WalletStatus::Sent
                            } else {
                                WalletStatus::Failed
                            },
                            gas_used: None,
                            block_number: None,
                            error: Some(err),
                        },
                    );
                }

                let (tx, raw, signed_hash, gas_limit, was_pre_signed) =
                    if let Some(prepared) = pre_signed_tx.take() {
                        nonce = prepared.tx.nonce;
                        max_fee = prepared.tx.max_fee;
                        max_priority_fee = prepared.tx.max_priority_fee;
                        if !sent_hashes.contains(&prepared.hash) {
                            sent_hashes.push(prepared.hash);
                        }
                        (
                            prepared.tx,
                            prepared.raw,
                            prepared.hash,
                            prepared.gas_limit,
                            true,
                        )
                    } else {
                        report_wallet(reporter.as_ref(),
                            &addr,
                            Some(WalletStatus::Calldata),
                            Some(format!("attempt {}/{}", attempt, max_attempts)),
                            None,
                            None,
                        );

                        // `.take()`: consume cache for this attempt; restore on estimate / fee / nonce retry.
                        let (to_addr, tx_value, calldata): (alloy_primitives::Address, U256, Bytes) =
                    if let Some((to, val, cd)) = cached_tx.take() {
                        if !logged_reuse && attempt > 1 {
                            logged_reuse = true;
                            log_always(
                                reporter.as_ref(),
                                format!(
                                    "[{}] reusing PREPARED calldata (no GQL re-fetch)",
                                    sign::shorten_address(&addr)
                                ),
                            );
                        }
                        (to, val, cd)
                    } else if stage_type_owned == "PUBLIC_SALE" && !use_gql_owned {
                        let local_start = std::time::Instant::now();
                        let local_built: Result<(alloy_primitives::Address, U256, Bytes), String> =
                            build_local_public_mint(
                                &nft_contract_owned,
                                quantity_owned,
                                calldata_value / U256::from(quantity_owned.max(1)),
                                seadrop_address_owned.as_deref(),
                                fee_recipient_owned.as_deref(),
                                addr,
                            )
                            .map_err(|error| error.to_string());
                        match local_built {
                            Ok(triple) => {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "[{}] LOCAL BUILD OK {}ms to={:?} value={} data={} bytes",
                                        sign::shorten_address(&addr),
                                        local_start.elapsed().as_millis(),
                                        triple.0,
                                        triple.1,
                                        triple.2.len()
                                    ),
                                );
                                triple
                            }
                            Err(e) => {
                                mint_log(
                                    reporter.as_ref(),
                                    quiet_w,
                                    format!(
                                        "[{}] LOCAL BUILD FAILED: {}, falling back to GQL",
                                        sign::shorten_address(&addr),
                                        e
                                    ),
                                );
                                if session.is_none() {
                                    break (
                                        addr,
                                        MintResult {
                                            address: addr,
                                            tx_hash: None,
                                            status: WalletStatus::Failed,
                                            gas_used: None,
                                            block_number: None,
                                            error: Some("no auth session".to_string()),
                                        },
                                    );
                                }
                                match fetch_calldata_reauth(
                                    reporter.as_ref(),
                                    &mut session,
                                    &signer,
                                    auth_chain_id,
                                    proxy_url_w.as_deref(),
                                    &slug_owned,
                                    &addr,
                                    &nft_contract_owned,
                                    &chain_owned,
                                    &stage_token_id_owned,
                                    quantity_owned,
                                    expected_stage_index_owned,
                                    &payment_asset_owned,
                                    &calldata_value,
                                    &mint_started_at,
                                    attempt,
                                    quiet_w,
                                )
                                .await
                                {
                                    Ok(result) => result,
                                    Err(e) => {
                                        let err_str = format!("{}", e);
                                        if let Some(wait) = rate_limit_backoff(
                                            &err_str,
                                            &addr,
                                            &mut rate_limit_budget_ms,
                                        ) {
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] OpenSea rate limit — waiting {}ms as instructed",
                                                    sign::shorten_address(&addr),
                                                    wait.as_millis()
                                                ),
                                            );
                                            last_error = err_str;
                                            sleep_cancellable(wait, &cancel_w).await;
                                            // The server asked us to wait; that
                                            // is not one of the three tries a
                                            // real error gets.
                                            attempt = attempt.saturating_sub(1);
                                            continue;
                                        }
                                        let now_wall = crate::timing::true_now_secs();
                                        if is_gql_action_not_ready(
                                            &err_str,
                                            stage_start_ts_w,
                                            now_wall,
                                        )
                                            && attempt < max_attempts
                                        {
                                            let wait = gql_action_not_ready_delay(attempt);
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] OpenSea mint action not ready at T0 (attempt {}/{}); retrying in {}ms: {}",
                                                    sign::shorten_address(&addr),
                                                    attempt,
                                                    max_attempts,
                                                    wait.as_millis(),
                                                    err_str
                                                ),
                                            );
                                            last_error = err_str;
                                            sleep_cancellable(wait, &cancel_w).await;
                                            continue;
                                        }
                                        if is_terminal_gql_action_error(
                                            &err_str,
                                            stage_start_ts_w,
                                            now_wall,
                                        ) {
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] OpenSea rejected the selected phase; not retrying a terminal action error: {}",
                                                    sign::shorten_address(&addr),
                                                    err_str
                                                ),
                                            );
                                            break (
                                                addr,
                                                MintResult {
                                                    address: addr,
                                                    tx_hash: None,
                                                    status: WalletStatus::Failed,
                                                    gas_used: None,
                                                    block_number: None,
                                                    error: Some(err_str),
                                                },
                                            );
                                        }
                                        if attempt > 3 {
                                            break (
                                                addr,
                                                MintResult {
                                                    address: addr,
                                                    tx_hash: None,
                                                    status: WalletStatus::Failed,
                                                    gas_used: None,
                                                    block_number: None,
                                                    error: Some(err_str),
                                                },
                                            );
                                        }
                                        last_error = err_str;
                                        tokio::time::sleep(std::time::Duration::from_millis(100))
                                            .await;
                                        continue;
                                    }
                                }
                            }
                        }
                    } else {
                        if session.is_none() {
                            break (
                                addr,
                                MintResult {
                                    address: addr,
                                    tx_hash: None,
                                    status: WalletStatus::Failed,
                                    gas_used: None,
                                    block_number: None,
                                    error: Some("no auth session".to_string()),
                                },
                            );
                        }
                        // UI surfaces re-auth on 401 via message (see mint_ops::reauth_required_message)
                match fetch_calldata_reauth(
                            reporter.as_ref(),
                            &mut session,
                            &signer,
                            auth_chain_id,
                            proxy_url_w.as_deref(),
                            &slug_owned,
                            &addr,
                            &nft_contract_owned,
                            &chain_owned,
                            &stage_token_id_owned,
                            quantity_owned,
                            expected_stage_index_owned,
                            &payment_asset_owned,
                            &calldata_value,
                            &mint_started_at,
                            attempt,
                            quiet_w,
                        )
                        .await
                        {
                            Ok(result) => result,
                            Err(e) => {
                                let err_str = format!("{}", e);
                                if let Some(wait) =
                                    rate_limit_backoff(&err_str, &addr, &mut rate_limit_budget_ms)
                                {
                                    log_always(
                                        reporter.as_ref(),
                                        format!(
                                            "[{}] OpenSea rate limit — waiting {}ms as instructed",
                                            sign::shorten_address(&addr),
                                            wait.as_millis()
                                        ),
                                    );
                                    last_error = err_str;
                                    sleep_cancellable(wait, &cancel_w).await;
                                    // The server asked us to wait; that is not
                                    // one of the three tries a real error gets.
                                    attempt = attempt.saturating_sub(1);
                                    continue;
                                }
                                let now_wall = crate::timing::true_now_secs();
                                if is_gql_action_not_ready(
                                    &err_str,
                                    stage_start_ts_w,
                                    now_wall,
                                ) && attempt < max_attempts
                                {
                                    let wait = gql_action_not_ready_delay(attempt);
                                    log_always(
                                        reporter.as_ref(),
                                        format!(
                                            "[{}] OpenSea mint action not ready at T0 (attempt {}/{}); retrying in {}ms: {}",
                                            sign::shorten_address(&addr),
                                            attempt,
                                            max_attempts,
                                            wait.as_millis(),
                                            err_str
                                        ),
                                    );
                                    last_error = err_str;
                                    sleep_cancellable(wait, &cancel_w).await;
                                    continue;
                                }
                                if is_terminal_gql_action_error(
                                    &err_str,
                                    stage_start_ts_w,
                                    now_wall,
                                ) {
                                    log_always(
                                        reporter.as_ref(),
                                        format!(
                                            "[{}] OpenSea rejected the selected phase; not retrying a terminal action error: {}",
                                            sign::shorten_address(&addr),
                                            err_str
                                        ),
                                    );
                                    break (
                                        addr,
                                        MintResult {
                                            address: addr,
                                            tx_hash: None,
                                            status: WalletStatus::Failed,
                                            gas_used: None,
                                            block_number: None,
                                            error: Some(err_str),
                                        },
                                    );
                                }
                                if attempt > 3 {
                                    break (
                                        addr,
                                        MintResult {
                                            address: addr,
                                            tx_hash: None,
                                            status: WalletStatus::Failed,
                                            gas_used: None,
                                            block_number: None,
                                            error: Some(err_str),
                                        },
                                    );
                                }
                                last_error = err_str;
                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                                continue;
                            }
                        }
                    };

                let gas_limit = if force_fixed_gas {
                    let fixed_raw = fixed_gas_limit_owned.unwrap_or(250_000);
                    let fixed = resolve_mint_gas_limit(fixed_raw, gas_multiplier, chain_id, true);
                    let label = if !dry_run_w {
                        "LIVE_FIXED"
                    } else if skip_preflight_w {
                        "SKIP_PREFLIGHT"
                    } else if skip_estimate_on_open {
                        "SKIP_ESTIMATE_ON_OPEN"
                    } else if auto_skip_estimate {
                        "AUTO_SKIP_ESTIMATE_ON_OPEN"
                    } else {
                        "FIXED_GAS_AFTER_NOT_ACTIVE"
                    };
                    if !logged_auto_skip {
                        logged_auto_skip = true;
                        log_always(
                            reporter.as_ref(),
                            format!(
                                "[{}] {label} gas_limit={fixed} (no eth_estimateGas)",
                                sign::shorten_address(&addr)
                            ),
                        );
                    }
                    if fixed != fixed_raw {
                        mint_log(
                            reporter.as_ref(),
                            quiet_w,
                            format!(
                                "[{}] {} gas_limit {} → {} (L2 floor)",
                                sign::shorten_address(&addr),
                                label,
                                fixed_raw,
                                fixed
                            ),
                        );
                    }
                    report_wallet(reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::Sim),
                        Some(format!("{label} gas={fixed}")),
                        None,
                        None,
                    );
                    mint_log(reporter.as_ref(), quiet_w,
                        format!(
                            "[{}] {} gas_limit={}",
                            sign::shorten_address(&addr),
                            label,
                            fixed
                        ),
                    );
                    fixed
                } else if let Some(fixed_raw) = fixed_gas_limit_owned {
                    // Manual gas: still prefer preflight OK before send, but NotActive near
                    // open → short wait + fixed-gas send (do not re-GQL).
                    let fixed = resolve_mint_gas_limit(fixed_raw, gas_multiplier, chain_id, true);
                    if fixed != fixed_raw {
                        mint_log(
                            reporter.as_ref(),
                            quiet_w,
                            format!(
                                "[{}] fixed gas {} → {} (L2 floor)",
                                sign::shorten_address(&addr),
                                fixed_raw,
                                fixed
                            ),
                        );
                    }
                    report_wallet(reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::Sim),
                        Some(format!("sim attempt {}", attempt)),
                        None,
                        None,
                    );
                    let sim_start = std::time::Instant::now();
                    match rpc.estimate_gas(&addr, &to_addr, tx_value, &calldata).await {
                        Ok(g) => {
                            mint_log(reporter.as_ref(), quiet_w,
                                format!(
                                    "[{}] pre-flight OK {}ms est={} limit={}",
                                    sign::shorten_address(&addr),
                                    sim_start.elapsed().as_millis(),
                                    g,
                                    fixed
                                ),
                            );
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Sim),
                                Some(format!("preflight ok est={} limit={}", g, fixed)),
                                None,
                                None,
                            );
                        }
                        Err(e) => {
                            let raw = format!("{}", e);
                            let now_wall = crate::timing::true_now_secs();
                            let (enriched, force_fixed, wait_override) =
                                estimate_fail_policy(&raw, stage_start_ts_w, now_wall);
                            mint_log(reporter.as_ref(), quiet_w,
                                format!(
                                    "[{}] PRE-FLIGHT FAIL {}ms: {}",
                                    sign::shorten_address(&addr),
                                    sim_start.elapsed().as_millis(),
                                    enriched
                                ),
                            );
                            // Always keep PREPARED calldata for the next attempt.
                            cached_tx = Some((to_addr, tx_value, calldata.clone()));
                            match classify_mint_error(&raw) {
                                "fatal" => {
                                    report_wallet(reporter.as_ref(),
                                        &addr,
                                        Some(WalletStatus::Failed),
                                        None,
                                        None,
                                        Some(enriched.clone()),
                                    );
                                    break (
                                        addr,
                                        MintResult {
                                            address: addr,
                                            tx_hash: None,
                                            status: WalletStatus::Failed,
                                            gas_used: None,
                                            block_number: None,
                                            error: Some(enriched),
                                        },
                                    );
                                }
                                _ => {
                                    if force_fixed {
                                        force_fixed_gas = true;
                                        log_always(
                                            reporter.as_ref(),
                                            format!(
                                                "[{}] NotActive near open — next attempt FIXED gas send (no re-estimate / no GQL)",
                                                sign::shorten_address(&addr)
                                            ),
                                        );
                                    }
                                    if attempt == 1 || attempt % 5 == 0 || force_fixed {
                                        log_always(reporter.as_ref(), format!(
                                            "[{}] pre-flight retry {}/{}: {}",
                                            sign::shorten_address(&addr),
                                            attempt,
                                            max_attempts,
                                            enriched
                                        ));
                                    }
                                    last_error = enriched;
                                    let delay_ms = if wait_override > 0 {
                                        wait_override
                                    } else if burst_idx < burst_delays.len() {
                                        burst_delays[burst_idx]
                                    } else {
                                        100
                                    };
                                    burst_idx += 1;
                                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms))
                                        .await;
                                    continue;
                                }
                            }
                        }
                    }
                    fixed
                } else {
                    let est_gas_start = std::time::Instant::now();
                    let gas_estimate =
                        match rpc.estimate_gas(&addr, &to_addr, tx_value, &calldata).await {
                            Ok(g) => {
                                log_always(reporter.as_ref(), format!("[{}] estimate_gas OK {}ms gas={}",
                                    sign::shorten_address(&addr),
                                    est_gas_start.elapsed().as_millis(),
                                    g));
                                report_wallet(reporter.as_ref(),
                                    &addr,
                                    Some(WalletStatus::Sim),
                                    Some(format!("est gas={}", g)),
                                    None,
                                    None,
                                );
                                g
                            }
                            Err(e) => {
                                let raw = format!("{}", e);
                                let now_wall = crate::timing::true_now_secs();
                                let (enriched, force_fixed, wait_override) =
                                    estimate_fail_policy(&raw, stage_start_ts_w, now_wall);
                                log_always(reporter.as_ref(), format!("[{}] estimate_gas FAIL {}ms: {}",
                                    sign::shorten_address(&addr),
                                    est_gas_start.elapsed().as_millis(),
                                    enriched));
                                // Keep PREPARED calldata — do not re-fetch GQL on estimate retries.
                                cached_tx = Some((to_addr, tx_value, calldata.clone()));
                                match classify_mint_error(&raw) {
                                    "fatal" => {
                                        report_wallet(reporter.as_ref(),
                                            &addr,
                                            Some(WalletStatus::Failed),
                                            None,
                                            None,
                                            Some(enriched.clone()),
                                        );
                                        break (
                                            addr,
                                            MintResult {
                                                address: addr,
                                                tx_hash: None,
                                                status: WalletStatus::Failed,
                                                gas_used: None,
                                                block_number: None,
                                                error: Some(enriched),
                                            },
                                        );
                                    }
                                    _ => {
                                        if force_fixed {
                                            force_fixed_gas = true;
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] NotActive near open — next attempt FIXED gas send (no re-estimate / no GQL)",
                                                    sign::shorten_address(&addr)
                                                ),
                                            );
                                        }
                                        if attempt == 1 || attempt % 5 == 0 || force_fixed {
                                            log_always(reporter.as_ref(), format!("[{}] estimate_gas retry {}/{}: {}",
                                                sign::shorten_address(&addr),
                                                attempt,
                                                max_attempts,
                                                enriched));
                                        }
                                        last_error = enriched;
                                        let delay_ms = if wait_override > 0 {
                                            wait_override
                                        } else if burst_idx < burst_delays.len() {
                                            burst_delays[burst_idx]
                                        } else {
                                            100
                                        };
                                        burst_idx += 1;
                                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms))
                                            .await;
                                        continue;
                                    }
                                }
                            }
                        };
                    let limit =
                        resolve_mint_gas_limit(gas_estimate, gas_multiplier, chain_id, false);
                    mint_log(
                        reporter.as_ref(),
                        quiet_w,
                        format!(
                            "[{}] gas_limit={} (est={} mult={})",
                            sign::shorten_address(&addr),
                            limit,
                            gas_estimate,
                            gas_multiplier
                        ),
                    );
                    limit
                };

                // Public dry-run stops before sign. Flashbots dry-run signs for eth_callBundle.
                if dry_run_w && !use_flashbots_w {
                    // Display-only costs via saturating wei math (no to::<u128>() panic).
                    let gas_cost_wei = max_fee.saturating_mul(U256::from(gas_limit));
                    // Report the value the live tx would actually carry
                    // (`tx_value`), not the phase price — they can legitimately
                    // differ, and mixing them made "Value:" print an ETH amount
                    // and a wei amount that disagreed.
                    let total_cost_wei = gas_cost_wei.saturating_add(tx_value);
                    let gas_cost_s = crate::amount::wei_to_eth_string(gas_cost_wei);
                    let price_s = crate::amount::wei_to_eth_string(tx_value);
                    let total_s = crate::amount::wei_to_eth_string(total_cost_wei);
                    log_always(reporter.as_ref(), format!("\n[{}] DRY RUN REPORT t+{}ms",
                        sign::shorten_address(&addr),
                        mint_started_at.elapsed().as_millis(),));
                    log_always(reporter.as_ref(), format!("  Contract:    {:?}", to_addr));
                    log_always(reporter.as_ref(), format!("  Value:       {} ETH ({} wei)", price_s, tx_value));
                    log_always(reporter.as_ref(), format!("  Gas limit:   {}", gas_limit));
                    log_always(reporter.as_ref(), format!("  Max fee:     {} gwei", max_fee / U256::from(1_000_000_000u64)));
                    log_always(reporter.as_ref(), format!("  Priority:    {} gwei", max_priority_fee / U256::from(1_000_000_000u64)));
                    log_always(reporter.as_ref(), format!("  Gas cost:    ~{} ETH", gas_cost_s));
                    log_always(reporter.as_ref(), format!("  Total cost:  ~{} ETH", total_s));
                    log_always(reporter.as_ref(), format!("  Calldata:    {} bytes", calldata.len()));
                    log_always(reporter.as_ref(), format!("  Nonce:       {}", nonce));
                    log_always(reporter.as_ref(), format!("  Chain:       {} (id={})", chain_owned, chain_id));
                    // Dry-run OK only if simulation path did not leave a preflight error.
                    // (Funds/sim failures already break Failed above when fixed gas + estimate.)
                    report_wallet(reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::DryRunOk),
                        Some(format!("~{} ETH total (sim OK, no broadcast)", total_s)),
                        None,
                        None,
                    );
                    break (
                        addr,
                        MintResult {
                            address: addr,
                            tx_hash: None,
                            status: WalletStatus::DryRunOk,
                            gas_used: Some(gas_limit),
                            block_number: None,
                            error: None,
                        },
                    );
                }

                let sign_start = std::time::Instant::now();
                let tx = sign::BuiltTx {
                    chain_id,
                    nonce,
                    to: to_addr,
                    value: tx_value,
                    data: calldata,
                    gas_limit,
                    max_fee,
                    max_priority_fee,
                };
                let (raw, signed_hash) = match sign::sign_transaction(&signer, &tx) {
                    Ok((r, h)) => {
                        log_always(reporter.as_ref(), format!("[{}] sign OK {}ms raw={} bytes",
                            sign::shorten_address(&addr),
                            sign_start.elapsed().as_millis(),
                            r.len()));
                        // Remember every hash we are about to put on the wire, so a
                        // later ambiguous rejection can be checked against it before
                        // we broadcast a replacement.
                        if !sent_hashes.contains(&h) {
                            sent_hashes.push(h);
                        }
                        (r, h)
                    }
                    Err(e) => {
                        break (
                            addr,
                            MintResult {
                                address: addr,
                                tx_hash: None,
                                status: WalletStatus::Failed,
                                gas_used: None,
                                block_number: None,
                                error: Some(format!("sign: {}", e)),
                            },
                        );
                    }
                };

                        (tx, raw, signed_hash, gas_limit, false)
                    };
                let to_addr = tx.to;
                let tx_value = tx.value;

                // Flashbots: collect signed txs; coordinator submits one bundle.
                if use_flashbots_w {
                    if let Ok(mut g) = fb_pieces_w.lock() {
                        g.push(BundleTx {
                            from: addr,
                            raw: raw.clone(),
                            tx_hash: signed_hash,
                        });
                    }
                    if dry_run_w {
                        report_wallet(
                            reporter.as_ref(),
                            &addr,
                            Some(WalletStatus::DryRunOk),
                            Some("signed for callBundle".into()),
                            Some(signed_hash),
                            None,
                        );
                        break (
                            addr,
                            MintResult {
                                address: addr,
                                tx_hash: Some(signed_hash),
                                status: WalletStatus::DryRunOk,
                                gas_used: Some(gas_limit),
                                block_number: None,
                                error: Some("__flashbots_dry__".into()),
                            },
                        );
                    }
                    report_wallet(
                        reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::Sent),
                        Some("queued for Flashbots bundle".into()),
                        Some(signed_hash),
                        None,
                    );
                    break (
                        addr,
                        MintResult {
                            address: addr,
                            tx_hash: Some(signed_hash),
                            status: WalletStatus::Sent,
                            gas_used: Some(gas_limit),
                            block_number: None,
                            error: Some("__flashbots_pending__".into()),
                        },
                    );
                }

                let send_start = std::time::Instant::now();
                if !was_pre_signed {
                    report_wallet(reporter.as_ref(),
                        &addr,
                        Some(WalletStatus::Sent),
                        Some("broadcasting...".into()),
                        None,
                        None,
                    );
                }
                // Report form (not `race_send`) so the accepting endpoint is
                // named in the log and the wallet row. Which node actually took
                // the transaction is the one fact that tells a paid endpoint
                // apart from the public fallback under real load.
                let send_result = if ink_broadcast {
                    rpc.send_raw_transaction_ink_report(&raw, wallet_lane)
                        .await
                } else {
                    rpc.send_raw_transaction_report(&raw).await
                };
                let tx_hash = match send_result {
                    Ok(send_report) => {
                        let h = send_report.hash;
                        let failed_note = if send_report.losers.is_empty() {
                            String::new()
                        } else {
                            format!(" after {} failed", send_report.losers.len())
                        };
                        log_always(reporter.as_ref(), format!("[{}] SEND OK {}ms via {}{} (fanout http={}, winner_ack={}ms) tx={}",
                            sign::shorten_address(&addr),
                            send_start.elapsed().as_millis(),
                            send_report.winner,
                            failed_note,
                            send_report.http_attempts,
                            send_report.winner_latency_ms,
                            sign::shorten_hash(&h)));
                        // Always wait for on-chain receipt — SENT alone is not success.
                        report_wallet(reporter.as_ref(),
                            &addr,
                            Some(WalletStatus::Sent),
                            Some(format!("pending receipt · via {}", send_report.winner)),
                            Some(h),
                            None,
                        );
                        h
                    }
                    Err(e) => {
                        log_always(reporter.as_ref(), format!("[{}] SEND FAIL {}ms: {}",
                            sign::shorten_address(&addr),
                            send_start.elapsed().as_millis(),
                            e));
                        let err_str = format!("{}", e);
                        if is_already_known(&err_str) {
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Sent),
                                Some("already known".into()),
                                Some(signed_hash),
                                None,
                            );
                            signed_hash
                        } else if is_nonce_too_low(&err_str) {
                            // `nonce too low` is ambiguous: it also means a prior
                            // attempt was already mined (we fan out to several
                            // nodes). Re-sending in that case is a double mint.
                            match first_landed_hash(&rpc, &sent_hashes).await {
                                crate::rpc::ReceiptLookup::Landed(landed, receipt) => {
                                    let info = crate::rpc::parse_receipt(&receipt);
                                    log_always(reporter.as_ref(), format!(
                                        "[{}] nonce too low, but {} already landed — not resending",
                                        sign::shorten_address(&addr),
                                        sign::shorten_hash(&landed)));
                                    break (addr, receipt_to_result(addr, landed, &info));
                                }
                                crate::rpc::ReceiptLookup::Unknown => {
                                    // Could not establish absence. Advancing the
                                    // nonce here would broadcast a second mint
                                    // for a transaction that may be mining.
                                    let msg = format!(
                                        "nonce too low and the chain could not be checked — \
                                         not resending to avoid a double mint ({err_str})"
                                    );
                                    log_always(reporter.as_ref(), format!(
                                        "[{}] {msg}", sign::shorten_address(&addr)));
                                    let last = sent_hashes.last().copied();
                                    report_wallet(reporter.as_ref(), &addr,
                                        Some(WalletStatus::Sent),
                                        Some("unresolved — check the hash".into()),
                                        last, Some(msg.clone()));
                                    break (addr, MintResult {
                                        address: addr,
                                        tx_hash: last,
                                        status: WalletStatus::Sent,
                                        gas_used: None,
                                        block_number: None,
                                        error: Some(msg),
                                    });
                                }
                                crate::rpc::ReceiptLookup::NotFound => {}
                            }
                            if let Some(hash) = initial_conditional_hash {
                                // Conditional acceptance can consume the nonce
                                // just before receipt indexes catch up. Never
                                // advance to a fresh nonce after an accepted
                                // conditional tx: that could mint twice.
                                match rpc.wait_for_any_receipt(&sent_hashes, 120).await {
                                    Ok((landed, receipt)) => {
                                        let info = crate::rpc::parse_receipt(&receipt);
                                        break (addr, receipt_to_result(addr, landed, &info));
                                    }
                                    Err(error) => {
                                        break (
                                            addr,
                                            MintResult {
                                                address: addr,
                                                tx_hash: Some(hash),
                                                status: WalletStatus::Sent,
                                                gas_used: None,
                                                block_number: None,
                                                error: Some(format!(
                                                    "conditional tx accepted; receipt timeout: {error}"
                                                )),
                                            },
                                        );
                                    }
                                }
                            }
                            // Same calldata next attempt; only nonce changes.
                            cached_tx = Some((tx.to, tx.value, tx.data.clone()));
                            // A stale nonce would make every retry re-send the same
                            // dead tx and report a misleading error, so a failed
                            // refresh must stop the worker instead.
                            match rpc.nonce(&addr).await {
                                Ok(n) => nonce = n,
                                Err(e) => {
                                    let msg = format!("nonce refresh failed after nonce-too-low: {e}");
                                    report_wallet(reporter.as_ref(), &addr,
                                        Some(WalletStatus::Failed), None, None, Some(msg.clone()));
                                    break (addr, MintResult {
                                        address: addr,
                                        tx_hash: None,
                                        status: WalletStatus::Failed,
                                        gas_used: None,
                                        block_number: None,
                                        error: Some(msg),
                                    });
                                }
                            }
                            let delay_ms = if burst_idx < burst_delays.len() {
                                burst_delays[burst_idx]
                            } else {
                                100
                            };
                            burst_idx += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            continue;
                        } else if is_underpriced(&err_str) {
                            // Same ambiguity as nonce-too-low: the pool may be
                            // rejecting a replacement because the original already
                            // landed. Check before broadcasting a bumped copy.
                            match first_landed_hash(&rpc, &sent_hashes).await {
                                crate::rpc::ReceiptLookup::Landed(landed, receipt) => {
                                    let info = crate::rpc::parse_receipt(&receipt);
                                    log_always(reporter.as_ref(), format!(
                                        "[{}] underpriced, but {} already landed — not resending",
                                        sign::shorten_address(&addr),
                                        sign::shorten_hash(&landed)));
                                    break (addr, receipt_to_result(addr, landed, &info));
                                }
                                crate::rpc::ReceiptLookup::Unknown => {
                                    let msg = format!(
                                        "underpriced and the chain could not be checked — \
                                         not bumping to avoid a double mint ({err_str})"
                                    );
                                    log_always(reporter.as_ref(), format!(
                                        "[{}] {msg}", sign::shorten_address(&addr)));
                                    let last = sent_hashes.last().copied();
                                    report_wallet(reporter.as_ref(), &addr,
                                        Some(WalletStatus::Sent),
                                        Some("unresolved — check the hash".into()),
                                        last, Some(msg.clone()));
                                    break (addr, MintResult {
                                        address: addr,
                                        tx_hash: last,
                                        status: WalletStatus::Sent,
                                        gas_used: None,
                                        block_number: None,
                                        error: Some(msg),
                                    });
                                }
                                crate::rpc::ReceiptLookup::NotFound => {}
                            }
                            // Same calldata next attempt; only gas bumps (×1.15).
                            // Sleep like nonce-too-low so we do not hammer RPC; cap fee at 4× start.
                            cached_tx = Some((tx.to, tx.value, tx.data.clone()));
                            let next_fee = gas::bump_fee_bps(max_fee, 11_500);
                            let next_prio = gas::bump_fee_bps(max_priority_fee, 11_500);
                            if next_fee > fee_ceiling {
                                let msg = format!(
                                    "underpriced fee bump would exceed 4× start ceiling ({} gwei)",
                                    fee_ceiling / U256::from(1_000_000_000u64)
                                );
                                report_wallet(
                                    reporter.as_ref(),
                                    &addr,
                                    Some(WalletStatus::Failed),
                                    None,
                                    None,
                                    Some(msg.clone()),
                                );
                                break (
                                    addr,
                                    MintResult {
                                        address: addr,
                                        tx_hash: None,
                                        status: WalletStatus::Failed,
                                        gas_used: None,
                                        block_number: None,
                                        error: Some(msg),
                                    },
                                );
                            }
                            max_fee = next_fee;
                            max_priority_fee = next_prio;
                            log_always(
                                reporter.as_ref(),
                                format!(
                                    "[{}] underpriced — bump gas ×1.15 → {} gwei (prio {} gwei), retry",
                                    sign::shorten_address(&addr),
                                    max_fee / U256::from(1_000_000_000u64),
                                    max_priority_fee / U256::from(1_000_000_000u64)
                                ),
                            );
                            let delay_ms = if burst_idx < burst_delays.len() {
                                burst_delays[burst_idx]
                            } else {
                                100
                            };
                            burst_idx += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            continue;
                        } else if crate::errors::classify_send_failure(&err_str)
                            == crate::errors::SendOutcome::Rejected
                        {
                            // Provably never entered a pool — no hash to keep.
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Failed),
                                None,
                                None,
                                Some(err_str.clone()),
                            );
                            break (
                                addr,
                                MintResult {
                                    address: addr,
                                    tx_hash: None,
                                    status: WalletStatus::Failed,
                                    gas_used: None,
                                    block_number: None,
                                    error: Some(err_str),
                                },
                            );
                        } else {
                            // Ambiguous: every endpoint failed, but a timeout
                            // means the request may have been accepted and only
                            // the response lost. Ask the chain before declaring
                            // failure — this branch used to drop `signed_hash`
                            // and report Failed, so a mint that actually landed
                            // was reported as a loss with nothing to look up.
                            if let crate::rpc::ReceiptLookup::Landed(landed, receipt) =
                                first_landed_hash(&rpc, &sent_hashes).await
                            {
                                let info = crate::rpc::parse_receipt(&receipt);
                                log_always(reporter.as_ref(), format!(
                                    "[{}] send errored but {} is on chain — not a failure",
                                    sign::shorten_address(&addr),
                                    sign::shorten_hash(&landed)));
                                break (addr, receipt_to_result(addr, landed, &info));
                            }
                            // Not found yet. Keep the hash and report Sent so
                            // the operator can check it, rather than Failed
                            // with `tx_hash: None`.
                            let msg = format!("send unresolved: {err_str}");
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Sent),
                                Some("send unclear — check the hash".into()),
                                Some(signed_hash),
                                Some(msg.clone()),
                            );
                            break (
                                addr,
                                MintResult {
                                    address: addr,
                                    tx_hash: Some(signed_hash),
                                    status: WalletStatus::Sent,
                                    gas_used: None,
                                    block_number: None,
                                    error: Some(msg),
                                },
                            );
                        }
                    }
                };

                log_always(reporter.as_ref(), format!("[{}] SENT t+{}ms tx={}",
                    sign::shorten_address(&addr),
                    mint_started_at.elapsed().as_millis(),
                    sign::shorten_hash(&tx_hash)));

                // Track original + every RBF hash; receipt on any candidate is success.
                // Seed with every hash this worker broadcast, not just the last
                // one: an earlier retry attempt may still be the copy that mines.
                let mut candidate_hashes: Vec<B256> = sent_hashes.clone();
                if !candidate_hashes.contains(&tx_hash) {
                    candidate_hashes.push(tx_hash);
                }
                let mut mined_hash = tx_hash;
                let mut rbf_count = 0u32;
                const MAX_RBF: u32 = 3;
                const RBF_WAIT_SECS: u64 = 15;
                // RBF bump ×1.30 in basis points.
                const RBF_BUMP_BPS: u64 = 13_000;

                let receipt_result = loop {
                    let wait = if rbf_count < MAX_RBF { RBF_WAIT_SECS } else { 75 };
                    match rpc.wait_for_any_receipt(&candidate_hashes, wait).await {
                        Ok((h, r)) => {
                            mined_hash = h;
                            break Ok(r);
                        }
                        Err(_) if rbf_count < MAX_RBF => {
                            let next_fee = gas::bump_fee_bps(max_fee, RBF_BUMP_BPS);
                            let next_prio = gas::bump_fee_bps(max_priority_fee, RBF_BUMP_BPS);
                            if next_fee > fee_ceiling {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "[{}] RBF stopped — fee would exceed 4× start ceiling; waiting on existing candidates",
                                        sign::shorten_address(&addr)
                                    ),
                                );
                                // Final wait without more fee bumps (do not burn RBF slots).
                                match rpc.wait_for_any_receipt(&candidate_hashes, 75).await {
                                    Ok((h, r)) => {
                                        mined_hash = h;
                                        break Ok(r);
                                    }
                                    Err(e) => break Err(e),
                                }
                            }
                            rbf_count += 1;
                            max_fee = next_fee;
                            max_priority_fee = next_prio;
                            log_always(reporter.as_ref(), format!("[{}] RBF #{} ({}s pending) gas->{} gwei candidates={}",
                                sign::shorten_address(&addr),
                                rbf_count,
                                rbf_count * RBF_WAIT_SECS as u32,
                                max_fee / U256::from(1_000_000_000u64),
                                candidate_hashes.len()));
                            let rbf_tx = sign::BuiltTx {
                                chain_id,
                                nonce,
                                to: to_addr,
                                value: tx_value,
                                data: tx.data.clone(),
                                gas_limit,
                                max_fee,
                                max_priority_fee,
                            };
                            if let Ok((rbf_raw, rbf_hash)) = sign::sign_transaction(&signer, &rbf_tx) {
                                match rpc.race_send(&rbf_raw).await {
                                    Ok(_) => {
                                        if !candidate_hashes.contains(&rbf_hash) {
                                            candidate_hashes.push(rbf_hash);
                                        }
                                    }
                                    Err(ref e) if is_already_known(&format!("{}", e)) => {
                                        if !candidate_hashes.contains(&rbf_hash) {
                                            candidate_hashes.push(rbf_hash);
                                        }
                                    }
                                    Err(_) => {}
                                }
                            }
                            continue;
                        }
                        Err(e) => break Err(e),
                    }
                };

                match receipt_result {
                    Ok(receipt) => {
                        let info = rpc::parse_receipt(&receipt);
                        if info.success {
                            mint_log(reporter.as_ref(), quiet_w,
                                format!(
                                    "[{}] CONFIRMED t+{}ms gas={} block={} tx={}",
                                    sign::shorten_address(&addr),
                                    mint_started_at.elapsed().as_millis(),
                                    info.gas_used,
                                    info.block_number,
                                    sign::shorten_hash(&mined_hash)
                                ),
                            );
                            maybe_beep(beep_w, &first_confirm_w);
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Confirmed),
                                Some(format!("gas={} blk={}", info.gas_used, info.block_number)),
                                Some(mined_hash),
                                None,
                            );
                            break (
                                addr,
                                MintResult {
                                    address: addr,
                                    // Mined hash may be original or any RBF replacement.
                                    tx_hash: Some(mined_hash),
                                    status: WalletStatus::Confirmed,
                                    gas_used: Some(info.gas_used),
                                    block_number: Some(info.block_number),
                                    error: None,
                                },
                            );
                        } else {
                            log_always(reporter.as_ref(), format!("[{}] REVERTED t+{}ms gas={} block={} tx={}",
                                sign::shorten_address(&addr),
                                mint_started_at.elapsed().as_millis(),
                                info.gas_used,
                                info.block_number,
                                sign::shorten_hash(&mined_hash)));

                            // A receipt status alone has no revert reason.  The
                            // mined block timestamp does give us one safe fact:
                            // when it is before this stage's start, SeaDrop
                            // could not possibly have accepted the mint yet.
                            // Only that proven case is eligible for one recovery
                            // transaction, and only after a fresh read-only
                            // estimate says the exact calldata now succeeds.
                            let mined_block_ts = rpc
                                .block_timestamp_at(info.block_number)
                                .await
                                .ok();
                            let proven_early = mined_block_ts.is_some_and(|timestamp| {
                                is_proven_pre_open_revert(stage_start_ts_w, timestamp)
                            });
                            // Receipt JSON contains only status=0. Re-run the
                            // exact call as a read-only estimate after a normal
                            // (non pre-open) revert so the operator sees the
                            // SeaDrop custom error. This happens after mining
                            // and therefore cannot delay the mint shot.
                            let contract_revert = if !proven_early {
                                match tokio::time::timeout(
                                    std::time::Duration::from_millis(1_500),
                                    rpc.estimate_gas(&addr, &tx.to, tx.value, &tx.data),
                                )
                                .await
                                {
                                    Ok(Err(error)) => {
                                        Some(enrich_mint_rpc_error(&format!("{error}")))
                                    }
                                    _ => None,
                                }
                            } else {
                                None
                            };
                            let mut recovery_failure: Option<String> = None;
                            if proven_early
                                && proven_early_recoveries
                                    < MAX_PROVEN_EARLY_REVERT_RECOVERIES
                            {
                                let timestamp = mined_block_ts.unwrap_or_default();
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "[{}] EARLY-BLOCK RECOVERY: block timestamp={} is before phase start={}; validating current contract state before one fresh-nonce retry",
                                        sign::shorten_address(&addr),
                                        timestamp,
                                        stage_start_ts_w.unwrap_or_default()
                                    ),
                                );

                                let validation_deadline = std::time::Instant::now()
                                    + std::time::Duration::from_secs(3);
                                let validation: Result<u64, String> = loop {
                                    if cancelled(&cancel_w) {
                                        break Err(
                                            "cancelled during early-block recovery".into(),
                                        );
                                    }
                                    match rpc
                                        .estimate_gas(&addr, &tx.to, tx.value, &tx.data)
                                        .await
                                    {
                                        Ok(estimate) => {
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] RECOVERY PREFLIGHT OK est={} — contract is active now",
                                                    sign::shorten_address(&addr),
                                                    estimate
                                                ),
                                            );
                                            break Ok(estimate);
                                        }
                                        Err(error) => {
                                            let validation_error =
                                                enrich_mint_rpc_error(&format!("{error}"));
                                            // Sold out, invalid proof, wallet
                                            // cap, payment and funds errors do
                                            // not improve by polling.
                                            if classify_mint_error(&validation_error) == "fatal"
                                            {
                                                break Err(validation_error);
                                            }
                                            if std::time::Instant::now() >= validation_deadline {
                                                break Err(validation_error);
                                            }
                                            tokio::time::sleep(
                                                std::time::Duration::from_millis(100),
                                            )
                                            .await;
                                        }
                                    }
                                };

                                if validation.is_ok() {
                                    match rpc.nonce(&addr).await {
                                        Ok(fresh_nonce) if fresh_nonce > nonce => {
                                            proven_early_recoveries += 1;
                                            nonce = fresh_nonce;
                                            cached_tx =
                                                Some((tx.to, tx.value, tx.data.clone()));
                                            pre_signed_tx = None;
                                            // Every old candidate used the
                                            // consumed nonce and is already
                                            // known reverted.  Keeping it in
                                            // ambiguity checks would make the
                                            // next send look landed and abort
                                            // the valid recovery.
                                            sent_hashes.clear();
                                            log_always(
                                                reporter.as_ref(),
                                                format!(
                                                    "[{}] RECOVERY ARMED with fresh nonce={} (paid recovery {}/{})",
                                                    sign::shorten_address(&addr),
                                                    nonce,
                                                    proven_early_recoveries,
                                                    MAX_PROVEN_EARLY_REVERT_RECOVERIES
                                                ),
                                            );
                                            continue;
                                        }
                                        Ok(fresh_nonce) => {
                                            recovery_failure = Some(format!(
                                                "early-block recovery stopped: pending nonce did not advance (old={nonce}, current={fresh_nonce})"
                                            ));
                                        }
                                        Err(error) => {
                                            recovery_failure = Some(format!(
                                                "early-block recovery stopped: nonce refresh failed: {error}"
                                            ));
                                        }
                                    }
                                } else if let Err(validation_error) = validation {
                                    recovery_failure = Some(format!(
                                        "early-block recovery blocked by current-state preflight: {validation_error}"
                                    ));
                                }
                            }

                            let final_error = recovery_failure.unwrap_or_else(|| {
                                if proven_early {
                                    "reverted in a pre-open block; bounded paid recovery exhausted"
                                        .to_string()
                                } else if let Some(diagnostic) = contract_revert {
                                    format!("contract reverted: {diagnostic}")
                                } else if let Some(timestamp) = mined_block_ts {
                                    format!(
                                        "reverted (mined block timestamp={timestamp}, not a proven pre-open revert)"
                                    )
                                } else {
                                    "reverted; block timestamp unavailable, automatic resend disabled"
                                        .to_string()
                                }
                            });
                            report_wallet(reporter.as_ref(),
                                &addr,
                                Some(WalletStatus::Failed),
                                Some(format!("reverted blk={}", info.block_number)),
                                Some(mined_hash),
                                Some(final_error.clone()),
                            );
                            break (
                                addr,
                                MintResult {
                                    address: addr,
                                    tx_hash: Some(mined_hash),
                                    status: WalletStatus::Failed,
                                    gas_used: Some(info.gas_used),
                                    block_number: Some(info.block_number),
                                    error: Some(final_error),
                                },
                            );
                        }
                    }
                    Err(e) => {
                        let err = format!("receipt: {}", e);
                        report_wallet(reporter.as_ref(),
                            &addr,
                            Some(WalletStatus::Sent),
                            Some("receipt timeout".into()),
                            Some(mined_hash),
                            Some(err.clone()),
                        );
                        break (
                            addr,
                            MintResult {
                                address: addr,
                                tx_hash: Some(mined_hash),
                                status: WalletStatus::Sent,
                                gas_used: None,
                                block_number: None,
                                error: Some(err),
                            },
                        );
                    }
                }
            }
        });
    }

    let n_workers = handles.len();
    // Let every pre-signed wallet enter `race_send` before synchronous file/UI
    // reporting resumes. This keeps the T0 blast free of reporter mutex skew.
    tokio::task::yield_now().await;
    report_phase(reporter.as_ref(), "fire", "Phase open — mints broadcast…");
    if let Some(fire_lag_ms) = scheduled_fire_lag_ms {
        log_always(
            reporter.as_ref(),
            format!(
                "Scheduled fire lag={fire_lag_ms}ms pre_signed={scheduled_pre_signed}/{n_workers} | A/B direct={scheduled_direct_pre_signed}/{scheduled_direct_ready} proxy={scheduled_proxy_pre_signed}/{scheduled_proxy_ready}"
            ),
        );
    }
    log_always(
        reporter.as_ref(),
        format!("Minting with {} wallet worker(s)...", n_workers),
    );
    report_phase(
        reporter.as_ref(),
        "confirm",
        format!("Waiting for confirmations ({n_workers} wallet(s))…"),
    );

    let mut results: Vec<MintResult> = Vec::new();
    while let Some(res) = handles.join_next().await {
        match res {
            Ok((_addr, result)) => {
                report_wallet(
                    reporter.as_ref(),
                    &result.address,
                    Some(result.status),
                    Some(format!("gas={:?}", result.gas_used)),
                    result.tx_hash,
                    result.error.clone(),
                );
                spawn_auto_sweep_for_confirmed(
                    &mut auto_sweep_jobs,
                    &mut auto_sweep_started,
                    &result,
                    &auto_sweep_signers,
                    &rpc,
                    auto_sweep_contract,
                    auto_sweep_destination,
                    &gas_params,
                );
                results.push(result);
            }
            Err(e) => {
                log_always(reporter.as_ref(), format!("Task panicked: {}", e));
            }
        }
    }

    // ── Final reconciliation ──
    //
    // Every worker that broadcast something but could not prove the outcome
    // leaves a `Sent` row carrying a hash. Settle those once against the chain
    // now that the T0 rush is over: by this point a transaction that landed has
    // almost certainly been indexed, and a run must not end telling the
    // operator "unknown" for a mint that is sitting confirmed on chain.
    //
    // Flashbots runs their own inclusion poll below, so skip them here.
    if !use_flashbots && !dry_run && !cancelled(&cancel) {
        let pending: Vec<(usize, B256)> = results
            .iter()
            .enumerate()
            .filter(|(_, r)| r.status == WalletStatus::Sent)
            .filter_map(|(i, r)| r.tx_hash.map(|h| (i, h)))
            .collect();
        if !pending.is_empty() {
            log_always(
                reporter.as_ref(),
                format!(
                    "Reconciling {} unresolved wallet(s) against the chain…",
                    pending.len()
                ),
            );
            let mut settled = 0usize;
            for (idx, hash) in pending {
                // Absent or unknown: leave the row as Sent with its hash.
                // Silently downgrading to Failed here would recreate the very
                // bug this pass exists to prevent.
                if let Ok(Some(receipt)) = rpc.transaction_receipt(&hash).await {
                    {
                        let info = rpc::parse_receipt(&receipt);
                        let addr = results[idx].address;
                        results[idx] = receipt_to_result(addr, hash, &info);
                        settled += 1;
                        log_always(
                            reporter.as_ref(),
                            format!(
                                "[{}] resolved: {} {} block={}",
                                sign::shorten_address(&addr),
                                sign::shorten_hash(&hash),
                                if info.success {
                                    "CONFIRMED"
                                } else {
                                    "REVERTED"
                                },
                                info.block_number
                            ),
                        );
                        report_wallet(
                            reporter.as_ref(),
                            &addr,
                            Some(results[idx].status),
                            Some(format!("reconciled blk={}", info.block_number)),
                            Some(hash),
                            results[idx].error.clone(),
                        );
                        if results[idx].status == WalletStatus::Confirmed {
                            maybe_beep(beep, &first_confirm);
                        }
                    }
                }
            }
            log_always(
                reporter.as_ref(),
                format!("Reconciliation: {settled} resolved on chain"),
            );
        }
    }

    // Flashbots coordinator: sim or send bundle, then receipt poll for pending pieces.
    if use_flashbots {
        let pieces = fb_pieces.lock().map(|g| g.clone()).unwrap_or_default();
        if pieces.is_empty() {
            log_always(
                reporter.as_ref(),
                "Flashbots: no signed pieces (all prep/sim failed)".to_string(),
            );
        } else {
            let fb_cfg = FlashbotsConfig::from_env(env);
            match FlashbotsClient::new(fb_cfg, actual_chain_id) {
                Ok(client) => {
                    let auth = &signers[0];
                    let current = rpc.block_number().await.unwrap_or(0);
                    if dry_run {
                        let target = current.saturating_add(1);
                        log_always(
                            reporter.as_ref(),
                            format!(
                                "Flashbots eth_callBundle: {} tx(s) @ block {}",
                                pieces.len(),
                                target
                            ),
                        );
                        match client.call_bundle(auth, &pieces, target).await {
                            Ok(res) => {
                                let errs = flashbots::call_bundle_errors(&res);
                                log_always(reporter.as_ref(), format!("callBundle result: {res}"));
                                for (i, p) in pieces.iter().enumerate() {
                                    if let Some(Some(err)) = errs.get(i) {
                                        if let Some(r) =
                                            results.iter_mut().find(|r| r.address == p.from)
                                        {
                                            r.status = WalletStatus::Failed;
                                            r.error = Some(format!("callBundle: {err}"));
                                        }
                                    } else if let Some(r) =
                                        results.iter_mut().find(|r| r.address == p.from)
                                    {
                                        r.status = WalletStatus::DryRunOk;
                                        r.error =
                                            Some("sim OK (callBundle) — not submitted".into());
                                    }
                                }
                            }
                            Err(e) => {
                                log_always(reporter.as_ref(), format!("callBundle failed: {e}"));
                                for r in results.iter_mut() {
                                    if r.error.as_deref() == Some("__flashbots_dry__") {
                                        r.status = WalletStatus::Failed;
                                        r.error = Some(format!("sim FAIL (callBundle): {e}"));
                                    }
                                }
                            }
                        }
                    } else {
                        log_always(
                            reporter.as_ref(),
                            format!(
                                "Flashbots eth_sendBundle: {} tx(s) from block {}",
                                pieces.len(),
                                current
                            ),
                        );
                        match client
                            .send_bundle_window(auth, &pieces, current, cancel.clone())
                            .await
                        {
                            Ok(sub) => {
                                log_always(
                                    reporter.as_ref(),
                                    format!(
                                        "submitted targets={:?} hash={:?}",
                                        sub.target_blocks, sub.bundle_hash
                                    ),
                                );
                                for r in results.iter_mut() {
                                    if r.error.as_deref() == Some("__flashbots_pending__") {
                                        r.error = Some("submitted — waiting inclusion".into());
                                    }
                                }
                            }
                            Err(e) => {
                                log_always(reporter.as_ref(), format!("sendBundle failed: {e}"));
                                for r in results.iter_mut() {
                                    if r.error.as_deref() == Some("__flashbots_pending__") {
                                        r.status = WalletStatus::Failed;
                                        r.error = Some(format!("submit FAIL: {e}"));
                                    }
                                }
                            }
                        }
                        // Receipt poll for pending bundle wallets
                        for p in &pieces {
                            if cancelled(&cancel) {
                                break;
                            }
                            let Some(r) = results.iter_mut().find(|r| r.address == p.from) else {
                                continue;
                            };
                            if r.status == WalletStatus::Failed
                                && r.error
                                    .as_ref()
                                    .map(|e| e.starts_with("submit FAIL"))
                                    .unwrap_or(false)
                            {
                                continue;
                            }
                            if r.error.as_deref() != Some("__flashbots_pending__")
                                && r.error.as_deref() != Some("submitted — waiting inclusion")
                                && r.status != WalletStatus::Sent
                            {
                                continue;
                            }
                            match rpc.wait_for_receipt(&p.tx_hash, 90).await {
                                Ok(receipt) => {
                                    let info = rpc::parse_receipt(&receipt);
                                    if info.success {
                                        r.status = WalletStatus::Confirmed;
                                        r.gas_used = Some(info.gas_used);
                                        r.block_number = Some(info.block_number);
                                        r.tx_hash = Some(p.tx_hash);
                                        r.error = Some("confirmed".into());
                                        maybe_beep(beep, &first_confirm);
                                        report_wallet(
                                            reporter.as_ref(),
                                            &p.from,
                                            Some(WalletStatus::Confirmed),
                                            Some(format!("confirmed block={}", info.block_number)),
                                            Some(p.tx_hash),
                                            None,
                                        );
                                    } else {
                                        r.status = WalletStatus::Failed;
                                        r.error = Some("included but reverted".into());
                                        r.tx_hash = Some(p.tx_hash);
                                    }
                                }
                                Err(e) => {
                                    r.status = WalletStatus::Sent;
                                    r.error = Some(format!("submitted — not included ({e})"));
                                    r.tx_hash = Some(p.tx_hash);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    log_always(reporter.as_ref(), format!("Flashbots client error: {e}"));
                    for r in results.iter_mut() {
                        if matches!(
                            r.error.as_deref(),
                            Some("__flashbots_pending__") | Some("__flashbots_dry__")
                        ) {
                            r.status = WalletStatus::Failed;
                            r.error = Some(format!("flashbots: {e}"));
                        }
                    }
                }
            }
        }
        // Strip internal markers
        for r in results.iter_mut() {
            if matches!(
                r.error.as_deref(),
                Some("__flashbots_pending__") | Some("__flashbots_dry__")
            ) {
                r.error = None;
            }
        }
    }

    // Reconciliation and Flashbots inclusion can turn a late Sent row into a
    // confirmed mint after its worker returned. Start those missing sweeps now;
    // already-started wallets are deduplicated by address.
    for result in &results {
        spawn_auto_sweep_for_confirmed(
            &mut auto_sweep_jobs,
            &mut auto_sweep_started,
            result,
            &auto_sweep_signers,
            &rpc,
            auto_sweep_contract,
            auto_sweep_destination,
            &gas_params,
        );
    }
    // Freeze mint timing before waiting for post-mint transfers. Sweep duration
    // must never make the sniper itself look slower in performance logs.
    let mint_elapsed = mint_started_at.elapsed().as_millis() as u64;
    if auto_sweep_destination.is_some() && !auto_sweep_jobs.is_empty() {
        report_phase(
            reporter.as_ref(),
            "sweep",
            format!(
                "Auto-sweeping {} confirmed wallet(s)…",
                auto_sweep_jobs.len()
            ),
        );
        while let Some(joined) = auto_sweep_jobs.join_next().await {
            match joined {
                Ok((address, Ok(report))) if report.skipped_destination => log_always(
                    reporter.as_ref(),
                    format!(
                        "[{}] AUTO-SWEEP skipped: wallet is the destination",
                        sign::shorten_address(&address)
                    ),
                ),
                Ok((address, Ok(report))) => {
                    let detail = report
                        .errors
                        .first()
                        .map(|error| format!(" · {error}"))
                        .unwrap_or_default();
                    log_always(
                        reporter.as_ref(),
                        format!(
                            "[{}] AUTO-SWEEP {}/{} confirmed, {} failed{}",
                            sign::shorten_address(&address),
                            report.swept,
                            report.discovered,
                            report.failed,
                            detail
                        ),
                    );
                }
                Ok((address, Err(error))) => log_always(
                    reporter.as_ref(),
                    format!(
                        "[{}] AUTO-SWEEP failed (mint remains successful): {}",
                        sign::shorten_address(&address),
                        error
                    ),
                ),
                Err(error) => log_always(
                    reporter.as_ref(),
                    format!("AUTO-SWEEP worker failed: {error}"),
                ),
            }
        }
    }

    let elapsed = mint_elapsed;
    // Success only after on-chain confirm (or dry-run OK). SENT is not enough.
    let confirmed = results
        .iter()
        .filter(|r| matches!(r.status, WalletStatus::Confirmed | WalletStatus::DryRunOk))
        .count();
    let failed = results
        .iter()
        .filter(|r| matches!(r.status, WalletStatus::Failed))
        .count();
    let unresolved = results
        .iter()
        .filter(|r| r.status == WalletStatus::Sent && r.error.is_some())
        .count();
    report_phase(
        reporter.as_ref(),
        "done",
        format!("Done: {confirmed} ok · {failed} fail · {unresolved} unresolved · {elapsed}ms"),
    );
    let total = results.len();
    log_always(
        reporter.as_ref(),
        format!(
            "Done: {}/{} ok, {} failed, {} unresolved, total={}ms",
            confirmed, total, failed, unresolved, elapsed
        ),
    );
    let mut export_json = None;
    let mut export_csv = None;
    if do_export {
        let run = export::MintRunExport {
            slug: info.slug.clone(),
            chain: info.chain.clone(),
            phase: stage_type_owned.clone(),
            started_at: chrono::Utc::now().to_rfc3339(),
            elapsed_ms: elapsed,
            quiet,
            skip_preflight,
            dry_run,
            wallets: results
                .iter()
                .map(|w| {
                    export::wallet_row(
                        w.address,
                        w.status,
                        w.tx_hash,
                        w.gas_used,
                        w.block_number,
                        w.error.clone(),
                        None,
                    )
                })
                .collect(),
            confirmed,
            failed,
            total,
        };
        match export::write_mint_results(&run) {
            Ok((json_p, csv_p)) => {
                export_json = Some(export::path_display(&json_p));
                export_csv = Some(export::path_display(&csv_p));
                log_always(
                    reporter.as_ref(),
                    format!(
                        "Results exported: {} | {}",
                        export_json.as_deref().unwrap_or("-"),
                        export_csv.as_deref().unwrap_or("-")
                    ),
                );
            }
            Err(e) => log_always(reporter.as_ref(), format!("Export failed: {}", e)),
        }

        // Structured run metrics (plan #9): wallet outcomes + failure histogram
        // + total elapsed, assembled from the finished run. Per-phase spans and
        // precise t0 are a follow-up (they require threading the collector
        // through the mint loop); this keeps the hot path untouched.
        let collector = crate::metrics::MetricsCollector::new(
            "opensea",
            info.slug.clone(),
            info.chain.clone(),
            dry_run,
            Some(stage_type_owned.clone()),
        );
        for w in &results {
            let mut wm = crate::metrics::WalletMetrics::new(format!("{:?}", w.address));
            wm.status = w.status.to_string();
            wm.tx_hash = w
                .tx_hash
                .map(|h| format!("0x{}", hex::encode(h.as_slice())));
            wm.gas_used = w.gas_used;
            if let Some(err) = w.error.clone() {
                wm = wm.with_error(err);
            }
            collector.upsert_wallet(wm);
        }
        let mut metrics = collector.finish();
        metrics.summary.elapsed_ms = elapsed; // authoritative run duration
        metrics.spans.done_ms = Some(elapsed);
        match export::write_run_metrics(&metrics) {
            Ok(p) => log_always(
                reporter.as_ref(),
                format!("Metrics: {}", export::path_display(&p)),
            ),
            Err(e) => log_always(reporter.as_ref(), format!("Metrics write failed: {}", e)),
        }
    }

    let wallets: Vec<crate::api::SweepResultRow> = results
        .iter()
        .map(|r| crate::api::SweepResultRow {
            address: format!("{:?}", r.address),
            status: r.status.to_string(),
            tx_hash: r
                .tx_hash
                .map(|h| format!("0x{}", hex::encode(h.as_slice()))),
            gas_used: r.gas_used,
            block_number: r.block_number,
            error: r.error.clone(),
            contract: None,
            token_id: None,
            token_type: None,
            amount: None,
        })
        .collect();

    Ok(MintRunSummary {
        slug: info.slug,
        chain: info.chain,
        phase: stage_type_owned,
        dry_run,
        elapsed_ms: elapsed,
        results,
        confirmed,
        failed,
        export_json,
        export_csv,
        wallets,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        GQL_STAGGER_MAX_SPREAD_MS, NOT_ACTIVE_CHAIN_WAIT_MAX_SECS, NotActiveInfo,
        OPENSEA_MINT_ACTION_BUDGET, OPENSEA_MINT_ACTION_REFILL_MS, PHASE_OPEN_LAG_WINDOW_SECS,
        RATE_LIMIT_WAIT_BUDGET_MS, ScheduledPreopenPlan, SeaDropPublicState, WalletAuth,
        assigned_proxy_routes, build_local_public_mint, classify_mint_error,
        decode_common_seadrop_revert, enrich_mint_rpc_error, estimate_fail_policy,
        fire_lag_ms_from_clock, format_not_active, format_rpc_plan, gql_action_not_ready_delay,
        gql_stagger_step_ms, in_phase_open_lag_window, initial_force_fixed_gas,
        is_gql_action_not_ready, is_proven_pre_open_revert, is_terminal_gql_action_error,
        keep_before_opensea_auth, parse_not_active, parse_tx_calldata_hex, pre_sign_ready_wallets,
        rate_limit_backoff, required_mint_balance, resolve_mint_gas_limit,
        resolve_public_fee_recipient, scheduled_preopen_plan, validate_seadrop_calldata,
        validate_seadrop_public_state, validate_wallet_subset_counts,
    };
    use crate::proxy::ProxyManager;
    use crate::types::Signer;
    use alloy_primitives::{Address, Bytes, U256};
    use std::collections::HashSet;

    fn probe(url: &str, ok: bool, ms: Option<u64>) -> crate::rpc::RpcNodeProbe {
        crate::rpc::RpcNodeProbe {
            url_short: url.to_string(),
            ok,
            latency_ms: ms,
        }
    }

    #[test]
    fn rpc_plan_names_the_lead_and_the_broadcast_set() {
        // Two healthy endpoints, fan-out covers both: the operator must be able
        // to read off which one leads and that the second also gets the tx.
        let plan = format_rpc_plan(
            "robinhood",
            &[
                probe("https://paid.example", true, Some(12)),
                probe("https://rpc.mainnet.chain.robinhood.com", true, Some(38)),
            ],
            3,
        );
        assert!(plan[0].contains("2 usable endpoint(s)"), "{:?}", plan[0]);
        assert!(plan[0].contains("broadcast reaches 2"), "{:?}", plan[0]);
        assert!(plan[1].contains("[1] LEAD") && plan[1].contains("paid.example"));
        assert!(plan[1].contains("ping=12ms"));
        assert!(plan[2].contains("[2] broadcast") && plan[2].contains("robinhood.com"));
    }

    #[test]
    fn rpc_plan_marks_endpoints_beyond_the_fanout_as_unused() {
        // A configured node the broadcast never reaches must say so, otherwise
        // the operator assumes redundancy that does not exist.
        let plan = format_rpc_plan(
            "ethereum",
            &[
                probe("https://a.example", true, Some(10)),
                probe("https://b.example", true, Some(20)),
            ],
            1,
        );
        assert!(plan[0].contains("broadcast reaches 1"));
        assert!(plan[1].contains("[1] LEAD"));
        assert!(plan[2].contains("[2] unused"), "{:?}", plan[2]);
    }

    #[test]
    fn rpc_plan_reports_excluded_nodes_without_ranking_them() {
        // A failed probe is dropped from the run, but stays visible — and must
        // never take a rank, or a dead node would read as the lead.
        let plan = format_rpc_plan(
            "base",
            &[
                probe("https://good.example", true, Some(30)),
                probe("https://dead.example", false, Some(2)),
            ],
            3,
        );
        assert!(plan[0].contains("1 usable endpoint(s)"));
        assert!(plan[1].contains("[1] LEAD") && plan[1].contains("good.example"));
        assert!(plan[2].contains("[x] EXCLUDED") && plan[2].contains("dead.example"));
        assert!(!plan[2].contains("LEAD"));
    }

    #[test]
    fn rpc_plan_handles_a_single_unmeasured_endpoint() {
        // One URL short-circuits the latency sort, so there is no ping to show.
        // It must still be reported as the lead rather than silently omitted.
        let plan = format_rpc_plan("robinhood", &[probe("https://only.example", true, None)], 3);
        assert!(plan[0].contains("1 usable endpoint(s)"));
        assert!(plan[1].contains("[1] LEAD"));
        assert!(plan[1].contains("not measured"));
    }

    #[test]
    fn six_wallet_ab_routes_three_direct_and_three_proxy() {
        let signers: Vec<Signer> = (1u8..=6)
            .map(|i| format!("{i:064x}").parse().unwrap())
            .collect();
        let proxies = ProxyManager::from_text(
            "proxy-a.example:8001\nproxy-b.example:8002\nproxy-c.example:8003",
        );
        let direct: HashSet<String> = signers[..3]
            .iter()
            .map(|s| crate::api::normalize_address(&format!("{:?}", s.address())))
            .collect();

        let routes = assigned_proxy_routes(&signers, &proxies, &direct);

        assert_eq!(routes.len(), 6);
        assert!(routes[..3].iter().all(Option::is_none));
        assert!(routes[3..].iter().all(Option::is_some));
        assert_eq!(routes[3].as_deref(), proxies.get(0));
        assert_eq!(routes[4].as_deref(), proxies.get(1));
        assert_eq!(routes[5].as_deref(), proxies.get(2));
    }

    #[test]
    fn whitelist_prefetch_is_fully_signed_before_fire() {
        let signer: Signer = "0000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap();
        let to: Address = "0x1111111111111111111111111111111111111111"
            .parse()
            .unwrap();
        let mut wallets = vec![WalletAuth {
            address: signer.address(),
            signer,
            session: None,
            auth_ok: true,
            auth_elapsed_ms: None,
            auth_cached: false,
            nonce: 7,
            prefetched_tx: Some((to, U256::from(123u64), Bytes::from(vec![1, 2, 3, 4]))),
            pre_signed_tx: None,
            conditional_hash: None,
            proxy_url: None,
        }];

        assert_eq!(
            pre_sign_ready_wallets(
                &mut wallets,
                4663,
                650_000,
                U256::from(2_000_000_000u64),
                U256::from(1_000_000_000u64),
            ),
            1
        );
        let prepared = wallets[0].pre_signed_tx.as_ref().unwrap();
        assert_eq!(prepared.tx.chain_id, 4663);
        assert_eq!(prepared.tx.nonce, 7);
        assert_eq!(prepared.tx.to, to);
        assert_eq!(prepared.gas_limit, 650_000);
        assert!(!prepared.raw.is_empty());
        // Already-armed wallets must not be signed a second time.
        assert_eq!(
            pre_sign_ready_wallets(
                &mut wallets,
                4663,
                650_000,
                U256::from(2_000_000_000u64),
                U256::from(1_000_000_000u64),
            ),
            0
        );
    }

    #[test]
    fn public_sale_can_be_built_and_signed_before_fire() {
        let signer: Signer = "0000000000000000000000000000000000000000000000000000000000000002"
            .parse()
            .unwrap();
        let prefetched = build_local_public_mint(
            "0x2222222222222222222222222222222222222222",
            2,
            U256::from(130_000_000_000_000u64),
            None,
            None,
            signer.address(),
        )
        .unwrap();
        assert_eq!(prefetched.1, U256::from(260_000_000_000_000u64));
        assert_eq!(prefetched.2.len(), 132);
        let mut wallets = vec![WalletAuth {
            address: signer.address(),
            signer,
            session: None,
            auth_ok: true,
            auth_elapsed_ms: None,
            auth_cached: false,
            nonce: 9,
            prefetched_tx: Some(prefetched),
            pre_signed_tx: None,
            conditional_hash: None,
            proxy_url: None,
        }];
        assert_eq!(
            pre_sign_ready_wallets(
                &mut wallets,
                4663,
                250_000,
                U256::from(2_000_000_000u64),
                U256::from(1_000_000u64),
            ),
            1
        );
        assert!(wallets[0].pre_signed_tx.is_some());
    }

    #[test]
    fn fire_lag_uses_millis_not_seconds() {
        // open at t=1000s, now = 1000s + 250ms
        assert_eq!(fire_lag_ms_from_clock(1000, 1_000_250), 250);
        // exactly on open
        assert_eq!(fire_lag_ms_from_clock(1000, 1_000_000), 0);
        // slightly early → 0
        assert_eq!(fire_lag_ms_from_clock(1000, 999_900), 0);
        // 1.5s late
        assert_eq!(fire_lag_ms_from_clock(1000, 1_001_500), 1500);
    }

    #[test]
    fn parse_at_time_invalid_is_err_not_silent() {
        // Core schedule path uses the same helper — invalid must error.
        assert!(crate::mint_ops::parse_at_time_unix("not-a-time").is_err());
        assert!(crate::mint_ops::parse_at_time_unix("2020-13-40T99:99:99Z").is_err());
        assert_eq!(
            crate::mint_ops::parse_at_time_unix("1700000000").unwrap(),
            Some(1_700_000_000)
        );
    }

    #[test]
    fn parse_calldata_rejects_empty() {
        assert!(parse_tx_calldata_hex("").is_err());
        assert!(parse_tx_calldata_hex("0x").is_err());
        assert!(parse_tx_calldata_hex("   ").is_err());
    }

    #[test]
    fn parse_calldata_rejects_invalid_hex() {
        assert!(parse_tx_calldata_hex("0xzz").is_err());
        assert!(parse_tx_calldata_hex("not-hex").is_err());
    }

    #[test]
    fn parse_calldata_ok_selector() {
        let b = parse_tx_calldata_hex("0xa0712d68").unwrap();
        assert_eq!(b.as_ref(), &[0xa0, 0x71, 0x2d, 0x68]);
        let b2 = parse_tx_calldata_hex("a0712d68").unwrap();
        assert_eq!(b.as_ref(), b2.as_ref());
    }

    #[test]
    fn validates_public_seadrop_wallet_contract_and_quantity() {
        let nft: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .unwrap();
        let minter: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .unwrap();
        let tx = crate::opensea::build_public_mint_tx(
            &format!("{nft:?}"),
            2,
            U256::from(1u64),
            None,
            None,
            Some(&format!("{minter:?}")),
        )
        .unwrap();
        let data = parse_tx_calldata_hex(tx["data"].as_str().unwrap()).unwrap();
        validate_seadrop_calldata(data.as_ref(), nft, minter, 2, None).unwrap();

        let other: Address = "0x4444444444444444444444444444444444444444"
            .parse()
            .unwrap();
        assert!(validate_seadrop_calldata(data.as_ref(), nft, other, 2, None).is_err());
        assert!(validate_seadrop_calldata(data.as_ref(), nft, minter, 1, None).is_err());
    }

    #[test]
    fn validates_signed_stage_and_signature_bounds() {
        use alloy_primitives::keccak256;
        let nft: Address = "0x2222222222222222222222222222222222222222"
            .parse()
            .unwrap();
        let minter: Address = "0x3333333333333333333333333333333333333333"
            .parse()
            .unwrap();
        let mut data = keccak256("mintSigned(address,address,address,uint256,(uint256,uint256,uint256,uint256,uint256,uint256,uint256,bool),uint256,bytes)".as_bytes())[..4].to_vec();
        let mut words = vec![[0u8; 32]; 14];
        words[0][12..].copy_from_slice(nft.as_slice());
        words[2][12..].copy_from_slice(minter.as_slice());
        words[3][24..].copy_from_slice(&2u64.to_be_bytes());
        words[8][24..].copy_from_slice(&7u64.to_be_bytes());
        words[11][31] = 1;
        words[13][24..].copy_from_slice(&(14u64 * 32).to_be_bytes());
        for word in words {
            data.extend_from_slice(&word);
        }
        let mut length = [0u8; 32];
        length[24..].copy_from_slice(&65u64.to_be_bytes());
        data.extend_from_slice(&length);
        data.extend_from_slice(&[0xabu8; 65]);

        validate_seadrop_calldata(&data, nft, minter, 2, Some(7)).unwrap();
        assert!(validate_seadrop_calldata(&data, nft, minter, 2, Some(8)).is_err());
        data.truncate(data.len() - 1);
        assert!(validate_seadrop_calldata(&data, nft, minter, 2, Some(7)).is_err());
    }

    #[test]
    fn mint_gas_estimate_applies_l2_floor() {
        // Base (8453): raw 21k estimate must floor to >= 150k
        let lim = resolve_mint_gas_limit(21_000, 1.15, 8453, false);
        assert!(lim >= 150_000, "lim={lim}");
        // Ethereum mainnet: no 150k floor
        let eth = resolve_mint_gas_limit(21_000, 1.15, 1, false);
        assert!(eth >= 21_000);
        assert!(eth < 150_000);
    }

    #[test]
    fn mint_gas_fixed_clamps_l2_floor() {
        let lim = resolve_mint_gas_limit(50_000, 1.0, 8453, true);
        assert_eq!(lim, 150_000);
        let eth = resolve_mint_gas_limit(50_000, 1.0, 1, true);
        assert_eq!(eth, 50_000);
    }

    #[test]
    fn generic_execution_reverted_is_retryable() {
        assert_eq!(classify_mint_error("execution reverted"), "retryable");
        assert_eq!(
            classify_mint_error("Error: execution reverted: unknown reason"),
            "retryable"
        );
        assert_eq!(
            classify_mint_error("RPC eth_estimateGas error: {\"message\":\"execution reverted\"}"),
            "retryable"
        );
    }

    #[test]
    fn known_contract_errors_are_fatal() {
        assert_eq!(
            classify_mint_error("execution reverted: InvalidProof"),
            "fatal"
        );
        assert_eq!(classify_mint_error("IncorrectPayment"), "fatal");
        assert_eq!(classify_mint_error("MintQuantityExceedsMaxSupply"), "fatal");
        assert_eq!(
            classify_mint_error("insufficient funds for gas * price + value"),
            "fatal"
        );
        assert_eq!(
            classify_mint_error(
                "RPC eth_estimateGas error: {\"code\":-32003,\"message\":\"EVM error: OutOfFunds\"}"
            ),
            "fatal"
        );
        assert_eq!(classify_mint_error("SignatureAlreadyUsed()"), "fatal");
        assert_eq!(classify_mint_error("PayerNotAllowed"), "fatal");
        assert_eq!(
            classify_mint_error("MintQuantityExceedsMaxMintedPerWallet"),
            "fatal"
        );
    }

    #[test]
    fn temporary_or_unknown_errors_are_retryable() {
        assert_eq!(classify_mint_error("timeout"), "retryable");
        assert_eq!(classify_mint_error("nonce too low"), "retryable");
        assert_eq!(
            classify_mint_error("replacement transaction underpriced"),
            "retryable"
        );
        assert_eq!(classify_mint_error(""), "retryable");
    }

    /// Real Wonkies / SeaDrop estimateGas blob from mint_wonkiescc0 log.
    const WONKIES_NOT_ACTIVE: &str = concat!(
        r#"RPC eth_estimateGas via https://eth-mainnet.g.alchemy....24dMEiRl error: {"code":3,"data":"0x"#,
        "13da22f2",
        "000000000000000000000000000000000000000000000000000000006a60daef",
        "000000000000000000000000000000000000000000000000000000006a60daf0",
        "000000000000000000000000000000000000000000000000000000006a60e900",
        r#"","message":"execution reverted"}"#
    );

    #[test]
    fn parse_not_active_from_wonkies_log() {
        let info = parse_not_active(WONKIES_NOT_ACTIVE).expect("decode NotActive");
        assert_eq!(info.chain_ts, 1_784_732_399); // 14:59:59 UTC
        assert_eq!(info.start_ts, 1_784_732_400); // 15:00:00
        assert_eq!(info.end_ts, 1_784_736_000); // 16:00:00
        let wait = info.start_ts.saturating_sub(info.chain_ts);
        assert_eq!(wait, 1);
    }

    #[test]
    fn phoenix_pre_open_block_is_the_only_paid_recovery_case() {
        let start = 1_787_252_400i64;
        assert!(is_proven_pre_open_revert(Some(start), 1_787_252_399));
        assert!(!is_proven_pre_open_revert(Some(start), 1_787_252_400));
        assert!(!is_proven_pre_open_revert(Some(start), 1_787_252_401));
        assert!(!is_proven_pre_open_revert(None, 1_787_252_399));
        assert!(!is_proven_pre_open_revert(Some(start), 1_787_252_300));
    }

    #[test]
    fn wallet_subset_must_match_the_requested_set_exactly() {
        assert!(validate_wallet_subset_counts(10, 10, 10).is_ok());
        let missing = validate_wallet_subset_counts(10, 10, 1).unwrap_err();
        assert!(missing.to_string().contains("requested 10, found 1"));
        let duplicate = validate_wallet_subset_counts(10, 9, 9).unwrap_err();
        assert!(duplicate.to_string().contains("10 entries but 9 unique"));
    }

    #[test]
    fn early_balance_gate_only_drops_proven_zero_balances() {
        assert!(!keep_before_opensea_auth(Some(U256::ZERO)));
        assert!(keep_before_opensea_auth(Some(U256::from(1u64))));
        // None models an RPC read failure. The wallet must survive for the
        // existing exact price-plus-gas check instead of disappearing silently.
        assert!(keep_before_opensea_auth(None));

        let mut fifty_selected = vec![Some(U256::ZERO); 50];
        for balance in fifty_selected.iter_mut().take(10) {
            *balance = Some(U256::from(1u64));
        }
        assert_eq!(
            fifty_selected
                .into_iter()
                .filter(|balance| keep_before_opensea_auth(*balance))
                .count(),
            10
        );
    }

    #[test]
    fn balance_gate_uses_the_exact_live_transaction_ceiling() {
        let mint_value = U256::ZERO;
        let gas_limit = 250_000u64;
        let max_fee = U256::from(77_888_000u64);
        assert_eq!(
            required_mint_balance(mint_value, gas_limit, max_fee),
            U256::from(19_472_000_000_000u64)
        );
        assert!(
            U256::from(10_000_000_000_000u64)
                < required_mint_balance(mint_value, gas_limit, max_fee),
            "the 0.00001 ETH ROBINMAP wallets must be rejected during preparation"
        );
    }

    #[test]
    fn one_hundred_signed_wallets_schedule_no_preopen_rpc_refresh() {
        for _ in 0..100 {
            assert_eq!(
                scheduled_preopen_plan("SIGNED_PRESALE", true),
                ScheduledPreopenPlan::CachedOnly
            );
            assert!(initial_force_fixed_gas(false, false, true, false));
        }
        assert_eq!(
            scheduled_preopen_plan("PUBLIC_SALE", false),
            ScheduledPreopenPlan::ValidatePublicState
        );
    }

    #[test]
    fn format_not_active_includes_wait() {
        let info = NotActiveInfo {
            chain_ts: 1_784_732_399,
            start_ts: 1_784_732_400,
            end_ts: 1_784_736_000,
        };
        let s = format_not_active(&info);
        assert!(s.contains("NotActive:"), "{s}");
        assert!(s.contains("wait ~1s"), "{s}");
        assert!(s.contains("1784732399"), "{s}");
        assert!(s.contains("1784732400"), "{s}");
    }

    #[test]
    fn enrich_mint_rpc_error_decodes_selector() {
        let e = enrich_mint_rpc_error(WONKIES_NOT_ACTIVE);
        assert!(e.starts_with("NotActive:"), "{e}");
        assert!(e.contains("wait ~1s"), "{e}");
        // still classified retryable (not fatal)
        assert_eq!(classify_mint_error(WONKIES_NOT_ACTIVE), "retryable");
    }

    #[test]
    fn estimate_fail_policy_forces_fixed_near_open() {
        let start = 1_784_732_400i64;
        // wall 10s after open
        let (enriched, force, wait_ms) =
            estimate_fail_policy(WONKIES_NOT_ACTIVE, Some(start), start + 10);
        assert!(force, "should force fixed gas");
        assert!((150..=2_000).contains(&wait_ms), "wait_ms={wait_ms}");
        assert!(enriched.contains("NotActive"), "{enriched}");
    }

    #[test]
    fn estimate_fail_policy_forces_fixed_from_chain_wait_alone() {
        // Phase start far in past by wall, but decoded wait is 1s → still force fixed.
        let (enriched, force, wait_ms) =
            estimate_fail_policy(WONKIES_NOT_ACTIVE, Some(1_000_000), 2_000_000);
        assert!(force, "chain wait 1s ≤ {NOT_ACTIVE_CHAIN_WAIT_MAX_SECS}");
        assert!(wait_ms >= 150, "wait_ms={wait_ms}");
        assert!(enriched.contains("wait ~1s"), "{enriched}");
    }

    #[test]
    fn phase_open_lag_window() {
        let start = 1_000_000i64;
        assert!(in_phase_open_lag_window(Some(start), start));
        assert!(in_phase_open_lag_window(
            Some(start),
            start + PHASE_OPEN_LAG_WINDOW_SECS
        ));
        assert!(!in_phase_open_lag_window(
            Some(start),
            start + PHASE_OPEN_LAG_WINDOW_SECS + 1
        ));
        assert!(!in_phase_open_lag_window(Some(start), start - 5));
        assert!(!in_phase_open_lag_window(None, start));
    }

    #[test]
    fn parse_not_active_rejects_unrelated() {
        assert!(parse_not_active("timeout").is_none());
        assert!(parse_not_active("execution reverted: InvalidProof").is_none());
    }

    fn any_addr(last_byte: u8) -> Address {
        let mut b = [0u8; 20];
        b[19] = last_byte;
        Address::from(b)
    }

    #[test]
    fn a_wallet_alone_on_its_proxy_is_never_delayed() {
        // The common well-proxied case: one wallet per exit IP competes with
        // nobody and must reach OpenSea at T0+0.
        assert_eq!(gql_stagger_step_ms(0), 0);
        assert_eq!(gql_stagger_step_ms(1), 0);
    }

    #[test]
    fn wallets_sharing_an_ip_get_the_full_gap() {
        // Five wallets on one IP is exactly the budget: space them so they do
        // not arrive in the same millisecond. 4 gaps of 25ms costs 100ms.
        assert_eq!(gql_stagger_step_ms(5), 25);
        assert_eq!(gql_stagger_step_ms(20), 25);
    }

    #[test]
    fn a_crowded_ip_compresses_rather_than_arriving_late() {
        // Arriving a second late is recoverable; arriving after a sell-out is
        // not. The spread is capped, so the step shrinks instead.
        for n in [61usize, 100, 250, 1000] {
            let step = gql_stagger_step_ms(n);
            let spread = step * (n as u64 - 1);
            assert!(
                spread <= GQL_STAGGER_MAX_SPREAD_MS,
                "{n} wallets spread over {spread}ms"
            );
        }
    }

    #[test]
    fn the_step_never_grows_with_the_group_size() {
        let mut prev = u64::MAX;
        for n in [2usize, 5, 10, 50, 61, 100, 500] {
            let step = gql_stagger_step_ms(n);
            assert!(step <= prev, "step grew at {n}: {prev} -> {step}");
            prev = step;
        }
    }

    #[test]
    fn the_measured_budget_is_recorded_where_the_code_uses_it() {
        // Both numbers come from probing the live endpoint, not from taste:
        // the fifth request reports remaining=0, the sixth is refused, and a
        // token returns at about 4.3s. A hundred wallets therefore need at
        // least twenty proxies to fire without waiting.
        assert_eq!(OPENSEA_MINT_ACTION_BUDGET, 5);
        assert_eq!(OPENSEA_MINT_ACTION_REFILL_MS, 4_300);
        assert!(100usize.div_ceil(OPENSEA_MINT_ACTION_BUDGET) == 20);
    }

    #[test]
    fn missing_signed_action_is_the_only_gql_parse_error_retried_to_full_budget() {
        let start = 1_000_000;
        assert!(is_gql_action_not_ready(
            "OpenSea mint action response has no transactionSubmissionData",
            Some(start),
            start,
        ));
        assert!(is_gql_action_not_ready(
            "OpenSea mint action response has no transactionSubmissionData; action errors: DropNotMintingError",
            Some(start),
            start,
        ));
        assert!(!is_gql_action_not_ready(
            "OpenSea mint action response has no transactionSubmissionData; action errors: MinterNotEligibleForActiveDropStageError",
            Some(start),
            start,
        ));
        assert!(!is_gql_action_not_ready(
            "OpenSea mint action response has no transactionSubmissionData; action errors: InsufficientMintsRemainingError",
            Some(start),
            start,
        ));
        assert!(is_terminal_gql_action_error(
            "OpenSea mint action response has no transactionSubmissionData; action errors: MinterNotEligibleForActiveDropStageError",
            Some(start),
            start,
        ));
        assert!(!is_gql_action_not_ready(
            "OpenSea transactionSubmissionData has no data",
            Some(start),
            start,
        ));
        assert!(!is_gql_action_not_ready(
            "invalid tx to",
            Some(start),
            start,
        ));
    }

    #[test]
    fn public_price_increase_is_never_implicitly_authorized() {
        let state = SeaDropPublicState {
            mint_price: U256::from(250_000_000_000_000u64),
            start_time: 900,
            end_time: 2_000,
            max_total_mintable_by_wallet: 25,
            restrict_fee_recipients: false,
            allowed_fee_recipients: vec![],
        };
        let error = validate_seadrop_public_state(&state, U256::ZERO, 25, 1_000)
            .expect_err("a free task must not turn into a paid mint");
        assert!(error.to_string().contains("price changed"));
    }

    #[test]
    fn public_state_accepts_only_the_exact_saved_price_inside_window() {
        let state = SeaDropPublicState {
            mint_price: U256::from(5u64),
            start_time: 900,
            end_time: 2_000,
            max_total_mintable_by_wallet: 25,
            restrict_fee_recipients: false,
            allowed_fee_recipients: vec![],
        };
        assert_eq!(
            validate_seadrop_public_state(&state, U256::from(5u64), 25, 1_000).unwrap(),
            U256::from(5u64)
        );
        assert!(validate_seadrop_public_state(&state, U256::from(10u64), 25, 1_000).is_err());
        assert!(validate_seadrop_public_state(&state, U256::from(5u64), 26, 1_000).is_err());
        assert!(validate_seadrop_public_state(&state, U256::from(5u64), 25, 2_000).is_err());
    }

    #[test]
    fn restricted_public_drop_uses_owner_approved_recipient() {
        let approved: Address = "0x07D3A100c3880830dD43FE5C938B5144721Ce9D6"
            .parse()
            .unwrap();
        let state = SeaDropPublicState {
            mint_price: U256::ZERO,
            start_time: 0,
            end_time: u64::MAX,
            max_total_mintable_by_wallet: 1,
            restrict_fee_recipients: true,
            allowed_fee_recipients: vec![approved],
        };
        assert_eq!(
            resolve_public_fee_recipient(&state, None).unwrap(),
            Some(format!("{approved:?}"))
        );
        assert!(
            resolve_public_fee_recipient(
                &state,
                Some("0x0000a26b00c1F0DF003000390027140000fAa719")
            )
            .is_err()
        );
    }

    #[test]
    fn common_seadrop_reverts_are_human_readable() {
        assert_eq!(
            decode_common_seadrop_revert("execution reverted: 0xf477d26f").unwrap(),
            "FeeRecipientNotAllowed: fee recipient is not approved by this collection"
        );
        let supply_error = format!(
            "rpc data=0xe12d2314{:064x}{:064x}",
            U256::from(3001u64),
            U256::from(3000u64)
        );
        assert_eq!(
            decode_common_seadrop_revert(&supply_error).unwrap(),
            "MintQuantityExceedsMaxSupply: requested total 3001, collection max supply 3000"
        );
    }

    #[test]
    fn missing_action_retry_stays_fast_at_t0_and_bounded_afterwards() {
        let waits: Vec<u128> = (1..=20)
            .map(|attempt| gql_action_not_ready_delay(attempt).as_millis())
            .collect();
        assert_eq!(&waits[..4], &[50, 75, 100, 150]);
        assert!(waits.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(waits.iter().all(|wait| *wait <= 500));
    }

    #[test]
    fn honours_the_wait_opensea_asked_for() {
        let mut budget = RATE_LIMIT_WAIT_BUDGET_MS;
        let wait = rate_limit_backoff(
            "Mint action failed: HTTP 429 Too Many Requests retry-after=2500ms",
            &any_addr(0),
            &mut budget,
        )
        .expect("a 429 with an explicit wait must be honoured");
        // 2500 asked + the lockstep jitter, and drawn from the budget.
        assert!(wait.as_millis() >= 2500, "{:?}", wait);
        assert_eq!(budget, RATE_LIMIT_WAIT_BUDGET_MS - 2500);
    }

    #[test]
    fn ordinary_errors_are_not_rate_limits() {
        let mut budget = RATE_LIMIT_WAIT_BUDGET_MS;
        for e in [
            "execution reverted",
            "no auth session",
            "HTTP 500 internal error",
            // A bare "429" inside a value must not read as a rate limit.
            "insufficient funds: have 1429000000000000",
        ] {
            assert!(
                rate_limit_backoff(e, &any_addr(1), &mut budget).is_none(),
                "{e}"
            );
        }
        assert_eq!(
            budget, RATE_LIMIT_WAIT_BUDGET_MS,
            "budget must be untouched"
        );
    }

    #[test]
    fn unmistakable_wording_without_a_number_still_waits() {
        let mut budget = RATE_LIMIT_WAIT_BUDGET_MS;
        assert!(rate_limit_backoff("Too Many Requests", &any_addr(2), &mut budget).is_some());
        assert!(rate_limit_backoff("opensea rate limit", &any_addr(2), &mut budget).is_some());
    }

    #[test]
    fn the_budget_runs_out_so_a_wallet_cannot_be_parked_forever() {
        let mut budget = RATE_LIMIT_WAIT_BUDGET_MS;
        let err = "HTTP 429 Too Many Requests retry-after=5000ms";
        let addr = any_addr(3);
        let mut honoured = 0;
        while rate_limit_backoff(err, &addr, &mut budget).is_some() {
            honoured += 1;
            assert!(honoured < 100, "backoff never gave up");
        }
        assert_eq!(budget, 0);
        // 12s of budget against a 5s ask: two full waits and a clipped one.
        assert_eq!(honoured, 3);
    }

    #[test]
    fn a_wait_is_clipped_to_what_is_left_not_refused() {
        // Better to spend the remainder than to fail immediately: the drop may
        // still be open.
        let mut budget = 400;
        let wait = rate_limit_backoff(
            "429 Too Many Requests retry-after=9000ms",
            &any_addr(4),
            &mut budget,
        )
        .expect("should spend what is left");
        assert!(wait.as_millis() < 9000);
        assert_eq!(budget, 0);
    }

    #[test]
    fn limited_wallets_do_not_come_back_in_lockstep() {
        // Returning together is what produced the limit. Same instruction,
        // different wallets, different wake-up times.
        let err = "429 Too Many Requests retry-after=2000ms";
        let waits: Vec<u128> = (0u8..8)
            .map(|i| {
                let mut b = RATE_LIMIT_WAIT_BUDGET_MS;
                rate_limit_backoff(err, &any_addr(i * 31), &mut b)
                    .unwrap()
                    .as_millis()
            })
            .collect();
        let unique: std::collections::HashSet<_> = waits.iter().collect();
        assert!(
            unique.len() > 1,
            "all wallets woke at the same instant: {waits:?}"
        );
        // Deterministic: the same wallet must behave the same way twice.
        let mut b = RATE_LIMIT_WAIT_BUDGET_MS;
        assert_eq!(
            rate_limit_backoff(err, &any_addr(0), &mut b)
                .unwrap()
                .as_millis(),
            waits[0]
        );
    }
}
