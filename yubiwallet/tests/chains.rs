//! Per-chain address-derivation tests — no YubiKey required.
//!
//! The hardware signing path needs a physical device (see multichain-tests/),
//! but the chain-specific *derivation* logic is pure and must stay correct.
//! These tests pin each chain's pubkey→address derivation against external
//! ground-truth vectors where they exist:
//!
//!   - Ethereum: lib `eth::address_from_pubkey` vs the canonical address for
//!     private key = 1 (pubkey = secp256k1 generator G).
//!   - Bitcoin (P2WPKH): the BIP-173 worked example (G → bc1qw508d6...).
//!   - Solana: base58 of the ed25519 pubkey (all-zeros → System Program id).
//!   - Sui: Blake2b256(0x00 || pubkey) (determinism + format + golden).
//!
//! The Bitcoin/Sui helpers mirror examples/{btc_sign,sui_sign}.rs; Solana
//! mirrors examples/sol_surfpool.rs. Keeping them here lets `cargo test`
//! guard the derivations in CI without hardware.

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest as _};
use ripemd::Ripemd160;
use sha2::{Digest as _, Sha256};

// secp256k1 generator G (public key for private key = 1).
const G_COMPRESSED: &str = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const G_UNCOMPRESSED: &str = concat!(
    "04",
    "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    "483ada7726a3c4655da4fbfc0e1108a8fd17b448a68554199c47d08ffb10d4b8",
);

fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).expect("valid hex fixture")
}

// ---------------------------------------------------------------- Ethereum

#[test]
fn ethereum_address_for_privkey_one() {
    // keccak256(X||Y)[12..]; the address for privkey=1 is well known.
    let addr =
        yubiwallet::eth::address_from_pubkey(&unhex(G_UNCOMPRESSED)).expect("derive eth address");
    assert_eq!(
        yubiwallet::eth::address_to_hex(&addr).to_lowercase(),
        "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"
    );
}

#[test]
fn ethereum_eip155_v_formula() {
    // recovery_id + chain_id*2 + 35; mainnet, recid 0 → 37.
    assert_eq!(yubiwallet::eth::eip155_v(0, 1), 37);
    assert_eq!(yubiwallet::eth::eip155_v(1, 1), 38);
}

// ----------------------------------------------------------------- Bitcoin
// P2WPKH = bech32 segwit-v0 of hash160(compressed_pubkey).
// hash160 = ripemd160(sha256(x)). Mirrors examples/btc_sign.rs.

fn hash160(d: &[u8]) -> [u8; 20] {
    let mut r = Ripemd160::new();
    let mut s = Sha256::new();
    s.update(d);
    r.update(s.finalize());
    r.finalize().into()
}

#[test]
fn bitcoin_p2wpkh_bip173_vector() {
    let h160 = hash160(&unhex(G_COMPRESSED));
    // BIP-173 worked example: G → this mainnet address.
    let mainnet = bech32::segwit::encode_v0(bech32::hrp::BC, &h160).expect("encode");
    assert_eq!(mainnet, "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4");

    // Same witness program, regtest HRP (what the example/demo uses).
    let regtest = bech32::segwit::encode_v0(bech32::hrp::BCRT, &h160).expect("encode");
    assert!(regtest.starts_with("bcrt1q"));
    assert_ne!(regtest, mainnet);
}

#[test]
fn bitcoin_hash160_is_20_bytes() {
    assert_eq!(hash160(&unhex(G_COMPRESSED)).len(), 20);
}

// ------------------------------------------------------------------ Solana
// Address = base58(ed25519 pubkey). Mirrors examples/sol_surfpool.rs.

fn solana_address(pubkey: &[u8; 32]) -> String {
    bs58::encode(pubkey).into_string()
}

#[test]
fn solana_all_zero_pubkey_is_system_program() {
    // 32 zero bytes base58-encode to the System Program id (32 '1's).
    assert_eq!(
        solana_address(&[0u8; 32]),
        "11111111111111111111111111111111"
    );
}

#[test]
fn solana_address_round_trips_to_pubkey() {
    let pk: [u8; 32] = unhex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
        .try_into()
        .unwrap();
    let addr = solana_address(&pk);
    let decoded = bs58::decode(&addr).into_vec().expect("base58");
    assert_eq!(
        decoded.as_slice(),
        &pk,
        "address must decode back to the key"
    );
}

// --------------------------------------------------------------------- Sui
// Address = "0x" || hex(Blake2b256(flag(0x00) || pubkey)). Mirrors
// examples/sui_sign.rs (flag 0x00 = Ed25519).

type Blake2b256 = Blake2b<U32>;

fn sui_address(pubkey: &[u8; 32]) -> String {
    let mut h = Blake2b256::new();
    h.update([0x00]); // Ed25519 scheme flag
    h.update(pubkey);
    format!("0x{}", hex::encode(h.finalize()))
}

#[test]
fn sui_address_format_and_determinism() {
    let pk = [7u8; 32];
    let a = sui_address(&pk);
    assert!(a.starts_with("0x"));
    assert_eq!(a.len(), 2 + 64, "Sui address is 32 bytes hex");
    assert_eq!(a, sui_address(&pk), "derivation must be deterministic");
    assert_ne!(
        a,
        sui_address(&[8u8; 32]),
        "different key → different address"
    );
}

#[test]
fn sui_address_golden_all_zero_pubkey() {
    // Golden value: Blake2b256(0x00 || 0x00*32). Pins the algorithm so a
    // change to the flag/hash is caught.
    assert_eq!(
        sui_address(&[0u8; 32]),
        "0xd8908c165dee785924e7421a0fd0418a19d5daeec395fd505a92a0fd3117e428"
    );
}
