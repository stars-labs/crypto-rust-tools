# YubiWallet

[![CI](https://github.com/stars-labs/yubiwallet/actions/workflows/ci.yml/badge.svg)](https://github.com/stars-labs/yubiwallet/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)

**Sign Solana, Ethereum, Sui and Bitcoin transactions with keys that never leave your YubiKey.**

A small Rust library + CLI that turns a YubiKey into a multi-account hardware
wallet. Private keys are generated on-card and never touch your disk or RAM —
every signature is computed inside the secure element.

```bash
# See every account on the key
cargo run -p yubiwallet -- list
# OpenPGP (SIG slot):  Solana: H73nCgxv5uUr62poKvJHXPhKk1pTaQp9DUtAdUxTcQir
# OpenPGP (AUT slot):  Ethereum: 0x9f2a...c41d
# PIV slots (Ed25519 → Solana):
#   slot 0x9a  Solana: 7Xk2...
#   slot 0x82  Solana: Df91...
```

---

## Why

Hardware wallets are great, but a YubiKey is something many developers already
carry. This crate lets you reuse it to custody blockchain keys:

- 🔐 **Keys never leave the device** — signing happens on the secure element.
- 🪪 **Multi-account on one key** — pick accounts by *applet + slot + curve*.
- ⛓️ **Solana & Ethereum** today; any Ed25519 or secp256k1 chain is reachable.
- 🦀 **Tiny, dependency-light core** — raw APDU over PC/SC, no vendor SDK.

## Account capacity

| Chain family | Curve | Applet | Slots | Accounts |
|--------------|-------|--------|-------|----------|
| Solana, Aptos, Sui, … | Ed25519 | **PIV** (fw 5.7+) | `9A/9C/9D/9E` + retired `82`–`95` | **~24** |
| Ethereum, BTC, Cosmos, Tron, … | secp256k1 | **OpenPGP** | `SIG` + `AUT` | **~2** |

> **Honest limits.** PIV cannot do secp256k1, so Ethereum-style accounts are
> capped at the 2 signing slots of the OpenPGP applet. There is no BIP32
> derivation — each account is an independent on-card key. If you need *many*
> secp256k1 accounts, use a BIP32 device (Ledger/Trezor) instead. Where this
> shines: lots of Solana accounts + a couple of EVM accounts on hardware you
> already own.

---

## Install

```bash
git clone https://github.com/stars-labs/yubiwallet
cd yubiwallet
cargo build --release -p yubiwallet
```

Prerequisites:

- PC/SC running (`pcscd`). Linux: `sudo apt-get install pcscd opensc gnupg scdaemon`.
- `ykman` (yubikey-manager) for PIV provisioning.
- YubiKey firmware **5.7+** for Ed25519 on PIV.

## CLI

```bash
# List all accounts (OpenPGP SIG/AUT + PIV Ed25519 slots)
cargo run -p yubiwallet -- list

# Show one account's address
cargo run -p yubiwallet -- address --applet piv     --slot 9a  --curve ed25519
cargo run -p yubiwallet -- address --applet openpgp --slot aut --curve secp256k1
```

`--slot` accepts aliases: `sig`/`aut` (OpenPGP); `auth`/`sign`/`keymgmt`/`cardauth`
or a hex id like `9a`/`82` (PIV).

## Library

```rust
use yubiwallet::{get_pubkey, sign, eth, Account, Applet, Curve, openpgp_slot};

// Solana account on PIV retired slot 0x82.
let sol = Account { applet: Applet::Piv, slot: 0x82, curve: Curve::Ed25519 };
let pubkey = get_pubkey(&sol)?;            // 32-byte Ed25519 key
let signature = sign(&sol, &message)?;     // 64-byte signature (prompts for PIN)

// Ethereum account on the OpenPGP AUT slot.
let eth_acc = Account { applet: Applet::OpenPgp, slot: openpgp_slot::AUT, curve: Curve::Secp256k1 };
let pk = get_pubkey(&eth_acc)?;                          // 65-byte uncompressed point
let address = eth::address_from_pubkey(&pk)?;           // 20-byte address
let rs: [u8; 64] = sign(&eth_acc, &tx_hash)?.try_into().unwrap();
let sig = eth::ethereum_signature(&pk, &tx_hash, &rs, chain_id)?; // r, s, v (low-S, EIP-155)
```

The original single-key helpers (`get_pubkey_from_yubikey` / `sign_with_yubikey`,
OpenPGP SIG, Ed25519) remain available.

## Examples

The example signers are the **multi-chain test suite** (see
[`multichain-tests/`](multichain-tests/README.md)). They use blockchain SDKs/RPC
and require a recent toolchain (**rustc ≥ 1.91**); the core library + CLI build on
older toolchains too.

```bash
# Solana transfer on a local node (SDK-free: raw JSON-RPC + manual tx)
cargo run -p yubiwallet --example sol_surfpool -- 9a

# Ethereum transfer (set ETHEREUM_RPC_URL in examples/ethereum.rs first).
# Use --slot aut if your SIG slot is an Ed25519 (Solana) key.
cargo run -p yubiwallet --example ethereum -- --slot aut
```

---

## Provisioning

### More Solana accounts (PIV, up to ~24)

PIV has no "read public key" command, so each slot needs a **key *and* a
certificate** (the public key is parsed from the cert):

```bash
ykman piv keys generate -a ED25519 9a /tmp/pub_9a.pem
ykman piv certificates generate -s "CN=sol-9a" 9a /tmp/pub_9a.pem
# repeat for 9c, 9d, 9e, 82..95
```

### An Ethereum account next to Solana (one key)

Keep the OpenPGP **SIG** slot as Ed25519 (Solana) and put a **secp256k1** key in
the **AUT** slot via `gpg --expert --edit-card`. ⚠️ When using `key-attr`, leave
Signature/Encryption at *Curve 25519* and only set **Authentication →
secp256k1** — changing the Signature slot would destroy your Solana key. The
slots are independent: SIG signs via PSO:CDS, AUT via INTERNAL AUTHENTICATE,
handled automatically.

---

## How signing works

| Applet / slot | Get public key | Sign |
|---------------|----------------|------|
| OpenPGP SIG | GET PUBLIC KEY (CRT `B6`) | PSO:COMPUTE DIGITAL SIGNATURE |
| OpenPGP AUT | GET PUBLIC KEY (CRT `A4`) | INTERNAL AUTHENTICATE |
| PIV slot | parse slot certificate (GET DATA) | GENERAL AUTHENTICATE (extended APDU) |

For Ethereum, the card returns a bare `R‖S`; the `eth` module recovers the
recovery id against the public key, normalizes to low-S, and emits an EIP-155
`(r, s, v)`.

## Coexisting with GnuPG / SSH

This tool talks to the card **directly over PC/SC**, independently of GnuPG. Two
interactions are worth knowing if you also use the same YubiKey with `gpg`:

**1. Card contention.** GnuPG's `scdaemon` opens the card exclusively, so the
tool fails with `PC/SC error: ... other connections outstanding`. Release the
card first:

```bash
gpgconf --kill scdaemon        # (and gpg-agent if needed)
```

`scdaemon` restarts on demand the next time `gpg` uses the card. Tip: `cargo run`
is slow enough to start that `scdaemon` can re-grab the card mid-launch — build
first, then kill `scdaemon`, then run the prebuilt binary.

**2. A PIV Ed25519 key can break `gpg-agent`'s SSH.** If you use `gpg-agent` for
SSH (`enable-ssh-support`), it auto-exposes the card's PIV **9A** key as an SSH
identity — and GnuPG (≤ 2.4.9) mis-encodes an Ed25519 PIV key's SSH blob (it adds
a stray `"Ed25519"` curve field). One bad blob makes `ssh-add -L` fail for the
*whole* list with `error fetching identities: invalid format`, breaking SSH.

Fix: tell `scdaemon` to ignore the PIV applet, so `gpg-agent` only serves your
OpenPGP keys. Add to `~/.gnupg/scdaemon.conf`:

```
disable-application piv
```

(home-manager: `programs.gpg.scdaemonSettings."disable-application" = "piv";`)

This does **not** affect this tool (it uses PC/SC, not scdaemon) or `ykman`; it
only stops GnuPG from touching PIV. Then `gpgconf --kill gpg-agent scdaemon` and
re-test `ssh-add -L`. If you instead want PIV-based SSH *through* GnuPG, use an
RSA-2048 or NIST P-256 key in the 9A slot (not Ed25519).

## Troubleshooting

- `gpg --card-status` / `ykman piv info` to inspect the card.
- `no Ed25519 SubjectPublicKeyInfo in certificate`: the PIV slot has no cert or
  holds a non-Ed25519 key — (re)generate it as above.
- `ssh-add -L` → `invalid format` after provisioning PIV: see *Coexisting with
  GnuPG / SSH* above (`disable-application piv`).
- `other connections outstanding`: `scdaemon` holds the card — `gpgconf --kill
  scdaemon`.
- Default PINs: OpenPGP user `123456` / admin `12345678`; PIV `123456`.

## Verified on hardware (multi-chain)

Every account on a real YubiKey (firmware 5.7.4) was used to sign and broadcast
**real transactions on local nodes** — 50 in total:

| Chain | Local node | Curve · slots | Result |
|-------|-----------|---------------|--------|
| Solana | surfpool | Ed25519 · 24 PIV | **24/24 broadcast** |
| Sui | `sui start` localnet | Ed25519 · 24 PIV | **24/24 executed** |
| Bitcoin | `bitcoind -regtest` | secp256k1 · OpenPGP SIG+AUT | **2/2 confirmed** |
| Ethereum | anvil | secp256k1 · OpenPGP | broadcast & mined |

Reproduce it with the suite in [`multichain-tests/`](multichain-tests/README.md)
(signing helpers live in `examples/{sol_surfpool,sui_sign,btc_sign}.rs`).

## Backup & recovery

Two provisioning models, pick per account:

- **On-card generation** (`ykman piv keys generate`) — key never leaves the
  secure element. Strongest, but **non-exportable = no backup**.
- **Off-card generation + import** — the key exists as a file you can back up
  (encrypt it!), then load onto one or more YubiKeys. Recoverable: restoring to a
  fresh key is the *same* import and yields the *same* address.

### Backup-able Ed25519 (Solana/Sui/Aptos) via PIV

```bash
# 1. Generate OFF-card — encrypt & store account.pem safely (this IS your backup)
openssl genpkey -algorithm ed25519 -out account.pem
openssl pkey -in account.pem -pubout -out account.pub.pem
# 2. Import into a PIV slot + self-signed cert (repeat on each YubiKey you keep)
ykman piv keys import 9c account.pem
ykman piv certificates generate -s "CN=account" 9c account.pub.pem
# 3. Verify
yubiwallet address --applet piv --slot 9c --curve ed25519
```

### Backup-able secp256k1 (Ethereum/Bitcoin) via OpenPGP

```bash
# 1. Generate OFF-card and back up the secret key
gpg --batch --pinentry-mode loopback --passphrase '' --gen-key <<'EOF'
%no-protection
Key-Type: ECDSA
Key-Curve: secp256k1
Key-Usage: sign
Name-Real: eth-account
Expire-Date: 0
%commit
EOF
gpg --export-secret-keys --armor <KEYID> > eth-account.backup.asc   # encrypt & store
# 2. Move it onto the card's signature slot: keytocard -> (1) Signature key -> save
gpg --edit-key <KEYID>
# 3. Verify
yubiwallet address --applet openpgp --slot sig --curve secp256k1
```

> Restore = re-run the import (PIV) or `keytocard` from the backup (OpenPGP) on a
> new YubiKey → identical address. `multichain-tests/run-backup.sh` proves this:
> the offline-derived address equals the on-card address across two slots.

Lost/broken key, forgotten PIN/PUK, cloning, and the full strategy (redundancy,
encrypted backups, multisig) are in the [FAQ](../README.md#faq--backup-loss--recovery).

## Status

Byte-level logic (APDU/TLV/signature recovery) is covered by unit tests
(run in CI), including a known Hardhat address vector. Live card round-trips
are exercised by the hardware suite above.

## License

MIT OR Apache-2.0
