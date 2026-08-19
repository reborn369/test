use minter_core::ws::WsClient;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let url = std::env::var("MINTER_WSS_PROBE_URL").expect("MINTER_WSS_PROBE_URL is required");
    let client = WsClient::spawn(url);
    let mut status = client.subscribe_status();
    let probe = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = client.status();
            if current.connected {
                match client
                    .eth_subscribe_timeout("newHeads", None, Duration::from_secs(4))
                    .await
                {
                    Ok(_) => {
                        println!("connected=true generation={}", current.generation);
                        println!("subscription=ok");
                    }
                    Err(error) => {
                        println!("subscription=error: {error:#}");
                        std::process::exit(2);
                    }
                }
                return;
            }
            tokio::select! {
                changed = status.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
        }
    })
    .await;
    if probe.is_err() {
        println!(
            "connected=false error={}",
            client
                .last_error()
                .unwrap_or_else(|| "connection timeout".to_string())
        );
        std::process::exit(1);
    }
}
