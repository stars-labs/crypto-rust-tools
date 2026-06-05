# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Structure

**YubiWallet** is a seedless, multi-chain hardware wallet built on the YubiKey —
sign Solana, Ethereum, Sui and Bitcoin transactions with keys generated on-card
that never leave the secure element. It is a single-product Rust workspace
(repo `stars-labs/yubiwallet`, crate published on crates.io as `yubiwallet`).

### Workspace crates

1. **yubiwallet** (`yubiwallet/`): the core library + CLI. Talks raw APDU to the
   card over PC/SC (no vendor SDK). The old `ssh-to-solana` tool is folded in as
   the `yubiwallet ssh-to-solana` subcommand.
2. **yubiwallet-host** (`yubiwallet-host/`): a Native Messaging host so a browser
   extension can drive the card. Performs key operations only; all chain logic
   stays in the (external) extension.
3. **yubiwallet-wasm** (`yubiwallet-wasm/`): wasm-bindgen package exposing the
   chain-independent logic (address derivation, EVM signature recovery) for JS
   integrators. It **cannot sign** — WASM can't reach the card.

History: this repo was renamed from `crypto-rust-tools`, and previously held
`frost-mpc-cli-node` / `webrtc-signal-server` crates (removed in commit 086dcf5).
It was also briefly named `yubisign` (yanked); don't reintroduce that name.

## Commands

### Building

```bash
# Build everything (library, CLI, host, wasm crate)
cargo build --release

# Build a specific crate
cargo build --release -p yubiwallet        # core lib + CLI (needs the `card` feature, default)
cargo build --release -p yubiwallet-host

# Build the wasm npm package (needs wasm-pack)
integration/wasm-usage/build.sh web        # → yubiwallet-wasm/pkg/
```

### Running

```bash
# CLI: list all accounts / show an address
cargo run -p yubiwallet -- list
cargo run -p yubiwallet -- address --applet piv --slot 9a --curve ed25519
cargo run -p yubiwallet -- address --applet openpgp --slot sig --curve secp256k1
cargo run -p yubiwallet -- ssh-to-solana "ssh-ed25519 AAAA..."

# Example signers (need rustc >= 1.91 for the alloy dev-dependency)
cargo run --release -p yubiwallet --example sol_surfpool -- --applet piv --slot 9a
cargo run --release -p yubiwallet --example sui_sign
cargo run --release -p yubiwallet --example btc_sign
cargo run --release -p yubiwallet --example ethereum
```

### Testing

```bash
# All tests (CI uses stable). Pure derivation tests live in yubiwallet/tests/chains.rs.
cargo test --workspace

cargo test -p yubiwallet
cargo test -p yubiwallet --test chains      # pubkey→address derivation vectors
cargo test --workspace <test_name>
```

The shell-based **hardware** test suite (real cards + local nodes) is in
`yubiwallet/multichain-tests/` (excluded from the published crate).

### Code Quality

```bash
cargo fmt                                   # format
cargo fmt --check                           # check formatting
cargo clippy --workspace --all-targets      # lint
cargo clippy --workspace --all-targets --fix
```

## Architecture

### yubiwallet (core library + CLI)

YubiKey hardware signing with **multi-account** support. Accounts are selected by
an `Account { applet, slot, curve }`:

- **PIV applet** (firmware 5.7+): Ed25519 across ~24 slots (`9A/9C/9D/9E` +
  retired `82`–`95`) → many Solana/Sui accounts. PIV does **not** support secp256k1.
- **OpenPGP applet**: Ed25519 or secp256k1, but only ~2–3 usable slots
  (`SIG`/`AUT`) → Ethereum/Bitcoin accounts. No BIP32 derivation on either applet.

Module layout (`yubiwallet/src/`):
- `account.rs`: `Account`/`Applet`/`Curve`, slot constants, slot/applet/curve parsing
- `apdu.rs`: short- and extended-length APDU construction + PC/SC transport
- `openpgp.rs`: OpenPGP get-pubkey/sign (PSO:CDS / INTERNAL AUTHENTICATE)
- `piv.rs`: PIV get-pubkey (via slot certificate) / sign (GENERAL AUTHENTICATE),
  BER-TLV helpers, `list_accounts`
- `pin.rs`: PIN prompting / `resolve` (caller-supplied vs prompted)
- `eth.rs`: Ethereum address derivation + `(r,s,v)` recovery (low-S/EIP-155)
- `error.rs`: `Error` enum + status-word decoding
- `main.rs`: CLI (`list`, `address`, `ssh-to-solana`); `examples/*.rs`

High-level entry points: `get_pubkey`, `sign` (prompts for PIN), `sign_with_pin`
(caller supplies the PIN — used by the host). A backward-compatible single-key
API (`get_pubkey_from_yubikey` / `sign_with_yubikey`, OpenPGP SIG + Ed25519) is
retained.

**The `card` feature** (default) gates all PC/SC + card I/O behind
`#[cfg(feature = "card")]`. Building with `--no-default-features` compiles only
the chain-independent logic (`account`/`eth`/`error`) without the `pcsc`
dependency — that's what `yubiwallet-wasm` does.

### yubiwallet-host

Native Messaging host (u32-le length prefix + UTF-8 JSON). Methods: `get_info`,
`get_status`, `list_accounts`, `sign_secp256k1` (→ `r,s,recovery_id`),
`sign_ed25519`. The PIN is read from `$YUBIWALLET_PIN` (testing) or `/dev/tty`,
never over the protocol. Browser-integration templates are in `integration/`;
the design RFC is `docs/dapp-bridge-design.md`.

### Dependencies (deliberate rule)

The core library + CLI depend ONLY on PC/SC + crypto + encoding (`pcsc`, `k256`,
`sha3`, `hex`, `bs58`, `base64`, `clap`, `thiserror`) — **no blockchain SDKs**.
This keeps the trusted surface small and lets the lib build on old toolchains.
Blockchain SDKs are `[dev-dependencies]` used only by `examples/` and tests:
`alloy` (Ethereum; the maintained successor to ethers), plus small encoding
crates (`sha2`, `blake2`, `ripemd`, `bech32`) for Sui/Bitcoin. `solana-sdk` is
intentionally NOT a dependency anywhere — the Solana example is SDK-free (raw
JSON-RPC + manual tx). When adding lib deps, keep them SDK-free and don't put
blockchain SDKs or card I/O in the wasm path.

Note: the example signers need **rustc ≥ 1.91** (alloy MSRV); the library, CLI
and unit tests build on older toolchains. CI uses stable.

## Prerequisites

For YubiKey operations:
- YubiKey with PIV (firmware 5.7+ for Ed25519) and/or OpenPGP configured
- PC/SC daemon running (`pcscd`); `ykman` for PIV provisioning
- Linux: `sudo apt-get install pcscd opensc gnupg scdaemon`
