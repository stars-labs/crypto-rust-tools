# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Structure

This repository is a collection of Rust command-line utilities for cryptographic operations, particularly focused on Solana and Ethereum blockchain operations. The repository is organized as a Rust workspace with multiple crates.

### Main Crates

1. **frost-mpc-cli-node**: Implementation of threshold signatures using the FROST protocol. Provides a CLI node for distributed key generation and signing operations via WebRTC mesh networking.
2. **ssh-to-solana**: Tool to convert GPG-exported ED25519 SSH keys to Solana addresses.
3. **yubikey-crpyto**: Library and CLI for constructing and signing Solana and Ethereum transactions using a YubiKey's OpenPGP card.
4. **webrtc-signal-server**: WebRTC signaling server implementation for peer-to-peer communication.
5. **webrtc-signal-server-cloudflare-worker**: WebRTC signaling server implemented as a Cloudflare Worker.

## Building and Running

### Build Commands

```bash
# Build all tools
cargo build --release

# Build specific tools
cargo build --release -p ssh-to-solana
cargo build --release -p yubikey-crpyto
cargo build --release -p frost-mpc-cli-node
cargo build --release -p webrtc-signal-server
cargo build --release -p webrtc-signal-server-cloudflare-worker
```

### Running Commands

```bash
# Run ssh-to-solana
cargo run --release -p ssh-to-solana -- "<ssh-key> <optional-comment>"

# Run yubikey-crypto examples
cargo run --release -p yubikey-crpyto --example ethereum
cargo run --release -p yubikey-crpyto --example solana

# Run FROST MPC CLI node
cargo run --release -p frost-mpc-cli-node

# Run WebRTC signaling server
cargo run --release -p webrtc-signal-server
```

## Key Components

### FROST MPC CLI Node

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

### YubiKey Crypto Library

This library provides integration with YubiKey's OpenPGP card implementation to:
- Sign Solana transactions using Ed25519
- Sign Ethereum transactions using secp256k1
- Access hardware secure element features

### WebRTC Signal Server

Handles WebRTC signaling for peer discovery and connection establishment:
- Peer registration
- Relaying offers, answers, and ICE candidates
- WebSocket-based communication
- Available as both a standalone server and Cloudflare Worker implementation

## Prerequisites

- For YubiKey tools: YubiKey with OpenPGP (Ed25519) configured, PC/SC middleware (`pcscd` running)
- Linux dependencies: `sudo apt-get install pcscd opensc gnupg scdaemon`

## Development Notes

1. The codebase heavily uses async/await patterns with Tokio
2. WebRTC is used for secure peer-to-peer communication
3. Error handling is done with anyhow/thiserror
4. The Terminal UI is built with ratatui and crossterm
5. The frost-mpc-cli-node was recently renamed from solnana-mpc-frost

When making changes to the WebRTC implementation or signaling protocol, make sure to refer to the detailed protocol documentation in the `frost-mpc-cli-node/docs/protocol/01_webrtc_signaling.md` file.