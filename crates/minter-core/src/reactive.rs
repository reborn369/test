use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::broadcast;

const SUBSCRIBE_TIMEOUT: Duration = Duration::from_secs(3);
const SUBSCRIBE_RETRY: Duration = Duration::from_secs(1);

/// Tracks healthy `newHeads` subscriptions on all configured WebSocket nodes.
///
/// A subscription is recreated after every reconnect. Block timestamps are kept
/// for diagnostics only: a remote producer timestamp is not a safe indication
/// that a sale is open, so it never advances the wall-clock fire decision.
pub struct ReactiveEngine {
    latest_block_timestamp: Arc<AtomicU64>,
    active_subscriptions: Arc<AtomicUsize>,
}

impl ReactiveEngine {
    pub fn new(ws_clients: &[crate::ws::WsClient]) -> Self {
        let latest_block_timestamp = Arc::new(AtomicU64::new(0));
        let active_subscriptions = Arc::new(AtomicUsize::new(0));

        for (index, ws) in ws_clients.iter().cloned().enumerate() {
            let timestamp = Arc::clone(&latest_block_timestamp);
            let active = Arc::clone(&active_subscriptions);
            tokio::spawn(async move {
                monitor_new_heads(index + 1, ws, timestamp, active).await;
            });
        }

        Self {
            latest_block_timestamp,
            active_subscriptions,
        }
    }

    pub fn latest_block_timestamp(&self) -> u64 {
        self.latest_block_timestamp.load(Ordering::Acquire)
    }

    pub fn active_subscriptions(&self) -> usize {
        self.active_subscriptions.load(Ordering::Acquire)
    }
}

async fn monitor_new_heads(
    node: usize,
    ws: crate::ws::WsClient,
    latest_timestamp: Arc<AtomicU64>,
    active_subscriptions: Arc<AtomicUsize>,
) {
    let mut status = ws.subscribe_status();
    loop {
        while !status.borrow().connected {
            if status.changed().await.is_err() {
                return;
            }
        }

        let generation = status.borrow().generation;
        let mut events = ws.subscribe();
        let subscription_id = match ws
            .eth_subscribe_timeout("newHeads", None, SUBSCRIBE_TIMEOUT)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                crate::rlog!("ReactiveEngine [Node {node}]: subscribe failed: {error}");
                tokio::select! {
                    _ = tokio::time::sleep(SUBSCRIBE_RETRY) => {},
                    changed = status.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
                continue;
            }
        };

        active_subscriptions.fetch_add(1, Ordering::AcqRel);
        crate::rlog!("ReactiveEngine [Node {node}]: newHeads active");

        loop {
            tokio::select! {
                changed = status.changed() => {
                    if changed.is_err() {
                        active_subscriptions.fetch_sub(1, Ordering::AcqRel);
                        return;
                    }
                    let current = *status.borrow();
                    if !current.connected || current.generation != generation {
                        break;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(params) => update_timestamp(
                            &params,
                            &subscription_id,
                            latest_timestamp.as_ref(),
                        ),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => {
                            active_subscriptions.fetch_sub(1, Ordering::AcqRel);
                            return;
                        }
                    }
                }
            }
        }

        active_subscriptions.fetch_sub(1, Ordering::AcqRel);
        crate::rlog!("ReactiveEngine [Node {node}]: reconnecting subscription");
    }
}

fn update_timestamp(params: &Value, subscription_id: &str, latest: &AtomicU64) {
    if params.get("subscription").and_then(Value::as_str) != Some(subscription_id) {
        return;
    }
    let Some(timestamp_hex) = params
        .get("result")
        .and_then(|result| result.get("timestamp"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let Ok(block_timestamp) = u64::from_str_radix(timestamp_hex.trim_start_matches("0x"), 16)
    else {
        return;
    };
    latest.fetch_max(block_timestamp, Ordering::AcqRel);
}

#[cfg(test)]
mod tests {
    use super::update_timestamp;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn only_matching_subscription_updates_timestamp() {
        let latest = AtomicU64::new(0);
        update_timestamp(
            &json!({"subscription":"other","result":{"timestamp":"0x64"}}),
            "wanted",
            &latest,
        );
        assert_eq!(latest.load(Ordering::Relaxed), 0);
        update_timestamp(
            &json!({"subscription":"wanted","result":{"timestamp":"0x64"}}),
            "wanted",
            &latest,
        );
        assert_eq!(latest.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn timestamp_never_moves_backwards() {
        let latest = AtomicU64::new(100);
        update_timestamp(
            &json!({"subscription":"wanted","result":{"timestamp":"0x50"}}),
            "wanted",
            &latest,
        );
        assert_eq!(latest.load(Ordering::Relaxed), 100);
    }
}
