//! WASM bindings for YubiWallet's **chain-independent logic**, for wallet
//! integrators (browser extensions / web apps).
//!
//! This package does **not** sign — a browser/WASM context cannot reach the
//! YubiKey's PIV/OpenPGP (CCID) interface. Signing must go through the native
//! host (`yubiwallet-host`). Here you get the pieces that don't need the card:
//! address derivation and turning a card `R||S` into an EIP-155 `(r, s, v)`.

use wasm_bindgen::prelude::*;
use yubiwallet::eth;

fn err<E: core::fmt::Display>(e: E) -> JsError {
    JsError::new(&e.to_string())
}

fn unhex(s: &str) -> Result<Vec<u8>, JsError> {
    hex::decode(s.trim_start_matches("0x")).map_err(err)
}

/// Solana address (base58) from a 32-byte Ed25519 public key (hex).
#[wasm_bindgen]
pub fn solana_address(pubkey_hex: &str) -> Result<String, JsError> {
    let pk = unhex(pubkey_hex)?;
    if pk.len() != 32 {
        return Err(JsError::new("expected a 32-byte Ed25519 public key"));
    }
    Ok(bs58::encode(pk).into_string())
}

/// Ethereum address (`0x…`) from a 65-byte uncompressed secp256k1 key (hex).
#[wasm_bindgen]
pub fn eth_address(pubkey_hex: &str) -> Result<String, JsError> {
    let addr = eth::address_from_pubkey(&unhex(pubkey_hex)?).map_err(err)?;
    Ok(eth::address_to_hex(&addr))
}

/// An EIP-155 signature: hex `r`, hex `s`, and `v`.
#[wasm_bindgen]
pub struct EvmSignature {
    r: String,
    s: String,
    v: u64,
}

#[wasm_bindgen]
impl EvmSignature {
    #[wasm_bindgen(getter)]
    pub fn r(&self) -> String {
        self.r.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn s(&self) -> String {
        self.s.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn v(&self) -> u64 {
        self.v
    }
}

/// Turn the card's raw `R||S` (hex, 64 bytes) into an EIP-155 `(r, s, v)`.
///
/// `pubkey_hex` is the 65-byte uncompressed signer key (to recover the id);
/// `hash_hex` is the 32-byte message hash the card signed.
#[wasm_bindgen]
pub fn eth_signature(
    pubkey_hex: &str,
    hash_hex: &str,
    rs_hex: &str,
    chain_id: u64,
) -> Result<EvmSignature, JsError> {
    let pk = unhex(pubkey_hex)?;
    let hash: [u8; 32] = unhex(hash_hex)?
        .try_into()
        .map_err(|_| JsError::new("hash must be 32 bytes"))?;
    let rs: [u8; 64] = unhex(rs_hex)?
        .try_into()
        .map_err(|_| JsError::new("R||S must be 64 bytes"))?;
    let sig = eth::ethereum_signature(&pk, &hash, &rs, chain_id).map_err(err)?;
    Ok(EvmSignature {
        r: hex::encode(sig.r),
        s: hex::encode(sig.s),
        v: sig.v,
    })
}
