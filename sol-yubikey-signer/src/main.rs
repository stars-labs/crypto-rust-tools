use pcsc::*;
use std::error::Error;
// Add library for reading PIN from stdin
use base64::Engine;
use hex;
use sha2::{Digest, Sha256};
use simple_asn1::{ASN1Block, from_der};
use solana_sdk::{
    message::Message, pubkey::Pubkey, signature::Signature, system_instruction,
    transaction::Transaction,
};
use std::io::{self, Write}; // <-- add this import

// OpenPGP Application ID (AID)
const OPENGPG_AID_HEX: &str = "D27600012401";

// --- APDU Constants (From OpenPGP Card Spec v3.4.1) ---
// SELECT Application
const INS_SELECT: u8 = 0xA4;
// VERIFY PIN
const INS_VERIFY: u8 = 0x20;
const P2_VERIFY_USER_PIN: u8 = 0x81; // User PIN / PW1 / PIN2
// Use 0x81 for PW1/PIN1 (older spec/compat), 0x83 for PW3/Admin PIN
// PERFORM SECURITY OPERATION (PSO) - Compute Digital Signature
const CLA_GP: u8 = 0x00; // GlobalPlatform / ISO / Default
const INS_PSO: u8 = 0x2A; // PERFORM SECURITY OPERATION
const P1_SIGN: u8 = 0x9E; // Reference of a key / Sign hash
const P2_COMPUTE_SIGNATURE: u8 = 0x9A; // Compute digital signature

// APDU for GET PUBLIC KEY (OpenPGP, see spec 7.2.11)
const INS_GET_PUBLIC_KEY: u8 = 0x47;
const P1_SIG_KEY: u8 = 0x81; // 0x81 = signature key slot
const P2_GET_PUBKEY: u8 = 0x00;

// --- Solana RPC URL (easy to switch between testnet/mainnet) ---
const SOLANA_RPC_URL: &str = "https://api.testnet.solana.com";

// --- Helper Function ---
/// Sends an APDU command to the card and returns the response data.
/// Checks for the standard success status word (90 00) and common PIN errors.
fn send_apdu<'a>(
    card: &Card,
    description: &str,
    cla: u8,
    ins: u8,
    p1: u8,
    p2: u8,
    data: Option<&[u8]>,
    le: Option<u8>,                // Use Option<u8> for Le, often 0x00 for max length
    response_buffer: &'a mut [u8], // Buffer to store the response
) -> Result<&'a [u8], Box<dyn Error>> {
    let mut command = Vec::new();
    command.extend_from_slice(&[cla, ins, p1, p2]);

    match data {
        Some(d) => {
            if d.len() > 255 {
                // Basic APDU format only supports Lc up to 255.
                // Extended length APDUs are more complex.
                return Err("Data field too long for simple APDU (max 255 bytes)".into());
            }
            command.push(d.len() as u8); // Lc
            command.extend_from_slice(d);
        }
        None => {
            // No data field, Lc is implicitly 0.
            // If Le is present, it might replace an explicit Lc=0 byte depending on case.
            // For simplicity, we always add Lc=0 if no data, unless Le is specified *and* takes its place.
            if le.is_none() {
                command.push(0x00); // Explicit Lc = 0 if no Le specified
            }
        }
    }

    if let Some(le_byte) = le {
        // Append Le byte. In case 4 APDUs (data present), it's appended at the end.
        // In case 2 APDUs (no data), it might replace the Lc=0 byte.
        // This simple logic appends it; works for case 3 & 4. Case 2 might need adjustment
        // if the card expects Le as the 5th byte directly. PCSC drivers often handle this.
        command.push(le_byte);
    }

    println!(">>> Sending APDU: {}", description);
    println!("    Command (Hex): {}", hex::encode(&command));

    // Transmit the command APDU to the card.
    // `transmit` writes the full response (data + sw1 + sw2) into response_buffer
    // and returns a slice `&[u8]` referring to that written part.
    let response_slice = card.transmit(&command, response_buffer)?;

    // The response slice must contain at least the two status bytes.
    if response_slice.len() < 2 {
        return Err("Received response is too short (less than 2 bytes)".into());
    }

    // Extract SW1 and SW2 from the end of the response slice.
    let response_len = response_slice.len();
    let sw1 = response_slice[response_len - 2];
    let sw2 = response_slice[response_len - 1];
    let status = ((sw1 as u16) << 8) | (sw2 as u16);

    // Extract the data part (everything except the last two status bytes).
    let data_slice = &response_slice[..response_len - 2];

    println!(
        "<<< Response Status: {:02X} {:02X} ({:04X})",
        sw1, sw2, status
    );

    // Check status word. 90 00 means success.
    if status == 0x9000 {
        if !data_slice.is_empty() {
            // Only print data if there is any
            println!("    Data (Hex): {}", hex::encode(data_slice));
        }
        Ok(data_slice) // Return only the data part on success
    } else {
        // Provide more specific error messages based on common SW codes
        let error_msg = match status {
            // PIN specific errors (SW1=63)
            0x63C0..=0x63CF => format!("PIN verification failed. Retries left: {}", status & 0x0F),
            0x6983 => {
                "PIN verification failed. Authentication method blocked (PIN Locked)".to_string()
            }
            // General errors
            0x6982 => "Security status not satisfied (e.g., PIN required or wrong PIN context)"
                .to_string(),
            0x6985 => "Conditions of use not satisfied (e.g., key usage not allowed)".to_string(),
            0x6A80 => "Wrong data field parameters (e.g., incorrect PIN length/format)".to_string(),
            0x6A86 => "Incorrect P1/P2 parameters".to_string(),
            0x6A88 => {
                "Referenced data not found (e.g., key invalid or PIN reference wrong)".to_string()
            }
            0x6D00 => "Instruction not supported".to_string(),
            0x6E00 => "Class not supported".to_string(),
            _ => format!("Card returned error status: {:04X}", status),
        };
        println!("    Error: {}", error_msg);
        // Even on error, print any data returned
        if !data_slice.is_empty() {
            println!("    Data (Hex): {}", hex::encode(data_slice));
        }
        Err(error_msg.into())
    }
}

