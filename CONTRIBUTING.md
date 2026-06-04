# Contributing to YubiWallet

Thanks for your interest! YubiWallet is a small Rust library + CLI that turns a
YubiKey into a multi-chain hardware signer.

## Project layout

```
yubiwallet/
  src/            # library + CLI (the only published, security-critical code)
    account.rs apdu.rs openpgp.rs piv.rs eth.rs error.rs pin.rs lib.rs main.rs
  examples/       # per-chain signing demos (use blockchain SDKs / RPC)
  multichain-tests/   # shell orchestration + backup demo (hardware required)
```

**Dependency rule:** the library + CLI must stay SDK-free — only PC/SC, crypto
and encoding crates. Anything blockchain-specific (alloy, etc.) goes in
`[dev-dependencies]` and is used by `examples/` only. This keeps the trusted
surface small and the crate buildable on older toolchains.

## Building & checking

```bash
cargo build -p yubiwallet          # library + CLI (builds on rustc 1.86+)
cargo test  -p yubiwallet          # unit tests (APDU/TLV/signature recovery)
cargo fmt   -p yubiwallet --check
cargo clippy -p yubiwallet --all-targets
```

The library, CLI and unit tests build on older Rust. The **example signers need
rustc ≥ 1.91** (modern blockchain crates). If your dev shell pins an older Rust
(e.g. a Nix flake), bump it before building examples.

## Hardware tests

`multichain-tests/` runs real transactions on local nodes (surfpool, `sui`
localnet, `bitcoind -regtest`) and needs a physical YubiKey. They are **not** in
CI. See `yubiwallet/multichain-tests/README.md`.

## Pull requests

- Keep commits focused; write clear messages explaining *why*.
- `cargo fmt`, `cargo clippy` (no warnings) and `cargo test` must pass.
- Add/adjust unit tests for byte-level logic (APDU/TLV/encoding/recovery).
- Don't add blockchain SDKs to the library's `[dependencies]`.
- Be mindful of the security model (see [SECURITY.md](SECURITY.md)).

## License

By contributing you agree your work is licensed under MIT OR Apache-2.0.
