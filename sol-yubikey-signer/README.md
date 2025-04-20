# Solana YubiKey Signer

A CLI tool that constructs Solana transfer transactions and signs them with a YubiKey's smart card functions.

## Prerequisites

- A YubiKey with OpenPGP or PIV/PKCS#15 configured
- For OpenPGP method: GPG installed and configured
- For PKCS#15 method: OpenSC tools installed
- PC/SC middleware (pcscd service running)

## Installation

```bash
# For OpenPGP method:
sudo apt-get install gnupg pcscd scdaemon

# For PKCS#15 method:
sudo apt-get install opensc pcscd pcsc-tools

# Install the tool
cargo install --path .
```

## Usage

```bash
sol-yubikey-signer --to <RECIPIENT_ADDRESS> --amount <SOL_AMOUNT> [OPTIONS]
```

### Basic Examples

Using OpenPGP method (default):
```bash
sol-yubikey-signer --to 9zkU8suQBdhZVax2DSGNAnyEhEzfEELSBm6bYzuW55LX --amount 0.1
```

Using PKCS#15 method:
```bash
sol-yubikey-signer --to 9zkU8suQBdhZVax2DSGNAnyEhEzfEELSBm6bYzuW55LX --amount 0.1 --signing-method pkcs15-crypt
```

Using devnet:
```bash
sol-yubikey-signer --to devwuNsNYACyiEYxRNqMNseBpNnGfnd4ZwNHL7sphqv --amount 0.1 --network devnet
```

### All Options

- `--to`, `-t`: Recipient Solana address
- `--amount`, `-a`: Amount of SOL to send
- `--from`, `-f`: Your Solana address (optional, will use ssh-to-solana if not provided)
- `--network`, `-n`: Network to use (devnet, testnet, mainnet-beta)
- `--url`: Custom RPC URL (overrides network selection)
- `--signing-method`: Method to use for signing (openpgp, pkcs15-crypt, opensc-tool)
- `--dump-tx`: Dump the unsigned and signed transactions to files
- `--verify`: Verify the transaction signature before sending
- `--skip-confirm`: Skip transaction confirmation check

## Signing Methods

1. **openpgp** (default): Uses GPG with the OpenPGP applet on YubiKey
2. **pkcs15-crypt**: Uses pkcs15-crypt from OpenSC
3. **opensc-tool**: Uses opensc-tool for direct APDU commands

## Troubleshooting

### OpenPGP Method
- Ensure gpg is properly configured with your YubiKey
- Test GPG configuration: `gpg --card-status`
- Make sure the scdaemon is running

### PKCS#15 Method
- Ensure pcscd service is running: `sudo systemctl status pcscd`
- Check if your YubiKey is detected: `pcsc_scan`
- Test smart card operations: `pkcs15-tool --list-keys`

## License

MIT