/// Prompts the user for their PIN securely (without echoing).
/// WARNING: Basic implementation. Consider using a dedicated crate for robust terminal handling.
fn get_pin_from_user(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{}", prompt);
    io::stdout().flush()?; // Ensure prompt is displayed before reading input

    // Use rpassword crate for better security if possible, otherwise basic stdin read
    // For simplicity here, we use basic stdin read. Add `rpassword = "5"` to Cargo.toml for the better way.
    // let pin = rpassword::prompt_password("Enter PIN: ")?;
    let mut pin = String::new();
    io::stdin().read_line(&mut pin)?;

    Ok(pin.trim().to_string()) // Trim whitespace (like newline)
}

/// Fetch the public key from the YubiKey (OpenPGP signature key)
fn get_pubkey_from_yubikey() -> Result<Pubkey, Box<dyn Error>> {
    let ctx = Context::establish(Scope::User)?;
    let mut readers_buf = [0; 2048];
    let mut readers = ctx.list_readers(&mut readers_buf)?;
    let reader = readers.next().ok_or("No smart card readers found.")?;
    let card = ctx.connect(reader, ShareMode::Shared, Protocols::T0 | Protocols::T1)?;

    let mut response_buffer = [0u8; 512];

    // 1. Select OpenPGP Application
    let aid_bytes = hex::decode(OPENGPG_AID_HEX)?;
    send_apdu(
        &card,
        "SELECT APP",
        CLA_GP,
        INS_SELECT,
        0x04,
        0x00,
        Some(aid_bytes.as_slice()),
        None,
        &mut response_buffer,
    )?;

    // 2. GET PUBLIC KEY for signature key slot (INS=0x47, CLA=0x00, P1=0x81, P2=0x00, Data=00 02 B6 00 00 00, Lc=0x06)
    response_buffer.fill(0);
    let pubkey_data = {
        // Manually build the APDU to ensure Lc=0x06 and data=00 02 B6 00 00 00
        let mut apdu = vec![
            CLA_GP,             // 0x00
            INS_GET_PUBLIC_KEY, // 0x47
            P1_SIG_KEY,         // 0x81
            P2_GET_PUBKEY,      // 0x00
            0x00,               // Lc = 6
            0x00,
            0x02,
            0xB6,
            0x00,
            0x00,
            0x00, // Data
        ];
        // No Le
        println!(">>> Sending APDU: GET PUBLIC KEY (SIG SLOT)");
        println!("    Command (Hex): {}", hex::encode(&apdu));
        let response_slice = card.transmit(&apdu, &mut response_buffer)?;
        if response_slice.len() < 2 {
            return Err("Received response is too short (less than 2 bytes)".into());
        }
        let response_len = response_slice.len();
        let sw1 = response_slice[response_len - 2];
        let sw2 = response_slice[response_len - 1];
        let status = ((sw1 as u16) << 8) | (sw2 as u16);
        let data_slice = &response_slice[..response_len - 2];
        println!(
            "<<< Response Status: {:02X} {:02X} ({:04X})",
            sw1, sw2, status
        );
        if status == 0x9000 {
            if !data_slice.is_empty() {
                println!("    Data (Hex): {}", hex::encode(data_slice));
            }
            data_slice
        } else {
            let error_msg = match status {
                0x63C0..=0x63CF => {
                    format!("PIN verification failed. Retries left: {}", status & 0x0F)
                }
                0x6983 => "PIN verification failed. Authentication method blocked (PIN Locked)"
                    .to_string(),
                0x6982 => "Security status not satisfied (e.g., PIN required or wrong PIN context)"
                    .to_string(),
                0x6985 => {
                    "Conditions of use not satisfied (e.g., key usage not allowed)".to_string()
                }
                0x6A80 => {
                    "Wrong data field parameters (e.g., incorrect PIN length/format)".to_string()
                }
                0x6A86 => "Incorrect P1/P2 parameters".to_string(),
                0x6A88 => "Referenced data not found (e.g., key invalid or PIN reference wrong)"
                    .to_string(),
                0x6D00 => "Instruction not supported".to_string(),
                0x6E00 => "Class not supported".to_string(),
                _ => format!("Card returned error status: {:04X}", status),
            };
            println!("    Error: {}", error_msg);
            if !data_slice.is_empty() {
                println!("    Data (Hex): {}", hex::encode(data_slice));
            }
            return Err(error_msg.into());
        }
    };

    if pubkey_data.len() < 2 {
        return Err("GET PUBLIC KEY response too short or empty (no public key returned)".into());
    }

    // OpenPGP returns a TLV structure. For Ed25519, look for 0x86 0x20 <32 bytes>
    let key_bytes = if let Some(pos) = pubkey_data.windows(2).position(|w| w == [0x86, 0x20]) {
        &pubkey_data[pos + 2..pos + 2 + 32]
    } else {
        return Err(format!(
            "Could not find Ed25519 public key in response (TLV: {})",
            hex::encode(pubkey_data)
        )
        .into());
    };

    Ok(Pubkey::from(*<&[u8; 32]>::try_from(key_bytes)?))
}

