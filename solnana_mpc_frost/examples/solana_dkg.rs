use bincode::serde::{decode_from_slice, encode_to_vec};
use frost::Identifier; // Keep only the needed import
use frost::rand_core::OsRng;
use frost_core::keys::dkg::{round1, round2}; // Keep only needed modules
use frost_core::{round1 as frost_round1, round2 as frost_round2}; // Import signing rounds
use frost_ed25519 as frost; // Alias for convenience
use frost_ed25519::Ed25519Sha512; // Import directly to use as generic parameter
use serde::{Deserialize, Serialize};
// Removed unused Solana imports: RpcClient, CommitmentConfig, Message, Signature, transfer, Transaction
// Removed unused curve25519_dalek::Scalar import
use frost_core::SigningPackage;
use frost_core::keys::{KeyPackage, PublicKeyPackage}; // Keep only KeyPackage, add PublicKeyPackage
// Removed unused IdentifierList, SigningShare, generate_with_dealer
use hex;
use solana_client::rpc_client::RpcClient;

use solana_sdk::{
    commitment_config::CommitmentConfig, message::Message, pubkey::Pubkey, signature::Signature,
    system_instruction::transfer, transaction::Transaction,
};
use std::collections::BTreeMap; // Removed HashMap
use std::convert::TryInto;
use std::env;
use std::error::Error;
use std::fs::{self}; // Removed File
// Removed unused BufRead import
use std::io::{self, Read, Write, stdin}; // Removed unused BufWriter
use std::net::{TcpListener, TcpStream};
// Removed unused Path import
use std::str::FromStr;
use std::thread;
use std::time::Duration;

// --- DKG Message Types ---
#[derive(Serialize, Deserialize, Clone)] // Add Clone
struct Round1Message {
    participant_index: u16,
    package: round1::Package<Ed25519Sha512>,
}

#[derive(Serialize, Deserialize, Clone)] // Add Clone
struct Round2Message {
    participant_index: u16,
    package: round2::Package<Ed25519Sha512>,
}

