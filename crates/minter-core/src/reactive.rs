
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Engine that listens to blockchain events (newHeads) to trigger mints
/// reactively based on actual block time.
pub struct ReactiveEngine {
    latest_block_timestamp: Arc<AtomicU64>,
}

impl ReactiveEngine {
    pub fn new(ws_clients: &[crate::ws::WsClient]) -> Self {
        let latest_block_timestamp = Arc::new(AtomicU64::new(0));

        for (i, ws) in ws_clients.iter().enumerate() {
            let ws_clone = ws.clone();
            let ts_ref = latest_block_timestamp.clone();
            
            tokio::spawn(async move {
                // Subscribe to newHeads
                if let Ok(_sub_id) = ws_clone.eth_subscribe("newHeads", None).await {
                    crate::rlog!("ReactiveEngine [Node {}]: Subscribed to newHeads for precise timing.", i + 1);
                    let mut rx = ws_clone.subscribe();
                    while let Ok(val) = rx.recv().await {
                        if let Some(result) = val.get("result") {
                            if let Some(timestamp_hex) = result.get("timestamp").and_then(|t| t.as_str()) {
                                let ts_clean = timestamp_hex.trim_start_matches("0x");
                                if let Ok(block_ts) = u64::from_str_radix(ts_clean, 16) {
                                    // Update if higher (race condition safe)
                                    let current = ts_ref.load(Ordering::Relaxed);
                                    if block_ts > current {
                                        ts_ref.store(block_ts, Ordering::SeqCst);
                                    }
                                }
                            }
                        }
                    }
                } else {
                    crate::rlog!("ReactiveEngine [Node {}]: Failed to subscribe to newHeads", i + 1);
                }
            });
        }

        Self {
            latest_block_timestamp,
        }
    }

    /// Returns the latest block timestamp observed from the WebSocket subscription.
    /// Returns 0 if no WebSocket is available or no block has been observed yet.
    pub fn latest_block_timestamp(&self) -> u64 {
        self.latest_block_timestamp.load(Ordering::SeqCst)
    }

    /// Spawns a background task to listen for pending transactions to the target contract.
    /// Useful for mempool sniping (back-running a state-changing transaction like unpause).
    pub fn listen_mempool(
        &self,
        ws_clients: &[crate::ws::WsClient],
        target_contract: alloy_primitives::Address,
        trigger_flag: Arc<std::sync::atomic::AtomicBool>,
    ) {
        for (i, ws) in ws_clients.iter().enumerate() {
            let ws_clone = ws.clone();
            let trigger = trigger_flag.clone();
            tokio::spawn(async move {
                // Subscribe to full pending transactions
                if let Ok(_sub_id) = ws_clone.eth_subscribe("newPendingTransactions", None).await {
                    crate::rlog!("ReactiveEngine [Node {}]: Subscribed to mempool for contract {:?}", i + 1, target_contract);
                    let mut rx = ws_clone.subscribe();
                    let target_hex = format!("{:?}", target_contract).to_lowercase();
                    
                    while let Ok(val) = rx.recv().await {
                        if let Some(result) = val.get("result") {
                            if let Some(to_addr) = result.get("to").and_then(|t| t.as_str()) {
                                if to_addr.to_lowercase() == target_hex {
                                    crate::rlog!("REACTIVE MEMPOOL TRIGGER [Node {}]: Found tx to target contract!", i + 1);
                                    trigger.store(true, Ordering::SeqCst);
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
    }
}
