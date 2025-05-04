use curve25519_dalek::Scalar; // <-- Import directly from the crate
use frost_core::keys::{
    IdentifierList, KeyPackage, PublicKeyPackage, SigningShare, generate_with_dealer,
};
use frost_core::{Identifier, SigningPackage, round1, round2};
use frost_ed25519::Ed25519Sha512;
use frost_ed25519::rand_core::OsRng;
use hex;
use std::collections::{BTreeMap, HashMap}; // <-- Add BTreeMap
use std::error::Error;
use std::fs::File;
use std::io::BufRead;
use std::io::{self, BufWriter, Read, Write, stdin};
use std::path::Path;
use std::str::FromStr;

fn generate_keys(
    total_participants: usize,
    threshold: usize,
) -> Result<(Vec<String>, Vec<u8>), Box<dyn Error>> {
    // Return PublicKeyPackage bytes
    // If keystore exists, load and return keys and group pubkey package
    let keystore_path = "keys.txt";
    let pubkey_pkg_path = "group_pubkey_pkg.txt"; // <-- Changed filename
    if Path::new(keystore_path).exists() && Path::new(pubkey_pkg_path).exists() {
        let file = File::open(keystore_path)?;
        let reader = io::BufReader::new(file);
        let keys: Vec<String> = reader.lines().filter_map(Result::ok).collect();
        let mut pubkey_pkg_bytes = Vec::new(); // <-- Changed variable name
        File::open(pubkey_pkg_path)?.read_to_end(&mut pubkey_pkg_bytes)?; // <-- Read pkg bytes
        println!("Loaded keys from {}: {:?}", keystore_path, keys);
        println!(
            "Loaded group pubkey package from {}: {}", // <-- Updated message
            pubkey_pkg_path,
            hex::encode(&pubkey_pkg_bytes) // <-- Log pkg bytes
        );
        return Ok((keys, pubkey_pkg_bytes)); // <-- Return pkg bytes
    }

    println!(
        "Generating keys for {} participants with threshold {}",
        total_participants, threshold
    );
    let mut rng = OsRng;
    // Use FROST to generate secret shares and group pubkey package
    let (shares, pubkey_pkg) = generate_with_dealer::<Ed25519Sha512, _>(
        total_participants as u16,
        threshold as u16,
        IdentifierList::Default,
        &mut rng,
    )?;
    // Convert the secret shares to hex strings for display
    let keys: Vec<String> = shares
        .values()
        .map(|share| hex::encode(share.signing_share().serialize()))
        .collect();
    println!("Keys generated: {:?}", keys);

    // Extract group public key package bytes
    let group_pubkey_pkg_bytes = pubkey_pkg.serialize()?; // <-- Serialize the whole package
    println!(
        "Group public key package: {}",
        hex::encode(&group_pubkey_pkg_bytes)
    ); // <-- Log pkg bytes

    // Save keys and group pubkey package to files for future tests
    let file = File::create(keystore_path)?;
    let mut writer = BufWriter::new(file);
    for key in &keys {
        writeln!(writer, "{}", key)?;
    }
    let mut pubkey_file = File::create(pubkey_pkg_path)?; // <-- Use new filename
    pubkey_file.write_all(&group_pubkey_pkg_bytes)?; // <-- Write pkg bytes
    println!("Keys saved to keys.txt");
    println!("Group public key package saved to group_pubkey_pkg.txt"); // <-- Updated message

    Ok((keys, group_pubkey_pkg_bytes)) // <-- Return pkg bytes
}

// Removed unused sign_transaction function

