# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for security problems. Report privately via
GitHub Security Advisories ("Report a vulnerability" on the repo's *Security* tab)
or email the maintainer at `xiongchenyu6@gmail.com`. We aim to acknowledge within
72 hours and to coordinate a fix and disclosure timeline with you.

## Security model

YubiSign is a thin client. **It never sees, stores, or transmits private keys.**

- **Keys live in the YubiKey's secure element.** With on-card key generation the
  private key is created on the device and is non-exportable — it cannot be read
  out, cloned, or backed up.
- **Every signature is computed on-card** after PIN verification. YubiSign only
  builds the message/transaction, sends an APDU, and receives the signature.
- **No seed phrase, no key files.** There is nothing on disk to leak. For PIV the
  public key is read from the slot's certificate.
- **Small trusted surface.** The core library/CLI depend only on PC/SC + crypto +
  encoding crates — no blockchain SDKs. (Examples that talk to chains use extra
  dev-dependencies and are not part of the library.)

## What it protects against

- **Lost or stolen YubiKey** — keys are PIN-protected; wrong-PIN attempts lock the
  applet, and exhausting the PUK/Admin PIN triggers a wipe. A thief without the
  PIN cannot sign.
- **Disk/host data theft at rest** — there is no key material on disk to steal.
- **Cloning** — secure-element keys are non-extractable.

## What it does NOT protect against (know your threats)

- **A compromised host while the key is inserted and unlocked.** The PIN is typed
  on the computer; malware on the host could capture it and ask the card to sign
  arbitrary transactions while the YubiKey is present. Verify what you sign; for
  high value, use a touch policy (`ykman ... --touch-policy`) and remove the key
  when idle.
- **Loss of an on-card-only key with no backup** — funds become unrecoverable.
  Use off-card generation + import to ≥2 keys, or a wallet multisig, for anything
  meaningful (see the README backup guide).
- **Supply-chain trust in the YubiKey secure element / firmware** (Yubico).
- **Address/transaction spoofing in your wallet UI** — YubiSign signs the bytes it
  is given.

## Defaults

Change the default PINs before real use: OpenPGP user `123456` / admin `12345678`;
PIV PIN `123456` / PUK `12345678` / management key.

## Scope

In scope: the `yubisign` library and CLI. Out of scope: the example signers
(test tooling), third-party nodes/RPCs, and the YubiKey hardware/firmware itself.
