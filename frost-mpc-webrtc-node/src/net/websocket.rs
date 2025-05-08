use futures_util::{SinkExt, StreamExt};
use lazy_static::lazy_static;
use tokio::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc_signal_server::ClientMsg;

#[allow(dead_code)]
pub struct WebSocketConfig {
    pub url: String,
    pub max_retries: usize,
    pub retry_interval: Duration,
}

lazy_static! {
    pub static ref WEBSOCKET_CONFIG: WebSocketConfig = WebSocketConfig {
        url: "wss://auto-life.tech".to_string(),
        max_retries: 5,
        retry_interval: Duration::from_secs(5),
    };
}

pub async fn connect_to_signal_server() {
    // Connect to the signal server
    let (ws_stream, _) = connect_async(WEBSOCKET_CONFIG.url.clone())
        .await
        .expect("Failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // Send a message to the server
    let msg = ClientMsg::Register {
        peer_id: "my_peer_id".to_string(),
    };
    let msg_str = serde_json::to_string(&msg).unwrap();
    write.send(Message::Text(msg_str)).await.unwrap();

    // Listen for messages from the server
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                println!("Received: {}", text);
            }
            Ok(Message::Close(_)) => {
                println!("Connection closed");
                break;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
            _ => {}
        }
    }
}
