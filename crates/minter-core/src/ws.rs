use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

/// Observable state of a WebSocket connection.
///
/// `generation` changes after every successful connection. Consumers use it to
/// detect reconnects and recreate subscriptions, which are scoped to one WS
/// session on Ethereum JSON-RPC servers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WsStatus {
    pub connected: bool,
    pub generation: u64,
}

/// JSON-RPC WebSocket client with bounded requests and reconnect support.
#[derive(Clone)]
pub struct WsClient {
    req_tx: mpsc::Sender<WsRequest>,
    sub_tx: broadcast::Sender<Value>,
    status_rx: watch::Receiver<WsStatus>,
    last_error: Arc<RwLock<Option<String>>>,
}

enum WsRequest {
    Call {
        method: String,
        params: Value,
        deadline: Instant,
        reply: oneshot::Sender<Result<Value>>,
    },
}

impl WsClient {
    /// Start a WebSocket connection loop in the background.
    pub fn spawn(url: String) -> Self {
        let (req_tx, req_rx) = mpsc::channel(1024);
        let (sub_tx, _) = broadcast::channel(1024);
        let (status_tx, status_rx) = watch::channel(WsStatus::default());
        let last_error = Arc::new(RwLock::new(None));
        let client = Self {
            req_tx,
            sub_tx: sub_tx.clone(),
            status_rx,
            last_error: Arc::clone(&last_error),
        };

        tokio::spawn(async move {
            Self::connection_loop(url, req_rx, sub_tx, status_tx, last_error).await;
        });
        client
    }

    pub fn is_connected(&self) -> bool {
        self.status_rx.borrow().connected
    }

