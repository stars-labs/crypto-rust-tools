# FROST MPC CLI Node Documentation

This directory contains the documentation for the frost-mpc-cli-node crate, which implements threshold signatures using the FROST protocol for both Solana (Ed25519) and Ethereum (secp256k1) blockchains.

## Documentation Structure

### User Guides
- [CLI Usage Guide](user_guides/cli_usage.md) - How to use the CLI application
- [Keystore Overview](user_guides/keystore.md) - Introduction to the keystore functionality
- [Keystore User Manual](user_guides/keystore_user_manual.md) - Detailed guide for using the keystore features

### Architecture Documentation
- [Keystore Architecture](architecture/keystore_architecture.md) - Technical design of the keystore system

### Protocol Documentation
- [Signal Protocol](protocol/signal_protocol.md) - WebRTC signaling protocol details

## Key Features

The FROST MPC CLI node implements threshold signatures with distributed key generation. It uses:

1. WebRTC for peer-to-peer communication in a mesh network
2. Terminal UI for user interaction
3. Supports both Solana (Ed25519) and Ethereum (secp256k1) keys
4. Signaling server for WebRTC connection establishment

The protocol flow involves:
- Node registration and peer discovery
- Session negotiation and WebRTC mesh formation
- Distributed Key Generation (DKG)
- Threshold signing

## Build and Run

```bash
# Build the CLI node
cargo build --release -p frost-mpc-cli-node

# Run the CLI node
cargo run --release -p frost-mpc-cli-node -- --peer-id <your-id> --curve <secp256k1|ed25519>
```

For detailed usage instructions, please refer to the [CLI Usage Guide](user_guides/cli_usage.md).