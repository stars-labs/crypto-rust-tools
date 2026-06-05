# RFC: dApp browser bridge for YubiWallet

Status: **draft** · Scope: let dApps sign with a YubiKey through YubiWallet.

## Problem & constraint

Browsers cannot talk to the YubiKey's **PIV/OpenPGP (CCID) interface** — that
channel is owned by the OS PC/SC stack; WebUSB/WebHID can't claim it, and
WebAuthn signs auth assertions over the wrong curves/format (P-256, not
secp256k1; can't produce raw tx signatures). Therefore a pure extension is
impossible; we need a **native host** the extension talks to.

## Architecture

```
dApp  ──EIP-1193 / Wallet Standard──▶  Extension (MV3)
                                         inpage · content · background · popup
Extension  ──Native Messaging (u32-le len + JSON)──▶  yubiwallet-host (Rust)
yubiwallet-host  ──PC/SC APDU──▶  YubiKey
```

**Split of responsibility**
- **host** = key primitives only (no network, no chain logic): list accounts,
  sign a secp256k1 prehash, sign an ed25519 message. Reuses the `yubiwallet`
  library. Collects the PIN itself (never via the extension).
- **extension** = chain logic + provider + approval UI: builds txs / computes
  EIP-191/712 / Solana messages with `viem` / `@solana/web3.js`, does RPC, shows
  confirmations.
- Adding a chain ⇒ extension-only change; the host stays the same.

## Host protocol (Native Messaging)

Frame: `u32` little-endian length + UTF-8 JSON.
Request `{ id, method, params }` → Response `{ id, result }` or `{ id, error: { code, message } }`.

| Method | Params | Result |
|--------|--------|--------|
| `list_accounts` | `{}` | `{ accounts: [Account] }` |
| `get_status` | `{}` | `{ card_present, accounts }` |
| `sign_secp256k1` | `{ account_id, prehash:<32B hex> }` | `{ r, s, recovery_id }` |
| `sign_ed25519` | `{ account_id, message:<hex> }` | `{ signature:<64B hex> }` |

`Account = { id:"applet:slot:curve", family:"evm"|"solana", curve, applet, slot, address, pubkey }`.
`account_id` maps directly to `Account { applet, slot, curve }`.

Errors: `USER_REJECTED`, `CARD_ABSENT`/`CARD_ERROR`, `PIN_FAILED`,
`UNKNOWN_ACCOUNT`, `UNSUPPORTED_METHOD`, `BAD_REQUEST`.

The host signs hashes/messages only — it never builds transactions, so it has no
network access and a small, auditable surface.

## Extension flows (EVM example)

- **eth_requestAccounts** → `list_accounts` → filter `family=="evm"` → approve in
  popup → return `[address]`.
- **eth_sendTransaction** → viem builds tx + computes sighash → approve → host
  `sign_secp256k1(prehash)` → viem assembles signed tx (`yParity`) → RPC
  `eth_sendRawTransaction`.
- **personal_sign / eth_signTypedData_v4** → viem computes EIP-191/712 hash →
  host `sign_secp256k1` → `r‖s‖(recovery_id+27)`.

**Solana** (later): Wallet Standard `signTransaction` → web3.js builds message →
host `sign_ed25519(message)` → attach.

## Security

- Native Messaging is scoped to the extension ID (`allowed_origins`) — no other
  page/extension can reach the host.
- PIN is entered on the host (pinentry), never seen by the extension/dApp.
- Every signature requires a human-readable approval; nothing signs silently.
- Host has no network; RPC is the extension's job.
- Known residual risk: a compromised host OS could request signatures while the
  key is inserted/unlocked — mitigate with a touch policy and removing the key
  when idle (see SECURITY.md).

## Repo layout

```
yubiwallet/        # signing library + CLI (existing)
yubiwallet-host/   # native-messaging host (this RFC, M0 in progress)
extension/         # MV3 TypeScript (M1+)
installer/         # registers the native-messaging manifest (M4)
```

## Milestones

- **M0** — host: framing + `list_accounts` / `sign_secp256k1` / `sign_ed25519`,
  reusing the library. *(in progress)*
- **M1** — MV3 extension: EIP-6963 + EIP-1193; connect / sendTransaction /
  personal_sign against a testnet dApp.
- **M2** — approval UI, PIN status, GUI pinentry, error model.
- **M3** — Solana Wallet Standard (`sign_ed25519`).
- **M4** — installer + cross-platform packaging (manifest registration, signing).

## PIN entry status

M0 reads the PIN from `$YUBIWALLET_PIN` (testing) or the controlling tty. The
browser-launched daemon has no tty, so M2 adds a GUI pinentry.
