//! Ethereum transfer signed by a YubiKey secp256k1 key (OpenPGP applet).
//!
//! The chosen OpenPGP slot must hold a **secp256k1** key. If your SIG slot is an
//! Ed25519 key for Solana, put the secp256k1 key in the AUT slot and run with
//! `--slot aut`.

use clap::Parser;
use ethers::core::types::transaction::eip2718::TypedTransaction;
use ethers::core::types::{Address, Signature as EthSig, TransactionRequest, U256};
use ethers::providers::{Http, Middleware, Provider};
use std::error::Error;
use std::io::{self, Write};
use yubisign::{Account, Applet, Curve, eth, parse_slot, sign};

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
    let pubkey = yubisign::get_pubkey(account)?; // 65-byte uncompressed point
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

    // 3. Build a legacy EIP-155 transaction with ethers.
    let to = read_line("Recipient address (0x...): ")?.parse::<Address>()?;
    let amount_wei = U256::from((read_line("Amount (ETH): ")?.parse::<f64>()? * 1e18) as u128);
    let tx: TypedTransaction = TransactionRequest::new()
        .to(to)
        .value(amount_wei)
        .nonce(nonce)
        .gas(21000u64)
        .gas_price(gas_price)
        .chain_id(CHAIN_ID)
        .into();

    // 4. Sign the EIP-155 sighash on the YubiKey (prompts for PIN).
    let tx_hash: [u8; 32] = tx.sighash().0;
    println!("Signing tx hash {} on YubiKey...", hex::encode(tx_hash));
    let rs: [u8; 64] = sign(account, &tx_hash)?
        .as_slice()
        .try_into()
        .map_err(|_| "expected 64-byte R||S")?;

    // 5. Recover v / low-S, assemble the signed tx with ethers, broadcast.
    let signed = eth::ethereum_signature(&pubkey, &tx_hash, &rs, CHAIN_ID)?;
    let eth_sig = EthSig {
        r: U256::from_big_endian(&signed.r),
        s: U256::from_big_endian(&signed.s),
        v: signed.v,
    };
    let raw = tx.rlp_signed(&eth_sig);
    println!("Signed raw tx: 0x{}", hex::encode(&raw));

    match provider.send_raw_transaction(raw).await {
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
