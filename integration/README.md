# Integrating YubiWallet

YubiWallet is **Rust-only**. The browser extension / wallet UI lives in *your*
project — this repo gives you three ways to plug a YubiKey's on-card keys into it.
Pick the surface that matches where your code runs:

| You are writing… | Use | What it does |
| --- | --- | --- |
| A native app (Rust) | the [`yubiwallet`](https://crates.io/crates/yubiwallet) crate | talk to the card directly |
| A browser extension | the **native-messaging host** | bridge browser ↔ card |
| Wallet UI / dApp logic | the **`yubiwallet-wasm`** package | address derivation + signature recovery (no signing) |

> **Why a host at all?** A browser can't reach the YubiKey's PIV/OpenPGP CCID
> interface (it's owned by the OS PC/SC stack; WebUSB/WebHID can't claim it and
> WebAuthn uses the wrong curves). So the extension speaks **Native Messaging**
> to a small local binary (`yubiwallet-host`) that does the card I/O.
> Full design: [`../docs/dapp-bridge-design.md`](../docs/dapp-bridge-design.md).

---

## 1. Native-messaging host

`yubiwallet-host` is a workspace binary that performs **key operations only** —
all chain logic stays in your extension. Messages are framed by the browser
(u32-le length + UTF-8 JSON); see the protocol in
[`docs/dapp-bridge-design.md`](../docs/dapp-bridge-design.md).

**Install** (builds the host, registers the manifest for installed browsers):

```bash
integration/native-messaging/install.sh <your-chrome-extension-id>
# or point at a prebuilt binary:
integration/native-messaging/install.sh <ext-id> /path/to/yubiwallet-host
```

`com.yubiwallet.host.json` is the manifest template the script fills in
(`PLACEHOLDER_*` → real path + extension id). Chrome/Chromium/Brave/Edge use
`allowed_origins: ["chrome-extension://<id>/"]`; Firefox uses
`allowed_extensions: ["<id>"]` — the installer writes the right form per browser.

| Method | Params | Result |
| --- | --- | --- |
| `get_info` | — | `{ name, version, protocol, methods }` |
| `get_status` | — | `{ card_present, accounts }` |
| `list_accounts` | — | `{ accounts: [...] }` |
| `sign_secp256k1` | `{ account_id, prehash }` (32B hex) | `{ r, s, recovery_id }` |
| `sign_ed25519` | `{ account_id, message }` (hex) | `{ signature }` (64B hex) |

The host collects the **PIN** itself (`$YUBIWALLET_PIN` for testing, else
`/dev/tty`). Never route a PIN through the extension or network.

## 2. TypeScript client ([`ts-client/`](./ts-client))

A ~100-line typed wrapper around `chrome.runtime.connectNative`, plus an
end-to-end example (connect → `get_info` → `list_accounts` → sign):

```ts
import { HostClient } from "./host-client";

const host = new HostClient();              // connects to com.yubiwallet.host
const { accounts } = await host.listAccounts();
const evm = accounts.find(a => a.curve === "secp256k1")!;
const { r, s, recovery_id } = await host.signSecp256k1(evm.id, txHashHex);
```

- `types.ts` — wire types (`Account`, requests/responses, results)
- `host-client.ts` — the `HostClient` port wrapper (id-correlated calls)
- `example.ts` — the full flow for both ed25519 and secp256k1 accounts

## 3. WASM package ([`wasm-usage/`](./wasm-usage))

`yubiwallet-wasm` exposes the **chain-independent** helpers as an npm package.
It **does not sign** (WASM can't reach the card) — use it for display addresses
and turning the host's raw `R||S` into a chain signature.

```bash
integration/wasm-usage/build.sh web      # → yubiwallet-wasm/pkg (npm-publishable)
```

```ts
import init, { solana_address, eth_address, eth_signature } from "yubiwallet-wasm";
await init();
const addr = solana_address(pubkeyHex);          // base58
const sig  = eth_signature(pubkeyHex, hashHex, rsHex, 1n); // {r, s, v}
```

Exports: `solana_address(pubkey_hex)`, `eth_address(pubkey_hex)`,
`eth_signature(pubkey_hex, hash_hex, rs_hex, chain_id) → { r, s, v }`.

---

## Typical extension flow

1. `init()` the WASM module; `new HostClient()` to connect to the host.
2. `getInfo()` to negotiate protocol; `listAccounts()` to populate the UI
   (use `solana_address`/`eth_address` if you only have pubkeys).
3. Build + serialize the transaction **in the extension**.
4. `signEd25519` / `signSecp256k1` to sign the bytes/hash on-card.
5. Assemble the signed tx (use `eth_signature` to derive `v` for EVM) and
   broadcast — all chain-specific work stays on your side.