fn main() -> Result<(), Box<dyn Error>> {
    println!("Starting the Solana demo process");

    let total_participants = 3;
    let threshold = 2;
    // Load key shares and the serialized PublicKeyPackage
    let (key_share_strings, group_pubkey_pkg_bytes) = generate_keys(total_participants, threshold)?;

    // --- Deserialize Keys and Pubkey Package ---
    // Deserialize the PublicKeyPackage first
    let group_public_key_package =
        PublicKeyPackage::<Ed25519Sha512>::deserialize(&group_pubkey_pkg_bytes)?;
    let group_verifying_key = group_public_key_package.verifying_key(); // Get VerifyingKey from package
    println!("Deserialized Group Public Key Package");

    let mut key_packages = HashMap::new(); // Keep as HashMap for initial loading convenience
    for (i, key_hex) in key_share_strings.iter().enumerate() {
        // Convert index to Scalar, serialize, then deserialize Identifier
        let participant_index = i as u16 + 1; // Removed parentheses
        let scalar = Scalar::from(participant_index); // Convert u16 index to Scalar
        let identifier_bytes = scalar.to_bytes(); // Serialize Scalar to 32 bytes
        let identifier = Identifier::<Ed25519Sha512>::deserialize(&identifier_bytes)?; // Deserialize Identifier from scalar bytes

        let key_bytes = hex::decode(key_hex)?;
        // Use frost_core::keys::SigningShare
        let signing_share = SigningShare::deserialize(&key_bytes)?;

        // Get the VerifyingShare from the PublicKeyPackage's map
        let verifying_share = group_public_key_package
            .verifying_shares()
            .get(&identifier)
            .cloned() // Clone the share from the map
            .ok_or_else(|| format!("VerifyingShare not found for identifier {:?}", identifier))?; // Use {:?} for Identifier

        // Use frost_core::keys::KeyPackage::new with 5 arguments
        let key_package = KeyPackage::new(
            identifier, // Identifier is moved here
            signing_share,
            verifying_share,      // Use the retrieved VerifyingShare
            *group_verifying_key, // Pass the VerifyingKey from the package
            threshold as u16,
        );
        // We need the identifier again for the map key, so clone it before moving to KeyPackage::new
        key_packages.insert(identifier.clone(), key_package); // Clone identifier for map key
        println!("Loaded KeyPackage for identifier {:?}", identifier); // Use {:?} for Identifier
    }
    // --- End Deserialize ---


    // Prompt for target address
    print!("Enter target Solana address: ");
    io::stdout().flush()?;
    let mut target_address_str = String::new();
    stdin().read_line(&mut target_address_str)?;
    let target_address = Pubkey::from_str(target_address_str.trim())?;

    // Prompt for amount in lamports
    print!("Enter amount to transfer in lamports: ");
    io::stdout().flush()?;
    let mut amount_str = String::new();
    stdin().read_line(&mut amount_str)?;
    let amount: u64 = amount_str.trim().parse()?;

    println!(
        "Preparing transaction to transfer {} lamports to {}",
        amount, target_address
    );

    let rpc_url = "https://api.testnet.solana.com";
    let rpc_client =
        RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());

    println!("Fetching recent blockhash...");
    let recent_blockhash = rpc_client.get_latest_blockhash()?;
    println!("Recent blockhash: {}", recent_blockhash); // Log the fetched blockhash

    let message = Message::new(
        &[transfer(&solana_pubkey, &target_address, amount)],
        Some(&solana_pubkey),
    );

    let mut transaction = Transaction::new_unsigned(message);
    transaction.message.recent_blockhash = recent_blockhash; // Assign the fetched blockhash

    let message_bytes = transaction.message.serialize();
    println!(
        "Message to be signed (hex): {}",
        hex::encode(&message_bytes)
    );

    // --- Simplified FROST Signing Simulation ---
    println!(
        "\n--- Starting FROST Signing Simulation ({} participants, threshold {}) ---",
        total_participants, threshold
    );
    let mut rng = OsRng;
    // Select participants - convert HashMap to Vec<&KeyPackage> for iteration
    let signing_participants_keys: Vec<&KeyPackage<Ed25519Sha512>> =
        key_packages.values().take(threshold).collect();

    // Round 1: Generate nonces and commitments
    let mut nonces = HashMap::new(); // Keep nonces in HashMap for easy lookup by identifier later
    // Commitments need to be collected into a BTreeMap for SigningPackage
    let mut commitments_map = BTreeMap::new();
    println!("Round 1: Generating nonces and commitments...");
    for participant_key_package in &signing_participants_keys {
        let identifier = participant_key_package.identifier();
        let (nonce, commitment) = round1::commit(participant_key_package.signing_share(), &mut rng);
        nonces.insert(identifier.clone(), nonce); // Clone identifier for nonce map key
        commitments_map.insert(identifier.clone(), commitment); // Clone identifier for commitment map key
        println!("  Participant {:?} generated nonce commitment.", identifier); // Use {:?} for Identifier
    }

    // (Simulated Broadcast: In reality, commitments are exchanged here)
    // Use frost_core::SigningPackage - requires BTreeMap<Identifier, ...>
    let signing_package = SigningPackage::new(commitments_map, &message_bytes);
    // Use signing_commitments() method
    println!(
        "Signing Package created with {} commitments.",
        signing_package.signing_commitments().len()
    );

    // Round 2: Generate partial signatures
    // Signature shares need to be collected into a BTreeMap for aggregation
    let mut signature_shares_map = BTreeMap::new();
    println!("Round 2: Generating partial signatures...");
    for participant_key_package in &signing_participants_keys {
        let participant_identifier = participant_key_package.identifier();
        // Ensure nonce exists before unwrapping
        // Use the cloned identifier from the nonces map key if needed, or clone participant_identifier
        let participant_nonce = nonces.remove(participant_identifier).ok_or_else(|| {
            // Use participant_identifier directly here for removal
            format!(
                "Nonce not found for participant {:?}",
                participant_identifier
            )
        })?; // Use {:?} for Identifier

        // Pass the whole KeyPackage to round2::sign
        let signature_share = round2::sign(
            &signing_package,
            &participant_nonce,
            participant_key_package, // Pass the KeyPackage reference
        )?;
        // Serialize the SignatureShare before printing
        let share_bytes = signature_share.serialize(); // Removed '?' operator
        println!(
            "  Participant {:?} generated partial signature: {}",
            participant_identifier,
            hex::encode(share_bytes)
        ); // Use {:?} for Identifier
        signature_shares_map.insert(participant_identifier.clone(), signature_share); // Clone identifier for share map key
    }

    // Aggregation
    println!("Aggregating partial signatures...");
    // Pass the BTreeMap<Identifier, ...> of signature shares to aggregate
    let group_signature = frost_ed25519::aggregate(
        &signing_package,
        &signature_shares_map, // Pass the BTreeMap
        &group_public_key_package,
    )?;
    println!("Aggregation successful!");

    let group_signature_bytes = group_signature.serialize()?;
    println!(
        "Final aggregated Ed25519 signature ({} bytes): {}",
        group_signature_bytes.len(),
        hex::encode(&group_signature_bytes)
    );

    // --- End FROST Signing Simulation ---

    // Add the aggregated signature to the transaction
    // Construct Solana Signature from bytes (must be 64 bytes for Ed25519)
    let solana_signature = Signature::try_from(group_signature_bytes).map_err(|e| {
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to create Solana signature: {:?}", e),
        ))
    })?;
    // Replace the placeholder signature with the real one
    if !transaction.signatures.is_empty() {
        transaction.signatures[0] = solana_signature; // Replace the first (fee payer) signature
    } else {
        // This case shouldn't happen with new_unsigned, but handle defensively
        transaction.signatures.push(solana_signature);
    }
    println!("Aggregated signature added to Solana transaction.");

    // NOTE: Sending this transaction will likely fail because the network
    // cannot verify a signature made with a FROST group public key directly
    // unless a specific program/verifier exists on-chain that understands FROST.
    // This demonstrates generating a valid Ed25519 signature using FROST.
    println!("\n--- Transaction Prepared ---");
    println!("Transaction details: {:?}", transaction);
    println!("Attempting to send (expected to fail verification on-chain)...");

    match rpc_client.send_and_confirm_transaction_with_spinner(&transaction) {
        Ok(sig) => {
            println!("Transaction successfully sent! Signature: {}", sig);
            println!(
                "(Note: Success here might indicate the testnet didn't fully verify, or used a different verification path. Standard Solana verification would fail.)"
            );
        }
        Err(e) => {
            println!("Transaction failed to send or confirm (as expected): {}", e);
        }
    }

    println!("\nProcess completed.");
    Ok(())
}
