//! Multi-chain test helper: Solana sign + broadcast using a PIV Ed25519 slot.
//!
//! Usage: `cargo run --example sol_surfpool -- <slot>`  (PIN on stdin)
//!
//! SDK-free on purpose: talks JSON-RPC over a plain TCP socket and serializes a
//! legacy transfer transaction by hand, so it pulls **no** solana-sdk tree. The
//! signature is produced on-card via `yubiwallet::sign`. Targets a local node
//! (surfpool default 127.0.0.1:8899); override with SOLANA_RPC=host:port.
use base64::Engine;
use std::io::{Read, Write};
use yubiwallet::{Account, Applet, Curve, get_pubkey, parse_slot, sign};

const SYSTEM_PROGRAM: [u8; 32] = [0u8; 32]; // 11111111111111111111111111111111
// wSOL mint — any valid pubkey works as the transfer recipient.
const RECIPIENT_B58: &str = "So11111111111111111111111111111111111111112";

fn rpc(method: &str, params: &str) -> String {
    let addr = std::env::var("SOLANA_RPC").unwrap_or_else(|_| "127.0.0.1:8899".into());
    let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{params}}}"#);
    let req = format!(
        "POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut s = std::net::TcpStream::connect(&addr).expect("connect to Solana RPC");
    s.write_all(req.as_bytes()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    resp.split_once("\r\n\r\n")
        .map(|x| x.1)
        .unwrap_or("")
        .to_string()
}

/// Crude `"key":"value"` string extractor (avoids a JSON dependency).
fn json_str(body: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let i = body.find(&pat)? + pat.len();
    let rest = &body[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Solana compact-u16 (shortvec) length prefix.
fn compact_u16(mut n: usize, out: &mut Vec<u8>) {
    loop {
        let mut b = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
        }
        out.push(b);
        if n == 0 {
            break;
        }
    }
}

fn b58(s: &str) -> [u8; 32] {
    <[u8; 32]>::try_from(bs58::decode(s).into_vec().expect("base58").as_slice()).expect("32 bytes")
}

fn main() {
    let slot_str = std::env::args().nth(1).expect("arg1=slot (e.g. 9a)");
    let slot = parse_slot(Applet::Piv, &slot_str).expect("slot");
    let acc = Account {
        applet: Applet::Piv,
        slot,
        curve: Curve::Ed25519,
    };

    let from = <[u8; 32]>::try_from(get_pubkey(&acc).expect("pubkey").as_slice()).unwrap();
    let from_b58 = bs58::encode(from).into_string();
    let to = b58(RECIPIENT_B58);

    // Fund via airdrop, then wait for it to land.
    rpc("requestAirdrop", &format!(r#"["{from_b58}", 1000000000]"#));
    std::thread::sleep(std::time::Duration::from_secs(8));

    let bh_resp = rpc("getLatestBlockhash", "[]");
    let blockhash = b58(&json_str(&bh_resp, "blockhash").expect("blockhash"));

    // Legacy message: header(1 sig, 0 ro-signed, 1 ro-unsigned) | accounts | blockhash | ixs.
    let mut msg = vec![1u8, 0, 1];
    compact_u16(3, &mut msg);
    msg.extend_from_slice(&from); // 0: signer, writable
    msg.extend_from_slice(&to); // 1: writable
    msg.extend_from_slice(&SYSTEM_PROGRAM); // 2: readonly program
    msg.extend_from_slice(&blockhash);
    compact_u16(1, &mut msg); // 1 instruction
    msg.push(2); // program_id_index = system program
    compact_u16(2, &mut msg);
    msg.extend_from_slice(&[0, 1]); // account indices: from, to
    let mut data = vec![2u8, 0, 0, 0]; // system transfer
    data.extend_from_slice(&1000u64.to_le_bytes()); // 1000 lamports
    compact_u16(data.len(), &mut msg);
    msg.extend_from_slice(&data);

    let sig = sign(&acc, &msg).expect("card sign"); // 64-byte ed25519

    // Transaction = compact-array(signatures) | message
    let mut tx = Vec::new();
    compact_u16(1, &mut tx);
    tx.extend_from_slice(&sig);
    tx.extend_from_slice(&msg);
    let tx_b64 = base64::engine::general_purpose::STANDARD.encode(&tx);

    let send = rpc(
        "sendTransaction",
        &format!(r#"["{tx_b64}", {{"encoding":"base64"}}]"#),
    );
    match json_str(&send, "result") {
        Some(s) => println!("slot {slot_str} {from_b58} BROADCAST_OK tx={s}"),
        None => println!("slot {slot_str} {from_b58} ERR {}", send.trim()),
    }
}
