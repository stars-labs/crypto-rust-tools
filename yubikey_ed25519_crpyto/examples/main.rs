use base64::Engine;
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    message::Message, pubkey::Pubkey, signature::Signature, system_instruction,
    transaction::Transaction,
};
use std::error::Error;
use std::io::{self, Write};
use yubikey_ed25519_crpyto::{get_pubkey_from_yubikey, sign_with_yubikey};

const SOLANA_RPC_URL: &str = "https://api.testnet.solana.com";

fn read_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn build_transfer_tx(
    from_pubkey: &Pubkey,
    to_pubkey: &Pubkey,
    lamports: u64,
    recent_blockhash: solana_sdk::hash::Hash,
) -> Transaction {
    let ix = system_instruction::transfer(from_pubkey, to_pubkey, lamports);
    let msg = Message::new(&[ix], Some(from_pubkey));
    let mut tx = Transaction::new_unsigned(msg);
    tx.message.recent_blockhash = recent_blockhash;
    tx
}

fn main() {
    if let Err(e) = solana_transfer_with_yubikey() {
        eprintln!("Transfer failed: {}", e);
        std::process::exit(1);
    }
}

fn solana_transfer_with_yubikey() -> Result<(), Box<dyn Error>> {
    println!("\n--- Solana Transfer (YubiKey Signing) ---");

    // Fetch sender pubkey from YubiKey
    let from_pubkey_bytes = get_pubkey_from_yubikey()?;
    let from_pubkey = Pubkey::from(from_pubkey_bytes);
    println!("Sender pubkey (from YubiKey): {}", from_pubkey);

    let client = RpcClient::new(SOLANA_RPC_URL.to_string());
    match client.get_balance(&from_pubkey) {
        Ok(balance) => println!("Sender balance (testnet): {} lamports", balance),
        Err(e) => println!("Could not fetch sender balance: {}", e),
    }

    let blockhash = match client.get_latest_blockhash() {
        Ok(blockhash) => {
            println!("Recent blockhash (from testnet): {}", blockhash);
            blockhash
        }
        Err(e) => {
            println!("Could not fetch recent blockhash: {}", e);
            return Err("Failed to fetch recent blockhash from testnet".into());
        }
    };

    let to_str = read_line("Recipient pubkey (base58): ")?;
    let lamports_str = read_line("Amount (lamports): ")?;
    let to_pubkey = to_str.parse::<Pubkey>()?;
    let lamports = lamports_str.parse::<u64>()?;

    match client.get_account(&to_pubkey) {
        Ok(_) => {}
        Err(_) => {
            let min_balance = client.get_minimum_balance_for_rent_exemption(0)?;
            if lamports < min_balance {
                println!(
                    "Warning: Recipient account does not exist. You must send at least {} lamports (rent-exempt minimum) to create it.",
                    min_balance
                );
            }
        }
    }

    let mut tx = build_transfer_tx(&from_pubkey, &to_pubkey, lamports, blockhash);

    let msg_data = tx.message.serialize();

    let signature_bytes = sign_with_yubikey(&msg_data)?;

    let signature_slice: &[u8];
    if signature_bytes.len() == 64 {
        signature_slice = &signature_bytes;
    } else if signature_bytes.len() == 65 {
        signature_slice = &signature_bytes[1..];
    } else {
        signature_slice = &signature_bytes;
    }

    let signature = Signature::from(<[u8; 64]>::try_from(signature_slice)?);
    tx.signatures = vec![signature];

    if !tx.verify_with_results().iter().all(|&res| res) {
        return Err("Local signature verification failed!".into());
    }
    println!("Local signature verification successful.");

    match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => {
            println!("Transaction sent and confirmed!");
            println!("Solana signature: {}", sig);
        }
        Err(e) => {
            println!("Failed to broadcast transaction: {}", e);
            println!(
                "If you are using a YubiKey with ECDSA/secp256k1, Solana will reject the signature."
            );
            println!(
                "Make sure your YubiKey is configured for Ed25519 and the public key matches the Solana account."
            );
            return Err("Failed to broadcast transaction".into());
        }
    }

    println!(
        "Signed transaction (base64): {}",
        base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx)?)
    );
    println!("Signature (base58): {}", tx.signatures[0]);
    Ok(())
}
