//! Ethereum signature post-processing for secp256k1 keys.
//!
//! The YubiKey returns a bare `R || S` ECDSA signature. Ethereum additionally
//! needs the recovery id (`v`), and requires low-S (EIP-2) signatures. These
//! pure helpers turn a card signature into a broadcastable `(r, s, v)`.

use crate::error::Error;
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

/// A fully-formed Ethereum signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthSignature {
    pub r: [u8; 32],
    pub s: [u8; 32],
    pub v: u64,
}

/// Derive a 20-byte Ethereum address from a 65-byte uncompressed secp256k1
/// public key (`0x04 || X || Y`): `keccak256(X || Y)[12..]`.
pub fn address_from_pubkey(pubkey: &[u8]) -> Result<[u8; 20], Error> {
    if pubkey.len() != 65 || pubkey[0] != 0x04 {
        return Err(Error::Parse(
            "expected 65-byte uncompressed secp256k1 public key".into(),
        ));
    }
    let mut hasher = Keccak256::new();
    hasher.update(&pubkey[1..]); // drop the 0x04 prefix
    let hash = hasher.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&hash[12..]);
    Ok(addr)
}

/// Lower-case hex address with `0x` prefix (no EIP-55 checksum casing).
pub fn address_to_hex(addr: &[u8; 20]) -> String {
    format!("0x{}", hex::encode(addr))
}

/// Turn a card `R || S` signature over `msg_hash` into an EIP-155 `(r, s, v)`.
///
/// `pubkey` is the signer's 65-byte uncompressed key, used to brute-force the
/// recovery id. The signature is normalized to low-S first (EIP-2).
pub fn ethereum_signature(
    pubkey: &[u8],
    msg_hash: &[u8; 32],
    rs: &[u8; 64],
    chain_id: u64,
) -> Result<EthSignature, Error> {
    let expected = VerifyingKey::from_sec1_bytes(pubkey)
        .map_err(|e| Error::Signature(format!("invalid public key: {e}")))?;

    let mut sig =
        Signature::from_slice(rs).map_err(|e| Error::Signature(format!("invalid R||S: {e}")))?;
    // EIP-2: Ethereum only accepts low-S signatures.
    if let Some(normalized) = sig.normalize_s() {
        sig = normalized;
    }

    for rid in 0u8..=1 {
        let recid = RecoveryId::from_byte(rid).expect("0 and 1 are valid recovery ids");
        if let Ok(vk) = VerifyingKey::recover_from_prehash(msg_hash, &sig, recid) {
            if vk == expected {
                let bytes = sig.to_bytes(); // 64 bytes, R || S
                let mut r = [0u8; 32];
                let mut s = [0u8; 32];
                r.copy_from_slice(&bytes[..32]);
                s.copy_from_slice(&bytes[32..]);
                return Ok(EthSignature {
                    r,
                    s,
                    v: eip155_v(rid, chain_id),
                });
            }
        }
    }
    Err(Error::RecoveryFailed)
}

/// EIP-155 `v` value: `recovery_id + chain_id * 2 + 35`.
pub fn eip155_v(recovery_id: u8, chain_id: u64) -> u64 {
    recovery_id as u64 + chain_id * 2 + 35
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::ecdsa::SigningKey;
    use k256::ecdsa::signature::hazmat::PrehashSigner;

    /// Hardhat/Anvil dev account #0 — a well-known key/address pair.
    const HARDHAT_SK: [u8; 32] =
        hex_literal(b"ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80");
    const HARDHAT_ADDR: &str = "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266";

    // Tiny const hex decoder so tests don't need an extra dependency.
    const fn hex_literal(s: &[u8; 64]) -> [u8; 32] {
        const fn nibble(c: u8) -> u8 {
            match c {
                b'0'..=b'9' => c - b'0',
                b'a'..=b'f' => c - b'a' + 10,
                _ => panic!("bad hex digit"),
            }
        }
        let mut out = [0u8; 32];
        let mut i = 0;
        while i < 32 {
            out[i] = (nibble(s[i * 2]) << 4) | nibble(s[i * 2 + 1]);
            i += 1;
        }
        out
    }

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&HARDHAT_SK.into()).unwrap()
    }

    fn uncompressed_pubkey(sk: &SigningKey) -> Vec<u8> {
        sk.verifying_key()
            .to_encoded_point(false)
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn known_address_from_pubkey() {
        let pk = uncompressed_pubkey(&signing_key());
        let addr = address_from_pubkey(&pk).unwrap();
        assert_eq!(address_to_hex(&addr), HARDHAT_ADDR);
    }

    #[test]
    fn address_requires_uncompressed_key() {
        assert!(address_from_pubkey(&[0u8; 33]).is_err());
        assert!(address_from_pubkey(&[0x02; 65]).is_err());
    }

    #[test]
    fn recovers_v_and_matches_signer() {
        let sk = signing_key();
        let pk = uncompressed_pubkey(&sk);
        let msg_hash = {
            let mut h = Keccak256::new();
            h.update(b"hello ethereum");
            let d = h.finalize();
            let mut out = [0u8; 32];
            out.copy_from_slice(&d);
            out
        };

        // Card-style signature: bare R || S.
        let sig: Signature = sk.sign_prehash(&msg_hash).unwrap();
        let rs: [u8; 64] = sig.to_bytes().into();

        let chain_id = 11155111; // Sepolia
        let eth = ethereum_signature(&pk, &msg_hash, &rs, chain_id).unwrap();

        // v must encode chain id and a 0/1 recovery id.
        let recid = eth.v - chain_id * 2 - 35;
        assert!(recid == 0 || recid == 1);

        // Re-derive the address from the recovered signature to prove correctness.
        let recovered = VerifyingKey::recover_from_prehash(
            &msg_hash,
            &Signature::from_scalars(eth.r, eth.s).unwrap(),
            RecoveryId::from_byte(recid as u8).unwrap(),
        )
        .unwrap();
        let recovered_pk = recovered.to_encoded_point(false).as_bytes().to_vec();
        assert_eq!(
            address_from_pubkey(&recovered_pk).unwrap(),
            address_from_pubkey(&pk).unwrap()
        );
    }

    #[test]
    fn eip155_v_formula() {
        assert_eq!(eip155_v(0, 1), 37); // mainnet, recid 0
        assert_eq!(eip155_v(1, 1), 38); // mainnet, recid 1
        assert_eq!(eip155_v(0, 11155111), 22310257); // Sepolia: 11155111*2 + 35
    }

    #[test]
    fn wrong_pubkey_fails_recovery() {
        let sk = signing_key();
        let msg_hash = [7u8; 32];
        let sig: Signature = sk.sign_prehash(&msg_hash).unwrap();
        let rs: [u8; 64] = sig.to_bytes().into();
        // A different (valid) key should not match.
        let other = SigningKey::from_bytes(&[1u8; 32].into()).unwrap();
        let other_pk = uncompressed_pubkey(&other);
        assert!(matches!(
            ethereum_signature(&other_pk, &msg_hash, &rs, 1),
            Err(Error::RecoveryFailed)
        ));
    }
}
