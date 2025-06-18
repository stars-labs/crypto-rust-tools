use crate::protocal::signal::WebRTCMessage;
use crate::utils::eth_helper;
use crate::utils::device::send_webrtc_message;
use crate::utils::state::AppState;
use serde_json;
use chrono;
use crate::utils::state::DkgState; // Import DkgState directly from solnana_mpc_frost
use frost_core::keys::dkg::{part1, part2, part3, round1, round2};
use frost_core::{Ciphersuite, keys::PublicKeyPackage};
use frost_ed25519::Ed25519Sha512; // Added for ciphersuite check
use frost_ed25519::rand_core::OsRng;
use frost_secp256k1::Secp256K1Sha256; // Added for ciphersuite check // Added for Ethereum address derivation

use std::any::TypeId; // Added for ciphersuite check
use std::mem; // Added for unsafe transmute
use std::sync::Arc;
use tokio::sync::Mutex;

// Handle DKG Round 1 Initialization
pub async fn handle_trigger_dkg_round1<C>(state: Arc<Mutex<AppState<C>>>, self_device_id: String)
where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar:
        Send + Sync,
{
    // --- Extract data under lock ---
    let extracted_data = {
        // Scope for guard
        let mut guard = state.lock().await;
        if guard.dkg_state != DkgState::Round1InProgress {
            guard
                .log
                .push("DKG Round 1 triggered but state is not Round1InProgress.".to_string());
            // Optionally return if state is not as expected
            // return;
        }
        guard.log.push("Starting DKG Round 1 logic...".to_string());

        // Ensure session and identifier_map are available
        let session = match &guard.session {
            Some(s) => s.clone(),
            None => {
                guard
                    .log
                    .push("Error: Session not found for DKG Round 1".to_string());
                guard.dkg_state = DkgState::Failed("Session not found".to_string());
                return; // Exit task
            }
        };
        let identifier_map = match &guard.identifier_map {
            Some(m) => m.clone(),
            None => {
                guard
                    .log
                    .push("Error: Identifier map not found for DKG Round 1".to_string());
                guard.dkg_state = DkgState::Failed("Identifier map not found".to_string());
                return; // Exit task
            }
        };

        let my_identifier = match identifier_map.get(&self_device_id) {
            Some(id) => *id,
            None => {
                guard.log.push(format!(
                    "Error: Own identifier not found in map for device_id {}",
                    self_device_id
                ));
                guard.dkg_state = DkgState::Failed("Identifier not found".to_string());
                return; // Exit task
            }
        };

        // --- Call frost_core::keys::dkg::part1 ---
        let mut rng = OsRng;
        let part1_result = part1::<C, _>(my_identifier, session.total, session.threshold, &mut rng);

        match part1_result {
            Ok((secret_package, public_package)) => {
                guard.log.push(format!(
                    "DKG Part 1 successful for identifier {:?}",
                    my_identifier
                ));
                // Store the secret package
                guard.dkg_part1_secret_package = Some(secret_package);
                // Store the public package (this is what needs to be broadcast and also stored locally)
                guard.dkg_part1_public_package = Some(public_package.clone()); // Clone for local storage

                // Add own public_package to received_dkg_packages
                guard
                    .received_dkg_packages
                    .insert(my_identifier, public_package.clone());
                let bytes = public_package.serialize().unwrap();
                guard.log.push(format!(
                    "DEBUG: Stored own Part1 Package (Hash: {}) to received_dkg_packages.",
                    hex::encode(&bytes)
                ));

                let dkg_msg = WebRTCMessage::DkgRound1Package {
                    package: public_package, // Use the original public_package for sending
                };
                let participants = session.participants.clone();
                // Return data needed for async operations
                Ok((participants, dkg_msg))
            }
            Err(e) => {
                guard.log.push(format!("DKG Part 1 failed: {:?}", e));
                guard.dkg_state = DkgState::Failed(format!("DKG Part 1 Error: {:?}", e));
                Err(()) // Indicate failure
            }
        }
    }; // --- MutexGuard `guard` is dropped here ---

    // --- Perform async operations if successful ---
    if let Ok((participants, dkg_msg)) = extracted_data {
        let mut sent_count = 0;
        let mut failed_count = 0;
        // Iterate by reference to avoid moving String
        for target_device_id_ref in &participants {
            // Renamed to avoid conflict in spawn
            if target_device_id_ref != &self_device_id {
                // Compare with reference
                match send_webrtc_message(target_device_id_ref, &dkg_msg, state.clone()).await {
                    Ok(_) => {
                        sent_count += 1;
                    }
                    Err(e) => {
                        failed_count += 1;
                        // Log individual failure immediately if desired, or wait for summary
                        let state_clone_err = state.clone();
                        let target_device_id_clone = target_device_id_ref.clone(); // Clone here
                        tokio::spawn(async move {
                            state_clone_err.lock().await.log.push(format!(
                                "Failed to send DKG Round 1 package to {}: {:?}",
                                target_device_id_clone,
                                e // Use cloned value
                            ));
                        });
                    }
                }
            }
        }

        // Log summary after all send attempts
        let state_clone = state.clone();
        tokio::spawn(async move {
            state_clone.lock().await.log.push(format!(
                "DKG Round 1 broadcast summary: {} sent successfully, {} failed.",
                sent_count, failed_count
            ));
        });
    }
}