// --- Main Logic ---
fn sign_with_yubikey(hash_bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    // Establish a PC/SC context.
    let ctx = Context::establish(Scope::User)?;

    // List available readers.
    let mut readers_buf = [0; 2048];
    let mut readers = ctx.list_readers(&mut readers_buf)?;

    // Choose the first reader found.
    let reader = match readers.next() {
        Some(reader) => {
            println!("Using reader: {:?}", reader);
            reader
        }
        None => {
            return Err("No smart card readers found.".into());
        }
    };

    // Connect to the card using the chosen reader.
    // Use Shared mode, as typically recommended.
    let card = ctx.connect(reader, ShareMode::Shared, Protocols::T0 | Protocols::T1)?;
    println!("Successfully connected to card.");

    // Buffer for APDU responses. Max signature size + status words.
    // 512 should be sufficient for most common signatures (RSA 2048/4096, ECC P256/P384/P521) + SW
    let mut response_buffer = [0u8; 512];

    // 1. Select the OpenPGP Application
    println!("\n--- Selecting OpenPGP Application ---");
    let aid_bytes = hex::decode(OPENGPG_AID_HEX)?;
    let select_apdu_data = Some(aid_bytes.as_slice());
    match send_apdu(
        &card,
        "SELECT APP",
        CLA_GP,     // CLA=00
        INS_SELECT, // INS=A4
        0x04,       // P1: Select by DF name/AID
        0x00,       // P2: First/Only occurrence
        select_apdu_data,
        None, // No Le needed for SELECT usually
        &mut response_buffer,
    ) {
        Ok(_) => println!("OpenPGP Application Selected Successfully."),
        Err(e) => {
            // card.disconnect drops automatically when card goes out of scope
            return Err(format!("Failed to select OpenPGP Application: {}", e).into());
        }
    }
    // The response data (FCI) is ignored here.

    // 2. Verify User PIN (PW1, context 2)
    println!("\n--- Verifying User PIN ---");
    // !!! SECURITY WARNING: Fetch PIN securely in real applications !!!
    // let user_pin = "123456"; // Example - DO NOT HARDCODE REAL PINS!
    let user_pin = get_pin_from_user("Enter User PIN (PW1/PIN2): ")?;

    let pin_bytes = user_pin.as_bytes();
    // Clear buffer before reuse
    response_buffer.fill(0);
    match send_apdu(
        &card,
        "VERIFY PIN",
        CLA_GP,             // CLA=00
        INS_VERIFY,         // INS=20
        0x00,               // P1=00 (always for VERIFY in OpenPGP spec)
        P2_VERIFY_USER_PIN, // P2=82 (User PIN / PW1 / PIN2)
        Some(pin_bytes),    // Data = PIN bytes
        None,               // No Le needed
        &mut response_buffer,
    ) {
        Ok(_) => println!("PIN Verification Successful."),
        Err(e) => {
            // Handle PIN failure (retries, blocked)
            return Err(format!("PIN Verification Failed: {}", e).into());
        }
    }

    // 3. Send PSO: Compute Digital Signature command
    println!("\n--- Performing PSO: Compute Digital Signature ---");
    let pso_data = Some(hash_bytes);
    // Clear the buffer before reuse
    response_buffer.fill(0);
    let signature_data_slice = send_apdu(
        &card,
        "PSO SIGN",
        CLA_GP,
        INS_PSO,
        P1_SIGN,
        P2_COMPUTE_SIGNATURE,
        pso_data,
        Some(0x00), // Le=0x00: Expect max length response
        &mut response_buffer,
    )?;

    println!("\n--- Signature Computed Successfully ---");
    // We clone the data from the slice into a Vec<u8> because the buffer
    // will be dropped when the function returns.
    let signature_vec = signature_data_slice.to_vec();
    println!("Signature (Hex): {}", hex::encode(&signature_vec));

    // Disconnection happens automatically when `card` goes out of scope due to RAII.
    println!("\nDisconnected from card (automatically).");

    // Return the signature bytes
    Ok(signature_vec)
}

