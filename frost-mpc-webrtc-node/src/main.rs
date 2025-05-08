mod net;
use net::websocket::connect_to_signal_server;

#[tokio::main]
async fn main() {
    connect_to_signal_server().await;
    println!("Connection established!");
}