// Handle processing of DKG Round 1 packages
pub async fn process_dkg_round1<C>(
    state: Arc<Mutex<AppState<C>>>,
    from_device_id: String,
    package: round1::Package<C>,
) where
    C: Ciphersuite,
{
    let mut guard = state.lock().await;
    // Ensure session and identifier map are ready before processing
    let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
        (Some(s), Some(m)) => (s.clone(), m.clone()),
        (_, _) => {
            // Match any other case, including one or both being None
            guard.log.push(format!(
                "DKG Round 1: Error processing package from {}. Session or identifier map not ready.",
                from_device_id
            ));
            guard.dkg_state =
                DkgState::Failed("Session/map not ready for DKG R1 processing".to_string());
            return;
        }
    };

    // Check if DKG is in the correct state
    if guard.dkg_state != DkgState::Round1InProgress && guard.dkg_state != DkgState::Idle {
        if guard.dkg_state == DkgState::Idle {
            guard.log.push(format!(
                "DKG Round 1: Received package from {} while DKG was Idle. Transitioning to Round1InProgress.",
                from_device_id
            ));
            guard.dkg_state = DkgState::Round1InProgress;
        } else {
            let current_dkg_state = guard.dkg_state.clone(); // Clone state before logging
            guard.log.push(format!(
                "DKG Round 1: Received package from {}, but DKG state is {:?}. Ignoring.",
                from_device_id,
                current_dkg_state // Use cloned state
            ));
            return;
        }
    }

    guard.log.push(format!(
        "Processing DKG Round 1 package from {}",
        from_device_id
    ));

    let from_identifier = match identifier_map.get(&from_device_id) {
        Some(id) => *id,
        None => {
            guard.log.push(format!(
                "Error: Identifier not found in map for device_id {} during DKG Round 1 processing",
                from_device_id
            ));
            // Optionally set DKG to failed state or just log and return
            return;
        }
    };

    // Check if we already have a package from this identifier
    if guard.received_dkg_packages.contains_key(&from_identifier) {
        guard.log.push(format!(
            "DKG Round 1: Duplicate package received from {}. Ignoring.",
            from_device_id
        ));
        return;
    }

    // Store the received package
    guard
        .received_dkg_packages
        .insert(from_identifier, package.clone());
    let bytes = package.serialize().unwrap();
    guard.log.push(format!(
        "DEBUG: Received Part1 Package Hash from {}: {}",
        from_device_id,
        hex::encode(&bytes),
    ));

    // Check if all packages for Round 1 have been received
    let num_participants = session.total as usize;
    let received_packages_count = guard.received_dkg_packages.len();

    guard.log.push(format!(
        "DKG Round 1: Received {}/{} packages.",
        received_packages_count, num_participants
    ));

    if received_packages_count == num_participants {
        guard.log.push(
            "All DKG Round 1 packages received. Setting state to Round1Complete.".to_string(),
        );
        guard.dkg_state = DkgState::Round1Complete;
        // The cli_node.rs ProcessDkgRound1 handler will detect Round1Complete and trigger Round 2.
    }
}

