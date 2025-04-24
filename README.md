# Rust Tools

Collection of Rust command-line utilities for Solana and cryptography workflows.

## Building

### Build all tools

From the root directory:

```bash
cargo build --release
```

### Build a specific tool

```bash
cargo build --release -p ssh-to-solana
cargo build --release -p yubikey_ed25519_crpyto
```

## Running

After building, you can run the tools directly from the target directory:

```bash
# Run ssh-to-solana
./target/release/ssh-to-solana "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E openpgp:0x98DF58F5"

# Run yubikey_ed25519_crpyto CLI example
cargo run --example main -p yubikey_ed25519_crpyto
```

Or use cargo to run them:

```bash
cargo run --release -p ssh-to-solana -- "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E openpgp:0x98DF58F5"
cargo run --release -p yubikey_ed25519_crpyto --example main
```

## Available Tools

- **ssh-to-solana**: Convert GPG-exported ED25519 SSH keys to Solana addresses.
- **yubikey_ed25519_crpyto**: Library and CLI for constructing and signing Solana transactions using a YubiKey's OpenPGP card (Ed25519).

## Prerequisites

- For YubiKey tools: YubiKey with OpenPGP (Ed25519) configured, PC/SC middleware (`pcscd` running).
- Linux: `sudo apt-get install pcscd opensc gnupg scdaemon`

## License

MIT OR Apache-2.0
