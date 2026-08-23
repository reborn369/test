//! Safe adapter for Archetype ERC-721A mint contracts.
//!
//! Phase keys are discovered from the public `Invited(bytes32,bytes32)` event,
//! then every phase is read and priced directly from the target contract. The
//! explorer is discovery-only: all values used for signing are validated by
//! `eth_call` and locked with a terms hash.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, B256, U256, keccak256};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::abi::{build_calldata, extract_selectors, function_selector};
use crate::amount;
use crate::raw_mint::{explorer_api_for_chain, resolve_implementation};
use crate::rpc::RpcClient;

pub const MINT_SIGNATURE: &str = "mint((bytes32,bytes32[]),uint256,address,bytes)";
const INVITES_SIGNATURE: &str = "invites(bytes32)";
const COMPUTE_PRICE_SIGNATURE: &str = "computePrice(bytes32,uint256,bool)";
const LIST_SUPPLY_SIGNATURE: &str = "listSupply(bytes32)";
const INVITED_TOPIC: &str = "0xe9a0c17645ed78ccc9996259f00297ffc75e6b9d22cd605ccc9992cc8ca3f4c1";
const NEVER_ENDS_U32: u64 = u32::MAX as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteState {
    pub price: U256,
    pub reserve_price: U256,
    pub delta: U256,
    pub start: u64,
    pub end: u64,
    pub limit: U256,
    pub max_supply: U256,
    pub interval: U256,
    pub unit_size: u64,
    pub token_address: Address,
    pub is_blacklist: bool,
    raw: Vec<u8>,
}

impl InviteState {
    fn exists(&self) -> bool {
        !self.limit.is_zero()
            || !self.max_supply.is_zero()
            || !self.price.is_zero()
            || self.start != 0
            || self.end != 0
    }

    fn expired_at(&self, now: i64) -> bool {
        self.end != 0 && self.end != NEVER_ENDS_U32 && self.end <= now.max(0) as u64
    }

