# SSH to Solana Address Converter

A simple CLI tool that converts GPG-exported ED25519 SSH keys to Solana addresses.

## Installation

```bash
cargo install --path .
```

## Usage

You can provide the SSH key directly as an argument:

```bash
ssh-to-solana "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICKoK0n5DHVMeDCw3XVZUYxhgdQIYJADIyaSF7J8Rz1E openpgp:0x98DF58F5"
```

Or you can pipe the output from GPG to the tool:

```bash
gpg --export-ssh-key 4EDE451FC10722D0C6662C6FB99B8189C7C153F6 | ssh-to-solana
```

## How It Works

1. The tool extracts the base64-encoded data from the SSH key
2. Decodes the data to get the binary representation
3. Extracts the 32-byte ED25519 public key
4. Base58-encodes the public key to produce a Solana address

## License

MIT
