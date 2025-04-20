# Rust Tools

Collection of Rust command-line utilities.

## Building

### Build all tools

From the root directory:

```bash
cargo build --release
```

### Build a specific tool

```bash
cargo build --release -p ssh-to-solana
cargo build --release -p sol-yubikey-signer
```

## Running

After building, you can run the tools directly from the target directory:

```bash
# Run ssh-to-solana
./target/release/ssh-to-solana "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E openpgp:0x98DF58F5"

# Run sol-yubikey-signer
./target/release/sol-yubikey-signer --to ADDRESS --amount 0.1 --from YOUR_ADDRESS --network devnet
```

Or use cargo to run them:

```bash
cargo run --release -p ssh-to-solana -- "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E openpgp:0x98DF58F5"
cargo run --release -p sol-yubikey-signer -- --to ADDRESS --amount 0.1 --network devnet --verify
```

## Available Tools

- **ssh-to-solana**: Convert GPG-exported ED25519 SSH keys to Solana addresses
- **sol-yubikey-signer**: Construct Solana transfer transactions and sign them with a YubiKey
