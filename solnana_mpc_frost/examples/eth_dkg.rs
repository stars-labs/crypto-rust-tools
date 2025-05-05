use bincode::serde::{decode_from_slice, encode_to_vec};
use ethers_core::types::{
    Address,
    H256,
    Signature as EthSignature,
    TransactionRequest,
    U256, // Removed H160
};
use ethers_core::utils::keccak256;
use ethers_providers::{Http, Middleware, Provider};
// Removed unused Signer import
use frost::Identifier;
use frost::rand_core::OsRng;
use frost_core::SigningPackage;
use frost_core::keys::dkg::{round1, round2};
use frost_core::keys::{KeyPackage, PublicKeyPackage};
use frost_core::{round1 as frost_round1, round2 as frost_round2};
use frost_secp256k1 as frost; // Use secp256k1
use frost_secp256k1::Secp256K1Sha256; // Use secp256k1
use hex;
// --- Import elliptic_curve/k256 traits ---
use elliptic_curve::point::AffineCoordinates; // Import for .x() on AffinePoint
// --- End elliptic_curve/k256 traits ---
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write, stdin}; // Keep sync stdin/stdout
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::time::timeout; // Use tokio's timeout
// --- Remove unused/unresolved imports ---
// use ethers_core::types::transaction::eip2718::TypedTransaction;
// use ethers_providers::JsonRpcClient; // Import JsonRpcClient for RpcError
// use jsonrpsee_types::error::ErrorObjectOwned as RpcError; // Import RpcError directly if needed

// --- DKG Message Types ---
#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct Round1Message {
    participant_index: u16,
    package: round1::Package<Secp256K1Sha256>, // Use Secp256K1Sha256
}

#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct Round2Message {
    participant_index: u16,
    package: round2::Package<Secp256K1Sha256>, // Use Secp256K1Sha256
}

// --- Signing Message Types ---
#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct TxMessage {
    // Store transaction hash bytes directly
    tx_hash_bytes: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct CommitmentMessage {
    sender_identifier: Identifier,
    commitment: frost_round1::SigningCommitments<Secp256K1Sha256>, // Use Secp256K1Sha256
}

#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct ShareMessage {
    sender_identifier: Identifier,
    share: frost_round2::SignatureShare<Secp256K1Sha256>, // Use Secp256K1Sha256
}

// --- New Message Type for Aggregated Signature ---
#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct AggregatedSignatureMessage {
    // Send r, s, v directly
    r: [u8; 32],
    s: [u8; 32],
    v: u8,
}

// --- New Message Type for Signer Selection ---
#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
struct SignerSelectionMessage {
    selected_identifiers: Vec<Identifier>,
}

// --- Generic Message Wrapper ---
#[derive(Serialize, Deserialize, Clone, Debug)] // Add Debug
enum MessageWrapper {
    DkgRound1(Round1Message),
    DkgRound2(Round2Message),
    SignTx(TxMessage),
    SignCommitment(CommitmentMessage),
    SignShare(ShareMessage),
    SignAggregated(AggregatedSignatureMessage),
    SignerSelection(SignerSelectionMessage),
}

