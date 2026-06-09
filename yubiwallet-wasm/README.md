# @stars-labs/yubiwallet-wasm

WASM bindings for [**YubiWallet**](https://github.com/stars-labs/yubiwallet)'s
**chain-independent logic** — address derivation and EVM signature recovery —
for wallet integrators (browser extensions and web apps).

> ⚠️ **This package cannot sign.** A browser/WASM context cannot reach the
> YubiKey's PIV/OpenPGP (CCID) interface. Signing must go through the native
> messaging host (`yubiwallet-host`). What you get here are the pure pieces that
> don't need the card: turning a public key into an address, and turning the
> card's raw `R‖S` output into an EIP-155 `(r, s, v)` signature.

## Install

```bash
npm install @stars-labs/yubiwallet-wasm
```

## Usage

The package ships a `web` target (ES module + `init()`). Initialize the wasm
module once, then call the functions.

```js
import init, {
  solana_address,
  eth_address,
  eth_signature,
} from "@stars-labs/yubiwallet-wasm";

await init(); // load the .wasm (once)

// Solana address (base58) from a 32-byte Ed25519 public key (hex)
const sol = solana_address("e3b0c44298fc1c149afbf4c8996fb924...");

// Ethereum address (0x…) from a 65-byte uncompressed secp256k1 key (hex)
const addr = eth_address("04a1b2c3...");

// Recover an EIP-155 signature from the card's raw R‖S (64-byte hex)
const sig = eth_signature(
  pubkeyHex, // 65-byte uncompressed signer key (hex)
  hashHex,   // 32-byte message hash the card signed (hex)
  rsHex,     // 64-byte R‖S from the card (hex)
  1n,        // chain id (bigint)
);
console.log(sig.r, sig.s, sig.v);
```

All hex arguments accept an optional `0x` prefix.

## API

| Function | Returns | Description |
| --- | --- | --- |
| `solana_address(pubkey_hex)` | `string` | Base58 Solana address from a 32-byte Ed25519 key. |
| `eth_address(pubkey_hex)` | `string` | `0x…` Ethereum address from a 65-byte uncompressed secp256k1 key. |
| `eth_signature(pubkey_hex, hash_hex, rs_hex, chain_id)` | `EvmSignature` | EIP-155 `(r, s, v)` recovered from the card's raw `R‖S`. |

`EvmSignature` exposes `.r` (hex string), `.s` (hex string), and `.v` (bigint).

## Where signing happens

```
 browser extension / web app
        │  address derivation, (r,s,v) recovery
        ▼
 @stars-labs/yubiwallet-wasm        ← this package (no card access)
        │  sign request (raw hash)
        ▼
 yubiwallet-host  ──APDU──▶  YubiKey (PIV / OpenPGP)
```

See the [main repository](https://github.com/stars-labs/yubiwallet) and
`docs/dapp-bridge-design.md` for the native-host bridge that performs the actual
signing.

## License

MIT OR Apache-2.0
