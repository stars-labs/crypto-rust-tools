# Multi-chain hardware test suite

End-to-end tests that sign **real transactions on local nodes** with keys that
never leave the YubiKey, across every account on the card:

| Chain | Local node | Curve · slots | Accounts | Result |
|-------|-----------|---------------|----------|--------|
| **Solana** | [surfpool](https://github.com/txtx/surfpool) `:8899` | Ed25519 · PIV `9A/9C/9D/9E`+`82`–`95` | 24 | **24/24 broadcast** |
| **Sui** | `sui start --with-faucet` `:9000` | Ed25519 · PIV (same 24) | 24 | **24/24 executed** |
| **Bitcoin** | `bitcoind -regtest` (Core v31) | secp256k1 · OpenPGP `SIG`+`AUT` | 2 | **2/2 confirmed** |

Each Ed25519 PIV key is one account on **any** Ed25519 chain (Solana, Sui, Aptos…);
each secp256k1 OpenPGP key is one account on **any** secp256k1 chain (Bitcoin,
Ethereum, Cosmos…). 26 on-card keys → 50 signed transactions verified here.

## How signing is built (per chain)

- **Solana** — sign the serialized message with the card's Ed25519 key (PIV
  `GENERAL AUTHENTICATE`); 64-byte signature.
- **Sui** — address = `Blake2b256(0x00 ‖ pubkey)`; sign
  `Blake2b256(intent ‖ tx_bytes)` with Ed25519; serialized signature =
  base64(`0x00 ‖ sig ‖ pubkey`); submit with `sui client execute-signed-tx`.
- **Bitcoin** — P2WPKH address from `hash160(compressed_pubkey)`; BIP143 sighash;
  card ECDSA (secp256k1) → low-S (k256) → DER + `SIGHASH_ALL`; segwit witness;
  `sendrawtransaction`.

The signing helpers live as crate examples: `examples/{sol_surfpool,sui_sign,btc_sign}.rs`.
The Solana example is **SDK-free** (raw JSON-RPC over a socket + manual tx
serialization), so it pulls no `solana-sdk` tree.

> **Toolchain:** the example signers use modern blockchain crates and need
> **rustc ≥ 1.89**. The core library + CLI build on older toolchains. If your dev
> shell pins an older Rust (e.g. a Nix flake), bump it before running these.

## Prerequisites

```bash
# nodes
surfpool start                                  # Solana RPC on :8899
sui start --with-faucet --force-regenesis       # Sui on :9000, faucet :9123
bitcoind -datadir=/tmp/btcreg                    # regtest (see ../README for conf)
sui client new-env --alias localnet --rpc http://127.0.0.1:9000 && sui client switch --env localnet

# card: PIV slots = Ed25519, OpenPGP SIG/AUT = secp256k1 (see ../README "Provisioning")
```

## Run

```bash
cd yubiwallet/multichain-tests
YK_PIN=123456 ./run-solana.sh                         # 24 PIV slots, surfpool
YK_PIN=123456 ./run-sui.sh                            # 24 PIV slots, sui localnet
YK_PIN=123456 BTC_DATADIR=/tmp/btcreg ./run-bitcoin.sh  # OpenPGP SIG+AUT, regtest
```

> These tests require a physical YubiKey and the local nodes; they are **not**
> part of CI. The crate's unit tests (APDU/TLV/signature recovery) run in CI.

## Backup & restore demo

`run-backup.sh` proves the **backup-able** workflow: a key generated **off-card**
can be restored to any YubiKey and yields the same account.

It generates an Ed25519 key off-card, derives its Solana address *offline* from
the file, imports the **same** key into two PIV slots, and asserts the offline
address equals what YubiWallet reads from both slots:

```bash
YK_PIN=123456 ./run-backup.sh          # ⚠ overwrites PIV slots 94 and 95
# offline address (from backup file): 9ttXUDCGx95Q1kwJ1aQxUJCFLpFCrx4yhD2FCRA1oeR5
# slot 94 on-card address:            9ttXUDCGx95Q1kwJ1aQxUJCFLpFCrx4yhD2FCRA1oeR5
# slot 95 on-card address:            9ttXUDCGx95Q1kwJ1aQxUJCFLpFCrx4yhD2FCRA1oeR5
# === BACKUP OK: offline file == slot 94 == slot 95 (restorable to any YubiKey) ===
```

`offline == slot 94 == slot 95` means: keep that key file encrypted, and you can
restore the account onto a replacement YubiKey by re-importing it. (See the
[Backup & recovery](../README.md#backup--recovery) section for the full
PIV-import and OpenPGP-`keytocard` recipes, and the
[FAQ](../../README.md#faq--backup-loss--recovery).)

## Sample output

```
slot 9a  CtodL3wQ1ySYhEfTwGdXJKnEGhbwdjhrRMGUTEGSzLNm  ... BROADCAST_OK tx=2uWEJCoG…
...
=== Solana: 24/24 broadcast OK ===

slot 9a  0xbca9203ede…  Transaction Digest: 7kswc5yi8XTQ…  Success
...
=== Sui: 24/24 execute Success ===

slot sig  bcrt1qvctjhr42kqqpxezt5gnrcn0ly7h0w733qw47mn  BROADCAST_OK txid=1d453524…
slot aut  bcrt1ququvfwa92jjfex7ttlnxap38gqdd8nlrx6kzh2  BROADCAST_OK txid=fb5954a4…
=== Bitcoin: 2/2 broadcast OK ===
```