fn parse_args() -> (u16, u16, u16, bool) {
    // ... (same as solana_dkg.rs) ...
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

// Async send_to using tokio with retry logic
async fn send_to(addr: &str, msg: &MessageWrapper) {
    let data = encode_to_vec(msg, bincode::config::standard()).unwrap();
    let sock_addr: SocketAddr = addr.parse().unwrap();

    // Add retry logic with backoff
    let mut attempt = 0;
    let max_attempts = 5;

    loop {
        match timeout(Duration::from_secs(2), TcpStream::connect(sock_addr)).await {
            Ok(Ok(mut stream)) => {
                if let Err(e) = stream.write_all(&data).await {
                    eprintln!("Failed to write to {}: {}", addr, e);
                } else {
                    // Only print success on retry attempts
                    if attempt > 0 {
                        println!(
                            "Successfully connected to {} after {} attempts",
                            addr,
                            attempt + 1
                        );
                    }
                }
                break; // Exit the retry loop on success
            }
            Ok(Err(e)) => {
                attempt += 1;
                if attempt >= max_attempts {
                    eprintln!(
                        "Failed to connect to {} after {} attempts: {}",
                        addr, attempt, e
                    );
                    break;
                }
                let backoff = Duration::from_millis(500 * attempt as u64); // Exponential-ish backoff
                eprintln!(
                    "Connection to {} failed (attempt {}), retrying in {:?}: {}",
                    addr, attempt, backoff, e
                );
                tokio::time::sleep(backoff).await;
            }
            Err(_) => {
                attempt += 1;
                if attempt >= max_attempts {
                    eprintln!("Timeout connecting to {} after {} attempts", addr, attempt);
                    break;
                }
                let backoff = Duration::from_millis(500 * attempt as u64);
                eprintln!(
                    "Connection to {} timed out (attempt {}), retrying in {:?}",
                    addr, attempt, backoff
                );
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

// Async broadcast using tokio
async fn broadcast(index: u16, total: u16, msg: &MessageWrapper) {
    println!("Node {} broadcasting message: {:?}", index, msg); // Log message being broadcast
    let mut tasks = Vec::new();
    for peer_idx in 1..=total {
        if peer_idx == index {
            continue;
        }
        let peer_addr = format!("127.0.0.1:1000{}", peer_idx);
        // Clone msg for each task
        let msg_clone = msg.clone();
        tasks.push(tokio::spawn(async move {
            send_to(&peer_addr, &msg_clone).await;
        }));
    }
    // Wait for all sends to complete (optional, fire-and-forget is also possible)
    for task in tasks {
        let _ = task.await; // Handle potential task errors if needed
    }
}

// Async receive_messages using tokio
async fn receive_messages<T: Send + 'static + std::fmt::Debug>(
    // Ensure T is Send and Debug
    listener: Arc<Mutex<TcpListener>>, // Use Arc<Mutex<TcpListener>>
    expected_count: usize,
    timeout_duration: Option<Duration>,
    extract_fn: fn(MessageWrapper) -> Option<T>,
) -> Result<Vec<T>, Box<dyn Error + Send + Sync>> {
    // Ensure error is Send + Sync
    let messages = Arc::new(Mutex::new(Vec::with_capacity(expected_count)));
    let listener_clone = listener.clone();
    let messages_clone = messages.clone(); // Clone the Arc before moving

    let processing_task = tokio::spawn(async move {
        let mut local_messages = Vec::with_capacity(expected_count);
        loop {
            let listener_guard = listener_clone.lock().await; // Removed 'mut' as it's not needed
            match listener_guard.accept().await {
                Ok((mut stream, addr)) => {
                    println!("Accepted connection from {}", addr);
                    drop(listener_guard); // Release lock before reading

                    let mut buf = Vec::new();
                    match stream.read_to_end(&mut buf).await {
                        Ok(_) => {
                            if buf.is_empty() {
                                println!("Received empty message from {}", addr);
                            } else {
                                match decode_from_slice::<MessageWrapper, _>(
                                    &buf,
                                    bincode::config::standard(),
                                ) {
                                    Ok((wrapped_msg, _)) => {
                                        println!("Node received message: {:?}", wrapped_msg); // Log received message
                                        if let Some(msg) = extract_fn(wrapped_msg) {
                                            local_messages.push(msg);
                                            let mut messages_guard = messages_clone.lock().await; // Use the cloned Arc
                                            messages_guard.push(local_messages.pop().unwrap()); // Move message
                                            let current_count = messages_guard.len();
                                            drop(messages_guard); // Release lock
                                            if current_count >= expected_count {
                                                println!(
                                                    "Received expected count ({}) messages.",
                                                    expected_count
                                                );
                                                break; // Exit loop once expected count is reached
                                            }
                                        } else {
                                            println!(
                                                "Received unexpected message type from {} (ignoring)",
                                                addr
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Failed to decode message from {}: {}", addr, e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Error reading from stream {}: {}", addr, e);
                        }
                    }
                }
                Err(e) => {
                    // This error might occur if the listener is closed, handle appropriately
                    eprintln!("Error accepting connection: {}", e);
                    // Potentially break or return error if accept fails critically
                    tokio::time::sleep(Duration::from_millis(50)).await; // Avoid busy-looping on transient errors
                }
            }
            // Check message count again after potential error handling
            let messages_guard = messages_clone.lock().await; // Use the cloned Arc
            let current_count = messages_guard.len();
            drop(messages_guard);
            if current_count >= expected_count {
                println!(
                    "Received expected count ({}) messages after error check.",
                    expected_count
                );
                break;
            }
        }
        // Return collected messages from the task (though we primarily use the shared Arc<Mutex<Vec>>)
        Ok::<_, Box<dyn Error + Send + Sync>>(local_messages) // Ensure error type matches outer function
    });

    if let Some(duration) = timeout_duration {
        match timeout(duration, processing_task).await {
            Ok(Ok(_)) => {
                // Task completed successfully
                println!("Message receiving task completed within timeout.");
            }
            Ok(Err(e)) => {
                // Task returned an error
                eprintln!("Message receiving task failed: {}", e);
                return Err(format!("Task error: {}", e).into()); // Convert to String error
            }
            Err(join_error) => {
                // JoinError occurred
                eprintln!(
                    "Message receiving task panicked or was cancelled: {}",
                    join_error
                );
                // Convert to String error instead of direct boxing
                return Err(format!("Join error: {}", join_error).into());
            }
        }
    } else {
        // No timeout, wait indefinitely for the task to complete
        match processing_task.await {
            Ok(Ok(_)) => { /* Task completed successfully */ }
            Ok(Err(e)) => {
                // Task returned an error
                eprintln!("Message receiving task failed: {}", e);
                return Err(format!("Task error: {}", e).into()); // Convert to String error
            }
            Err(join_error) => {
                // JoinError occurred
                eprintln!(
                    "Message receiving task panicked or was cancelled: {}",
                    join_error
                );
                // Convert to String error instead of direct boxing
                return Err(format!("Join error: {}", join_error).into());
            }
        }
    }

    // Take ownership of the messages from the Arc<Mutex<Vec>>
    let final_messages = Arc::try_unwrap(messages)
        .expect("Mutex still has multiple owners") // Requires Debug on Vec<T>, thus T
        .into_inner();

    Ok(final_messages)
}

// Helper to derive Ethereum address from FROST PublicKeyPackage
fn derive_eth_address(
    pubkey_package: &PublicKeyPackage<Secp256K1Sha256>,
) -> Result<Address, Box<dyn Error + Send + Sync>> {
    use k256::elliptic_curve::sec1::ToEncodedPoint; // Keep this for PublicKey::to_encoded_point

    let group_public_key = pubkey_package.verifying_key();
    // Serialize the key in uncompressed format (0x04 prefix + 64 bytes)
    let compressed_bytes = group_public_key.serialize()?; // Handle Result using ?

    // Need to decompress first. Use the `k256` crate internally used by frost-secp256k1
    let compressed_point =
        k256::PublicKey::from_sec1_bytes(&compressed_bytes) // Use the unwrapped bytes
            .map_err(|e| format!("Failed to parse compressed public key: {}", e))?;
    let uncompressed_point = compressed_point.to_encoded_point(false); // Get uncompressed EncodedPoint
    let uncompressed_bytes_slice = uncompressed_point.as_bytes(); // Get as byte slice

    // Ensure it's uncompressed (starts with 0x04) and is 65 bytes long
    if uncompressed_bytes_slice.len() != 65 || uncompressed_bytes_slice[0] != 0x04 {
        return Err(format!(
            "Unexpected uncompressed public key format (len={}, prefix={})",
            uncompressed_bytes_slice.len(),
            uncompressed_bytes_slice[0]
        )
        .into());
    }

    // Hash the uncompressed key (excluding the 0x04 prefix)
    let hash = keccak256(&uncompressed_bytes_slice[1..]);

    // Take the last 20 bytes of the hash
    let address_bytes = &hash[12..];
    Ok(Address::from_slice(address_bytes))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    // Ensure error is Send + Sync
    let (index, total, threshold, is_initiator) = parse_args();
    let my_addr = format!("127.0.0.1:1000{}", index);
    let mut rng = OsRng;
    let network_timeout = Duration::from_secs(10); // Increased timeout for potential network latency

    // My Identifier
    let my_identifier = Identifier::try_from(index).expect("Invalid identifier");

    // Add a small startup delay based on index to ensure nodes don't all try to connect simultaneously
    // This gives nodes time to start up and bind to their ports
    let startup_delay = Duration::from_millis(300 * index as u64);
    println!(
        "Node {} waiting {:?} before starting DKG to ensure all peers are ready...",
        index, startup_delay
    );
    tokio::time::sleep(startup_delay).await;

    // --- Key Loading/Generation ---
    let key_package_file = format!("eth_key_package_{}.bin", index);
    let pubkey_package_file = format!("eth_pubkey_package_{}.bin", index);

    let (key_package, pubkey_package): (
        KeyPackage<Secp256K1Sha256>,
        PublicKeyPackage<Secp256K1Sha256>,
    ) = if fs::metadata(&key_package_file).is_ok() && fs::metadata(&pubkey_package_file).is_ok() {
        println!("Node {} loading keys from cache...", index);
        let key_bytes = fs::read(&key_package_file)?;
        let pubkey_bytes = fs::read(&pubkey_package_file)?;

        let kp: KeyPackage<Secp256K1Sha256> =
            decode_from_slice(&key_bytes, bincode::config::standard())?.0;
        let pkp: PublicKeyPackage<Secp256K1Sha256> =
            decode_from_slice(&pubkey_bytes, bincode::config::standard())?.0;

        println!("Node {} loaded keys successfully.", index);
        (kp, pkp)
    } else {
        println!("Node {} generating new keys via DKG...", index);
        let listener = Arc::new(Mutex::new(
            TcpListener::bind(&my_addr).await.expect("Failed to bind"),
        ));
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

        broadcast(index, total, &wrapped_r1_msg).await;

        // Receive Round 1 Packages (blocking - DKG requires all)
        println!("Node {} receiving DKG Round 1 packages...", index);
        let received_r1_messages = receive_messages(
            listener.clone(), // Clone Arc
            (total - 1) as usize,
            None, // No timeout for DKG
            |msg| match msg {
                MessageWrapper::DkgRound1(m) => Some(m),
                _ => None,
            },
        )
        .await?;

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
        let mut send_tasks = Vec::new();
        for (receiver_id, package) in round2_packages {
            if received_round1_packages_from_others.contains_key(&receiver_id) {
                let id_bytes = receiver_id.serialize();
                let receiver_idx = u16::from_le_bytes(id_bytes[0..2].try_into()?);
                let round2_message = Round2Message {
                    participant_index: index,
                    package,
                };
                let wrapped_r2_msg = MessageWrapper::DkgRound2(round2_message);
                let peer_addr = format!("127.0.0.1:1000{}", receiver_idx);
                send_tasks.push(tokio::spawn(async move {
                    send_to(&peer_addr, &wrapped_r2_msg).await;
                }));
            }
        }
        for task in send_tasks {
            let _ = task.await;
        } // Wait for sends

        // Receive Round 2 Packages (blocking - DKG requires all)
        println!("Node {} receiving DKG Round 2 packages...", index);
        let received_r2_messages = receive_messages(
            listener.clone(), // Clone Arc
            (total - 1) as usize,
            None, // No timeout for DKG
            |msg| match msg {
                MessageWrapper::DkgRound2(m) => Some(m),
                _ => None,
            },
        )
        .await?;

        let mut received_round2_packages = BTreeMap::new();
        for msg in received_r2_messages {
            let sender_id = Identifier::try_from(msg.participant_index)?;
            if received_round1_packages_from_others.contains_key(&sender_id) {
                received_round2_packages.insert(sender_id, msg.package);
                println!(
                    "Node {} received DKG R2 package from participant {}",
                    index, msg.participant_index
                );
            }
        }

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
        "Node {} using KeyPackage identifier: {:?}",
        index,
        key_package.identifier()
    );
    let eth_address = derive_eth_address(&pubkey_package)?;
    println!("Node {} derived Ethereum address: {:?}", index, eth_address);

    // --- Signing Phase ---
    let listener = Arc::new(Mutex::new(
        TcpListener::bind(&my_addr)
            .await
            .expect("Failed to bind for signing phase"),
    ));
    println!("Node {} listening on {} for signing phase", index, my_addr);

    let tx_hash_bytes: Vec<u8>;
    let mut transaction_request: Option<TransactionRequest> = None; // Store the request

    // Use an RPC provider (e.g., Infura, Alchemy, or a local node like Ganache/Anvil)
    // Replace with your actual RPC URL
    let rpc_url = env::var("ETH_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
    let provider = Provider::<Http>::try_from(rpc_url)?;
    let chain_id = provider.get_chainid().await?;

    if is_initiator {
        // Initiator Node
        println!(
            "Node {} (Initiator) prompting for transaction details...",
            index
        );
        print!("Enter target Ethereum address (0x...): ");
        io::stdout().flush()?;
        let mut target_address_str = String::new();
        stdin().read_line(&mut target_address_str)?;
        let target_address = Address::from_str(target_address_str.trim())?;

        print!("Enter amount to transfer in Wei: ");
        io::stdout().flush()?;
        let mut amount_str = String::new();
        stdin().read_line(&mut amount_str)?;
        let amount = U256::from_dec_str(amount_str.trim())?;

        println!(
            "Node {} preparing transaction: {} Wei to {}",
            index, amount, target_address
        );

        // Estimate gas (optional but recommended)
        let gas_price = provider.get_gas_price().await?;
        let nonce = provider.get_transaction_count(eth_address, None).await?; // Use derived address

        let mut tx = TransactionRequest::new()
            .to(target_address)
            .value(amount)
            .from(eth_address) // Set the FROST group address as sender
            .gas_price(gas_price)
            .nonce(nonce)
            .chain_id(chain_id.as_u64()); // Set chain ID

        let gas_estimate = provider.estimate_gas(&tx.clone().into(), None).await?;
        tx = tx.gas(gas_estimate); // Set estimated gas

        println!("Node {} prepared TxRequest: {:?}", index, tx);

        // Get the hash to sign (EIP-155)
        let sighash = tx.sighash(); // This computes the hash correctly
        tx_hash_bytes = sighash.as_bytes().to_vec();
        transaction_request = Some(tx); // Store the unsigned tx request

        // Broadcast the transaction hash
        let tx_message = TxMessage {
            tx_hash_bytes: tx_hash_bytes.clone(),
        };
        let wrapped_tx_msg = MessageWrapper::SignTx(tx_message);
        broadcast(index, total, &wrapped_tx_msg).await;
    } else {
        // Participant Node waits for TX message (blocking)
        println!(
            "Node {} waiting for transaction message from initiator...",
            index
        );
        let received_tx_msgs = receive_messages(
            listener.clone(),
            1,
            None, // No timeout for receiving the TX hash
            |msg| match msg {
                MessageWrapper::SignTx(m) => Some(m),
                _ => None,
            },
        )
        .await?;
        tx_hash_bytes = received_tx_msgs[0].tx_hash_bytes.clone();
        println!(
            "Node {} received transaction hash: {}",
            index,
            hex::encode(&tx_hash_bytes)
        );
    }

    // --- FROST Signing Rounds ---
    let (my_nonce, my_commitment) = frost_round1::commit(key_package.signing_share(), &mut rng);

    // Broadcast commitment
    let commitment_message = CommitmentMessage {
        sender_identifier: my_identifier,
        commitment: my_commitment.clone(),
    };
    let wrapped_commit_msg = MessageWrapper::SignCommitment(commitment_message);
    broadcast(index, total, &wrapped_commit_msg).await;

    // Receive commitments from others (with timeout)
    println!(
        "Node {} receiving commitments (timeout: {:?})...",
        index, network_timeout
    );
    let received_commit_msgs = receive_messages(
        listener.clone(),
        (total - 1) as usize,
        Some(network_timeout), // Apply timeout
        |msg| match msg {
            MessageWrapper::SignCommitment(m) => Some(m),
            _ => None,
        },
    )
    .await?;

    let mut commitments_map = BTreeMap::new();
    commitments_map.insert(my_identifier, my_commitment); // Add self
    for msg in received_commit_msgs {
        commitments_map.insert(msg.sender_identifier, msg.commitment);
        println!(
            "Node {} received commitment from {:?}",
            index, msg.sender_identifier
        );
    }

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
        // Initiator selects signers MANUALLY
        // ... (Signer selection logic identical to solana_dkg.rs) ...
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
                                if available_other_signer_ids.contains(&id) {
                                    if !temp_selected_others.contains(&id) {
                                        temp_selected_others.push(id);
                                    } else {
                                        eprintln!("Error: Index {} entered more than once.", idx);
                                        input_valid = false;
                                        break;
                                    }
                                } else if id == my_identifier {
                                    eprintln!(
                                        "Error: Initiator (Node {}) is already included.",
                                        index
                                    );
                                    input_valid = false;
                                    break;
                                } else {
                                    eprintln!(
                                        "Error: Participant {} not available/committed.",
                                        idx
                                    );
                                    input_valid = false;
                                    break;
                                }
                            }
                            Err(_) => {
                                eprintln!("Error: Invalid index {}.", idx);
                                input_valid = false;
                                break;
                            }
                        },
                        Err(_) => {
                            eprintln!("Error: Invalid input '{}'.", part);
                            input_valid = false;
                            break;
                        }
                    }
                }

                if input_valid {
                    manually_selected_ids.extend(temp_selected_others);
                    break;
                }
            }
        } else {
            println!("Node {} is the only signer required (threshold 1).", index);
        }

        manually_selected_ids.sort();
        selected_signer_identifiers = manually_selected_ids;

        if selected_signer_identifiers.len() != threshold as usize {
            return Err(format!(
                "Internal Error: Selection resulted in {} signers, expected {}.",
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
        broadcast(index, total, &selection_msg).await;
    } else {
        // Participants wait for selection message (blocking)
        println!(
            "Node {} waiting for signer selection from initiator...",
            index
        );
        let received_selection = receive_messages(
            listener.clone(),
            1,
            None, // No timeout for selection message
            |msg| match msg {
                MessageWrapper::SignerSelection(m) => Some(m),
                _ => None,
            },
        )
        .await?;
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
        // Use the tx_hash_bytes as the message
        let signing_package = SigningPackage::new(final_commitments.clone(), &tx_hash_bytes);

        println!("Node {} starting FROST Round 2 (Sign)...", index);
        let my_signature_share = frost_round2::sign(&signing_package, &my_nonce, &key_package)?;
        println!("Node {} generated signature share.", index);

        if is_initiator {
            // Initiator logic
            let mut signature_shares_map = BTreeMap::new();
            signature_shares_map.insert(my_identifier, my_signature_share.clone());
            println!("Node {} added its own share to map.", index);

            let shares_to_receive = selected_signer_identifiers
                .iter()
                .filter(|&&id| id != my_identifier)
                .count();

            println!(
                "Node {} waiting for signature shares from {} selected participants...",
                index, shares_to_receive
            );

            if shares_to_receive > 0 {
                // Receive shares (blocking - selected signers *must* respond)
                let received_share_msgs = receive_messages(
                    listener.clone(),
                    shares_to_receive,
                    None, // No timeout for receiving shares
                    |msg| match msg {
                        MessageWrapper::SignShare(m) => Some(m),
                        _ => None,
                    },
                )
                .await?;

                for msg in received_share_msgs {
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

            if signature_shares_map.len() != threshold as usize {
                return Err(format!(
                    "Error: Not enough shares collected. Got {}, expected {}",
                    signature_shares_map.len(),
                    threshold
                )
                .into());
            }
            println!(
                "Node {} collected all {} required signature shares.",
                index,
                signature_shares_map.len()
            );

            // Aggregation (only by Initiator)
            println!("Node {} aggregating partial signatures...", index);
            // Explicitly type group_signature
            let group_signature: frost::Signature =
                frost::aggregate(&signing_package, &signature_shares_map, &pubkey_package)?;
            println!("Node {} aggregation successful!", index);

            // --- Convert FROST signature to Ethereum signature (r, s, v) ---
            // frost-secp256k1 signature has R (point) and z (scalar 's')
            let r_point = group_signature.R(); // Returns &ProjectivePoint
            let z_scalar = group_signature.z(); // Returns &Scalar

            // Get r from the x-coordinate of R
            // Convert R to affine, get x-coordinate (FieldBytes), then convert to standard byte array
            let r_bytes: [u8; 32] = r_point
                .to_affine() // Convert ProjectivePoint to AffinePoint
                .x() // Get the x-coordinate (FieldBytes)
                .into(); // Convert GenericArray to [u8; 32] using Into trait

            // Get s from the z scalar
            let s_bytes: [u8; 32] = z_scalar.to_bytes().into(); // Convert Scalar to [u8; 32]

            // Try both recovery IDs (0 and 1)
            let mut final_v: Option<u64> = None;
            for potential_v in [0u64, 1u64] {
                let eth_sig = EthSignature {
                    r: U256::from_big_endian(&r_bytes),
                    s: U256::from_big_endian(&s_bytes),
                    v: potential_v + 27 + (chain_id.as_u64() * 2 + 35), // EIP-155 v calculation
                };
                // Verify if this signature recovers the correct address
                let recovered_address = eth_sig.recover(H256::from_slice(&tx_hash_bytes));
                match recovered_address {
                    Ok(addr) if addr == eth_address => {
                        final_v = Some(eth_sig.v); // Found the correct v
                        println!(
                            "Successfully recovered address {:?} with v={}",
                            addr, eth_sig.v
                        );
                        break;
                    }
                    Ok(addr) => {
                        println!("Recovered wrong address {:?} with v={}", addr, eth_sig.v);
                    }
                    Err(e) => {
                        println!("Failed to recover address with v={}: {}", eth_sig.v, e);
                    }
                }
            }

            let v = final_v.ok_or("Failed to find correct recovery ID (v)")?;
            let final_eth_signature = EthSignature {
                r: U256::from_big_endian(&r_bytes),
                s: U256::from_big_endian(&s_bytes),
                v,
            };

            println!(
                "Node {} final Ethereum signature: r={}, s={}, v={}",
                index,
                hex::encode(r_bytes),
                hex::encode(s_bytes),
                v
            );

            // Broadcast the aggregated signature components
            let agg_sig_msg = MessageWrapper::SignAggregated(AggregatedSignatureMessage {
                r: r_bytes,
                s: s_bytes,
                v: v as u8, // Assuming v fits in u8 (should for EIP-155)
            });
            broadcast(index, total, &agg_sig_msg).await;

            // Add signature to transaction and send
            if let Some(tx_req) = transaction_request {
                // RLP encode the transaction with the signature
                let signed_tx_bytes = tx_req.rlp_signed(&final_eth_signature);

                println!("\n--- Transaction Prepared ---");
                println!("Signed RLP: {}", hex::encode(&signed_tx_bytes));
                println!("Attempting to send transaction...");

                match provider.send_raw_transaction(signed_tx_bytes).await {
                    Ok(pending_tx) => {
                        println!(
                            "Transaction successfully sent! TxHash: {:?}",
                            pending_tx.tx_hash()
                        );
                        // Optionally wait for confirmation
                        // match pending_tx.confirmations(1).await {
                        //     Ok(Some(receipt)) => println!("Transaction confirmed: {:?}", receipt),
                        //     Ok(None) => println!("Transaction dropped from mempool?"),
                        //     Err(e) => println!("Error waiting for confirmation: {}", e),
                        // }
                    }
                    Err(e) => {
                        println!("Transaction failed to send: {}", e);
                        // Provide more context if possible
                        // Match on ProviderError to access underlying error kinds
                        match e {
                            ethers_providers::ProviderError::JsonRpcClientError(rpc_err_box) => {
                                // Attempt to downcast to access specific RPC error details
                                // Note: The exact type might depend on the underlying JsonRpcClient implementation
                                // Let's try accessing common methods first if downcasting is complex.
                                // The Display impl often provides useful info.
                                eprintln!("RPC Client Error: {}", rpc_err_box);
                                // Remove downcasting attempts
                                // // Example: If using jsonrpsee, you might try downcasting
                                // if let Some(ws_err) =
                                //     rpc_err_box.downcast_ref::<jsonrpsee_core::Error>()
                                // {
                                //     eprintln!("  (Underlying jsonrpsee error: {})", ws_err);
                                // } else if let Some(http_err) =
                                //     rpc_err_box.downcast_ref::<reqwest::Error>()
                                // {
                                //     eprintln!("  (Underlying reqwest error: {})", http_err);
                                // }
                            }
                            ethers_providers::ProviderError::EnsError(ens_err) => {
                                eprintln!("ENS Error: {}", ens_err);
                            }
                            ethers_providers::ProviderError::EnsNotOwned(ens_not_owned_err) => {
                                println!("ENS Not Owned Error: {}", ens_not_owned_err);
                            }
                            ethers_providers::ProviderError::SerdeJson(serde_err) => {
                                println!("Serde JSON Error: {}", serde_err);
                            }
                            ethers_providers::ProviderError::HexError(hex_err) => {
                                println!("Hex Error: {}", hex_err);
                            }
                            ethers_providers::ProviderError::HTTPError(http_err) => {
                                println!("HTTP Error: {}", http_err);
                            }
                            ethers_providers::ProviderError::CustomError(custom_err) => {
                                println!("Custom Provider Error: {}", custom_err);
                            }
                            ethers_providers::ProviderError::UnsupportedRPC => {
                                println!("Unsupported RPC method.");
                            }
                            ethers_providers::ProviderError::UnsupportedNodeClient => {
                                println!("Unsupported Node Client.");
                            }
                            // Add other variants as needed or use a wildcard
                            _ => {
                                println!("Other Provider Error: {}", e);
                            }
                        }
                    }
                }
            } else {
                return Err(format!("Node {} transaction request object was lost", index).into());
            }
        } else {
            // Participant Node logic (Selected)
            println!("\n--- Node {} Action Required ---", index);
            println!("Received request to sign the following transaction hash (hex):");
            println!("{}", hex::encode(&tx_hash_bytes));
            print!("Press ENTER to confirm signing and send your share to initiator: ");
            io::stdout().flush()?;
            let mut confirmation = String::new();
            stdin().read_line(&mut confirmation)?; // Sync input is okay here

            println!("Node {} sending signature share to initiator...", index);
            let share_message = ShareMessage {
                sender_identifier: my_identifier,
                share: my_signature_share,
            };
            let wrapped_share_msg = MessageWrapper::SignShare(share_message);
            // TODO: Determine initiator address dynamically if not always node 1
            let initiator_addr = "127.0.0.1:10001"; // Default to 10001 for initiator
            send_to(&initiator_addr, &wrapped_share_msg).await;
            println!("Node {} sent share.", index);

            // Wait for aggregated signature from initiator (blocking)
            println!(
                "Node {} waiting for aggregated signature from initiator...",
                index
            );
            let received_agg_sig_msg = receive_messages(
                listener.clone(),
                1,
                None, // No timeout for final signature
                |msg| match msg {
                    MessageWrapper::SignAggregated(m) => Some(m),
                    _ => None,
                },
            )
            .await?;

            let agg_sig = &received_agg_sig_msg[0];
            println!(
                "Node {} received aggregated signature: r={}, s={}, v={}",
                index,
                hex::encode(agg_sig.r),
                hex::encode(agg_sig.s),
                agg_sig.v
            );
            // Optionally verify signature locally
            // let eth_sig = EthSignature { r: U256::from_big_endian(&agg_sig.r), s: U256::from_big_endian(&agg_sig.s), v: agg_sig.v as u64 };
            // match eth_sig.recover(H256::from_slice(&tx_hash_bytes)) {
            //     Ok(addr) if addr == eth_address => println!("Node {} successfully verified aggregated signature.", index),
            //     Ok(addr) => println!("Node {} verification failed: recovered wrong address {:?}", index, addr),
            //     Err(e) => println!("Node {} verification failed: recovery error {}", index, e),
            // }
        }
    } else {
        // Node was NOT selected for signing
        println!("Node {} was not selected for signing.", index);
        // Wait for aggregated signature from initiator (blocking)
        println!(
            "Node {} waiting for aggregated signature from initiator...",
            index
        );
        let received_agg_sig_msg = receive_messages(
            listener.clone(),
            1,
            None, // No timeout for final signature
            |msg| match msg {
                MessageWrapper::SignAggregated(m) => Some(m),
                _ => None,
            },
        )
        .await?;

        let agg_sig = &received_agg_sig_msg[0];
        println!(
            "Node {} received aggregated signature: r={}, s={}, v={}",
            index,
            hex::encode(agg_sig.r),
            hex::encode(agg_sig.s),
            agg_sig.v
        );
    }

    println!("\nProcess completed for Node {}.", index);
    Ok(())
}
