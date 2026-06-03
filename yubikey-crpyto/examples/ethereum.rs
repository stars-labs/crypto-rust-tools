//! Ethereum transfer signed by a YubiKey secp256k1 key (OpenPGP applet).
//!
//! The chosen OpenPGP slot must hold a **secp256k1** key. If your SIG slot is an
//! Ed25519 key for Solana, put the secp256k1 key in the AUT slot and run with
//! `--slot aut`.

use clap::Parser;
use ethers::core::types::{Address, Bytes, U64, U256};
use ethers::providers::{Http, Middleware, Provider};
use rlp::RlpStream;
use sha3::{Digest, Keccak256};
use std::error::Error;
use std::io::{self, Write};
use yubikey_crypto::{Account, Applet, Curve, eth, parse_slot, sign};

// Replace with your own RPC endpoint (Sepolia testnet shown).
const ETHEREUM_RPC_URL: &str = "https://sepolia.infura.io/v3/YOUR_INFURA_PROJECT_ID";
const CHAIN_ID: u64 = 11155111; // Sepolia

/// OpenPGP secp256k1 account selection for the Ethereum signer.
#[derive(Parser)]
struct Args {
    /// OpenPGP slot holding the secp256k1 key: `sig` or `aut`.
    #[arg(long, default_value = "sig")]
    slot: String,
}

fn read_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn eth_account(args: &Args) -> Result<Account, Box<dyn Error>> {
    let slot = parse_slot(Applet::OpenPgp, &args.slot)?;
    Ok(Account {
        applet: Applet::OpenPgp,
        slot,
        curve: Curve::Secp256k1,
    })
}

async fn ethereum_transaction_with_yubikey(account: &Account) -> Result<(), Box<dyn Error>> {
    if ETHEREUM_RPC_URL.contains("YOUR_INFURA_PROJECT_ID") {
        return Err("Set ETHEREUM_RPC_URL to a real RPC endpoint first.".into());
    }

    // 1. Fetch the secp256k1 public key and derive the real Ethereum address.
    let pubkey = yubikey_crypto::get_pubkey(account)?; // 65-byte uncompressed point
    let address_bytes = eth::address_from_pubkey(&pubkey)?;
    let from = Address::from(address_bytes);
    println!(
        "Sender address (from YubiKey): {}",
        eth::address_to_hex(&address_bytes)
    );

    // 2. Query chain state.
    let provider = Provider::<Http>::try_from(ETHEREUM_RPC_URL)?;
    let balance = provider.get_balance(from, None).await?;
    let nonce = provider.get_transaction_count(from, None).await?;
    let gas_price = provider.get_gas_price().await?;
    println!("Balance: {balance} wei | nonce: {nonce} | gas price: {gas_price} wei");

    // 3. Transaction parameters.
    let to = read_line("Recipient address (0x...): ")?.parse::<Address>()?;
    let amount_eth = read_line("Amount (ETH): ")?.parse::<f64>()?;
    let amount_wei = U256::from((amount_eth * 1e18) as u128);
    let gas_limit = U256::from(21000);

    // 4. RLP for EIP-155 signing: rlp([nonce, gasPrice, gas, to, value, data, chainId, 0, 0]).
    let mut signing_rlp = RlpStream::new_list(9);
    signing_rlp.append(&nonce);
    signing_rlp.append(&gas_price);
    signing_rlp.append(&gas_limit);
    signing_rlp.append(&to.as_bytes());
    signing_rlp.append(&amount_wei);
    signing_rlp.append(&Bytes::default().as_ref());
    signing_rlp.append(&U64::from(CHAIN_ID));
    signing_rlp.append(&U256::zero());
    signing_rlp.append(&U256::zero());
    let signing_payload = signing_rlp.out();

    // 5. Keccak256 hash, then sign on the YubiKey (prompts for PIN).
    let tx_hash: [u8; 32] = Keccak256::digest(&signing_payload).into();
    println!("Signing tx hash {} on YubiKey...", hex::encode(tx_hash));
    let card_sig = sign(account, &tx_hash)?; // bare R || S (64 bytes)
    let rs: [u8; 64] = card_sig
        .as_slice()
        .try_into()
        .map_err(|_| format!("expected 64-byte R||S, got {} bytes", card_sig.len()))?;

    // 6. Recover v, normalize to low-S, and assemble the signed transaction.
    let signed = eth::ethereum_signature(&pubkey, &tx_hash, &rs, CHAIN_ID)?;
    let mut tx_rlp = RlpStream::new_list(9);
    tx_rlp.append(&nonce);
    tx_rlp.append(&gas_price);
    tx_rlp.append(&gas_limit);
    tx_rlp.append(&to.as_bytes());
    tx_rlp.append(&amount_wei);
    tx_rlp.append(&Bytes::default().as_ref());
    tx_rlp.append(&signed.v);
    tx_rlp.append(&U256::from_big_endian(&signed.r));
    tx_rlp.append(&U256::from_big_endian(&signed.s));
    let raw_tx = tx_rlp.out().freeze();
    println!("Signed raw tx: 0x{}", hex::encode(&raw_tx));

    // 7. Broadcast.
    match provider.send_raw_transaction(raw_tx.into()).await {
        Ok(pending) => println!("Broadcast! tx hash: {:?}", pending.tx_hash()),
        Err(e) => println!("Broadcast failed: {e}"),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let account = match eth_account(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Invalid account selection: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = ethereum_transaction_with_yubikey(&account).await {
        eprintln!("Operation failed: {e}");
        std::process::exit(1);
    }
}