// --- Signing Message Types ---
#[derive(Serialize, Deserialize, Clone)]
struct TxMessage {
    message_bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CommitmentMessage {
    sender_identifier: Identifier, // Removed <Ed25519Sha512>
    commitment: frost_round1::SigningCommitments<Ed25519Sha512>,
}

#[derive(Serialize, Deserialize, Clone)]
struct ShareMessage {
    sender_identifier: Identifier, // Removed <Ed25519Sha512>
    share: frost_round2::SignatureShare<Ed25519Sha512>,
}

// --- New Message Type for Aggregated Signature ---
#[derive(Serialize, Deserialize, Clone)]
struct AggregatedSignatureMessage {
    signature_bytes: Vec<u8>,
}

// --- Generic Message Wrapper ---
#[derive(Serialize, Deserialize, Clone)]
enum MessageWrapper {
    DkgRound1(Round1Message),
    DkgRound2(Round2Message),
    SignTx(TxMessage),
    SignCommitment(CommitmentMessage),
    SignShare(ShareMessage),
    SignAggregated(AggregatedSignatureMessage), // New variant
}

fn parse_args() -> (u16, u16, u16, bool) {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 || args.len() > 6 {
        eprintln!(
            "Usage: {} <index> <total> <threshold> [--isInitiator <true|false>]",
            args[0]
        );
        std::process::exit(1);
    }
    let index = args[1].parse().expect("Invalid index");
    let total = args[2].parse().expect("Invalid total");
    let threshold = args[3].parse().expect("Invalid threshold");
    let mut is_initiator = false;
    if args.len() == 6 {
        if args[4] == "--isInitiator" {
            is_initiator = match args[5].as_str() {
                "true" => true,
                "false" => false,
                _ => {
                    eprintln!("Expected --isInitiator <true|false>");
                    std::process::exit(1);
                }
            };
        } else {
            eprintln!(
                "Usage: {} <index> <total> <threshold> [--isInitiator <true|false>]",
                args[0]
            );
            std::process::exit(1);
        }
    }
    (index, total, threshold, is_initiator)
}

// Simple send/recv helpers for TCP
fn send_to(addr: &str, msg: &MessageWrapper) {
    let data = encode_to_vec(msg, bincode::config::standard()).unwrap();
    let mut stream = loop {
        match TcpStream::connect(addr) {
            Ok(s) => break s,
            Err(_e) => {
                // Mark e as unused
                // Add a small delay and print error if connection fails immediately
                // eprintln!("Failed to connect to {}: {}. Retrying...", addr, e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    };
    stream.write_all(&data).unwrap();
}

// Broadcast to all peers except self
fn broadcast(index: u16, total: u16, msg: &MessageWrapper) {
    println!("Node {} broadcasting message...", index);
    for peer_idx in 1..=total {
        if peer_idx == index {
            continue;
        }
        let peer_addr = format!("127.0.0.1:1000{}", peer_idx);
        send_to(&peer_addr, msg);
    }
}

// Receive a specific number of messages of a certain type
fn receive_messages<T>(
    listener: &TcpListener,
    count: usize,
    extract_fn: fn(MessageWrapper) -> Option<T>,
) -> Result<Vec<T>, Box<dyn Error>> {
    let mut messages = Vec::with_capacity(count);
    let mut received_count = 0; // Track received messages matching the type
    while received_count < count {
        let (mut stream, addr) = listener.accept()?;
        println!("Accepted connection from {}", addr);
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?; // Read entire message
        if buf.is_empty() {
            println!("Received empty message from {}", addr);
            continue; // Or handle error
        }
        // Extract the message (.0) from the tuple returned by decode_from_slice
        let wrapped_msg: MessageWrapper = match decode_from_slice(&buf, bincode::config::standard())
        {
            Ok((msg, _)) => msg,
            Err(e) => {
                eprintln!("Failed to decode message from {}: {}", addr, e);
                continue; // Skip malformed messages
            }
        };

        if let Some(msg) = extract_fn(wrapped_msg.clone()) {
            // Clone wrapped_msg here
            messages.push(msg);
            received_count += 1;
        } else {
            // Handle unexpected but valid message types if necessary
            // e.g., log it, store it for later, or ignore it
            match wrapped_msg {
                MessageWrapper::SignAggregated(_) => {
                    println!(
                        "Node {} received aggregated signature (ignoring for now)",
                        addr
                    );
                }
                _ => {
                    eprintln!("Received unexpected message type from {}", addr);
                    // Decide if this should be a hard error or just ignored
                    // return Err("Received unexpected message type".into());
                }
            }
        }
    }
    Ok(messages)
}

// Change main return type to Result
fn main() -> Result<(), Box<dyn Error>> {
    let (index, total, threshold, is_initiator) = parse_args();
    let my_addr = format!("127.0.0.1:1000{}", index);
    let mut rng = OsRng;

    // My Identifier
    let my_identifier = Identifier::try_from(index).expect("Invalid identifier");

    // --- Key Loading/Generation ---
    let key_package_file = format!("key_package_{}.bin", index);
    let pubkey_package_file = format!("pubkey_package_{}.bin", index); // Same for all nodes, but generate per node for simplicity

    let (key_package, pubkey_package): (
        KeyPackage<Ed25519Sha512>,
        PublicKeyPackage<Ed25519Sha512>,
    ) = if fs::metadata(&key_package_file).is_ok() && fs::metadata(&pubkey_package_file).is_ok() {
        println!("Node {} loading keys from cache...", index);
        let key_bytes = fs::read(&key_package_file)?;
        let pubkey_bytes = fs::read(&pubkey_package_file)?;

        let kp: KeyPackage<Ed25519Sha512> =
            decode_from_slice(&key_bytes, bincode::config::standard())?.0; // decode_from_slice returns (T, usize)
        let pkp: PublicKeyPackage<Ed25519Sha512> =
            decode_from_slice(&pubkey_bytes, bincode::config::standard())?.0;

        println!("Node {} loaded keys successfully.", index);
        (kp, pkp)
    } else {
        println!("Node {} generating new keys via DKG...", index);
        // Listen for incoming connections
        let listener = TcpListener::bind(&my_addr).expect("Failed to bind");
        println!("Node {} listening on {} for DKG", index, my_addr);

        // DKG Round 1
        println!("Node {} starting DKG Round 1...", index);
        let (round1_secret_package, round1_package) =
            frost::keys::dkg::part1(my_identifier, total, threshold, &mut rng)
                .expect("DKG part 1 failed");

        let round1_message = Round1Message {
            participant_index: index,
            package: round1_package.clone(),
        };
        let wrapped_r1_msg = MessageWrapper::DkgRound1(round1_message);

        broadcast(index, total, &wrapped_r1_msg);

        // Receive Round 1 Packages
        println!("Node {} receiving DKG Round 1 packages...", index);
        let received_r1_messages =
            receive_messages(&listener, (total - 1) as usize, |msg| match msg {
                MessageWrapper::DkgRound1(m) => Some(m),
                _ => None,
            })?;

        let mut received_round1_packages = BTreeMap::new();
        received_round1_packages.insert(my_identifier, round1_package); // Add self
        for msg in received_r1_messages {
            let participant_id = Identifier::try_from(msg.participant_index)?;
            received_round1_packages.insert(participant_id, msg.package);
            println!(
                "Node {} received DKG R1 package from participant {}",
                index, msg.participant_index
            );
        }

        if received_round1_packages.len() != total as usize {
            return Err(format!(
                "Error: Incorrect number of Round 1 packages. Got {}, expected {}",
                received_round1_packages.len(),
                total
            )
            .into());
        }
        println!("Node {} finished receiving DKG Round 1 packages.", index);

        // DKG Round 2
        println!("Node {} starting DKG Round 2...", index);
        let received_round1_packages_from_others: BTreeMap<_, _> = received_round1_packages
            .iter()
            .filter(|(id_ref, _)| **id_ref != my_identifier)
            .map(|(id, pkg)| (*id, pkg.clone()))
            .collect();

        let (round2_secret_package, round2_packages) =
            frost::keys::dkg::part2(round1_secret_package, &received_round1_packages_from_others)
                .expect("DKG part 2 failed");

        // Send Round 2 Packages
        println!("Node {} sending DKG Round 2 packages...", index);
        for (receiver_id, package) in round2_packages {
            let id_bytes = receiver_id.serialize();
            let receiver_idx = u16::from_le_bytes(id_bytes[0..2].try_into()?);
            let round2_message = Round2Message {
                participant_index: index,
                package,
            };
            let wrapped_r2_msg = MessageWrapper::DkgRound2(round2_message);
            let peer_addr = format!("127.0.0.1:1000{}", receiver_idx);
            send_to(&peer_addr, &wrapped_r2_msg);
        }

        // Receive Round 2 Packages
        println!("Node {} receiving DKG Round 2 packages...", index);
        let received_r2_messages =
            receive_messages(&listener, (total - 1) as usize, |msg| match msg {
                MessageWrapper::DkgRound2(m) => Some(m),
                _ => None,
            })?;

        let mut received_round2_packages = BTreeMap::new();
        for msg in received_r2_messages {
            let sender_id = Identifier::try_from(msg.participant_index)?;
            received_round2_packages.insert(sender_id, msg.package);
            println!(
                "Node {} received DKG R2 package from participant {}",
                index, msg.participant_index
            );
        }

        if received_round2_packages.len() != (total - 1) as usize {
            return Err(format!(
                "Error: Incorrect number of Round 2 packages. Got {}, expected {}",
                received_round2_packages.len(),
                total - 1
            )
            .into());
        }
        println!("Node {} finished receiving DKG Round 2 packages.", index);

        // DKG Finalize (Part 3)
        println!("Node {} starting DKG Finalize (Part 3)...", index);
        let (kp, pkp) = frost::keys::dkg::part3(
            &round2_secret_package,
            &received_round1_packages_from_others,
            &received_round2_packages,
        )
        .expect("DKG part 3 failed");

        println!("DKG completed for Node {}", index);

        // Save keys to cache
        let key_bytes = encode_to_vec(&kp, bincode::config::standard())?;
        let pubkey_bytes = encode_to_vec(&pkp, bincode::config::standard())?;
        fs::write(&key_package_file, key_bytes)?;
        fs::write(&pubkey_package_file, pubkey_bytes)?;
        println!("Node {} saved keys to cache.", index);

        (kp, pkp)
    };

    // --- Common Code: Key Packages are now loaded or generated ---
    println!(
        "Node {} using KeyPackage: {}",
        index,
        hex::encode(key_package.serialize().unwrap()) // Assuming serialize is infallible or handle error
    );
    println!(
        "Node {} using Group PublicKey: {}",
        index,
        hex::encode(pubkey_package.verifying_key().serialize().unwrap()) // Assuming serialize is infallible
    );

    // Convert FROST group public key to Solana Pubkey
    let group_verifying_key_bytes = pubkey_package.verifying_key().serialize()?;
    let mut pubkey_arr = [0u8; 32];
    if group_verifying_key_bytes.len() == 32 {
        pubkey_arr.copy_from_slice(&group_verifying_key_bytes);
    } else {
        return Err(format!(
            "FROST verifying key size ({}) is not 32 bytes",
            group_verifying_key_bytes.len()
        )
        .into());
    }
    let solana_pubkey = Pubkey::new_from_array(pubkey_arr);
    println!("Node {} derived Solana address: {}", index, solana_pubkey);

    // --- Signing Phase ---
    let listener = TcpListener::bind(&my_addr).expect("Failed to bind for signing phase");
    println!("Node {} listening on {} for signing phase", index, my_addr);

    let message_bytes: Vec<u8>;
    let mut transaction: Option<Transaction> = None; // Only initiator will build the final tx

    if is_initiator {
        // Initiator Node
        println!(
            "Node {} (Initiator) prompting for transaction details...",
            index
        );
        print!("Enter target Solana address: ");
        io::stdout().flush()?;
        let mut target_address_str = String::new();
        stdin().read_line(&mut target_address_str)?;
        let target_address = Pubkey::from_str(target_address_str.trim())?;

        print!("Enter amount to transfer in lamports: ");
        io::stdout().flush()?;
        let mut amount_str = String::new();
        stdin().read_line(&mut amount_str)?;
        let amount: u64 = amount_str.trim().parse()?;

        println!(
            "Node {} preparing transaction: {} lamports to {}",
            index, amount, target_address
        );

        let rpc_url = "https://api.testnet.solana.com"; // Consider making this configurable
        let rpc_client =
            RpcClient::new_with_commitment(rpc_url.to_string(), CommitmentConfig::confirmed());
        let recent_blockhash = rpc_client.get_latest_blockhash()?;
        println!(
            "Node {} fetched recent blockhash: {}",
            index, recent_blockhash
        );

        let message = Message::new(
            &[transfer(&solana_pubkey, &target_address, amount)],
            Some(&solana_pubkey),
        );
        let mut tx = Transaction::new_unsigned(message);
        tx.message.recent_blockhash = recent_blockhash;
        message_bytes = tx.message.serialize();
        transaction = Some(tx); // Store the unsigned tx

        // Broadcast the message to sign
        let tx_message = TxMessage {
            message_bytes: message_bytes.clone(),
        };
        let wrapped_tx_msg = MessageWrapper::SignTx(tx_message);
        broadcast(index, total, &wrapped_tx_msg);
    } else {
        // Participant Node
        println!(
            "Node {} waiting for transaction message from initiator...",
            index
        );
        let received_tx_msgs = receive_messages(&listener, 1, |msg| match msg {
            MessageWrapper::SignTx(m) => Some(m),
            _ => None,
        })?;
        if received_tx_msgs.is_empty() {
            return Err("Node {} did not receive transaction message".into());
        }
        message_bytes = received_tx_msgs[0].message_bytes.clone();
        println!("Node {} received transaction message.", index);
    }

    // --- FROST Signing Rounds ---
    println!("Node {} starting FROST Round 1 (Commit)...", index);
    let (my_nonce, my_commitment) = frost_round1::commit(key_package.signing_share(), &mut rng);

    // Broadcast commitment
    let commitment_message = CommitmentMessage {
        sender_identifier: my_identifier,
        commitment: my_commitment.clone(),
    };
    let wrapped_commit_msg = MessageWrapper::SignCommitment(commitment_message);
    broadcast(index, total, &wrapped_commit_msg);

    // Receive commitments from others
    println!("Node {} receiving commitments...", index);
    let received_commit_msgs =
        receive_messages(&listener, (total - 1) as usize, |msg| match msg {
            MessageWrapper::SignCommitment(m) => Some(m),
            _ => None,
        })?;

    let mut commitments_map = BTreeMap::new();
    commitments_map.insert(my_identifier, my_commitment); // Add self
    for msg in received_commit_msgs {
        commitments_map.insert(msg.sender_identifier, msg.commitment);
        println!(
            "Node {} received commitment from {:?}",
            index, msg.sender_identifier
        );
    }

    if commitments_map.len() != total as usize {
        return Err(format!(
            "Error: Incorrect number of commitments received. Got {}, expected {}",
            commitments_map.len(),
            total
        )
        .into());
    }
    println!("Node {} collected all commitments.", index);

    // Create SigningPackage
    // For demo: allow all nodes to send shares, Node 1 will aggregate first `threshold` shares received.
    let mut signing_commitments = BTreeMap::new();
    let mut signer_identifiers = Vec::new();
    for (identifier, commitment) in commitments_map.iter() {
        signing_commitments.insert(*identifier, commitment.clone());
        signer_identifiers.push(*identifier);
    }
    // Sort signer_identifiers for deterministic threshold selection
    signer_identifiers.sort();

    // Node <initiator_index> (Initiator) logic
    if is_initiator {
        println!("Node {} creating SigningPackage...", index); // Node initiator always creates package to aggregate
        // Only use the first `threshold` identifiers for aggregation
        let threshold_signers: Vec<_> = signer_identifiers
            .iter()
            .take(threshold as usize)
            .cloned()
            .collect();
        let mut signing_commitments_threshold = BTreeMap::new();
        for id in &threshold_signers {
            if let Some(commitment) = signing_commitments.get(id) {
                signing_commitments_threshold.insert(*id, commitment.clone());
            }
        }
        let signing_package =
            SigningPackage::new(signing_commitments_threshold.clone(), &message_bytes);

        let mut signature_shares_map = BTreeMap::new();

        println!("Node {} starting FROST Round 2 (Sign)...", index);
        let my_signature_share = frost_round2::sign(&signing_package, &my_nonce, &key_package)?;
        println!("Node {} generated signature share.", index);

        // Only add own share if this node is in the threshold set
        if threshold_signers.contains(&my_identifier) {
            signature_shares_map.insert(my_identifier, my_signature_share.clone());
            println!("Node {} added its own share to map.", index);
        }

        // Only need threshold-1 shares in addition to own (from threshold set)
        let shares_to_receive = if threshold_signers.contains(&my_identifier) {
            threshold as usize - 1
        } else {
            threshold as usize
        };

        println!(
            "Node {} waiting for signature shares from {} participants...",
            index, shares_to_receive
        );

        if shares_to_receive > 0 {
            let received_share_msgs =
                receive_messages(&listener, shares_to_receive, |msg| match msg {
                    MessageWrapper::SignShare(m) => Some(m),
                    _ => None,
                })?;

            for msg in received_share_msgs {
                // Only accept shares from expected threshold signers
                if threshold_signers.contains(&msg.sender_identifier) {
                    signature_shares_map.insert(msg.sender_identifier, msg.share);
                    println!(
                        "Node {} received and added share from {:?}",
                        index, msg.sender_identifier
                    );
                } else {
                    println!(
                        "Node {} received share from unknown identifier {:?} (ignored)",
                        index, msg.sender_identifier
                    );
                }
            }
        } else if threshold == 0 {
            println!("Warning: Threshold is 0, no shares needed.");
        } else if threshold == 1 {
            println!(
                "Node {} is the only signer (threshold 1), no shares to receive.",
                index
            );
        }

        if signature_shares_map.len() != threshold as usize {
            return Err(format!(
                "Error: Incorrect number of signature shares collected for aggregation. Got {}, expected {}",
                signature_shares_map.len(),
                threshold
            )
            .into());
        }
        println!(
            "Node {} collected all required signature shares for threshold {}.",
            index, threshold
        );

        // Aggregation (only by Initiator)
        println!("Node {} aggregating partial signatures...", index);
        let group_signature =
            frost::aggregate(&signing_package, &signature_shares_map, &pubkey_package)?;
        println!("Node {} aggregation successful!", index);

        let group_signature_bytes = group_signature.serialize()?;
        println!(
            "Node {} final aggregated signature ({} bytes): {}",
            index,
            group_signature_bytes.len(),
            hex::encode(&group_signature_bytes)
        );

        // Broadcast the aggregated signature to all participants
        let agg_sig_msg = MessageWrapper::SignAggregated(AggregatedSignatureMessage {
            signature_bytes: group_signature_bytes.clone(), // Clone bytes
        });
        broadcast(index, total, &agg_sig_msg);

        // Add signature to transaction and send
        if let Some(mut final_tx) = transaction {
            let solana_signature = Signature::try_from(group_signature_bytes)
                .map_err(|e| format!("Failed to create Solana signature: {:?}", e))?;
            if !final_tx.signatures.is_empty() {
                final_tx.signatures[0] = solana_signature;
            } else {
                final_tx.signatures.push(solana_signature);
            }
            println!("Aggregated signature added to Solana transaction.");

            println!("\n--- Transaction Prepared ---");
            println!("Transaction details: {:?}", final_tx);
            println!("Attempting to send (expected to fail verification on-chain)...");

            let rpc_client = RpcClient::new_with_commitment(
                "https://api.testnet.solana.com".to_string(),
                CommitmentConfig::confirmed(),
            ); // Recreate client if needed
            match rpc_client.send_and_confirm_transaction_with_spinner(&final_tx) {
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
        } else {
            return Err(format!("Node {} transaction object was lost", index).into());
        }
    } else {
        // Participant Node logic

        // Determine the threshold signers deterministically (same as initiator)
        let threshold_signers: Vec<_> = signer_identifiers
            .iter()
            .take(threshold as usize)
            .cloned()
            .collect();
        let mut signing_commitments_threshold = BTreeMap::new();
        for id in &threshold_signers {
            if let Some(commitment) = signing_commitments.get(id) {
                signing_commitments_threshold.insert(*id, commitment.clone());
            }
        }

        // Only generate and send share if this node is part of the threshold set
        if threshold_signers.contains(&my_identifier) {
            println!("Node {} creating SigningPackage (threshold)...", index);
            let signing_package =
                SigningPackage::new(signing_commitments_threshold.clone(), &message_bytes);

            println!("Node {} starting FROST Round 2 (Sign)...", index);
            let my_signature_share = frost_round2::sign(&signing_package, &my_nonce, &key_package)?;
            println!("Node {} generated signature share.", index);

            // --- Send share to initiator ---
            println!("\n--- Node {} Action Required ---", index);
            println!("Received request to sign the following transaction message (hex):");
            println!("{}", hex::encode(&message_bytes));
            print!("Press ENTER to confirm signing and send your share to initiator: ");
            io::stdout().flush()?;
            let mut confirmation = String::new();
            stdin().read_line(&mut confirmation)?;

            println!("Node {} sending signature share to initiator...", index);
            let share_message = ShareMessage {
                sender_identifier: my_identifier,
                share: my_signature_share,
            };
            let wrapped_share_msg = MessageWrapper::SignShare(share_message);
            let initiator_addr = "127.0.0.1:10001"; // Default to 10001 for initiator
            send_to(&initiator_addr, &wrapped_share_msg);
            println!("Node {} sent share.", index);

            // Optionally wait for and verify the aggregated signature broadcasted by the initiator
            println!(
                "Node {} waiting for aggregated signature from initiator...",
                index
            );
            let received_agg_sig = receive_messages(&listener, 1, |msg| match msg {
                MessageWrapper::SignAggregated(m) => Some(m),
                _ => None,
            })?;

            if let Some(agg_sig_msg) = received_agg_sig.first() {
                println!(
                    "Node {} received aggregated signature: {}",
                    index,
                    hex::encode(&agg_sig_msg.signature_bytes)
                );
                // Here you could potentially verify the aggregated signature against the pubkey_package
                // let agg_sig = frost::Signature::deserialize(&agg_sig_msg.signature_bytes)?;
                // pubkey_package.verify(&message_bytes, &agg_sig)?;
                // println!("Node {} successfully verified aggregated signature.", index);
            } else {
                eprintln!("Node {} did not receive aggregated signature.", index);
            }
        } else {
            println!(
                "Node {} is not part of the threshold signing set, skipping share generation/sending.",
                index
            );
            // Participant still needs to wait or exit gracefully.
            // Optionally wait for the aggregated signature broadcast
            println!(
                "Node {} waiting for aggregated signature from initiator (as non-signer)...",
                index
            );
            let received_agg_sig = receive_messages(&listener, 1, |msg| match msg {
                MessageWrapper::SignAggregated(m) => Some(m),
                _ => None,
            })?;
            if let Some(agg_sig_msg) = received_agg_sig.first() {
                println!(
                    "Node {} received aggregated signature: {}",
                    index,
                    hex::encode(&agg_sig_msg.signature_bytes)
                );
            } else {
                eprintln!("Node {} did not receive aggregated signature.", index);
            }
        }
    }

    println!("\nProcess completed for Node {}.", index);
    Ok(())
}
