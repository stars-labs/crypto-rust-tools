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

// --- New Message Type for Signer Selection ---
#[derive(Serialize, Deserialize, Clone)]
struct SignerSelectionMessage {
    selected_identifiers: Vec<Identifier>,
}

// --- Generic Message Wrapper ---
#[derive(Serialize, Deserialize, Clone)]
enum MessageWrapper {
    DkgRound1(Round1Message),
    DkgRound2(Round2Message),
    SignTx(TxMessage),
    SignCommitment(CommitmentMessage),
    SignShare(ShareMessage),
    SignAggregated(AggregatedSignatureMessage),
    SignerSelection(SignerSelectionMessage), // New variant
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

// Receive a specific number of messages of a certain type (blocking)
fn receive_messages<T>(
    listener: &TcpListener,
    count: usize,
    extract_fn: fn(MessageWrapper) -> Option<T>,
) -> Result<Vec<T>, Box<dyn Error>> {
    let mut messages = Vec::with_capacity(count);

    // Loop until 'count' messages of the correct type are received
    while messages.len() < count {
        // Block until a connection is accepted
        let (mut stream, addr) = listener.accept()?;
        println!("Accepted connection from {}", addr);

        let mut buf = Vec::new();
        // Block until the entire message is read
        match stream.read_to_end(&mut buf) {
            Ok(_) => {
                if buf.is_empty() {
                    println!("Received empty message from {}", addr);
                    continue; // Skip empty messages
                }
                let wrapped_msg: MessageWrapper =
                    match decode_from_slice(&buf, bincode::config::standard()) {
                        Ok((msg, _)) => msg,
                        Err(e) => {
                            eprintln!("Failed to decode message from {}: {}", addr, e);
                            continue; // Skip malformed messages
                        }
                    };

                if let Some(msg) = extract_fn(wrapped_msg.clone()) {
                    messages.push(msg);
                } else {
                    // Handle unexpected but valid message types if necessary
                    match wrapped_msg {
                        MessageWrapper::SignAggregated(_) | MessageWrapper::SignerSelection(_) => {
                            println!(
                                "Received unexpected but valid message type while waiting (ignoring)"
                            );
                        }
                        _ => {
                            eprintln!("Received unexpected message type from {}", addr);
                        }
                    }
                }
            }
            Err(e) => {
                // Handle stream read errors (other than timeout, which is removed)
                eprintln!("Error reading from stream {}: {}", addr, e);
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

        // Receive Round 1 Packages (blocking)
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

        // DKG requires all participants, check if we received enough R1 packages
        if received_round1_packages.len() < total as usize {
            return Err(format!(
                "Error: Not enough Round 1 packages received for DKG. Got {}, expected {}",
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
            // Only send to participants from whom we received R1 package
            if received_round1_packages_from_others.contains_key(&receiver_id) {
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
        }

        // Receive Round 2 Packages (blocking)
        println!("Node {} receiving DKG Round 2 packages...", index);
        let received_r2_messages =
            receive_messages(&listener, (total - 1) as usize, |msg| match msg {
                MessageWrapper::DkgRound2(m) => Some(m),
                _ => None,
            })?;

        let mut received_round2_packages = BTreeMap::new();
        for msg in received_r2_messages {
            let sender_id = Identifier::try_from(msg.participant_index)?;
            // Only accept from participants from whom we received R1 package
            if received_round1_packages_from_others.contains_key(&sender_id) {
                received_round2_packages.insert(sender_id, msg.package);
                println!(
                    "Node {} received DKG R2 package from participant {}",
                    index, msg.participant_index
                );
            }
        }

        // DKG requires all participants, check if we received enough R2 packages
        if received_round2_packages.len() < (total - 1) as usize {
            return Err(format!(
                "Error: Not enough Round 2 packages received for DKG. Got {}, expected {}",
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
            &received_round1_packages_from_others, // Use the original map here
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
        // Participant Node waits for TX message (blocking)
        println!(
            "Node {} waiting for transaction message from initiator...",
            index
        );
        let received_tx_msgs = receive_messages(&listener, 1, |msg| match msg {
            MessageWrapper::SignTx(m) => Some(m),
            _ => None,
        })?;
        // No need to check is_empty, receive_messages blocks until 1 is received or errors
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

    // Receive commitments from others (blocking)
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

    // Check if we have at least threshold commitments to proceed with signing
    if commitments_map.len() < threshold as usize {
        return Err(format!(
            "Error: Not enough commitments received to meet threshold. Got {}, expected at least {}",
            commitments_map.len(),
            threshold
        )
        .into());
    }
    println!(
        "Node {} collected {} commitments (threshold is {}).",
        index,
        commitments_map.len(),
        threshold
    );

    // --- Signer Selection Phase ---
    let selected_signer_identifiers: Vec<Identifier>;

    if is_initiator {
        // Initiator selects signers MANUALLY, automatically including self
        let mut manually_selected_ids = Vec::with_capacity(threshold as usize);
        manually_selected_ids.push(my_identifier); // Automatically include self

        let needed_others = threshold.saturating_sub(1); // Number of *other* participants needed

        if needed_others > 0 {
            let available_other_signer_ids: Vec<_> = commitments_map
                .keys()
                .filter(|&&id| id != my_identifier) // Exclude self
                .cloned()
                .collect();

            if available_other_signer_ids.len() < needed_others as usize {
                return Err(format!(
                    "Error: Not enough other participants ({}) available to meet threshold ({}). Need {} more.",
                    available_other_signer_ids.len(), threshold, needed_others
                ).into());
            }

            println!("Available other participants (who sent commitments):");
            for id in &available_other_signer_ids {
                let id_bytes = id.serialize();
                let idx = u16::from_le_bytes(id_bytes[0..2].try_into()?);
                println!(" - Node {}", idx);
            }

            loop {
                print!(
                    "Enter {} other participant indices (comma-separated) to select for signing: ",
                    needed_others
                );
                io::stdout().flush()?;
                let mut input_str = String::new();
                stdin().read_line(&mut input_str)?;

                let parts: Vec<&str> = input_str.trim().split(',').collect();
                if parts.len() != needed_others as usize {
                    eprintln!(
                        "Error: Expected {} indices, but got {}. Please try again.",
                        needed_others,
                        parts.len()
                    );
                    continue;
                }

                let mut temp_selected_others = Vec::with_capacity(needed_others as usize);
                let mut input_valid = true;
                for part in parts {
                    match part.trim().parse::<u16>() {
                        Ok(idx) => match Identifier::try_from(idx) {
                            Ok(id) => {
                                // Check if it's one of the *other* available participants
                                if available_other_signer_ids.contains(&id) {
                                    if !temp_selected_others.contains(&id) {
                                        temp_selected_others.push(id);
                                    } else {
                                        eprintln!(
                                            "Error: Index {} entered more than once. Please try again.",
                                            idx
                                        );
                                        input_valid = false;
                                        break;
                                    }
                                } else if id == my_identifier {
                                    eprintln!(
                                        "Error: Initiator (Node {}) is already included. Please select other participants.",
                                        index
                                    );
                                    input_valid = false;
                                    break;
                                } else {
                                    eprintln!(
                                        "Error: Participant with index {} is not available or did not send a commitment. Please try again.",
                                        idx
                                    );
                                    input_valid = false;
                                    break;
                                }
                            }
                            Err(_) => {
                                eprintln!(
                                    "Error: Invalid index {} entered. Please try again.",
                                    idx
                                );
                                input_valid = false;
                                break;
                            }
                        },
                        Err(_) => {
                            eprintln!(
                                "Error: Invalid input '{}'. Please enter numbers separated by commas.",
                                part
                            );
                            input_valid = false;
                            break;
                        }
                    }
                }

                if input_valid {
                    manually_selected_ids.extend(temp_selected_others);
                    break; // Exit loop on valid input
                }
            }
        } else {
            println!("Node {} is the only signer required (threshold 1).", index);
        }

        // Sort the final list for deterministic broadcast message
        manually_selected_ids.sort();
        selected_signer_identifiers = manually_selected_ids;

        // Check is redundant now due to input validation loop, but keep as safeguard
        if selected_signer_identifiers.len() != threshold as usize {
            return Err(format!(
                "Internal Error: Selection process resulted in {} signers, expected {}.",
                selected_signer_identifiers.len(),
                threshold
            )
            .into());
        }

        println!(
            "Node {} (Initiator) selected signers: {:?}",
            index, selected_signer_identifiers
        );

        // Broadcast the selection
        let selection_msg = MessageWrapper::SignerSelection(SignerSelectionMessage {
            selected_identifiers: selected_signer_identifiers.clone(),
        });
        broadcast(index, total, &selection_msg);
    } else {
        // Participants wait for selection message (blocking)
        println!(
            "Node {} waiting for signer selection from initiator...", // Removed timeout mention
            index
        );
        let received_selection = receive_messages(&listener, 1, |msg| match msg {
            MessageWrapper::SignerSelection(m) => Some(m),
            _ => None,
        })?;

        // No need to check is_empty
        selected_signer_identifiers = received_selection[0].selected_identifiers.clone();
        println!(
            "Node {} received selected signers: {:?}",
            index, selected_signer_identifiers
        );
    }

    // --- Signing Round 2 (Conditional based on selection) ---
    let i_am_selected = selected_signer_identifiers.contains(&my_identifier);

    if i_am_selected {
        // Create SigningPackage using ONLY commitments from SELECTED signers
        let mut final_commitments = BTreeMap::new();
        for id in &selected_signer_identifiers {
            if let Some(commitment) = commitments_map.get(id) {
                final_commitments.insert(*id, commitment.clone());
            } else {
                // This should not happen if initiator selected correctly
                return Err(
                    format!("Error: Commitment missing for selected signer {:?}", id).into(),
                );
            }
        }
        println!(
            "Node {} (Selected) creating SigningPackage for selected signers ({})...",
            index,
            final_commitments.len()
        );
        let signing_package = SigningPackage::new(final_commitments.clone(), &message_bytes); // Use final_commitments

        println!("Node {} starting FROST Round 2 (Sign)...", index);
        let my_signature_share = frost_round2::sign(&signing_package, &my_nonce, &key_package)?;
        println!("Node {} generated signature share.", index);

        if is_initiator {
            // Initiator logic (continues from here)
            let mut signature_shares_map = BTreeMap::new();
            signature_shares_map.insert(my_identifier, my_signature_share.clone());
            println!("Node {} added its own share to map.", index);

            // Calculate how many shares to receive from *other* selected signers
            let shares_to_receive = selected_signer_identifiers
                .iter()
                .filter(|&&id| id != my_identifier)
                .count();

            println!(
                "Node {} waiting for signature shares from {} selected participants...", // Removed timeout mention
                index, shares_to_receive
            );

            if shares_to_receive > 0 {
                // Receive shares (blocking)
                let received_share_msgs = receive_messages(
                    &listener,
                    shares_to_receive, // Expect shares only from other selected signers
                    |msg| match msg {
                        MessageWrapper::SignShare(m) => Some(m),
                        _ => None,
                    },
                )?;

                for msg in received_share_msgs {
                    // Accept share only if the sender is in the selected list
                    if selected_signer_identifiers.contains(&msg.sender_identifier) {
                        signature_shares_map.insert(msg.sender_identifier, msg.share);
                        println!(
                            "Node {} received and added share from selected signer {:?}",
                            index, msg.sender_identifier
                        );
                    } else {
                        println!(
                            "Node {} received share from non-selected identifier {:?} (ignored)",
                            index, msg.sender_identifier
                        );
                    }
                }
            }

            // Check if all selected signers provided shares
            if signature_shares_map.len() != threshold as usize {
                // Check against threshold, which is the size of selected_signer_identifiers
                return Err(format!(
                    "Error: Not enough signature shares collected from selected signers. Got {}, expected {}",
                    signature_shares_map.len(),
                    threshold // or selected_signer_identifiers.len()
                )
                .into());
            }
            println!(
                "Node {} collected all {} required signature shares from selected signers.",
                index,
                signature_shares_map.len()
            );

            // Aggregation (only by Initiator)
            println!("Node {} aggregating partial signatures...", index);
            // Use the signing_package created earlier with selected commitments
            let group_signature = frost::aggregate(
                &signing_package,
                &signature_shares_map, // Contains shares only from selected signers
                &pubkey_package,
            )?;
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
            broadcast(index, total, &agg_sig_msg); // Broadcast to original 'total'

            // Add signature to transaction and send
            if let Some(mut final_tx) = transaction {
                // ... (rest of initiator transaction sending unchanged) ...
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
            // Participant Node logic (Selected)

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
            // TODO: Determine initiator address dynamically if not always node 1
            let initiator_addr = "127.0.0.1:10001"; // Default to 10001 for initiator
            send_to(&initiator_addr, &wrapped_share_msg);
            println!("Node {} sent share.", index);

            // Wait for aggregated signature from initiator (blocking)
            println!(
                "Node {} waiting for aggregated signature from initiator...", // Removed timeout mention
                index
            );
            let received_agg_sig = receive_messages(&listener, 1, |msg| match msg {
                MessageWrapper::SignAggregated(m) => Some(m),
                _ => None,
            })?;

            // No need to check is_empty
            println!(
                "Node {} received aggregated signature: {}",
                index,
                hex::encode(&received_agg_sig[0].signature_bytes)
            );
            // Optionally verify signature
            // let agg_sig = frost::Signature::deserialize(&received_agg_sig[0].signature_bytes)?;
            // pubkey_package.verify(&message_bytes, &agg_sig)?;
            // println!("Node {} successfully verified aggregated signature.", index);
        }
    } else {
        // Node was NOT selected for signing
        println!("Node {} was not selected for signing.", index);
        // Wait for aggregated signature from initiator (blocking)
        println!(
            "Node {} waiting for aggregated signature from initiator...", // Removed timeout mention
            index
        );
        let received_agg_sig = receive_messages(&listener, 1, |msg| match msg {
            MessageWrapper::SignAggregated(m) => Some(m),
            _ => None,
        })?;

        // No need to check is_empty
        println!(
            "Node {} received aggregated signature: {}",
            index,
            hex::encode(&received_agg_sig[0].signature_bytes)
        );
    }

    println!("\nProcess completed for Node {}.", index);
    Ok(())
}
