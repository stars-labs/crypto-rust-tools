# webrtc-signal-server

A general WebRTC signal server for peer-to-peer communication, written in Rust and powered by async networking and WebSockets.

## Features

- Simple WebSocket-based signaling for WebRTC peers
- Peer registration and discovery
- Message relay between peers
- Asynchronous, scalable, and easy to deploy

## Usage

Add to your workspace or build as a standalone binary:

```sh
cargo build --release
```

Run the server (default port: 9000):

```sh
cargo run --release
```

The server listens for WebSocket connections on `0.0.0.0:9000`.

## Protocol

Clients communicate with the server using JSON messages:

### Register

```json
{ "type": "register", "peer_id": "your-unique-id" }a
```

### List Peers

```json
{ "type": "list_peers" }
```

### Relay Message

```json
{ "type": "relay", "to": "target-peer-id", "data": { ... } }
```

### Server Responses

- List of peers:
  ```json
  { "type": "peers", "peers": ["peer1", "peer2"] }
  ```
- Relayed message:
  ```json
  { "type": "relay", "from": "peer1", "data": { ... } }
  ```
- Error:
  ```json
  { "type": "error", "error": "description" }
  ```

## License

MIT OR Apache-2.0

## Repository

[https://github.com/stars-labs/cypto-rust-tools](https://github.com/stars-labs/cypto-rust-tools)
