//! Multi-chain test helper: Sui Ed25519 signing with a PIV slot.
//!
//!   cargo run --example sui_sign -- <slot>                -> print Sui address
//!   cargo run --example sui_sign -- <slot> <tx_bytes_b64> -> print base64 signature (PIN on stdin)
//!
//! Sui address = Blake2b256(flag(0x00) || pubkey). The signed message is
//! Blake2b256(intent || tx_bytes); the serialized signature is
//! base64(flag(0x00) || sig(64) || pubkey(32)).
use base64::Engine;
use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use yubikey_crypto::{Account, Applet, Curve, get_pubkey, parse_slot, sign};

type Blake2b256 = Blake2b<U32>;
fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

fn main() {
    let slot = parse_slot(Applet::Piv, &std::env::args().nth(1).expect("arg1=slot")).unwrap();
    let acc = Account {
        applet: Applet::Piv,
        slot,
        curve: Curve::Ed25519,
    };
    let pk = get_pubkey(&acc).expect("pubkey"); // 32-byte ed25519

    let mut h = Blake2b256::new();
    h.update([0x00]);
    h.update(&pk);
    let addr = format!("0x{}", hex::encode(h.finalize()));

    match std::env::args().nth(2) {
        None => println!("{addr}"),
        Some(txb64) => {
            let tx = b64().decode(txb64.trim()).expect("decode tx bytes");
            let mut msg = vec![0u8, 0, 0]; // intent: TransactionData, V0, Sui
            msg.extend_from_slice(&tx);
            let mut hh = Blake2b256::new();
            hh.update(&msg);
            let digest = hh.finalize();

            let sig = sign(&acc, &digest).expect("card sign"); // ed25519 over digest
            let mut ser = vec![0x00u8];
            ser.extend_from_slice(&sig);
            ser.extend_from_slice(&pk);
            println!("{}", b64().encode(ser));
        }
    }
}