/// ASN.1 DER encoding for Ed25519 signature (RFC 8410 does not use ASN.1 for Ed25519, but Solana expects raw 64 bytes)
/// For ECDSA/secp256k1, Solana expects a 64-byte [r||s] signature, not ASN.1 DER.
/// If your YubiKey returns ASN.1 DER, you must convert it to raw [r||s].
/// If your YubiKey returns raw [r||s], you must use it as is.
/// For Ed25519, Solana expects raw 64 bytes, not ASN.1 DER.
/// For secp256k1, Solana expects raw 64 bytes, not ASN.1 DER.
/// If your YubiKey returns ASN.1 DER, decode it to raw [r||s].

fn der_to_rs(sig: &[u8]) -> Result<[u8; 64], Box<dyn Error>> {
    // Decode ASN.1 DER ECDSA signature to raw [r||s]
    let asn1 = from_der(sig)?;
    if asn1.len() != 1 {
        return Err("ASN.1 signature does not have one top-level element".into());
    }
    if let ASN1Block::Sequence(_, items) = &asn1[0] {
        if items.len() != 2 {
            return Err("ASN.1 signature sequence does not have two elements".into());
        }
        let mut out = [0u8; 64];
        for (i, item) in items.iter().enumerate() {
            if let ASN1Block::Integer(_, v) = item {
                let bytes = v.to_signed_bytes_be();
                if bytes.len() > 32 {
                    return Err("r or s too large".into());
                }
                let offset = 32 * i + (32 - bytes.len());
                out[offset..offset + bytes.len()].copy_from_slice(&bytes);
            } else {
                return Err("ASN.1 signature element is not integer".into());
            }
        }
        Ok(out)
    } else {
        Err("ASN.1 signature is not a sequence".into())
    }
}