// Handle processing of DKG Round 2 packages
pub async fn process_dkg_round2<C>(
    state: Arc<Mutex<AppState<C>>>,
    from_device_id: String,
    package: round2::Package<C>,
) -> Result<bool, String>
where
    C: Ciphersuite,
{
    let mut guard = state.lock().await;
    guard.log.push(format!(
        "Processing Round 2 package from {}...",
        from_device_id
    ));

    // Need to look up the identifier for from_device_id
    let identifier_map = match &guard.identifier_map {
        Some(m) => m.clone(),
        None => return Err("Identifier map not found".to_string()),
    };

    // Get the identifier for the device
    let from_identifier = match identifier_map.get(&from_device_id) {
        Some(id) => *id,
        None => return Err(format!("Identifier not found for device {}", from_device_id)),
    };

    // Store the received package with the correct identifier type
    guard
        .received_dkg_round2_packages
        .insert(from_identifier, package);

    // Check if we have received packages from all devices
    if let Some(session) = &guard.session {
        let expected_packages = session.participants.len() - 1; // Exclude self
        let received_packages = guard.received_dkg_round2_packages.len();

        guard.log.push(format!(
            "Round 2 packages received: {}/{}",
            received_packages, expected_packages
        ));

        // Return true if we have all packages
        return Ok(received_packages >= expected_packages);
    } else {
        return Err("No active session found".to_string());
    }
}

// Helper function to call the solana module
fn generate_public_key<C>(guard: &mut AppState<C>, group_public_key: &PublicKeyPackage<C>)
where
    C: Ciphersuite,
{
    let c_type_id = TypeId::of::<C>();

    if c_type_id == TypeId::of::<Ed25519Sha512>() {
        // Call the solana module's function
        if let Some(pubkey) =
            crate::utils::solana_helper::derive_solana_public_key::<C>(group_public_key)
        {
            guard
                .log
                .push(format!("Generated Solana Public Key: {}", pubkey));
            guard.solana_public_key = Some(pubkey);
        } else {
            guard
                .log
                .push("Error generating Solana public key from group key".to_string());
        }
    } else if c_type_id == TypeId::of::<Secp256K1Sha256>() {
        let concrete_group_public_key: &PublicKeyPackage<Secp256K1Sha256> =
            unsafe { mem::transmute(group_public_key) };
        match eth_helper::derive_eth_address(concrete_group_public_key) {
            Ok(address) => {
                // Format the address to ensure it shows the complete string
                // ethers Address has a fixed 20-byte length (40 hex chars + 0x prefix)
                let address_str = format!("0x{}", hex::encode(address.as_bytes()));
                guard
                    .log
                    .push(format!("Generated Ethereum Address: 0x{:x}", address));
                guard.etherum_public_key = Some(address_str); // Store the full address
            }
            Err(e) => {
                guard
                    .log
                    .push(format!("Error generating Ethereum address: {}", e));
            }
        }
    } else {
        guard.log.push(format!(
            "Unsupported ciphersuite for public key derivation: {:?}",
            c_type_id
        ));
    }
}

