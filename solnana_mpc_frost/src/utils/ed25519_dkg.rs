use crate::utils::peer::send_webrtc_message;
use crate::utils::signal::WebRTCMessage;
use crate::utils::state::{AppState, DkgState};
use frost_core::keys::PublicKeyPackage;
use frost_core::keys::dkg::{part1, part2, part3, round1, round2};
use frost_ed25519::Ed25519Sha512;
use frost_ed25519::rand_core::OsRng;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex as StdMutex};

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
    state: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
) {
    // --- Extract data under lock ---
    let extracted_data = {
        // Scope for guard
        let mut guard = state.lock().unwrap();
        if guard.dkg_state != DkgState::Round1InProgress {
            guard
                .log
                .push("DKG Round 1 already started or not ready.".to_string());
            return; // Exit task
        }
        guard.log.push("Starting DKG Round 1 logic...".to_string());

        let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
            (Some(s), Some(m)) => (s.clone(), m.clone()),
            _ => {
                guard.log.push(
                    "Error: Session or identifier map not ready for DKG Round 1.".to_string(),
                );
                guard.dkg_state = DkgState::Failed("Session/Map not ready".to_string());
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
                // --- Log Hash of Generated Part 1 Package ---
                let package_hash = hash_data(&public_package);
                guard.log.push(format!(
                    "DEBUG: Generated Part1 Package Hash: {}",
                    package_hash
                ));
                // --- End Log ---
                guard.local_dkg_part1_data = Some((secret_package, public_package.clone()));
                let dkg_msg = WebRTCMessage::DkgRound1Package {
                    package: public_package,
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
        // Iterate by reference to avoid moving String
        for target_peer_id in &participants {
            if target_peer_id != &self_peer_id {
                // Compare with reference
                if let Err(e) = send_webrtc_message(target_peer_id, &dkg_msg, state.clone()).await {
                    // Log error without holding lock for long
                    state.lock().unwrap().log.push(format!(
                        "Error sending DKG Round 1 package to {}: {}",
                        target_peer_id, e
                    ));
                } else {
                    sent_count += 1;
                }
            }
        }
        state.lock().unwrap().log.push(format!(
            "Broadcasted DKG Round 1 package to {} peers.",
            sent_count
        ));
    }
}

// Handle processing of DKG Round 1 packages
pub fn process_dkg_round1(
    state: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    from_peer_id: String,
    package: round1::Package<Ed25519Sha512>,
) {
    let mut guard = state.lock().unwrap();

    // Ensure session and identifier map are ready before processing
    let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
        (Some(s), Some(m)) => (s.clone(), m.clone()),
        _ => {
            guard.log.push(format!(
                "DKG Round 1: Session/Map not ready. Queuing package from {}.",
                from_peer_id
            ));
            guard.queued_dkg_round1.push((from_peer_id, package));
            return; // Skip processing for now
        }
    };

    guard.log.push(format!(
        "Processing DKG Round 1 package from {}",
        from_peer_id
    ));
    // --- Log Hash of Received Part 1 Package ---
    let package_hash = hash_data(&package);
    guard.log.push(format!(
        "DEBUG: Received Part1 Package Hash from {}: {}",
        from_peer_id, package_hash
    ));

    let sender_identifier = match identifier_map.get(&from_peer_id) {
        Some(id) => *id,
        None => {
            guard.log.push(format!(
                "Error: Identifier not found for sender {}",
                from_peer_id
            ));
            return; // Skip processing
        }
    };

    // Insert the received package
    guard
        .received_dkg_packages
        .insert(sender_identifier, package);

    // --- FIX: Ensure our own package is present ---
    let local_package_data_clone = guard.local_dkg_part1_data.clone();
    let own_peer_id_clone = guard.peer_id.clone();
    if let Some((_, local_public_package)) = local_package_data_clone {
        if let Some(my_identifier) = identifier_map.get(&own_peer_id_clone) {
            // Simpler check: if my identifier is not in the map, insert it.
            if !guard.received_dkg_packages.contains_key(my_identifier) {
                guard.log.push(format!(
                    "Adding own DKG Round 1 package for {:?} to received map.",
                    my_identifier
                ));
                guard
                    .received_dkg_packages
                    .insert(*my_identifier, local_public_package);
            }
        } else {
            guard.log.push(format!(
                "Error: Own identifier not found in map for peer_id {}",
                own_peer_id_clone
            ));
            guard.dkg_state = DkgState::Failed("Own identifier missing".to_string());
            return;
        }
    } else {
        // This case might happen if ProcessDkgRound1 is called before TriggerDkgRound1 finishes
        guard.log.push(
            "Warning: Local DKG Part 1 data not yet available while processing received package."
                .to_string(),
        );
        // We don't fail here, just wait for the local data to be added later or by another ProcessDkgRound1 call
    }

    let current_package_count = guard.received_dkg_packages.len();
    let expected_packages = session.total as usize;
    guard.log.push(format!(
        "DKG Round 1: Received {}/{} packages.",
        current_package_count, expected_packages
    ));

    // --- Check for Round 1 completion and Trigger Round 2 ---
    if current_package_count == expected_packages && guard.dkg_state == DkgState::Round1InProgress {
        guard
            .log
            .push("All DKG Round 1 packages received. Proceeding to Round 2...".to_string());
        guard.dkg_state = DkgState::Round1Complete; // Mark Round 1 as complete

        // --- Execute DKG Part 2 ---
        let local_secret_package_opt: Option<round1::SecretPackage<Ed25519Sha512>> =
            guard.local_dkg_part1_data.as_ref().map(|(s, _)| s.clone());
        let received_packages_clone = guard.received_dkg_packages.clone(); // Clone for part2
        let my_identifier_opt = guard
            .identifier_map
            .as_ref()
            .and_then(|map| map.get(&guard.peer_id).copied()); // Get own identifier

        let local_secret_package = match local_secret_package_opt {
            Some(secret) => secret,
            None => {
                guard
                    .log
                    .push("Error: Local DKG Part 1 secret share missing for Round 2.".to_string());
                guard.dkg_state = DkgState::Failed("Missing local secret share".to_string());
                return; // Skip
            }
        };

        // Drop guard before potentially long computation
        drop(guard);

        // --- FIX: Remove own package before calling part2 ---
        let mut packages_for_part2 = received_packages_clone;
        // Calculate expected count *before* removing own package
        let expected_part2_input_count = packages_for_part2.len().saturating_sub(1); // Expect N-1 packages

        if let Some(my_identifier) = my_identifier_opt {
            packages_for_part2.remove(&my_identifier);
        } else {
            // This should ideally not happen if identifier map is correct
            // Re-acquire lock briefly to log error and set state
            let mut guard = state.lock().unwrap();
            guard.log.push(
                "Error: Could not find own identifier to remove package for part2.".to_string(),
            );
            guard.dkg_state = DkgState::Failed("Own identifier missing for part2".to_string());
            return; // Skip part2 call
        }
        let actual_part2_input_count = packages_for_part2.len(); // Actual count after removal

        // --- Log Hashes and Counts before Part 2 ---
        let mut guard = state.lock().unwrap();
        guard.log.push("DEBUG: Preparing for Part 2:".to_string());
        guard
            .log
            .push(format!("  My Identifier: {:?}", my_identifier_opt));
        guard.log.push(format!(
            "  Expected input package count (N-1): {}",
            expected_part2_input_count
        ));
        guard.log.push(format!(
            "  Actual input package count: {}",
            actual_part2_input_count
        ));
        guard
            .log
            .push("  Input Package Hashes (should be N-1):".to_string());
        for (id, pkg) in &packages_for_part2 {
            guard.log.push(format!("    {:?}: {}", id, hash_data(pkg)));
        }
        // --- End Log ---
        drop(guard); // Drop guard before check and call

        // --- Strict Check: Ensure correct number of packages for part2 ---
        if actual_part2_input_count != expected_part2_input_count {
            let mut guard = state.lock().unwrap();
            guard.log.push(format!(
                "Error: Incorrect number of packages for part2. Expected {}, Got {}. Aborting.",
                expected_part2_input_count, actual_part2_input_count
            ));
            guard.dkg_state = DkgState::Failed("Part 2 package count mismatch".to_string());
            return; // Skip part2 call
        }

        let part2_result = part2::<Ed25519Sha512>(
            local_secret_package,
            &packages_for_part2, // Use the map *without* own package (N-1 entries)
        );

        // Re-acquire lock to update state and broadcast
        let mut guard = state.lock().unwrap();

        match part2_result {
            Ok((round2_secret, round2_packages_to_send)) => {
                guard.log.push(format!(
                    "DKG Part 2 successful for identifier {:?}. Storing local Round 2 secret.",
                    round2_secret.identifier()
                ));
                guard.round2_secret_package = Some(round2_secret); // Store the secret needed for part3

                // --- Broadcast Round 2 Packages ---
                guard
                    .log
                    .push("Broadcasting DKG Round 2 packages...".to_string());
                let identifier_map_clone = guard.identifier_map.clone().unwrap(); // Assumed safe due to earlier checks
                let state_clone = state.clone();

                let self_peer_id_clone = guard.peer_id.clone(); // Use guard.peer_id

                let mut broadcast_count = 0;
                for (target_identifier, round2_package) in round2_packages_to_send {
                    // Find the peer_id corresponding to the target_identifier
                    let target_peer_id = identifier_map_clone.iter().find_map(|(peer_id, &id)| {
                        if id == target_identifier {
                            Some(peer_id.clone())
                        } else {
                            None
                        }
                    });

                    if let Some(target_peer_id) = target_peer_id {
                        // Don't send to self
                        if target_peer_id != self_peer_id_clone {
                            // --- Log Hash of Generated Part 2 Package ---
                            let package_hash = hash_data(&round2_package);
                            guard.log.push(format!(
                                "DEBUG: Generated Part2 Package Hash for {:?}: {}",
                                target_identifier, package_hash
                            ));
                            // --- End Log ---
                            let dkg_msg = WebRTCMessage::DkgRound2Package {
                                package: round2_package,
                            };
                            let state_task_clone = state_clone.clone();

                            let target_peer_id_task = target_peer_id.clone();

                            tokio::spawn(async move {
                                if let Err(e) = send_webrtc_message(
                                    &target_peer_id_task,
                                    &dkg_msg,
                                    state_task_clone.clone(),
                                )
                                .await
                                {
                                    state_task_clone.lock().unwrap().log.push(format!(
                                        "Error sending DKG Round 2 package to {}: {}",
                                        target_peer_id_task, e
                                    ));
                                }
                            });
                            broadcast_count += 1;
                        }
                    } else {
                        guard.log.push(format!(
                            "Error: Could not find peer_id for target identifier {:?} during Round 2 broadcast.",
                            target_identifier
                        ));
                    }
                }
                guard.log.push(format!(
                    "Sent DKG Round 2 packages to {} peers.",
                    broadcast_count
                ));
                guard.dkg_state = DkgState::Round2InProgress; // Update state
            }
            Err(e) => {
                guard.log.push(format!("DKG Part 2 failed: {:?}", e));
                guard.dkg_state = DkgState::Failed(format!("DKG Part 2 Error: {:?}", e));
            }
        }
    } else if current_package_count == expected_packages
        && guard.dkg_state != DkgState::Round1InProgress
    {
        // FIX: Read dkg_state before the log.push call
        let current_dkg_state = guard.dkg_state.clone();
        // Log if all packages arrived but state wasn't Round1InProgress (e.g., already completed/failed)
        guard.log.push(format!(
            "DKG Round 1: Received all packages, but DKG state is {:?}. No action taken.",
            current_dkg_state // Use the cloned state here
        ));
    }
}

// Handle processing of DKG Round 2 packages
pub async fn process_dkg_round2(
    state: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    from_peer_id: String,
    package: round2::Package<Ed25519Sha512>,
) {
    let mut guard = state.lock().unwrap();

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
            filtered_round1.keys().collect::<Vec<_>>()
        ));
        guard.log.push(format!(
            "  Round 1 Packages (Filtered Count): {}",
            filtered_round1.len()
        ));
        // --- End FIX ---
        guard.log.push(format!(
            "  Round 2 Packages (Filtered Keys): {:?}",
            filtered_round2.keys().collect::<Vec<_>>()
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
        let mut guard = state.lock().unwrap();

        // --- FIX: Log filtered Round 1 keys ---
        guard.log.push(format!(
            "part3: my_identifier={:?}, round1_keys={:?}, round2_keys={:?}",
            my_identifier,
            filtered_round1.keys().collect::<Vec<_>>(), // Log filtered R1 keys
            filtered_round2.keys().collect::<Vec<_>>()
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
                guard.local_dkg_part1_data = None;
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
                    filtered_round2.keys().collect::<Vec<_>>()
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
