use ethers::core::types::{Address, Bytes, U64, U256}; // Removed NameOrAddress, TransactionRequest
use ethers::providers::{Http, Middleware, Provider};
use hex;
use rlp::RlpStream;
use sha3::{Digest, Keccak256};
// use std::convert::TryFrom; // Removed unused import
use std::error::Error;
use std::io::{self, Write};
use yubikey_crpyto::{get_pubkey_from_yubikey, sign_with_yubikey};

// Replace with your desired Ethereum RPC URL (e.g., Infura, Alchemy, or a local node)
// Using Sepolia testnet as an example
const ETHEREUM_RPC_URL: &str = "https://sepolia.infura.io/v3/YOUR_INFURA_PROJECT_ID"; // PLEASE REPLACE!
const CHAIN_ID: u64 = 11155111; // Sepolia chain ID

fn read_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

async fn ethereum_transaction_with_yubikey() -> Result<(), Box<dyn Error>> {
    if ETHEREUM_RPC_URL.contains("YOUR_INFURA_PROJECT_ID") {
        eprintln!(
            "Please replace 'YOUR_INFURA_PROJECT_ID' in ETHEREUM_RPC_URL with your actual Infura project ID."
        );
        return Err("RPC URL not configured".into());
    }

    // 1. Fetch secp256k1 public key from YubiKey
    let yubikey_secp256k1_pubkey_bytes = get_pubkey_from_yubikey()?;
    println!(
        "secp256k1 Public Key from YubiKey (hex): {}",
        hex::encode(yubikey_secp256k1_pubkey_bytes)
    );

    // 2. Derive a "pseudo" Ethereum address from the secp256k1 public key
    // WARNING: This is NOT how standard Ethereum addresses are derived.
    // Standard Ethereum addresses come from secp256k1 public keys.
    // This is purely for demonstration within this example's context.
    let mut hasher = Keccak256::new();
    hasher.update(yubikey_secp256k1_pubkey_bytes);
    let pubkey_hash = hasher.finalize();
    let pseudo_eth_address = Address::from_slice(&pubkey_hash[12..]); // Last 20 bytes
    println!(
        "Pseudo Ethereum Address (derived from secp256k1 PubKey): {:x}",
        pseudo_eth_address
    );

    // 3. Connect to Ethereum node
    let provider = Provider::<Http>::try_from(ETHEREUM_RPC_URL)?;
    println!("Connected to Ethereum RPC: {}", ETHEREUM_RPC_URL);

    // 4. Get account details (balance, nonce) for the pseudo address
    // Note: This pseudo address will likely have 0 balance and nonce unless funded.
    let balance = provider.get_balance(pseudo_eth_address, None).await?;
    println!("Balance of pseudo address: {} Wei", balance);
    let nonce = provider
        .get_transaction_count(pseudo_eth_address, None)
        .await?;
    println!("Nonce for pseudo address: {}", nonce);

    // 5. Get gas price
    let gas_price = provider.get_gas_price().await?;
    println!("Current gas price: {} Wei", gas_price);

    // 6. Get user input for transaction
    let to_address_str = read_line("Recipient Ethereum address (hex, e.g., 0x...): ")?;
    let to_address = to_address_str.parse::<Address>()?;
    let amount_eth_str = read_line("Amount (ETH): ")?;
    let amount_eth = amount_eth_str.parse::<f64>()?;
    let amount_wei = U256::from((amount_eth * 1e18) as u128); // Simplified conversion

    // 7. Build unsigned EIP-155 transaction
    // For a simple ETH transfer, gas limit is typically 21000
    let gas_limit = U256::from(21000);

    // RLP encoding for EIP-155 signing:
    // rlp([nonce, gasPrice, gasLimit, to, value, data, chainId, 0, 0])
    let mut rlp_stream = RlpStream::new_list(9);
    rlp_stream.append(&nonce);
    rlp_stream.append(&gas_price);
    rlp_stream.append(&gas_limit);
    rlp_stream.append(&to_address.as_bytes());
    rlp_stream.append(&amount_wei);
    rlp_stream.append(&Bytes::default().as_ref()); // Empty data for ETH transfer
    rlp_stream.append(&U64::from(CHAIN_ID));
    rlp_stream.append(&U256::zero()); // Placeholder for r
    rlp_stream.append(&U256::zero()); // Placeholder for s

    let unsigned_tx_rlp = rlp_stream.out();
    println!(
        "RLP encoded unsigned tx (for signing): {}",
        hex::encode(&unsigned_tx_rlp)
    );

    // 8. Hash the RLP-encoded transaction (Keccak256)
    let mut tx_hasher = Keccak256::new();
    tx_hasher.update(&unsigned_tx_rlp);
    let tx_hash = tx_hasher.finalize();
    println!(
        "Keccak256 Hash of tx (to be signed): {}",
        hex::encode(tx_hash)
    );

    // 9. Sign the hash with YubiKey (secp256k1 signature)
    // This will prompt for PIN via the yubikey-secp256k1-crpyto library
    println!("Please verify on YubiKey and enter PIN if prompted...");
    let secp256k1_signature_bytes = sign_with_yubikey(tx_hash.as_slice())?;
    println!(
        "Raw secp256k1 Signature from YubiKey (hex): {}",
        hex::encode(&secp256k1_signature_bytes)
    );
    println!(
        "WARNING: This is an secp256k1 signature. It is NOT a valid secp256k1 ECDSA signature required by Ethereum."
    );

    // 10. Constructing and sending the transaction (Conceptual - will fail on Ethereum)
    // An Ethereum signature consists of r, s, and v.
    // secp256k1 signature is typically 64 bytes (R || S). We cannot directly get Ethereum's v, r, s.
    // For a real Ethereum transaction, you would use a secp256k1 key and the signature
    // would be used to populate a TransactionRequest or a signed raw transaction.

    // Example of how a signed transaction *would* be formed if we had a valid secp256k1 sig:
    // let r = U256::from_big_endian(&secp256k1_sig[0..32]);
    // let s = U256::from_big_endian(&secp256k1_sig[32..64]);
    // let v = calculate_v_for_eip155(recovery_id, CHAIN_ID); // recovery_id from secp256k1 sig
    //
    // let tx = TransactionRequest::new()
    //     .to(to_address)
    //     .value(amount_wei)
    //     .nonce(nonce)
    //     .gas_price(gas_price)
    //     .gas(gas_limit)
    //     .chain_id(CHAIN_ID)
    //     .with_signature(ethers::core::types::Signature { r, s, v: U64::from(v) });
    //
    // let raw_tx_bytes = tx.rlp_signed();
    // println!("Attempting to send (this will likely be rejected by the node due to invalid signature type)...");
    // match provider.send_raw_transaction(raw_tx_bytes).await {
    //     Ok(pending_tx) => {
    //         println!("Transaction submitted to mempool (but likely invalid): {:?}", pending_tx.tx_hash());
    //         println!("Waiting for confirmation (will likely not confirm)...");
    //         // pending_tx.await?; // This would wait for confirmation
    //         // println!("Transaction confirmed!");
    //     }
    //     Err(e) => {
    //         println!("Failed to send transaction: {}", e);
    //         println!("This is expected because the secp256k1 signature is not valid for Ethereum.");
    //     }
    // }

    println!("\n--- End of Ethereum YubiKey secp256k1 Signing DEMO ---");
    println!("To interact with Ethereum, a YubiKey with a secp256k1 key (e.g., PIV applet)");
    println!("and a compatible library for Ethereum signing would be required.");

    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(e) = ethereum_transaction_with_yubikey().await {
        eprintln!("Operation failed: {}", e);
        std::process::exit(1);
    }
}
