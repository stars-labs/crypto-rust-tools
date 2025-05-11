use crate::protocal::signal::WebRTCMessage;
use crate::utils::peer::send_webrtc_message;
use crate::utils::state::AppState;
use crate::utils::state::DkgState; // Import DkgState directly from solnana_mpc_frost
use frost_core::keys::PublicKeyPackage;
use frost_core::keys::dkg::{part1, part3, round1, round2}; // Removed unused 'part2'
use frost_ed25519::Ed25519Sha512;
use frost_ed25519::rand_core::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// Helper function to hash serializable data
pub fn hash_data<T: Serialize>(data: &T) -> String {
    match bincode::serde::encode_to_vec(data, bincode::config::standard()) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        }
        Err(_) => "SerializationError".to_string(),
    }
}

// Handle DKG Round 1 Initialization
pub async fn handle_trigger_dkg_round1(
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
) {
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

        let my_identifier = match identifier_map.get(&self_peer_id) {
            Some(id) => *id,
            None => {
                guard.log.push(format!(
                    "Error: Own identifier not found in map for peer_id {}",
                    self_peer_id
                ));
                guard.dkg_state = DkgState::Failed("Identifier not found".to_string());
                return; // Exit task
            }
        };

        // --- Call frost_core::keys::dkg::part1 ---
        let mut rng = OsRng;
        let part1_result =
            part1::<Ed25519Sha512, _>(my_identifier, session.total, session.threshold, &mut rng);

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
                guard.log.push(format!(
                    "DEBUG: Stored own Part1 Package (Hash: {}) to received_dkg_packages.",
                    hash_data(&public_package)
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
        for target_peer_id_ref in &participants {
            // Renamed to avoid conflict in spawn
            if target_peer_id_ref != &self_peer_id {
                // Compare with reference
                match send_webrtc_message(target_peer_id_ref, &dkg_msg, state.clone()).await {
                    Ok(_) => {
                        sent_count += 1;
                    }
                    Err(e) => {
                        failed_count += 1;
                        // Log individual failure immediately if desired, or wait for summary
                        let state_clone_err = state.clone();
                        let target_peer_id_clone = target_peer_id_ref.clone(); // Clone here
                        tokio::spawn(async move {
                            state_clone_err.lock().await.log.push(format!(
                                "Failed to send DKG Round 1 package to {}: {:?}",
                                target_peer_id_clone,
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
pub async fn process_dkg_round1(
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    from_peer_id: String,
    package: round1::Package<Ed25519Sha512>,
) {
    let mut guard = state.lock().await;
    // Ensure session and identifier map are ready before processing
    let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
        (Some(s), Some(m)) => (s.clone(), m.clone()),
        (_, _) => {
            // Match any other case, including one or both being None
            guard.log.push(format!(
                "DKG Round 1: Error processing package from {}. Session or identifier map not ready.",
                from_peer_id
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
                from_peer_id
            ));
            guard.dkg_state = DkgState::Round1InProgress;
        } else {
            let current_dkg_state = guard.dkg_state.clone(); // Clone state before logging
            guard.log.push(format!(
                "DKG Round 1: Received package from {}, but DKG state is {:?}. Ignoring.",
                from_peer_id,
                current_dkg_state // Use cloned state
            ));
            return;
        }
    }

    guard.log.push(format!(
        "Processing DKG Round 1 package from {}",
        from_peer_id
    ));

    let from_identifier = match identifier_map.get(&from_peer_id) {
        Some(id) => *id,
        None => {
            guard.log.push(format!(
                "Error: Identifier not found in map for peer_id {} during DKG Round 1 processing",
                from_peer_id
            ));
            // Optionally set DKG to failed state or just log and return
            return;
        }
    };

    // Check if we already have a package from this identifier
    if guard.received_dkg_packages.contains_key(&from_identifier) {
        guard.log.push(format!(
            "DKG Round 1: Duplicate package received from {}. Ignoring.",
            from_peer_id
        ));
        return;
    }

    // Store the received package
    guard
        .received_dkg_packages
        .insert(from_identifier, package.clone());
    guard.log.push(format!(
        "DEBUG: Received Part1 Package Hash from {}: {}",
        from_peer_id,
        hash_data(&package)
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
pub async fn process_dkg_round2(
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    from_peer_id: String,
    package: round2::Package<Ed25519Sha512>,
) {
    let mut guard = state.lock().await;
    // Ensure session and identifier map are ready
    let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
        (Some(s), Some(m)) => (s.clone(), m.clone()),
        _ => {
            // This shouldn't happen if Round 1/2 completed, but log just in case
            let current_dkg_state = guard.dkg_state.clone();
            guard.log.push(format!(
                "DKG Round 2: Session/Map not ready when receiving package from {}. DKG state: {:?}",
                from_peer_id, current_dkg_state
            ));
            return; // Skip processing
        }
    };

    // Only process if Round 2 is in progress
    if guard.dkg_state != DkgState::Round2InProgress {
        let current_dkg_state = guard.dkg_state.clone();
        guard.log.push(format!(
            "DKG Round 2: Received package from {}, but DKG state is {:?}. Ignoring.",
            from_peer_id, current_dkg_state
        ));
        return;
    }

    guard.log.push(format!(
        "Processing DKG Round 2 package from {}",
        from_peer_id
    ));
    // --- Log Hash of Received Part 2 Package ---
    let package_hash = hash_data(&package);
    guard.log.push(format!(
        "DEBUG: Received Part2 Package Hash from {}: {}",
        from_peer_id, package_hash
    ));
    // --- End Log ---

    let sender_identifier = match identifier_map.get(&from_peer_id) {
        Some(id) => *id,
        None => {
            guard.log.push(format!(
                "Error: Identifier not found for sender {} in Round 2.",
                from_peer_id
            ));
            guard.dkg_state = DkgState::Failed(format!("Identifier missing for {}", from_peer_id));
            return; // Skip processing
        }
    };

    // Store the received package
    guard
        .received_dkg_round2_packages
        .insert(sender_identifier, package);

    // --- FIX: Only count packages from other participants (exclude self) ---
    let my_identifier = match identifier_map.get(&guard.peer_id) {
        Some(id) => *id,
        None => {
            guard.log.push(
                "Error: Own identifier not found in identifier_map during DKG Round 2.".to_string(),
            );
            guard.dkg_state = DkgState::Failed("Own identifier missing".to_string());
            return;
        }
    };

    // Count only packages from other participants
    let filtered_count = guard
        .received_dkg_round2_packages
        .iter()
        .filter(|(id, _)| **id != my_identifier)
        .count();
    let expected_packages = session.total as usize - 1;
    guard.log.push(format!(
        "DKG Round 2: Received {}/{} packages from other participants.",
        filtered_count, expected_packages
    ));

    // --- Check for Round 2 completion and Trigger Part 3 ---
    if filtered_count == expected_packages {
        guard.log.push("All DKG Round 2 packages from other participants received. Proceeding to Part 3 (Finalization)...".to_string());

        // --- Execute DKG Part 3 ---
        let round2_secret_package_opt = guard.round2_secret_package.clone(); // Clone needed data
        // --- FIX: Filter Round 1 packages similar to Round 2 ---
        let filtered_round1: BTreeMap<_, _> = guard
            .received_dkg_packages // Use the map containing N packages
            .iter()
            .filter(|(id, _)| **id != my_identifier) // Exclude self
            .map(|(id, pkg)| (*id, pkg.clone()))
            .collect();
        // --- End FIX ---
        // Only include packages from other participants (already done)
        let filtered_round2: BTreeMap<_, _> = guard
            .received_dkg_round2_packages
            .iter()
            .filter(|(id, _)| **id != my_identifier)
            .map(|(id, pkg)| (*id, pkg.clone()))
            .collect();

        // --- Add More Detailed Logging Right Before part3 ---
        guard
            .log
            .push("--- Pre-part3 Input Verification ---".to_string());
        guard
            .log
            .push(format!("  My Identifier: {:?}", my_identifier));
        if let Some(secret) = &round2_secret_package_opt {
            guard.log.push(format!(
                "  Round 2 Secret Package Identifier: {:?}",
                secret.identifier()
            ));
        } else {
            guard.log.push("  Round 2 Secret Package: None".to_string());
        }
        // --- FIX: Log filtered Round 1 packages ---
        guard.log.push(format!(
            "  Round 1 Packages (Filtered Keys): {:?}",
            filtered_round1.keys().collect::<Vec<_>>(),
        ));
        guard.log.push(format!(
            "  Round 1 Packages (Filtered Count): {}",
            filtered_round1.len()
        ));
        // --- End FIX ---
        guard.log.push(format!(
            "  Round 2 Packages (Filtered Keys): {:?}",
            filtered_round2.keys().collect::<Vec<_>>(),
        ));
        guard.log.push(format!(
            "  Round 2 Packages (Filtered Count): {}",
            filtered_round2.len()
        ));
        guard
            .log
            .push("--- End Pre-part3 Input Verification ---".to_string());
        // --- End Detailed Logging ---

        // --- Add Type and Count Logging from References ---
        let round2_secret_ref_opt = round2_secret_package_opt.as_ref(); // Get optional ref
        // --- FIX: Use filtered Round 1 map ---
        let round1_packages_ref = &filtered_round1;
        // --- End FIX ---
        let round2_packages_ref = &filtered_round2;

        // Calculate expected counts based on session total (N)
        // --- FIX: Both Round 1 and Round 2 expect N-1 packages for part3 ---
        let expected_part3_round1_count = session.total as usize - 1; // Expect N-1 packages
        let expected_part3_round2_count = session.total as usize - 1; // Expect N-1 packages
        // --- End FIX ---

        guard
            .log
            .push("--- Pre-part3 Type/Count Verification (Refs) ---".to_string());
        if let Some(secret_ref) = round2_secret_ref_opt {
            guard.log.push(format!(
                "  Type R2SecretRef: {}",
                std::any::type_name::<&frost_core::keys::dkg::round2::SecretPackage<Ed25519Sha512>>(
                )
            ));
            guard.log.push(format!(
                "  Identifier R2SecretRef: {:?}",
                secret_ref.identifier()
            ));
        } else {
            guard.log.push("  Type R2SecretRef: None".to_string());
        }
        guard.log.push(format!(
            "  Type R1MapRef: {}",
            std::any::type_name::<
                &BTreeMap<
                    frost_core::Identifier<Ed25519Sha512>,
                    frost_core::keys::dkg::round1::Package<Ed25519Sha512>,
                >,
            >()
        ));
        guard.log.push(format!(
            "  Count R1MapRef: {} (Expected: {})",
            round1_packages_ref.len(),
            expected_part3_round1_count // Log expected N-1
        ));
        guard.log.push(format!(
            "  Type R2MapRef: {}",
            std::any::type_name::<
                &BTreeMap<
                    frost_core::Identifier<Ed25519Sha512>,
                    frost_core::keys::dkg::round2::Package<Ed25519Sha512>,
                >,
            >()
        ));
        guard.log.push(format!(
            "  Count R2MapRef: {} (Expected: {})",
            round2_packages_ref.len(),
            expected_part3_round2_count // Log expected N-1
        ));
        guard
            .log
            .push("--- End Pre-part3 Type/Count Verification (Refs) ---".to_string());
        // --- End Type/Count Logging ---

        // --- Strict Check: Ensure correct number of packages for part3 ---
        if round1_packages_ref.len() != expected_part3_round1_count {
            guard.log.push(format!(
                "Error: Incorrect number of Round 1 packages for part3. Expected {}, Got {}. Aborting.",
                expected_part3_round1_count, round1_packages_ref.len()
            ));
            guard.dkg_state = DkgState::Failed("Part 3 Round 1 package count mismatch".to_string()); // Corrected string
            return; // Skip part3 call
        }
        if round2_packages_ref.len() != expected_part3_round2_count {
            guard.log.push(format!(
                "Error: Incorrect number of Round 2 packages for part3. Expected {}, Got {}. Aborting.",
                expected_part3_round2_count, round2_packages_ref.len()
            ));
            guard.dkg_state = DkgState::Failed("Part 3 Round 2 package count mismatch".to_string()); // Corrected string
            return; // Skip part3 call
        }
        // --- End Strict Check ---

        let round2_secret_package = match round2_secret_package_opt {
            Some(secret) => secret,
            None => {
                guard
                    .log
                    .push("Error: Local DKG Round 2 secret share missing for Part 3.".to_string());
                guard.dkg_state = DkgState::Failed("Missing Round 2 secret share".to_string()); // Corrected string
                return; // Skip
            }
        };

        // Drop guard before potentially long computation
        drop(guard);

        // Call part3 with references derived from clones
        let part3_result = part3::<Ed25519Sha512>(
            &round2_secret_package, // Use the owned secret from match
            round1_packages_ref,    // Use the ref to the filtered R1 clone (N-1 packages)
            round2_packages_ref,    // Use the ref to the filtered R2 clone (N-1 packages)
        );

        // Re-acquire lock to update state
        let mut guard = state.lock().await;
        // --- FIX: Log filtered Round 1 keys ---
        guard.log.push(format!(
            "part3: my_identifier={:?}, round1_keys={:?}, round2_keys={:?}",
            my_identifier,
            filtered_round1.keys().collect::<Vec<_>>(), // Log filtered R1 keys
            filtered_round2.keys().collect::<Vec<_>>(), // Log filtered R1 keys
        ));
        // --- End FIX ---

        match part3_result {
            Ok((key_package, group_public_key)) => {
                guard.log.push(format!(
                    "DKG Part 3 successful! Generated KeyPackage for identifier {:?} and GroupPublicKey.",
                    key_package.identifier()
                ));
                guard.key_package = Some(key_package);
                guard.group_public_key = Some(group_public_key.clone()); // Clone group_public_key here
                guard.dkg_state = DkgState::Complete; // Mark DKG as complete

                // Generate Solana public key from group key
                generate_solana_public_key(&mut *guard, &group_public_key);

                // Optionally clear intermediate DKG data now
                guard.dkg_part1_public_package = None;
                guard.dkg_part1_secret_package = None;
                guard.received_dkg_packages.clear();
                guard.round2_secret_package = None;
                guard.received_dkg_round2_packages.clear();
                guard.queued_dkg_round1.clear();
                guard
                    .log
                    .push("DKG process completed successfully.".to_string());
            }
            Err(e) => {
                // Log the inputs again on error for easier comparison
                guard.log.push(format!("DKG Part 3 failed: {:?}", e));
                // --- FIX: Log filtered Round 1 keys on error ---
                guard.log.push(format!(
                    "Failed part3 inputs: R1 keys={:?}, R2 keys={:?}",
                    filtered_round1.keys().collect::<Vec<_>>(), // Log filtered R1 keys
                    filtered_round2.keys().collect::<Vec<_>>(), // Log filtered R1 keys
                ));
                guard.dkg_state = DkgState::Failed(format!("DKG Part 3 Error: {:?}", e));
            }
        }
    }
}

// Helper function to call the solana module
fn generate_solana_public_key<G>(guard: &mut AppState<G>, group_public_key: &PublicKeyPackage<G>)
where
    G: frost_core::Ciphersuite,
{
    // Call the solana module's function
    if let Some(pubkey) =
        crate::utils::solana_helper::derive_solana_public_key::<G>(group_public_key)
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
}

/// Handle trigger for DKG Round 2 (Share distribution)
pub async fn handle_trigger_dkg_round2(state: Arc<Mutex<AppState<Ed25519Sha512>>>) {
    let mut state_guard = state.lock().await;
    state_guard
        .log
        .push("Starting share distribution phase...".to_string());

    // Extract the data we need before mutating state_guard
    let session_clone = state_guard.session.clone();
    let own_peer_id = state_guard.peer_id.clone();
    let round1_data_exists = state_guard.dkg_part1_public_package.is_some();

    // Now handle the logic based on extracted data
    if let Some(session) = session_clone {
        if round1_data_exists {
            state_guard
                .log
                .push("Generating round 2 packages...".to_string());

            // Generate round 2 packages for each participant
            let participant_peers: Vec<String> = session
                .participants
                .into_iter()
                .filter(|p| *p != own_peer_id)
                .collect();

            // Log for each peer
            for peer_id in &participant_peers {
                state_guard
                    .log
                    .push(format!("Sending share to peer {}...", peer_id));
            }

            // This is where you'd actually create and send the shares
            // For now we'll just log it
        } else {
            state_guard
                .log
                .push("Error: Cannot start DKG round 2 - missing round 1 data".to_string());
            state_guard.dkg_state = DkgState::Failed("Missing round 1 data".to_string());
        }
    }
}

/// Finalize the DKG process
pub async fn finalize_dkg(state: Arc<Mutex<AppState<Ed25519Sha512>>>) {
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
    // and we need to use our own dkg_part1_public_package.
    // Otherwise, we are a participant and we need to find our package in received_dkg_packages.
    // This logic seems to be based on an older model.
    // The current model is that dkg_part1_public_package holds *our* public package,
    // and received_dkg_packages holds *all* public packages, including our own.

    // Collect all round 1 public packages.
    // This should already include our own if handle_trigger_dkg_round1 worked correctly.
    let round1_packages = guard.received_dkg_packages.clone(); // Clone to satisfy borrow checker for part3

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
        &round1_packages, // Should be all public packages from round 1
        &round2_packages, // Should be all shares received in round 2
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
            generate_solana_public_key(&mut *guard, &group_public_key);

            // Optionally clear intermediate DKG data now
            guard.dkg_part1_public_package = None;
            guard.dkg_part1_secret_package = None;
            guard.received_dkg_packages.clear();
            guard.round2_secret_package = None;
            guard.received_dkg_round2_packages.clear();
            guard.queued_dkg_round1.clear();
            guard
                .log
                .push("DKG process completed successfully.".to_string());
        }
        Err(e) => {
            // Log the inputs again on error for easier comparison
            guard.log.push(format!("DKG Finalization failed: {:?}", e));
            guard.dkg_state = DkgState::Failed(format!("DKG Finalization Error: {:?}", e));
        }
    }
}
