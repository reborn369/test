use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

/// JSON-RPC WebSocket client with automatic reconnects and pub/sub support.
#[derive(Clone)]
pub struct WsClient {
    /// Send requests to the background connection loop
    req_tx: mpsc::Sender<WsRequest>,
    /// Broadcast channel for Ethereum subscription events (e.g. newHeads, logs)
    sub_tx: broadcast::Sender<Value>,
    next_id: Arc<AtomicU64>,
}

enum WsRequest {
    /// A standard JSON-RPC call awaiting a response
    Call {
        method: String,
        params: Value,
        reply: oneshot::Sender<Result<Value>>,
    },
    /// Fire-and-forget raw message (useful for raw transaction blast)
    SendRaw(String),
}

impl WsClient {
    /// Start a WebSocket connection loop in the background.
    /// URL should start with ws:// or wss://
    pub fn spawn(url: String) -> Self {
        let (req_tx, req_rx) = mpsc::channel(1024);
        let (sub_tx, _) = broadcast::channel(1024);
        let client = Self {
            req_tx,
            sub_tx: sub_tx.clone(),
            next_id: Arc::new(AtomicU64::new(1)),
        };

        let url_clone = url.clone();
        tokio::spawn(async move {
            Self::connection_loop(url_clone, req_rx, sub_tx).await;
        });

        client
    }

    /// Subscribe to background broadcast events.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.sub_tx.subscribe()
    }

    /// Issue a JSON-RPC method call over WebSocket.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let (reply, rx) = oneshot::channel();
        self.req_tx
            .send(WsRequest::Call {
                method: method.to_string(),
                params,
                reply,
            })
            .await
            .context("WS client loop dead")?;

        rx.await.context("WS request dropped")?
    }

    /// Send a pre-built raw JSON string (fire-and-forget).
    /// Used for low-latency transaction blasts at T0.
    pub fn send_raw_json(&self, json_payload: String) {
        let _ = self.req_tx.try_send(WsRequest::SendRaw(json_payload));
    }

    /// Set up an `eth_subscribe` subscription.
    pub async fn eth_subscribe(&self, sub_type: &str, params: Option<Value>) -> Result<String> {
        let p = if let Some(v) = params {
            json!([sub_type, v])
        } else {
            json!([sub_type])
        };
        let res = self.call("eth_subscribe", p).await?;
        res.as_str()
            .map(|s| s.to_string())
            .context("eth_subscribe returned non-string")
    }

    /// Background connection loop: handles reconnects, pings, and routing.
    async fn connection_loop(
        url: String,
        mut req_rx: mpsc::Receiver<WsRequest>,
        sub_tx: broadcast::Sender<Value>,
    ) {
        loop {
            crate::rlog!("WS connecting to {}", crate::rpc::RpcClient::short_url(&url));
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    crate::rlog!("WS connected!");
                    let (mut write, mut read) = ws_stream.split();
                    
                    // Pending requests awaiting an RPC ID match
                    let mut pending = std::collections::HashMap::<u64, oneshot::Sender<Result<Value>>>::new();
                    let mut next_req_id = 1u64;

                    // Ping interval to keep connection alive (prevent idle drop)
                    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));

                    loop {
                        tokio::select! {
                            // 1. Send outgoing requests
                            Some(req) = req_rx.recv() => {
                                match req {
                                    WsRequest::Call { method, params, reply } => {
                                        let id = next_req_id;
                                        next_req_id += 1;
                                        let payload = json!({
                                            "jsonrpc": "2.0",
                                            "id": id,
                                            "method": method,
                                            "params": params,
                                        });
                                        let msg = Message::Text(payload.to_string());
                                        if let Err(e) = write.send(msg).await {
                                            let _ = reply.send(Err(anyhow::anyhow!("WS write error: {e}")));
                                            break; // drop connection, reconnect
                                        }
                                        pending.insert(id, reply);
                                    }
                                    WsRequest::SendRaw(payload) => {
                                        if let Err(_) = write.send(Message::Text(payload)).await {
                                            break;
                                        }
                                    }
                                }
                            }
                            
                            // 2. Keep-alive Pings
                            _ = ping_interval.tick() => {
                                if let Err(_) = write.send(Message::Ping(vec![])).await {
                                    break;
                                }
                            }

                            // 3. Receive incoming messages
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(t))) => {
                                        if let Ok(val) = serde_json::from_str::<Value>(&t) {
                                            // Is it a subscription event?
                                            if val.get("method").and_then(|m| m.as_str()) == Some("eth_subscription") {
                                                if let Some(params) = val.get("params") {
                                                    let _ = sub_tx.send(params.clone());
                                                }
                                                continue;
                                            }

                                            // Is it an RPC response?
                                            if let Some(id_val) = val.get("id") {
                                                if let Some(id) = id_val.as_u64() {
                                                    if let Some(reply) = pending.remove(&id) {
                                                        if let Some(err) = val.get("error") {
                                                            let _ = reply.send(Err(anyhow::anyhow!("RPC error: {}", err)));
                                                        } else if let Some(res) = val.get("result") {
                                                            let _ = reply.send(Ok(res.clone()));
                                                        } else {
                                                            let _ = reply.send(Err(anyhow::anyhow!("No result or error")));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        crate::rlog!("WS closed by server");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        crate::rlog!("WS read error: {e}");
                                        break;
                                    }
                                    None => break,
                                    _ => {} // Ignore binary/pings/pongs
                                }
                            }
                        }
                    }
                    
                    // Cancel all pending requests on disconnect
                    for (_, reply) in pending.drain() {
                        let _ = reply.send(Err(anyhow::anyhow!("WS connection lost")));
                    }
                }
                Err(e) => {
                    crate::rlog!("WS connect failed: {e}");
                }
            }
            // Reconnect backoff
            sleep(Duration::from_secs(2)).await;
        }
    }
}