    pub fn status(&self) -> WsStatus {
        *self.status_rx.borrow()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<WsStatus> {
        self.status_rx.clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.read().ok().and_then(|value| value.clone())
    }

    fn record_error(&self, message: impl Into<String>) {
        if let Ok(mut value) = self.last_error.write() {
            *value = Some(message.into());
        }
    }

    /// Receive Ethereum subscription events. Values are JSON-RPC `params`
    /// objects and contain both `subscription` and `result`.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.sub_tx.subscribe()
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, DEFAULT_CALL_TIMEOUT)
            .await
    }

    /// Issue a call that cannot survive past `timeout` or be replayed after a
    /// later reconnect.
    pub async fn call_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        if !self.is_connected() {
            bail!("WS is not connected");
        }

        let deadline = Instant::now() + timeout;
        let (reply, rx) = oneshot::channel();
        let request = WsRequest::Call {
            method: method.to_string(),
            params,
            deadline,
            reply,
        };
        tokio::time::timeout(timeout, self.req_tx.send(request))
            .await
            .context("WS request queue timeout")?
            .context("WS client loop dead")?;

        let result =
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => {
                    self.record_error(format!("WS request dropped: {error}"));
                    bail!("WS request dropped: {error}");
                }
                Err(error) => {
                    self.record_error("WS response timeout");
                    return Err(error).context("WS response timeout");
                }
            };
        if let Err(error) = &result {
            self.record_error(error.to_string());
        }
        result
    }

    pub async fn eth_subscribe_timeout(
        &self,
        sub_type: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<String> {
        let params = match params {
            Some(value) => json!([sub_type, value]),
            None => json!([sub_type]),
        };
        let result = self.call_timeout("eth_subscribe", params, timeout).await?;
        result
            .as_str()
            .map(str::to_owned)
            .context("eth_subscribe returned non-string")
    }

    async fn connection_loop(
        url: String,
        mut req_rx: mpsc::Receiver<WsRequest>,
        sub_tx: broadcast::Sender<Value>,
        status_tx: watch::Sender<WsStatus>,
        last_error: Arc<RwLock<Option<String>>>,
    ) {
        let mut generation = 0u64;
        loop {
            crate::rlog!(
                "WS connecting to {}",
                crate::rpc::RpcClient::short_url(&url)
            );
            let connected = tokio::time::timeout(CONNECT_TIMEOUT, connect_async(&url)).await;
            match connected {
                Ok(Ok((stream, _))) => {
                    if let Ok(mut error) = last_error.write() {
                        *error = None;
                    }
                    generation = generation.wrapping_add(1).max(1);
                    status_tx.send_replace(WsStatus {
                        connected: true,
                        generation,
                    });
                    crate::rlog!("WS connected: {}", crate::rpc::RpcClient::short_url(&url));
                    let (mut write, mut read) = stream.split();
                    let mut pending =
                        std::collections::HashMap::<u64, oneshot::Sender<Result<Value>>>::new();
                    let mut next_id = 1u64;
                    let ping_start = tokio::time::Instant::now() + Duration::from_secs(30);
                    let mut ping_interval =
                        tokio::time::interval_at(ping_start, Duration::from_secs(30));

                    loop {
                        tokio::select! {
                            request = req_rx.recv() => {
                                let Some(WsRequest::Call { method, params, deadline, reply }) = request else {
                                    return;
                                };
                                if reply.is_closed() || Instant::now() >= deadline {
                                    let _ = reply.send(Err(anyhow::anyhow!("WS request expired")));
                                    continue;
                                }
                                let id = next_id;
                                next_id = next_id.wrapping_add(1).max(1);
                                let payload = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "method": method,
                                    "params": params,
                                });
                                if let Err(error) = write.send(Message::Text(payload.to_string().into())).await {
                                    store_error(&last_error, sanitize_error(&url, &error.to_string()));
                                    let _ = reply.send(Err(anyhow::anyhow!("WS write error: {error}")));
                                    break;
                                }
                                pending.insert(id, reply);
                            }
                            _ = ping_interval.tick() => {
                                if write.send(Message::Ping(Vec::new().into())).await.is_err() {
                                    break;
                                }
                            }
                            message = read.next() => {
                                match message {
                                    Some(Ok(Message::Text(text))) => {
                                        let Ok(value) = serde_json::from_str::<Value>(&text) else {
                                            continue;
                                        };
                                        if value.get("method").and_then(Value::as_str)
                                            == Some("eth_subscription")
                                        {
                                            if let Some(params) = value.get("params") {
                                                let _ = sub_tx.send(params.clone());
                                            }
                                            continue;
                                        }
                                        if let Some(id) = value.get("id").and_then(Value::as_u64)
                                            && let Some(reply) = pending.remove(&id)
                                        {
                                            if let Some(error) = value.get("error") {
                                                store_error(&last_error, format!("RPC error: {error}"));
                                                let _ = reply.send(Err(anyhow::anyhow!("RPC error: {error}")));
                                            } else if let Some(result) = value.get("result") {
                                                let _ = reply.send(Ok(result.clone()));
                                            } else {
                                                let _ = reply.send(Err(anyhow::anyhow!("No result or error")));
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => break,
                                    Some(Err(error)) => {
                                        store_error(&last_error, sanitize_error(&url, &error.to_string()));
                                        crate::rlog!("WS read error: {error}");
                                        break;
                                    }
                                    None => break,
                                    _ => {}
                                }
                            }
                        }
                    }

                    status_tx.send_replace(WsStatus {
                        connected: false,
                        generation,
                    });
                    for (_, reply) in pending.drain() {
                        let _ = reply.send(Err(anyhow::anyhow!("WS connection lost")));
                    }
                }
                Ok(Err(error)) => {
                    store_error(&last_error, sanitize_error(&url, &error.to_string()));
                    crate::rlog!("WS connect failed: {error}");
                }
                Err(_) => {
                    store_error(&last_error, "connection timeout".to_string());
                    crate::rlog!("WS connect timeout");
                }
            }

            status_tx.send_replace(WsStatus {
                connected: false,
                generation,
            });
            tokio::time::sleep(RECONNECT_DELAY).await;
        }
    }
}

fn store_error(target: &RwLock<Option<String>>, message: String) {
    if let Ok(mut value) = target.write() {
        *value = Some(message);
    }
}

fn sanitize_error(url: &str, message: &str) -> String {
    let short = crate::rpc::RpcClient::short_url(url);
    let mut safe = message.replace(url, &short);
    if let Some(scheme_end) = url.find("://") {
        let after_scheme = &url[scheme_end + 3..];
        if let Some(path_start) = after_scheme.find('/') {
            let path = &after_scheme[path_start + 1..];
            for segment in path.split('/').filter(|segment| !segment.is_empty()) {
                if segment.len() >= 8 {
                    safe = safe.replace(segment, "…");
                }
            }
        }
    }
    crate::safe_truncate(&safe, 240).to_string()
}

#[cfg(test)]
mod tests {
    use super::{WsClient, sanitize_error};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::time::Duration;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    async fn wait_connected(client: &WsClient) {
        let mut status = client.subscribe_status();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.borrow().connected {
                    return;
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .expect("WS client did not connect");
    }

    #[tokio::test]
    async fn acknowledged_call_returns_server_result() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut websocket = accept_async(stream).await.unwrap();
            let Message::Text(text) = websocket.next().await.unwrap().unwrap() else {
                panic!("expected text request");
            };
            let request: Value = serde_json::from_str(&text).unwrap();
            websocket
                .send(Message::Text(
                    json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": "0x1234"
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
        });

        let client = WsClient::spawn(format!("ws://{address}"));
        wait_connected(&client).await;
        let result = client
            .call_timeout("eth_test", json!([]), Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result, json!("0x1234"));
    }

    #[tokio::test]
    async fn disconnected_call_fails_without_entering_reconnect_queue() {
        let client = WsClient::spawn("ws://127.0.0.1:9".to_string());
        let started = std::time::Instant::now();
        let error = client
            .call_timeout("eth_test", json!([]), Duration::from_secs(1))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("not connected"), "{error}");
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn connection_errors_do_not_expose_rpc_credentials() {
        let url = "wss://example.invalid/v2/super-secret-api-key";
        let safe = sanitize_error(url, &format!("TLS failed for {url}: super-secret-api-key"));
        assert!(!safe.contains("super-secret-api-key"), "{safe}");
        assert!(safe.contains("example.invalid"), "{safe}");
    }
}