/// Handle trigger for DKG Round 2 (Share distribution)
pub async fn handle_trigger_dkg_round2<C>(
    state: Arc<Mutex<AppState<C>>>,
) -> Result<(), anyhow::Error>
where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar:
        Send + Sync,
{
    // --- Extract data and perform part2 under lock ---
    let round2_broadcast_data = {
        // Scope for state_guard
        let mut state_guard = state.lock().await;
        state_guard
            .log
            .push("handle_trigger_dkg_round2: Starting share distribution phase...".to_string());

        // Check DKG state
        if state_guard.dkg_state != DkgState::Round1Complete {
            let err_msg = format!(
                "DKG Round 2 triggered but state is {:?}, not Round1Complete.",
                state_guard.dkg_state
            );
            state_guard.log.push(err_msg.clone());
            return Err(anyhow::anyhow!(err_msg));
        }

        // Retrieve necessary components from state
        let secret_package_p1 = match state_guard.dkg_part1_secret_package.as_ref() {
            Some(sp) => sp,
            None => {
                let err_msg = "DKG Round 2 Error: DKG Part 1 secret package not found.".to_string();
                state_guard.log.push(err_msg.clone());
                state_guard.dkg_state = DkgState::Failed(err_msg.clone());
                return Err(anyhow::anyhow!(err_msg));
            }
        };

        let received_packages_p1 = &state_guard.received_dkg_packages;
        if received_packages_p1.is_empty() {
            let err_msg = "DKG Round 2 Error: No DKG Part 1 packages received.".to_string();
            state_guard.log.push(err_msg.clone());
            state_guard.dkg_state = DkgState::Failed(err_msg.clone());
            return Err(anyhow::anyhow!(err_msg));
        }

        // Determine self_identifier before the iterator chain
        let self_identifier = match state_guard.identifier_map.as_ref() {
            Some(map) => match map.get(&state_guard.device_id) {
                Some(id) => *id,
                None => {
                    let err_msg = format!(
                        "DKG Round 2 Error: Self identifier not found in map for device_id {}.",
                        state_guard.device_id
                    );
                    state_guard.log.push(err_msg.clone());
                    state_guard.dkg_state = DkgState::Failed(err_msg.clone());
                    return Err(anyhow::anyhow!(err_msg));
                }
            },
            None => {
                let err_msg = "DKG Round 2 Error: Identifier map not found.".to_string();
                state_guard.log.push(err_msg.clone());
                state_guard.dkg_state = DkgState::Failed(err_msg.clone());
                return Err(anyhow::anyhow!(err_msg));
            }
        };

        let round1_packages_from_others: std::collections::BTreeMap<_, _> = received_packages_p1
            .iter()
            .filter(|(id, _)| **id != self_identifier) // Compare with pre-fetched self_identifier
            .map(|(id, pkg)| (*id, pkg.clone()))
            .collect();

        match part2(secret_package_p1.clone(), &round1_packages_from_others) {
            // Pass as reference
            Ok((round2_secret_package, round2_packages_to_send)) => {
                state_guard.log.push(
                    "DKG Round 2: Successfully generated shares (part2 complete).".to_string(),
                );
                // insert the round2_secret_package into the state
                // Store the round2 secret package, which is needed for part3 (finalization)
                state_guard.round2_secret_package = Some(round2_secret_package);

                // Prepare data for broadcasting outside the lock
                let identifier_map = match state_guard.identifier_map.as_ref() {
                    Some(map) => map.clone(),
                    None => {
                        let err_msg = "DKG Round 2 Error: Identifier map not found.".to_string();
                        state_guard.log.push(err_msg.clone());
                        state_guard.dkg_state = DkgState::Failed(err_msg.clone());
                        return Err(anyhow::anyhow!(err_msg));
                    }
                };

                let self_device_id = state_guard.device_id.clone(); // Not strictly needed for sending logic if using device_id_map

                // Update DKG state
                state_guard.dkg_state = DkgState::Round2InProgress;
                state_guard
                    .log
                    .push("DKG state transitioned to Round2InProgress.".to_string());

                Ok((round2_packages_to_send, identifier_map, self_device_id))
            }
            Err(e) => {
                let err_msg = format!("DKG Round 2 (part2) failed: {:?}", e);
                state_guard.log.push(err_msg.clone());
                state_guard.dkg_state = DkgState::Failed(err_msg.clone());
                Err(anyhow::anyhow!(err_msg))
            }
        }
    }; // --- MutexGuard `state_guard` is dropped here ---

    // --- Perform async broadcast operations if successful ---
    match round2_broadcast_data {
        Ok((round2_packages_to_send, identifier_map, _self_device_id)) => {
            // Destructure self_identifier
            let mut sent_count = 0;
            let mut failed_count = 0;

            // Create a reverse map from Identifier to DeviceId for easier lookup
            // This avoids iterating the identifier_map for each package to send.
            let device_id_map: std::collections::HashMap<_, _> = identifier_map
                .iter()
                .map(|(device_id, identifier)| (*identifier, device_id.clone()))
                .collect();

            for (target_identifier, package) in round2_packages_to_send {
                // Find the device_id for the target_identifier
                if let Some(target_device_id) = device_id_map.get(&target_identifier) {
                    let dkg_msg = WebRTCMessage::DkgRound2Package {
                        package: package.clone(),
                    }; // Clone package for sending

                    // Log package details before sending
                    let state_clone_log_pre = state.clone();
                    let target_device_id_clone_pre = target_device_id.clone();
                    let bytes = package.serialize().unwrap();
                    let package_hash = hex::encode(&bytes);

                    tokio::spawn(async move {
                        state_clone_log_pre.lock().await.log.push(format!(
                            "DKG Round 2: Preparing to send package (Hash: {}) to {} (identifier {:?})",
                            package_hash, target_device_id_clone_pre, target_identifier
                        ));
                    });

                    match send_webrtc_message(target_device_id, &dkg_msg, state.clone()).await {
                        Ok(_) => {
                            sent_count += 1;
                            let state_clone_log = state.clone();
                            let target_device_id_clone = target_device_id.clone();
                            tokio::spawn(async move {
                                state_clone_log.lock().await.log.push(format!(
                                    "DKG Round 2: Successfully sent package to {} (identifier {:?})",
                                    target_device_id_clone, target_identifier
                                ));
                            });
                        }
                        Err(e) => {
                            failed_count += 1;
                            let state_clone_err = state.clone();
                            let target_device_id_clone = target_device_id.clone();
                            tokio::spawn(async move {
                                state_clone_err.lock().await.log.push(format!(
                                    "DKG Round 2: Failed to send package to {} (identifier {:?}): {:?}",
                                    target_device_id_clone, target_identifier, e
                                ));
                            });
                        }
                    }
                } else {
                    failed_count += 1;
                    let state_clone_err = state.clone();
                    tokio::spawn(async move {
                        state_clone_err.lock().await.log.push(format!(
                            "DKG Round 2: Error - Device ID not found for identifier {:?}. Cannot send package.",
                            target_identifier
                        ));
                    });
                }
            }

            // Log summary
            let state_clone_summary = state.clone();
            tokio::spawn(async move {
                state_clone_summary.lock().await.log.push(format!(
                    "DKG Round 2 broadcast summary: {} packages sent successfully, {} failed.",
                    sent_count, failed_count
                ));
            });

            Ok(())
        }
        Err(e) => {
            // Error already logged by the inner scope, just propagate it
            Err(e)
        }
    }
}

