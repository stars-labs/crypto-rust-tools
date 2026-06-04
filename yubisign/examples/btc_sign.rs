//! Multi-chain test helper: Bitcoin P2WPKH (BIP143) signing with an OpenPGP
//! secp256k1 slot.
//!
//!   cargo run --example btc_sign -- <slot>
//!       -> print bcrt1 (regtest) P2WPKH address + compressed pubkey
//!   cargo run --example btc_sign -- <slot> sign <txid> <vout> <in_sat> <dest_spk_hex> <out_sat>
//!       -> print the raw signed segwit tx hex (PIN on stdin)
use bech32::hrp::BCRT;
use bech32::segwit;
use k256::ecdsa::Signature;
use ripemd::{Digest as _, Ripemd160};
use sha2::Sha256;
use yubisign::{Account, Applet, Curve, get_pubkey, parse_slot, sign};

fn sha256(d: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(d);
    h.finalize().into()
}
fn sha256d(d: &[u8]) -> [u8; 32] {
    sha256(&sha256(d))
}
fn hash160(d: &[u8]) -> [u8; 20] {
    let mut r = Ripemd160::new();
    r.update(sha256(d));
    r.finalize().into()
}
fn compress(pk65: &[u8]) -> [u8; 33] {
    let mut c = [0u8; 33];
    c[0] = if pk65[64] & 1 == 0 { 0x02 } else { 0x03 };
    c[1..].copy_from_slice(&pk65[1..33]);
    c
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let slot = parse_slot(Applet::OpenPgp, &args[1]).unwrap();
    let acc = Account {
        applet: Applet::OpenPgp,
        slot,
        curve: Curve::Secp256k1,
    };
    let pk = get_pubkey(&acc).expect("pubkey"); // 65-byte uncompressed
    let cpk = compress(&pk);
    let h160 = hash160(&cpk);

    if args.len() < 3 {
        let addr = segwit::encode_v0(BCRT, &h160).expect("valid v0 witness program");
        println!("addr={addr}");
        println!("cpk={}", hex::encode(cpk));
        return;
    }

    let mut txid_le = hex::decode(&args[3]).unwrap();
    txid_le.reverse();
    let vout: u32 = args[4].parse().unwrap();
    let in_sat: u64 = args[5].parse().unwrap();
    let dest_spk = hex::decode(&args[6]).unwrap();
    let out_sat: u64 = args[7].parse().unwrap();
    let (version, sequence, locktime): (u32, u32, u32) = (2, 0xffffffff, 0);

    let mut outpoint = txid_le.clone();
    outpoint.extend_from_slice(&vout.to_le_bytes());
    let hash_prevouts = sha256d(&outpoint);
    let hash_sequence = sha256d(&sequence.to_le_bytes());

    let mut script_code = vec![0x19u8, 0x76, 0xa9, 0x14];
    script_code.extend_from_slice(&h160);
    script_code.extend_from_slice(&[0x88, 0xac]);

    let mut outputs = Vec::new();
    outputs.extend_from_slice(&out_sat.to_le_bytes());
    outputs.push(dest_spk.len() as u8);
    outputs.extend_from_slice(&dest_spk);
    let hash_outputs = sha256d(&outputs);

    let mut pre = Vec::new();
    pre.extend_from_slice(&version.to_le_bytes());
    pre.extend_from_slice(&hash_prevouts);
    pre.extend_from_slice(&hash_sequence);
    pre.extend_from_slice(&outpoint);
    pre.extend_from_slice(&script_code);
    pre.extend_from_slice(&in_sat.to_le_bytes());
    pre.extend_from_slice(&sequence.to_le_bytes());
    pre.extend_from_slice(&hash_outputs);
    pre.extend_from_slice(&locktime.to_le_bytes());
    pre.extend_from_slice(&1u32.to_le_bytes()); // SIGHASH_ALL
    let sighash = sha256d(&pre);

    let rs = sign(&acc, &sighash).expect("card sign");
    let mut s = Signature::from_slice(&rs).unwrap();
    if let Some(n) = s.normalize_s() {
        s = n;
    }
    let mut sig = s.to_der().as_bytes().to_vec();
    sig.push(0x01); // SIGHASH_ALL

    let mut tx = Vec::new();
    tx.extend_from_slice(&version.to_le_bytes());
    tx.push(0x00);
    tx.push(0x01); // segwit marker + flag
    tx.push(0x01); // 1 input
    tx.extend_from_slice(&outpoint);
    tx.push(0x00); // empty scriptSig
    tx.extend_from_slice(&sequence.to_le_bytes());
    tx.push(0x01); // 1 output
    tx.extend_from_slice(&out_sat.to_le_bytes());
    tx.push(dest_spk.len() as u8);
    tx.extend_from_slice(&dest_spk);
    tx.push(0x02); // witness items
    tx.push(sig.len() as u8);
    tx.extend_from_slice(&sig);
    tx.push(cpk.len() as u8);
    tx.extend_from_slice(&cpk);
    tx.extend_from_slice(&locktime.to_le_bytes());

    println!("{}", hex::encode(tx));
}