    fn open_at(&self, now: i64) -> bool {
        let now = now.max(0) as u64;
        self.limit > U256::ZERO
            && (self.start == 0 || self.start <= now)
            && (self.end == 0 || self.end == NEVER_ENDS_U32 || now < self.end)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypePhaseRow {
    pub key: String,
    pub label: String,
    pub price_wei: String,
    pub price_eth: String,
    /// Exact total native value for the requested quantity.
    pub value_wei: String,
    pub value_eth: String,
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
    pub limit: String,
    pub max_supply: String,
    pub list_supply: String,
    pub unit_size: u64,
    pub open: bool,
    pub expired: bool,
    pub requires_proof: bool,
    pub erc20_payment: bool,
    pub dynamic_price: bool,
    pub selectable: bool,
    pub disabled_reason: Option<String>,
    /// Binds phase state + quantity + computed msg.value.
    pub terms_hash: String,
}

#[derive(Debug, Clone)]
pub struct ArchetypeInspection {
    pub implementation: Option<Address>,
    pub phases: Vec<ArchetypePhaseRow>,
    pub recommended_index: Option<usize>,
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn word(data: &[u8], index: usize) -> Result<&[u8]> {
    let start = index.saturating_mul(32);
    let end = start.saturating_add(32);
    data.get(start..end)
        .with_context(|| format!("Archetype response missing ABI word {index}"))
}

fn word_u256(data: &[u8], index: usize) -> Result<U256> {
    Ok(U256::from_be_slice(word(data, index)?))
}

fn word_u64(data: &[u8], index: usize, label: &str) -> Result<u64> {
    u64::try_from(word_u256(data, index)?)
        .with_context(|| format!("Archetype {label} does not fit u64"))
}

fn decode_invite(data: impl AsRef<[u8]>) -> Result<InviteState> {
    let data = data.as_ref();
    if data.len() < 11 * 32 {
        bail!(
            "Archetype invites(bytes32) returned {} bytes, expected at least 352",
            data.len()
        );
    }
    let token_word = word(data, 9)?;
    let mut token = [0u8; 20];
    token.copy_from_slice(&token_word[12..]);
    Ok(InviteState {
        price: word_u256(data, 0)?,
        reserve_price: word_u256(data, 1)?,
        delta: word_u256(data, 2)?,
        start: word_u64(data, 3, "start")?,
        end: word_u64(data, 4, "end")?,
        limit: word_u256(data, 5)?,
        max_supply: word_u256(data, 6)?,
        interval: word_u256(data, 7)?,
        unit_size: word_u64(data, 8, "unitSize")?.max(1),
        token_address: Address::from(token),
        is_blacklist: !word_u256(data, 10)?.is_zero(),
        raw: data.to_vec(),
    })
}

async fn read_invite(rpc: &RpcClient, contract: &Address, key: &B256) -> Result<InviteState> {
    let calldata = build_calldata(INVITES_SIGNATURE, &[format!("{key:?}")])?;
    let data = rpc
        .eth_call(&Address::ZERO, contract, &calldata)
        .await
        .context("Archetype invites(bytes32)")?;
    decode_invite(data)
}

async fn compute_price(
    rpc: &RpcClient,
    contract: &Address,
    key: &B256,
    effective_quantity: u64,
) -> Result<U256> {
    let calldata = build_calldata(
        COMPUTE_PRICE_SIGNATURE,
        &[
            format!("{key:?}"),
            effective_quantity.max(1).to_string(),
            "false".into(),
        ],
    )?;
    let data = rpc
        .eth_call(&Address::ZERO, contract, &calldata)
        .await
        .context("Archetype computePrice(bytes32,uint256,bool)")?;
    word_u256(&data, 0)
}

async fn list_supply(rpc: &RpcClient, contract: &Address, key: &B256) -> U256 {
    let Ok(calldata) = build_calldata(LIST_SUPPLY_SIGNATURE, &[format!("{key:?}")]) else {
        return U256::ZERO;
    };
    match rpc.eth_call(&Address::ZERO, contract, &calldata).await {
        Ok(data) => word_u256(&data, 0).unwrap_or(U256::ZERO),
        Err(_) => U256::ZERO,
    }
}

fn public_key(key: &B256) -> bool {
    key.as_slice()[..31].iter().all(|b| *b == 0)
}

fn key_number(key: &B256) -> u8 {
    key.as_slice()[31]
}

fn phase_label(key: &B256) -> String {
    if public_key(key) {
        format!("PUBLIC #{}", key_number(key))
    } else {
        format!(
            "ALLOWLIST {}…{}",
            &format!("{key:?}")[..10],
            &format!("{key:?}")[60..]
        )
    }
}

fn terms_hash(key: &B256, invite: &InviteState, quantity: u64, value: U256) -> String {
    let mut data = Vec::with_capacity(32 + invite.raw.len() + 8 + 32);
    data.extend_from_slice(key.as_slice());
    data.extend_from_slice(&invite.raw);
    data.extend_from_slice(&quantity.to_be_bytes());
    let mut value_word = [0u8; 32];
    value.to_be_bytes::<32>().clone_into(&mut value_word);
    data.extend_from_slice(&value_word);
    format!("{:?}", keccak256(data))
}

async fn explorer_phase_keys(chain: &str, contract: &Address) -> Vec<B256> {
    let Some(base) = explorer_api_for_chain(Some(chain)) else {
        return vec![];
    };
    let url = format!(
        "{}/api?module=logs&action=getLogs&fromBlock=0&toBlock=latest&address={contract:?}&topic0={INVITED_TOPIC}",
        base.trim_end_matches('/')
    );
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
    {
        Ok(client) => client,
        Err(_) => return vec![],
    };
    let body: serde_json::Value = match client.get(url).send().await {
        Ok(response) if response.status().is_success() => match response.json().await {
            Ok(body) => body,
            Err(_) => return vec![],
        },
        _ => return vec![],
    };
    body.get("result")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|event| event.get("topics")?.as_array()?.get(1)?.as_str())
        .filter_map(|key| key.parse::<B256>().ok())
        .collect()
}

pub async fn detect(rpc: &RpcClient, contract: &Address) -> Result<Option<Option<Address>>> {
    let bytecode = rpc
        .get_code(contract)
        .await
        .context("failed to get bytecode")?;
    if bytecode.is_empty() {
        return Ok(None);
    }
    let implementation = resolve_implementation(rpc, contract, &bytecode).await?;
    let code = if let Some((implementation, _)) = implementation {
        rpc.get_code(&implementation).await?.to_vec()
    } else {
        bytecode.to_vec()
    };
    let selectors = extract_selectors(&code);
    let required = [
        function_selector(INVITES_SIGNATURE),
        function_selector(COMPUTE_PRICE_SIGNATURE),
        function_selector(MINT_SIGNATURE),
    ];
    if required.iter().all(|required| selectors.contains(required)) {
        Ok(Some(implementation.map(|(address, _)| address)))
    } else {
        Ok(None)
    }
}

pub async fn inspect(
    rpc: &RpcClient,
    chain: &str,
    contract: &Address,
    quantity: u64,
) -> Result<Option<ArchetypeInspection>> {
    let Some(implementation) = detect(rpc, contract).await? else {
        return Ok(None);
    };
    let quantity = quantity.max(1);
    let mut keys = BTreeSet::new();
    keys.insert(B256::ZERO);
    keys.insert(B256::from(U256::from(1u64).to_be_bytes::<32>()));
    keys.extend(explorer_phase_keys(chain, contract).await);

    let now = now_unix();
    let mut phases = Vec::new();
    for key in keys {
        let invite = match read_invite(rpc, contract, &key).await {
            Ok(invite) if invite.exists() => invite,
            _ => continue,
        };
        let effective_quantity = quantity.saturating_mul(invite.unit_size.max(1));
        let value = compute_price(rpc, contract, &key, effective_quantity).await?;
        let supply = list_supply(rpc, contract, &key).await;
        let requires_proof = !public_key(&key);
        let erc20_payment = invite.token_address != Address::ZERO;
        let dynamic_price = !invite.delta.is_zero() || !invite.interval.is_zero();
        let expired = invite.expired_at(now);
        let selectable = !requires_proof
            && !erc20_payment
            && !dynamic_price
            && !invite.limit.is_zero()
            && !expired;
        let disabled_reason = if requires_proof {
            Some("individual Merkle proof required".into())
        } else if erc20_payment {
            Some("ERC-20 payment is not supported by safe auto mode".into())
        } else if dynamic_price {
            Some("dynamic/Dutch price requires an explicit price policy".into())
        } else if invite.limit.is_zero() {
            Some("minting paused".into())
        } else if expired {
            Some("phase ended".into())
        } else {
            None
        };
        phases.push(ArchetypePhaseRow {
            key: format!("{key:?}"),
            label: phase_label(&key),
            price_wei: invite.price.to_string(),
            price_eth: amount::wei_to_eth_string(invite.price),
            value_wei: value.to_string(),
            value_eth: amount::wei_to_eth_string(value),
            start_time: (invite.start > 0).then_some(invite.start as i64),
            end_time: (invite.end > 0 && invite.end != NEVER_ENDS_U32).then_some(invite.end as i64),
            limit: invite.limit.to_string(),
            max_supply: invite.max_supply.to_string(),
            list_supply: supply.to_string(),
            unit_size: invite.unit_size,
            open: invite.open_at(now),
            expired,
            requires_proof,
            erc20_payment,
            dynamic_price,
            selectable,
            disabled_reason,
            terms_hash: terms_hash(&key, &invite, quantity, value),
        });
    }

    phases.sort_by_key(|phase| {
        (
            !phase.open,
            phase.start_time.unwrap_or(i64::MIN),
            phase.key.clone(),
        )
    });
    let recommended_index = phases
        .iter()
        .position(|phase| phase.selectable && phase.open)
        .or_else(|| {
            phases
                .iter()
                .position(|phase| phase.selectable && !phase.expired)
        });
    Ok(Some(ArchetypeInspection {
        implementation,
        phases,
        recommended_index,
    }))
}

pub fn mint_params(key: &str, quantity: u64) -> Result<Vec<String>> {
    let key = key.parse::<B256>().context("invalid Archetype phase key")?;
    if !public_key(&key) {
        bail!("Archetype allowlist phase requires a wallet-specific Merkle proof");
    }
    Ok(vec![
        format!("({key:?},[])"),
        quantity.max(1).to_string(),
        format!("{:#x}", Address::ZERO),
        "0x".into(),
    ])
}

pub async fn validate_public_terms(
    rpc: &RpcClient,
    contract: &Address,
    key: &str,
    quantity: u64,
    expected_terms_hash: &str,
    fire_at: Option<i64>,
) -> Result<U256> {
    let key = key.parse::<B256>().context("invalid Archetype phase key")?;
    if !public_key(&key) {
        bail!("Archetype phase requires a wallet-specific Merkle proof");
    }
    let invite = read_invite(rpc, contract, &key).await?;
    if invite.token_address != Address::ZERO {
        bail!("Archetype phase uses ERC-20 payment; native-value auto mode is blocked");
    }
    if !invite.delta.is_zero() || !invite.interval.is_zero() {
        bail!("Archetype dynamic/Dutch price is blocked without an explicit price policy");
    }
    if invite.limit.is_zero() {
        bail!("Archetype phase is paused");
    }
    let check_at = fire_at.unwrap_or_else(now_unix);
    if invite.start > 0 && check_at < invite.start as i64 {
        bail!(
            "fire time {check_at} is before Archetype phase start {}",
            invite.start
        );
    }
    if invite.end > 0 && invite.end != NEVER_ENDS_U32 && check_at >= invite.end as i64 {
        bail!("Archetype phase is already ended at {}", invite.end);
    }
    let effective_quantity = quantity.max(1).saturating_mul(invite.unit_size.max(1));
    let value = compute_price(rpc, contract, &key, effective_quantity).await?;
    let actual_hash = terms_hash(&key, &invite, quantity.max(1), value);
    if !actual_hash.eq_ignore_ascii_case(expected_terms_hash.trim()) {
        bail!(
            "Archetype phase terms changed after selection (price/time/limits); reload contract phases and review before minting"
        );
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_keys_are_only_the_reserved_byte_range() {
        assert!(public_key(&B256::ZERO));
        assert!(public_key(&B256::from(
            U256::from(255u64).to_be_bytes::<32>()
        )));
        assert!(!public_key(&B256::from(
            U256::from(256u64).to_be_bytes::<32>()
        )));
        assert!(!public_key(&keccak256(b"allowlist")));
    }

    #[test]
    fn archetype_public_calldata_matches_verified_selector() {
        let params = mint_params(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            2,
        )
        .unwrap();
        let calldata = build_calldata(MINT_SIGNATURE, &params).unwrap();
        assert_eq!(&calldata[..4], &[0x4a, 0x21, 0xa2, 0xdf]);
    }
}