/// Finalize the DKG process
pub async fn handle_finalize_dkg<C>(state: Arc<Mutex<AppState<C>>>)
where
    C: Ciphersuite,
{
    let mut guard = state.lock().await;

    guard.log.push("Finalizing DKG...".to_string());

    let secret_package = match guard.round2_secret_package.take() {
        Some(sp) => sp,
        None => {
            guard
                .log
                .push("DKG Finalization Error: Round 2 secret package not found.".to_string());
            guard.dkg_state = DkgState::Failed("Round 2 secret missing".to_string());
            return;
        }
    };

    // Clear dkg_part1_public_package and dkg_part1_secret_package as they are no longer needed
    // and to prevent accidental reuse or large data lingering in memory.
    guard.dkg_part1_public_package = None;
    guard.dkg_part1_secret_package = None;

    // Check if dkg_part1_public_package is Some, if so, it means we are the dealer/proposer
    // and we need to use our own dkg_part1_public_package.Daily
    // Otherwise, we are a participant and we need to find our package in received_dkg_packages.
    // This logic seems to be based on an older model.
    // The current model is that dkg_part1_public_package holds *our* public package,
    // and received_dkg_packages holds *all* public packages, including our own.

    // Collect all round 1 public packages.
    // This should already include our own if handle_trigger_dkg_round1 worked correctly.
    let round1_packages = guard.received_dkg_packages.clone(); // Clone to satisfy borrow checker for part3
    // Determine self_identifier before the iterator chain
    let self_identifier = match guard.identifier_map.as_ref() {
        Some(map) => match map.get(&guard.device_id) {
            Some(id) => *id,
            None => {
                let err_msg = format!(
                    "DKG Round 2 Error: Self identifier not found in map for device_id {}.",
                    guard.device_id
                );
                guard.log.push(err_msg.clone());
                guard.dkg_state = DkgState::Failed(err_msg.clone());
                return;
            }
        },
        None => {
            let err_msg = "DKG Round 2 Error: Identifier map not found.".to_string();
            guard.log.push(err_msg.clone());
            guard.dkg_state = DkgState::Failed(err_msg.clone());
            return;
        }
    };

    let round1_packages_for_others: std::collections::BTreeMap<_, _> = round1_packages
        .iter()
        .filter(|(id, _)| **id != self_identifier) // Compare with pre-fetched self_identifier
        .map(|(id, pkg)| (*id, pkg.clone()))
        .collect();

    // Collect all round 2 packages (shares sent to us by others).
    let round2_packages = guard.received_dkg_round2_packages.clone(); // Clone for part3

    guard.log.push(format!(
        "Number of Round 1 packages for finalization: {}",
        round1_packages.len()
    ));
    guard.log.push(format!(
        "Number of Round 2 packages for finalization: {}",
        round2_packages.len()
    ));

    match part3(
        &secret_package,
        &round1_packages_for_others, // Should be all public packages from round 1
        &round2_packages,            // Should be all shares received in round 2
    ) {
        Ok((key_package, group_public_key)) => {
            guard.log.push(format!(
                "DKG Finalization successful! Generated KeyPackage for identifier {:?} and GroupPublicKey.",
                key_package.identifier()
            ));
            guard.key_package = Some(key_package);
            guard.group_public_key = Some(group_public_key.clone()); // Clone group_public_key here
            guard.dkg_state = DkgState::Complete; // Mark DKG as complete

            // Generate Solana public key from group key
            generate_public_key(&mut *guard, &group_public_key);

            // Optionally clear intermediate DKG data now
            guard.dkg_part1_public_package = None;
            guard.dkg_part1_secret_package = None;
            guard.received_dkg_packages.clear();
            guard.round2_secret_package = None;
            guard.received_dkg_round2_packages.clear();

            guard
                .log
                .push("DKG process completed successfully.".to_string());
                
            // Automatically create wallet if keystore is initialized
            if let Some(keystore_arc) = guard.keystore.clone() {
                guard.log.push("🔑 Keystore detected - automatically saving key share...".to_string());
                
                // Prepare data for wallet creation
                let session_id = guard.session.as_ref().map_or_else(
                    || "unknown-session".to_string(),
                    |s| s.session_id.clone()
                );
                
                let name = format!("wallet-{}", session_id);
                let description = Some(format!(
                    "Threshold wallet created on {}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M")
                ));
                
                // Determine curve type
                let curve_type = if guard.solana_public_key.is_some() {
                    "ed25519"
                } else if guard.etherum_public_key.is_some() {
                    "secp256k1"
                } else {
                    "unknown"
                };
                
                // Clone necessary data before dropping the guard
                let key_package_json = serde_json::to_string(&guard.key_package).unwrap_or_default();
                let group_public_key_json = serde_json::to_string(&guard.group_public_key).unwrap_or_default();
                let device_id = guard.device_id.clone();
                let threshold = guard.session.as_ref().map_or(0, |s| s.threshold);
                let total_participants = guard.session.as_ref().map_or(0, |s| s.total);
                let public_address = guard.solana_public_key.clone().or_else(|| guard.etherum_public_key.clone())
                    .unwrap_or_else(|| "N/A".to_string());
                
                // Prepare key share data
                let key_share_data = serde_json::json!({
                    "key_package": key_package_json,
                    "group_public_key": group_public_key_json,
                    "session_id": session_id,
                    "device_id": device_id
                }).to_string();
                
                // Drop the guard before attempting to modify the keystore
                drop(guard);
                
                // Use atomic operation on the Arc to avoid borrowing issues
                // Convert to raw pointer (unsafe but necessary)
                let keystore_ptr = Arc::into_raw(keystore_arc) as *mut crate::keystore::Keystore;
                let result = unsafe {
                    let keystore_mut = &mut *keystore_ptr;
                    
                    // Simple password - in production you'd want a better scheme
                    let password = device_id.clone();
                    let tags = vec![curve_type.to_string()];
                    
                    keystore_mut.create_wallet(
                        &name,
                        curve_type,
                        if curve_type == "ed25519" { "solana" } else { "ethereum" },
                        &public_address,
                        threshold,
                        total_participants,
                        &group_public_key_json,
                        key_share_data.as_bytes(),
                        &password,
                        tags,
                        description,
                    )
                };
                
                // Recreate the Arc to ensure proper memory management
                let _keystore = unsafe { Arc::from_raw(keystore_ptr) };
                
                // Regain the lock to log the result
                let mut final_guard = state.lock().await;
                
                match result {
                    Ok(wallet_id) => {
                        final_guard.log.push(format!("✅ Successfully created wallet '{}' with key share!", name));
                        final_guard.log.push(format!("🔐 Wallet ID: {}", wallet_id));
                        final_guard.log.push("🔑 Password is set to your device ID".to_string());
                        final_guard.current_wallet_id = Some(wallet_id);
                    },
                    Err(e) => {
                        final_guard.log.push(format!("❌ Failed to create wallet: {}", e));
                        final_guard.log.push("📝 You can still manually run /create_wallet to try again".to_string());
                    }
                }
            }
        }
        Err(e) => {
            // Log the inputs again on error for easier comparison
            guard.log.push(format!("DKG Finalization failed: {:?}", e));
            guard.dkg_state = DkgState::Failed(format!("DKG Finalization Error: {:?}", e));
        }
    }
}

#[cfg(test)]
#[path = "dkg_test.rs"]
mod tests;