/// Helper to read a line from stdin
fn read_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

/// Build a Solana transfer transaction (unsigned)
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

/// Example: Transfer lamports from one account to another, signing with YubiKey
fn solana_transfer_with_yubikey() -> Result<(), Box<dyn Error>> {
    println!("\n--- Solana Transfer (YubiKey Signing) ---");

    // Fetch sender pubkey from YubiKey
    let from_pubkey = get_pubkey_from_yubikey()?;
    println!("Sender pubkey (from YubiKey): {}", from_pubkey);

    // Show balance in testnet before input recipient
    use solana_client::rpc_client::RpcClient;
    let client = RpcClient::new(SOLANA_RPC_URL.to_string());
    match client.get_balance(&from_pubkey) {
        Ok(balance) => println!("Sender balance (testnet): {} lamports", balance),
        Err(e) => println!("Could not fetch sender balance: {}", e),
    }

    // Fetch recent blockhash from API
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

    // Prompt for recipient and amount only
    let to_str = read_line("Recipient pubkey (base58): ")?;
    let lamports_str = read_line("Amount (lamports): ")?;
    let to_pubkey = to_str.parse::<Pubkey>()?;
    let lamports = lamports_str.parse::<u64>()?;

    // Build unsigned transaction
    let mut tx = build_transfer_tx(&from_pubkey, &to_pubkey, lamports, blockhash);

    // Get the message data to sign (NOT the hash)
    let msg_data = tx.message.serialize();
    // let hash = Sha256::digest(&msg_data); // <-- REMOVE or comment out this line

    // --- IMPORTANT: Solana expects Ed25519 signatures (raw 64 bytes) ---
    // The signature should be over the raw message data, not a pre-hash.
    // The YubiKey's OpenPGP Ed25519 implementation should handle hashing internally.

    // Sign the raw message data with YubiKey
    // Pass msg_data directly, NOT the hash
    let signature_bytes = sign_with_yubikey(&msg_data)?;

    // For Ed25519, Solana expects raw 64 bytes. OpenPGP might return 65 bytes (e.g., 0x00 || R || S).
    let signature_slice: &[u8];
    if signature_bytes.len() == 64 {
        println!("Received 64-byte signature from YubiKey.");
        signature_slice = &signature_bytes;
    } else if signature_bytes.len() == 65 {
        println!("Received 65-byte signature from YubiKey, assuming leading byte needs stripping.");
        // Skip the first byte
        signature_slice = &signature_bytes[1..];
    } else {
        println!(
            "Warning: Signature length is {} bytes. Solana expects Ed25519 signatures (raw 64 bytes).",
            signature_bytes.len()
        );
        // Attempt to use it anyway, but conversion below will likely fail
        signature_slice = &signature_bytes;
    }

    // Attach signature to transaction
    let signature = Signature::from(<[u8; 64]>::try_from(signature_slice)?);
    tx.signatures = vec![signature];

    // Verify signature locally (checks if the signature format is valid and associated with the pubkey)
    // Note: This local check might pass even if the signature is wrong but structurally valid.
    // The real test is broadcasting to the network.
    if !tx.verify_with_results().iter().all(|&res| res) {
        // Use verify_with_results for potentially more detailed info if needed later
        // For now, just check if all results are true (Ok)
        return Err("Local signature verification failed!".into());
    }
    println!("Local signature verification successful.");

    // Broadcast transaction
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

// --- Execution ---
fn main() {
    println!("Attempting to sign a Solana transfer using YubiKey/OpenPGP card...");

    if let Err(e) = solana_transfer_with_yubikey() {
        eprintln!("\nSolana transfer failed: {}", e);
        std::process::exit(1);
    }
}
