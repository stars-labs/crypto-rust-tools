# Solana YubiKey Signer (`yubikey_ed25519_crpyto`)

A Rust library and CLI example for constructing and signing Solana transactions using a YubiKey's OpenPGP card (Ed25519).

---

## Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
yubikey_ed25519_crpyto = { path = "./yubikey_ed25519_crpyto" }
solana-sdk = "1.17"
solana-client = "1.17"
```

### Fetching the Public Key

```rust
use yubikey_ed25519_crpyto::get_pubkey_from_yubikey;

let pubkey_bytes = get_pubkey_from_yubikey()?;
// Use pubkey_bytes as a Solana Pubkey, e.g.:
let pubkey = solana_sdk::pubkey::Pubkey::from(pubkey_bytes);
```

### Signing a Solana Transaction Message

```rust
use yubikey_ed25519_crpyto::sign_with_yubikey;

// `msg_data` is the serialized Solana message bytes
let signature_bytes = sign_with_yubikey(&msg_data)?;
// Use signature_bytes as the signature for your transaction
```

---

## Example CLI

A ready-to-use CLI is provided in `examples/main.rs`.

### Build and Run

```bash
cargo run --example main -p yubikey_ed25519_crpyto
```

### Example CLI Flow

- Fetches your YubiKey Ed25519 public key
- Prompts for recipient and amount
- Builds and signs a Solana transfer transaction
- Verifies and sends the transaction to Solana testnet

#### Example Output

```
--- Solana Transfer (YubiKey Signing) ---
Sender pubkey (from YubiKey): <YOUR_PUBKEY>
Sender balance (testnet): <LAMPORTS>
Recent blockhash (from testnet): <BLOCKHASH>
Recipient pubkey (base58): <ENTER>
Amount (lamports): <ENTER>
Enter User PIN (PW1/PIN2): <PIN>
Local signature verification successful.
Transaction sent and confirmed!
Solana signature: <SIGNATURE>
Signed transaction (base64): <BASE64>
Signature (base58): <SIGNATURE>
```

---

## Prerequisites

- YubiKey with OpenPGP (Ed25519) configured
- PC/SC middleware (`pcscd` running)
- Linux: `sudo apt-get install pcscd opensc gnupg scdaemon`

---

## Troubleshooting

- Ensure your YubiKey is inserted and supports Ed25519 OpenPGP
- Check with `gpg --card-status` or `pcsc_scan`
- If prompted, enter your YubiKey PIN (default: 123456)

---

## License

MIT OR Apache-2.0
