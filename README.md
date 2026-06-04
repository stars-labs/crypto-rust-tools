<div align="center">

# YubiSign

**A seedless, multi-chain hardware wallet on the YubiKey you already own.**

English | [简体中文](README.zh.md)

[![CI](https://github.com/stars-labs/yubisign/actions/workflows/ci.yml/badge.svg)](https://github.com/stars-labs/yubisign/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)](#license)
[![Rust](https://img.shields.io/badge/rust-edition%202024-orange)](#)

Sign **Solana, Ethereum, Sui and Bitcoin** transactions with keys that are
generated on-card and **never leave the secure element** — no seed phrase, no
browser extension, no extra $50 device.

</div>

```console
$ yubisign list
OpenPGP (SIG slot):  Ethereum: 0xc370580ab2b42762347b76899abaa2a261c95c82
OpenPGP (AUT slot):  Ethereum: 0xeca4518f33df44ee11233139565a48b2225e389e
PIV slots (Ed25519 → Solana):
  slot 0x9a  Solana: CtodL3wQ1ySYhEfTwGdXJKnEGhbwdjhrRMGUTEGSzLNm
  slot 0x82  Solana: NYCYnX1iiUetEJJZo9f6fzX1KSXuGYepTWRHpncTJG6
  ... (24 PIV accounts)
```

<sub>Record a GIF of your own setup with [`vhs demo/demo.tape`](demo/demo.tape).</sub>

## Why YubiSign

- 🌱 **No seed phrase.** The #1 way people lose crypto is a lost or stolen
  24-word seed. YubiSign keys are generated inside the YubiKey and can never be
  exported — there is no seed to back up, leak, or phish.
- 🔌 **Reuse hardware you own.** Your YubiKey becomes a hardware wallet. No new
  device to buy or carry.
- 🪪 **One key, many accounts, many chains.** 26 independent on-card keys →
  Solana / Sui / Aptos (Ed25519) and Ethereum / Bitcoin / Cosmos (secp256k1).
- 🦀 **Open source & hardware-verified.** Tiny Rust core talking raw APDU over
  PC/SC — no vendor SDK. Every signing path proven on real nodes (below).

## Supported chains

| Family | Curve | Applet · slots | Accounts |
|--------|-------|----------------|----------|
| Solana, Sui, Aptos, … | Ed25519 | PIV (fw 5.7+) `9A/9C/9D/9E`+`82`–`95` | ~24 |
| Ethereum, Bitcoin, Cosmos, … | secp256k1 | OpenPGP `SIG`+`AUT` | ~2 |

> No BIP32 derivation — each account is an independent on-card key. secp256k1
> accounts are capped at the OpenPGP slots; for *many* EVM/BTC accounts use a
> BIP32 device. YubiSign shines at: lots of Ed25519 accounts + a couple of
> secp256k1 accounts on hardware you already have.

## Quickstart

```bash
git clone https://github.com/stars-labs/yubisign && cd yubisign
cargo build --release -p yubisign

yubisign list                                                   # all accounts
yubisign address --applet piv --slot 9a --curve ed25519         # a Solana address
yubisign address --applet openpgp --slot sig --curve secp256k1  # an Ethereum/Bitcoin key
yubisign ssh-to-solana "ssh-ed25519 AAAA..."                    # SSH key → Solana address
```

Prerequisites: `pcscd` running, `ykman` for PIV provisioning, YubiKey firmware
**5.7+** for Ed25519 on PIV. Full docs, provisioning, and the library API:
**[yubisign/README.md](yubisign/README.md)**.

## Verified on real hardware

Every account on a YubiKey (fw 5.7.4) signed and broadcast **real transactions
on local nodes** — 50 in total:

| Chain | Node | Result |
|-------|------|--------|
| Solana | surfpool | **24/24 broadcast** |
| Sui | `sui` localnet | **24/24 executed** |
| Bitcoin | `bitcoind -regtest` | **2/2 confirmed** |
| Ethereum | anvil | broadcast & mined |

Reproduce with the suite in
**[yubisign/multichain-tests/](yubisign/multichain-tests/README.md)**.

## How it works

| Applet · slot | Get public key | Sign |
|---------------|----------------|------|
| OpenPGP SIG | GET PUBLIC KEY (CRT `B6`) | PSO:COMPUTE DIGITAL SIGNATURE |
| OpenPGP AUT | GET PUBLIC KEY (CRT `A4`) | INTERNAL AUTHENTICATE |
| PIV slot | parse slot certificate | GENERAL AUTHENTICATE (extended APDU) |

Per-chain encoding (address derivation, Sui Blake2b intent digest, Bitcoin
BIP143 + low-S DER, Ethereum EIP-155 `v` recovery) is documented in the crate.

## FAQ — backup, loss & recovery

**Can I back up an on-card key?**
It depends how you provision the slot:
- **On-card generation** (`ykman piv keys generate`) — the private key is created
  inside the secure element and can *never* be exported. Strongest security, but
  **no backup**: if that key is gone, funds at its address are unrecoverable.
- **Off-card + import** — generate the key in software, keep an encrypted backup,
  then load it onto one or more YubiKeys. You trade a little security (the key
  briefly existed off-device) for recoverability.

Pick per account: recoverable funds → off-card + import to ≥2 keys; cold / max
security → on-card and accept there is no backup.

**What if I lose my YubiKey?**
It is PIN-protected — a finder can't sign without your PIN, and wrong-PIN
attempts lock the applet (exhausting the PUK / Admin PIN forces a reset that
*wipes* the keys). So:
- Have a **backup** (same key imported to a 2nd YubiKey, or an encrypted key
  backup)? Use it to move funds to a fresh address immediately.
- **On-card-only with no backup?** You can't move them either — treat as lost.
  This is why redundancy matters for any real balance.
- Always change PINs from the defaults.

**What if my YubiKey breaks or dies?**
Same as loss: backed-up (off-card) keys restore onto a new YubiKey; on-card-only
keys are unrecoverable. Keep a second provisioned key as redundancy.

**Can someone clone my YubiKey?**
No. Secure-element keys are non-extractable — they cannot be copied off the device.

**I forgot my PIN.**
- PIV: unblock with the **PUK** (`ykman piv access unblock-pin`); if the PUK is
  also exhausted you must reset the PIV applet (wipes its keys).
- OpenPGP: the **Admin PIN (PW3)** resets the User PIN; if PW3 is lost, resetting
  the OpenPGP applet wipes its keys.
Back up your PIN / PUK / Admin PIN somewhere safe too.

**Recommended backup strategy?**
1. **Redundancy** — generate off-card and import the same key to 2–3 YubiKeys
   kept in separate locations.
2. **Encrypted backup** — store the off-card private key with `age`/`gpg` in ≥2
   places.
3. **Multisig** — use a wallet multisig (Solana Squads, Gnosis Safe, …) with keys
   on different YubiKeys: losing one device, or one key being compromised, does
   not lose the funds. Best choice for large balances.

**Does YubiSign store my keys or a seed anywhere?**
No. There is no seed phrase and YubiSign writes no key file — it only talks to
the card; signing happens on-card after PIN entry.

## Contributing & security

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). The keep-it-tiny
dependency rule and threat model are in [SECURITY.md](SECURITY.md); please report
vulnerabilities privately (see that file).

## License

MIT OR Apache-2.0
