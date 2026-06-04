//! Multi-chain test helper: Solana sign + broadcast using a PIV Ed25519 slot.
//!
//! Usage: `cargo run --example sol_surfpool -- <slot>`  (PIN on stdin)
//! Airdrops to the slot's address on a local node (default surfpool :8899),
//! then signs and broadcasts a tiny transfer.
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig, message::Message, native_token::LAMPORTS_PER_SOL,
    pubkey::Pubkey, signature::Signature, system_instruction, transaction::Transaction,
};
use std::str::FromStr;
use yubikey_crypto::{Account, Applet, Curve, get_pubkey, parse_slot, sign};

fn main() {
    let slot_str = std::env::args().nth(1).expect("arg1=slot (e.g. 9a)");
    let rpc = std::env::var("SOLANA_RPC").unwrap_or_else(|_| "http://127.0.0.1:8899".into());
    let slot = parse_slot(Applet::Piv, &slot_str).expect("slot");
    let acc = Account {
        applet: Applet::Piv,
        slot,
        curve: Curve::Ed25519,
    };

    let from =
        Pubkey::from(<[u8; 32]>::try_from(get_pubkey(&acc).expect("pubkey").as_slice()).unwrap());
    let c = RpcClient::new_with_commitment(rpc, CommitmentConfig::confirmed());

    if let Ok(sig) = c.request_airdrop(&from, LAMPORTS_PER_SOL) {
        for _ in 0..30 {
            if c.confirm_transaction(&sig).unwrap_or(false) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }
    let bal = c.get_balance(&from).unwrap_or(0);

    let to = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
    let mut tx = Transaction::new_unsigned(Message::new(
        &[system_instruction::transfer(&from, &to, 1000)],
        Some(&from),
    ));
    tx.message.recent_blockhash = c.get_latest_blockhash().expect("blockhash");

    let sig_bytes = sign(&acc, &tx.message.serialize()).expect("card sign");
    tx.signatures = vec![Signature::from(
        <[u8; 64]>::try_from(sig_bytes.as_slice()).unwrap(),
    )];
    let local_ok = tx.verify_with_results().iter().all(|&r| r);

    match c.send_and_confirm_transaction(&tx) {
        Ok(s) => {
            println!("slot {slot_str} {from} bal={bal} localVerify={local_ok} BROADCAST_OK tx={s}")
        }
        Err(e) => println!("slot {slot_str} {from} bal={bal} localVerify={local_ok} ERR {e}"),
    }
}
