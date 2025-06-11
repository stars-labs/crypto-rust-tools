use crate::utils::state::{AppState, InternalCommand, SigningState};
use crate::protocal::signal::WebRTCMessage;
use crate::utils::device::send_webrtc_message;
use frost_core::{Ciphersuite, Identifier};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use std::collections::BTreeMap;

/// Handles initiating a signing process
pub async fn handle_initiate_signing<C>(
    transaction_data: String,
    state: Arc<Mutex<AppState<C>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        // Check if DKG is complete
        let dkg_complete = guard.key_package.is_some() && guard.group_public_key.is_some();
        if !dkg_complete {
            guard.log.push("Cannot initiate signing: DKG not completed yet".to_string());
            return;
        }
        
        // Check if already signing
        if guard.signing_state.is_active() {
            guard.log.push("Cannot initiate signing: Another signing process is already in progress".to_string());
            return;
        }
        
        // Check if session exists
        let session = match &guard.session {
            Some(s) => s.clone(),
            None => {
                guard.log.push("Cannot initiate signing: No active session".to_string());
                return;
            }
        };
        
        // Generate unique signing ID
        let signing_id = format!("sign_{}_{}", guard.device_id, chrono::Utc::now().timestamp());
        let required_signers = session.threshold as usize;
        
        // Initialize accepted signers with ourselves (initiator auto-accepts)
        let mut accepted_signers = std::collections::HashSet::new();
        accepted_signers.insert(guard.device_id.clone());
        
        // Set signing state to awaiting acceptance
        guard.signing_state = SigningState::AwaitingAcceptance {
            signing_id: signing_id.clone(),
            transaction_data: transaction_data.clone(),
            initiator: guard.device_id.clone(),
            required_signers,
            accepted_signers,
        };
        
        guard.log.push(format!(
            "Initiating signing process: {} (requires {} signers)",
            signing_id, required_signers
        ));
        
        // Send signing request to all session participants
        let session_participants = session.participants.clone();
        let self_device_id = guard.device_id.clone();
        drop(guard);
        
        let signing_request = WebRTCMessage::SigningRequest {
            signing_id: signing_id.clone(),
            transaction_data: transaction_data.clone(),
            required_signers,
        };
        
        for device_id in session_participants {
            if device_id != self_device_id {
                if let Err(e) = send_webrtc_message(&device_id, &signing_request, state_clone.clone()).await {
                    state_clone.lock().await.log.push(format!(
                        "Failed to send signing request to {}: {}",
                        device_id, e
                    ));
                }
            }
        }
        
        state_clone.lock().await.log.push(format!(
            "Sent signing request {} to all session participants",
            signing_id
        ));
        
        // Check if we already have enough signers (just ourselves) to proceed
        let mut guard = state_clone.lock().await;
        
        // Extract the needed values from the signing state to avoid borrowing conflicts
        let extracted_data = if let SigningState::AwaitingAcceptance { 
            accepted_signers, 
            required_signers,
            signing_id: current_id,
            transaction_data,
            ..
        } = &guard.signing_state {
            if accepted_signers.len() >= *required_signers && current_id == &signing_id {
                // Extract data needed for immediate progression
                Some((
                    accepted_signers.clone(),
                    *required_signers,
                    transaction_data.clone(),
                ))
            } else {
                None
            }
        } else {
            None
        };
        
        if let Some((accepted_signers, required_signers, transaction_data)) = extracted_data {
            guard.log.push(format!(
                "Sufficient signers gathered ({}/{}), starting commitment phase immediately",
                accepted_signers.len(), required_signers
            ));
            
            // We already have enough signers, proceed directly to commitment phase
            let identifier_map = match guard.identifier_map.clone() {
                Some(map) => map,
                None => {
                    guard.log.push("Error: No identifier map available. DKG may not be complete.".to_string());
                    guard.signing_state = SigningState::Failed {
                        signing_id: signing_id.clone(),
                        reason: "No identifier map available".to_string(),
                    };
                    return;
                }
            };
            
            // Select signers and map them to FROST Identifiers
            let selected_signers: Result<Vec<Identifier<C>>, String> = accepted_signers
                .iter()
                .take(required_signers)
                .map(|device_id| {
                    identifier_map.get(device_id)
                        .cloned()
                        .ok_or_else(|| format!("No FROST identifier found for device {}", device_id))
                })
                .collect();
            
            let selected_signers = match selected_signers {
                Ok(signers) => signers,
                Err(error) => {
                    guard.log.push(format!("Error mapping device IDs to FROST identifiers: {}", error));
                    guard.signing_state = SigningState::Failed {
                        signing_id: signing_id.clone(),
                        reason: error,
                    };
                    return;
                }
            };
            
            // Transition to commitment phase
            guard.signing_state = SigningState::CommitmentPhase {
                signing_id: signing_id.clone(),
                transaction_data: transaction_data.clone(),
                selected_signers: selected_signers.clone(),
                commitments: BTreeMap::new(),
                own_commitment: None,
                nonces: None,
            };
            
            // Send signer selection message to all participants
            let session = guard.session.as_ref().unwrap().clone();
            let selection_message = WebRTCMessage::SignerSelection {
                signing_id: signing_id.clone(),
                selected_signers: selected_signers.clone(),
            };
            
            // Prepare data for internal command dispatch
            let cmd_data = (signing_id.clone(), transaction_data.clone(), selected_signers);
            let self_device_id_copy = guard.device_id.clone();
            drop(guard);
            
            // Send selection message to all session participants
            for device_id in session.participants {
                if device_id != self_device_id_copy {
                    if let Err(e) = send_webrtc_message(&device_id, &selection_message, state_clone.clone()).await {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send signer selection to {}: {}",
                            device_id, e
                        ));
                    }
                }
            }
            
            // Now initiate FROST Round 1 commitment generation using proper internal command
            if let Err(e) = internal_cmd_tx.send(InternalCommand::InitiateFrostRound1 {
                signing_id: cmd_data.0,
                transaction_data: cmd_data.1,
                selected_signers: cmd_data.2,
            }) {
                state_clone.lock().await.log.push(format!("Failed to send InitiateFrostRound1 command: {}", e));
                let mut guard = state_clone.lock().await;
                guard.signing_state = SigningState::Failed {
                    signing_id: signing_id.clone(),
                    reason: "Failed to dispatch internal command".to_string(),
                };
            }
        }
    });
}

/// Handles accepting a signing request
pub async fn handle_accept_signing<C>(
    signing_id: String,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        // Check if this matches the current signing request
        let (current_signing_id, initiator) = match &guard.signing_state {
            SigningState::AwaitingAcceptance { signing_id: current_id, initiator, .. } => {
                (current_id.clone(), initiator.clone())
            },
            _ => {
                guard.log.push(format!("No pending signing request with ID: {}", signing_id));
                return;
            }
        };
        
        if current_signing_id != signing_id {
            guard.log.push(format!("Signing ID mismatch: expected {}, got {}", current_signing_id, signing_id));
            return;
        }
        
        guard.log.push(format!("Accepting signing request: {}", signing_id));
        
        // Send acceptance message to initiator
        let acceptance = WebRTCMessage::SigningAcceptance {
            signing_id: signing_id.clone(),
            accepted: true,
        };
        
        drop(guard);
        
        if let Err(e) = send_webrtc_message(&initiator, &acceptance, state_clone.clone()).await {
            state_clone.lock().await.log.push(format!(
                "Failed to send signing acceptance to {}: {}",
                initiator, e
            ));
        } else {
            state_clone.lock().await.log.push(format!(
                "Sent signing acceptance for {} to {}",
                signing_id, initiator
            ));
        }
    });
}

/// Handles processing a signing request from a device
pub async fn handle_process_signing_request<C>(
    from_device_id: String,
    signing_id: String,
    transaction_data: String,
    _timestamp: String,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        // Check if DKG is complete
        let dkg_complete = guard.key_package.is_some() && guard.group_public_key.is_some();
        if !dkg_complete {
            guard.log.push(format!("Cannot process signing request from {}: DKG not completed", from_device_id));
            return;
        }
        
        // Check if already signing
        if guard.signing_state.is_active() {
            guard.log.push(format!("Cannot process signing request from {}: Already in signing process", from_device_id));
            return;
        }
        
        // Check if session exists
        let session = match &guard.session {
            Some(s) => s.clone(),
            None => {
                guard.log.push(format!("Cannot process signing request from {}: No active session", from_device_id));
                return;
            }
        };
        
        // Verify the requesting device is in our session
        if !session.participants.contains(&from_device_id) {
            guard.log.push(format!("Rejecting signing request from {}: Not in session", from_device_id));
            return;
        }
        
        guard.log.push(format!("Received signing request from {}: id={}", from_device_id, signing_id));
        
        // Update signing state to awaiting acceptance
        let required_signers = session.threshold as usize;
        let mut accepted_signers = std::collections::HashSet::new();
        accepted_signers.insert(from_device_id.clone()); // Initiator is automatically accepted
        
        guard.signing_state = SigningState::AwaitingAcceptance {
            signing_id: signing_id.clone(),
            transaction_data: transaction_data.clone(),
            initiator: from_device_id.clone(),
            required_signers,
            accepted_signers,
        };
        
        guard.log.push(format!("Use '/acceptSign {}' to accept this signing request", signing_id));
    });
}

/// Handles processing a signing acceptance from a device
pub async fn handle_process_signing_acceptance<C>(
    from_device_id: String,
    signing_id: String,
    _timestamp: String,
    state: Arc<Mutex<AppState<C>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    let internal_cmd_tx_clone = internal_cmd_tx.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        // Check if we're in the right state and this is the right signing ID
        let signing_info = match &guard.signing_state {
            SigningState::AwaitingAcceptance { 
                signing_id: current_id, 
                accepted_signers, 
                required_signers,
                transaction_data,
                .. 
            } => {
                if current_id != &signing_id {
                    let msg = format!("Signing acceptance ID mismatch: expected {}, got {}", current_id, signing_id);
                    Some((None, msg))
                } else {
                    Some((Some((current_id.clone(), accepted_signers.clone(), *required_signers, transaction_data.clone())), String::new()))
                }
            },
            _ => {
                let msg = format!("Received signing acceptance from {} but not awaiting acceptance", from_device_id);
                Some((None, msg))
            }
        };
        
        let (signing_info_result, error_msg) = signing_info.unwrap();
        if let Some((current_signing_id, mut accepted_signers, required_signers, transaction_data)) = signing_info_result {
            // Add the accepting device
            accepted_signers.insert(from_device_id.clone());
            guard.log.push(format!("Signing acceptance from {}: {}/{} signers", from_device_id, accepted_signers.len(), required_signers));
            
            // Update the state with the new acceptances
            if let SigningState::AwaitingAcceptance { accepted_signers: current_accepted, .. } = &mut guard.signing_state {
                *current_accepted = accepted_signers.clone();
            }
            
            // Check if we have enough signers
            if accepted_signers.len() >= required_signers {
                guard.log.push(format!("Sufficient signers gathered ({}/{}), starting commitment phase", accepted_signers.len(), required_signers));
                
                // Get the identifier map to convert device IDs to FROST Identifiers
                let identifier_map = match guard.identifier_map.clone() {
                    Some(map) => map,
                    None => {
                        guard.log.push("Error: No identifier map available. DKG may not be complete.".to_string());
                        guard.signing_state = SigningState::Failed {
                            signing_id: current_signing_id,
                            reason: "No identifier map available".to_string(),
                        };
                        return;
                    }
                };
                
                // Select the first threshold number of signers and map them to FROST Identifiers
                let selected_signers: Result<Vec<Identifier<C>>, String> = accepted_signers
                    .into_iter()
                    .take(required_signers)
                    .map(|device_id| {
                        identifier_map.get(&device_id)
                            .cloned()
                            .ok_or_else(|| format!("No FROST identifier found for device {}", device_id))
                    })
                    .collect();
                
                let selected_signers = match selected_signers {
                    Ok(signers) => signers,
                    Err(error) => {
                        guard.log.push(format!("Error mapping device IDs to FROST identifiers: {}", error));
                        guard.signing_state = SigningState::Failed {
                            signing_id: current_signing_id,
                            reason: error,
                        };
                        return;
                    }
                };
                
                // Transition to commitment phase
                guard.signing_state = SigningState::CommitmentPhase {
                    signing_id: current_signing_id.clone(),
                    transaction_data: transaction_data.clone(),
                    selected_signers: selected_signers.clone(),
                    commitments: BTreeMap::new(),
                    own_commitment: None,
                    nonces: None,
                };
                
                // Send signer selection message to all participants
                let session = guard.session.as_ref().unwrap().clone();
                let selection_message = WebRTCMessage::SignerSelection {
                    signing_id: current_signing_id.clone(),
                    selected_signers: selected_signers.clone(),
                };
                
                // Drop guard before async operations
                let self_device_id = guard.device_id.clone();
                drop(guard);
                
                // Send selection message to all session participants
                for device_id in session.participants {
                    if device_id != self_device_id {
                        if let Err(e) = send_webrtc_message(&device_id, &selection_message, state_clone.clone()).await {
                            state_clone.lock().await.log.push(format!(
                                "Failed to send signer selection to {}: {}",
                                device_id, e
                            ));
                        }
                    }
                }
                
                // Now initiate FROST Round 1 commitment generation
                let _ = internal_cmd_tx_clone.send(InternalCommand::InitiateFrostRound1 {
                    signing_id: current_signing_id,
                    transaction_data,
                    selected_signers,
                });
            }
        } else {
            guard.log.push(error_msg);
            return;
        }
    });
}

/// Handles processing a signing commitment from a device
pub async fn handle_process_signing_commitment<C>(
    from_device_id: String,
    signing_id: String,
    commitment: frost_core::round1::SigningCommitments<C>,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        guard.log.push(format!(
            "Processing signing commitment from {} for signing {}",
            from_device_id, signing_id
        ));
        
        // Extract identifier map to avoid borrow conflicts
        let identifier_map = match guard.identifier_map.clone() {
            Some(map) => map,
            None => {
                guard.log.push("Error: No identifier map available for commitment processing".to_string());
                return;
            }
        };
        
        let sender_identifier = match identifier_map.get(&from_device_id) {
            Some(id) => *id,
            None => {
                guard.log.push(format!("Error: No FROST identifier found for device {}", from_device_id));
                return;
            }
        };
        
        // Check if we're in the commitment phase and this is the correct signing process
        let (current_signing_id, selected_signers, transaction_data) = match &guard.signing_state {
            SigningState::CommitmentPhase { 
                signing_id: current_id, 
                selected_signers, 
                transaction_data,
                ..
            } if current_id == &signing_id => {
                (current_id.clone(), selected_signers.clone(), transaction_data.clone())
            },
            _ => {
                guard.log.push(format!(
                    "Ignoring commitment from {} - not in commitment phase for signing {}",
                    from_device_id, signing_id
                ));
                return;
            }
        };
        
        // Verify sender is one of the selected signers
        if !selected_signers.contains(&sender_identifier) {
            guard.log.push(format!(
                "Ignoring commitment from {} - not a selected signer for {}",
                from_device_id, signing_id
            ));
            return;
        }
        
        // Store the commitment and check if we have all
        let should_proceed = if let SigningState::CommitmentPhase { 
            commitments, 
            ..
        } = &mut guard.signing_state {
            // Check if we already have this commitment
            if commitments.contains_key(&sender_identifier) {
                guard.log.push(format!(
                    "Duplicate commitment from {} for signing {} - ignoring",
                    from_device_id, signing_id
                ));
                return;
            }
            
            commitments.insert(sender_identifier, commitment);
            
            let commitment_count = commitments.len();
            let selected_count = selected_signers.len();
            
            // Log outside the mutable borrow
            let _ = commitments;
            guard.log.push(format!(
                "Stored commitment from {} ({}/{})",
                from_device_id, commitment_count, selected_count
            ));
            
            // Check if we have all commitments
            commitment_count == selected_count
        } else {
            false
        };
        
        if should_proceed {
            guard.log.push("All commitments received, transitioning to share phase".to_string());
            
            // Extract required data
            let (commitments, our_nonces) = if let SigningState::CommitmentPhase { 
                commitments, 
                nonces,
                ..
            } = &mut guard.signing_state {
                let comms = commitments.clone();
                let nonces = nonces.take();
                (comms, nonces)
            } else {
                return;
            };
            
            // Create signing package
            let signing_package = frost_core::SigningPackage::new(
                commitments.clone(),
                transaction_data.as_bytes(),
            );
            
            // Get our key package for signature generation
            let key_package = match &guard.key_package {
                Some(kp) => kp.clone(),
                None => {
                    guard.log.push("Error: No key package available for signature generation".to_string());
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: "No key package available".to_string(),
                    };
                    return;
                }
            };
            
            let our_nonces = match our_nonces {
                Some(n) => n,
                None => {
                    guard.log.push("Error: No nonces available for signature generation".to_string());
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: "No nonces available".to_string(),
                    };
                    return;
                }
            };
            
            // Generate our signature share
            guard.log.push("Generating signature share".to_string());
            
            let signature_share = match frost_core::round2::sign(&signing_package, &our_nonces, &key_package) {
                Ok(share) => share,
                Err(e) => {
                    guard.log.push(format!("Error generating signature share: {:?}", e));
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: format!("Signature generation failed: {:?}", e),
                    };
                    return;
                }
            };
            
            // Get our identifier
            let self_identifier = match identifier_map.get(&guard.device_id) {
                Some(id) => *id,
                None => {
                    let device_id = guard.device_id.clone();
                    guard.log.push(format!("Error: No FROST identifier found for self ({})", device_id));
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: "No self identifier available".to_string(),
                    };
                    return;
                }
            };
            
            // Initialize shares map with our own share
            let mut shares = BTreeMap::new();
            shares.insert(self_identifier, signature_share.clone());
            
            // Transition to share phase
            guard.signing_state = SigningState::SharePhase {
                signing_id: current_signing_id.clone(),
                transaction_data: transaction_data.clone(),
                selected_signers: selected_signers.clone(),
                signing_package: Some(signing_package),
                shares,
                own_share: Some(signature_share.clone()),
            };
            
            // Send our signature share to all other selected signers
            let share_message = WebRTCMessage::SignatureShare {
                signing_id: current_signing_id.clone(),
                sender_identifier: self_identifier,
                share: signature_share,
            };
            
            // Get reverse identifier map to convert FROST identifiers back to device IDs
            let device_id_map: BTreeMap<Identifier<C>, String> = identifier_map
                .iter()
                .map(|(device_id, frost_id)| (*frost_id, device_id.clone()))
                .collect();
            
            drop(guard);
            
            // Send signature share to all other selected signers
            for signer_id in selected_signers {
                if signer_id != self_identifier {
                    if let Some(device_id) = device_id_map.get(&signer_id) {
                        if let Err(e) = send_webrtc_message(device_id, &share_message, state_clone.clone()).await {
                            state_clone.lock().await.log.push(format!(
                                "Failed to send signature share to {}: {}",
                                device_id, e
                            ));
                        }
                    }
                }
            }
            
            state_clone.lock().await.log.push(format!(
                "Sent signature share for signing {} to other selected signers",
                current_signing_id
            ));
        }
    });
}

/// Handles processing a signature share from a device
pub async fn handle_process_signature_share<C>(
    from_device_id: String,
    signing_id: String,
    share: frost_core::round2::SignatureShare<C>,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        guard.log.push(format!(
            "Processing signature share from {} for signing {}",
            from_device_id, signing_id
        ));
        
        // Get identifier map to convert device ID to FROST identifier
        let identifier_map = match guard.identifier_map.clone() {
            Some(map) => map,
            None => {
                guard.log.push("Error: No identifier map available for share processing".to_string());
                return;
            }
        };
        
        let sender_identifier = match identifier_map.get(&from_device_id) {
            Some(id) => *id,
            None => {
                guard.log.push(format!("Error: No FROST identifier found for device {}", from_device_id));
                return;
            }
        };
        
        // Check if we're in the share phase and this is the correct signing process
        let (current_signing_id, selected_signers, signing_package) = match &guard.signing_state {
            SigningState::SharePhase { 
                signing_id: current_id, 
                selected_signers, 
                signing_package,
                ..
            } if current_id == &signing_id => {
                (current_id.clone(), selected_signers.clone(), signing_package.clone())
            },
            _ => {
                guard.log.push(format!(
                    "Ignoring signature share from {} - not in share phase for signing {}",
                    from_device_id, signing_id
                ));
                return;
            }
        };
        
        // Verify sender is one of the selected signers
        if !selected_signers.contains(&sender_identifier) {
            guard.log.push(format!(
                "Ignoring signature share from {} - not a selected signer for {}",
                from_device_id, signing_id
            ));
            return;
        }
        
        // Store the signature share and check if we have all
        let should_aggregate = if let SigningState::SharePhase { 
            shares,
            ..
        } = &mut guard.signing_state {
            // Check if we already have this share
            if shares.contains_key(&sender_identifier) {
                guard.log.push(format!(
                    "Duplicate signature share from {} for signing {} - ignoring",
                    from_device_id, signing_id
                ));
                return;
            }
            
            shares.insert(sender_identifier, share);
            
            let shares_count = shares.len();
            let selected_count = selected_signers.len();
            
            // Log outside the mutable borrow
            let _ = shares;
            guard.log.push(format!(
                "Stored signature share from {} ({}/{})",
                from_device_id, shares_count, selected_count
            ));
            
            // Check if we have all signature shares
            shares_count == selected_count
        } else {
            false
        };
        
        if should_aggregate {
            guard.log.push("All signature shares received, aggregating signature".to_string());
            
            // Extract required data
            let shares = if let SigningState::SharePhase { shares, .. } = &guard.signing_state {
                shares.clone()
            } else {
                return;
            };
            
            // Get the signing package
            let signing_package = match signing_package {
                Some(pkg) => pkg,
                None => {
                    guard.log.push("Error: No signing package available for aggregation".to_string());
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: "No signing package available".to_string(),
                    };
                    return;
                }
            };
            
            // Get the group public key for verification
            let group_public_key = match guard.group_public_key.clone() {
                Some(key) => key,
                None => {
                    guard.log.push("Error: No group public key available for signature aggregation".to_string());
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: "No group public key available".to_string(),
                    };
                    return;
                }
            };
            
            // Aggregate the signature
            guard.log.push("Aggregating FROST signature...".to_string());
            
            let aggregated_signature = match frost_core::aggregate(&signing_package, &shares, &group_public_key) {
                Ok(sig) => sig,
                Err(e) => {
                    guard.log.push(format!("Error aggregating signature: {:?}", e));
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: format!("Signature aggregation failed: {:?}", e),
                    };
                    return;
                }
            };
            
            // Convert signature to bytes
            let signature_bytes: Vec<u8> = match aggregated_signature.serialize() {
                Ok(bytes) => {
                    let bytes_ref: &[u8] = bytes.as_ref();
                    bytes_ref.to_vec()
                },
                Err(e) => {
                    guard.log.push(format!("Error serializing signature: {:?}", e));
                    guard.signing_state = SigningState::Failed {
                        signing_id: current_signing_id,
                        reason: format!("Signature serialization failed: {:?}", e),
                    };
                    return;
                }
            };
            
            guard.log.push(format!(
                "Successfully aggregated signature for {} ({} bytes): {}",
                current_signing_id,
                signature_bytes.len(),
                hex::encode(&signature_bytes)
            ));
            
            // Transition to complete state
            guard.signing_state = SigningState::Complete {
                signing_id: current_signing_id.clone(),
                signature: signature_bytes.clone(),
            };
            
            // Broadcast the aggregated signature to all session participants
            let aggregated_sig_message = WebRTCMessage::AggregatedSignature {
                signing_id: current_signing_id.clone(),
                signature: signature_bytes.clone(),
            };
            
            let session = guard.session.as_ref().unwrap().clone();
            let self_device_id = guard.device_id.clone();
            drop(guard);
            
            // Send aggregated signature to all session participants
            for device_id in session.participants {
                if device_id != self_device_id {
                    if let Err(e) = send_webrtc_message(&device_id, &aggregated_sig_message, state_clone.clone()).await {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send aggregated signature to {}: {}",
                            device_id, e
                        ));
                    }
                }
            }
            
            state_clone.lock().await.log.push(format!(
                "Broadcasted aggregated signature for {} to all session participants",
                current_signing_id
            ));
        }
    });
}

/// Handles processing an aggregated signature from a device
pub async fn handle_process_aggregated_signature<C>(
    from_device_id: String,
    signing_id: String,
    signature: Vec<u8>,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        guard.log.push(format!(
            "Processing aggregated signature from {} for signing {} ({} bytes)",
            from_device_id, signing_id, signature.len()
        ));
        
        // Update signing state to complete
        guard.signing_state = SigningState::Complete {
            signing_id: signing_id.clone(),
            signature: signature.clone(),
        };
        
        guard.log.push(format!(
            "Signing process {} completed successfully. Signature: {}",
            signing_id,
            hex::encode(&signature)
        ));
    });
}

/// Handles processing signer selection from a device
pub async fn handle_process_signer_selection<C>(
    from_device_id: String,
    signing_id: String,
    selected_signers: Vec<Identifier<C>>,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        guard.log.push(format!(
            "Processing signer selection from {} for signing {} ({} signers selected)",
            from_device_id, signing_id, selected_signers.len()
        ));
        
        // Get identifier map to check if we are selected
        let identifier_map = match guard.identifier_map.clone() {
            Some(map) => map,
            None => {
                guard.log.push("Error: No identifier map available for signer selection processing".to_string());
                return;
            }
        };
        
        let self_identifier = match identifier_map.get(&guard.device_id) {
            Some(id) => *id,
            None => {
                let device_id = guard.device_id.clone();
                guard.log.push(format!("Error: No FROST identifier found for self ({})", device_id));
                return;
            }
        };
        
        // Check if we are one of the selected signers
        let is_selected = selected_signers.contains(&self_identifier);
        
        if !is_selected {
            guard.log.push(format!(
                "Not selected for signing {}, waiting for final signature",
                signing_id
            ));
            return;
        }
        
        guard.log.push(format!(
            "Selected for signing {}, transitioning to commitment phase",
            signing_id
        ));
        
        // Check if we're in the awaiting acceptance state for this signing process
        let transaction_data = match &guard.signing_state {
            SigningState::AwaitingAcceptance { 
                signing_id: current_id, 
                transaction_data,
                ..
            } if current_id == &signing_id => {
                transaction_data.clone()
            },
            _ => {
                guard.log.push(format!(
                    "Ignoring signer selection from {} - not in awaiting acceptance state for signing {}",
                    from_device_id, signing_id
                ));
                return;
            }
        };
        
        // Transition to commitment phase
        guard.signing_state = SigningState::CommitmentPhase {
            signing_id: signing_id.clone(),
            transaction_data: transaction_data.clone(),
            selected_signers: selected_signers.clone(),
            commitments: BTreeMap::new(),
            own_commitment: None,
            nonces: None,
        };
        
        // Get our key package for signing
        let key_package = match &guard.key_package {
            Some(kp) => kp.clone(),
            None => {
                guard.log.push("Error: No key package available for signing".to_string());
                guard.signing_state = SigningState::Failed {
                    signing_id: signing_id.clone(),
                    reason: "No key package available".to_string(),
                };
                return;
            }
        };
        
        guard.log.push("Generating FROST commitment for signing".to_string());
        
        // Generate FROST Round 1 commitment - using the compatible RNG
        let (nonces, commitments) = frost_core::round1::commit(
            key_package.signing_share(),
            &mut frost_ed25519::rand_core::OsRng,
        );
        
        // Update signing state with our commitment and nonces
        if let SigningState::CommitmentPhase { 
            commitments: commitment_map, 
            own_commitment, 
            nonces: nonces_field,
            ..
        } = &mut guard.signing_state {
            commitment_map.insert(self_identifier, commitments.clone());
            *own_commitment = Some(commitments.clone());
            *nonces_field = Some(nonces);
        }
        
        guard.log.push("Broadcasting commitment to other selected signers".to_string());
        
        // Send commitment to all other selected signers
        let commitment_message = WebRTCMessage::SigningCommitment {
            signing_id: signing_id.clone(),
            sender_identifier: self_identifier,
            commitment: commitments,
        };
        
        // Get reverse identifier map to convert FROST identifiers back to device IDs
        let device_id_map: BTreeMap<Identifier<C>, String> = identifier_map
            .iter()
            .map(|(device_id, frost_id)| (*frost_id, device_id.clone()))
            .collect();
        
        drop(guard);
        
        // Send commitments to all other selected signers
        for signer_id in selected_signers {
            if signer_id != self_identifier {
                if let Some(device_id) = device_id_map.get(&signer_id) {
                    if let Err(e) = send_webrtc_message(device_id, &commitment_message, state_clone.clone()).await {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send commitment to {}: {}",
                            device_id, e
                        ));
                    }
                }
            }
        }
        
        state_clone.lock().await.log.push(format!(
            "Sent FROST commitment for signing {} to other selected signers",
            signing_id
        ));
    });
}

/// Handles initiating FROST Round 1 commitment generation
pub async fn handle_initiate_frost_round1<C>(
    signing_id: String,
    _transaction_data: String,
    selected_signers: Vec<Identifier<C>>,
    state: Arc<Mutex<AppState<C>>>,
    _internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    
    tokio::spawn(async move {
        let mut guard = state_clone.lock().await;
        
        guard.log.push(format!(
            "Initiating FROST Round 1 for signing {}",
            signing_id
        ));
        
        // Check if we are one of the selected signers
        let identifier_map = match guard.identifier_map.clone() {
            Some(map) => map,
            None => {
                guard.log.push("Error: No identifier map available for FROST Round 1".to_string());
                return;
            }
        };
        
        let self_identifier = match identifier_map.get(&guard.device_id) {
            Some(id) => *id,
            None => {
                let device_id = guard.device_id.clone();
                guard.log.push(format!("Error: No FROST identifier found for device {}", device_id));
                return;
            }
        };
        
        let is_selected = selected_signers.contains(&self_identifier);
        
        if !is_selected {
            guard.log.push(format!("Not selected for signing {}, waiting for final signature", signing_id));
            return;
        }
        
        // Check if we have a key package for signing
        let key_package = match &guard.key_package {
            Some(kp) => kp.clone(),
            None => {
                guard.log.push("Error: No key package available for signing".to_string());
                return;
            }
        };
        
        guard.log.push("Generating FROST commitment for signing".to_string());
        
        // Generate FROST Round 1 commitment - using the compatible RNG
        let (nonces, commitments) = frost_core::round1::commit(
            key_package.signing_share(),
            &mut frost_ed25519::rand_core::OsRng,
        );
        
        // Update signing state with our commitment and nonces
        if let SigningState::CommitmentPhase { 
            commitments: commitment_map, 
            own_commitment, 
            nonces: nonces_field,
            ..
        } = &mut guard.signing_state {
            commitment_map.insert(self_identifier, commitments.clone());
            *own_commitment = Some(commitments.clone());
            *nonces_field = Some(nonces);
        }
        
        guard.log.push("Broadcasting commitment to other selected signers".to_string());
        
        // Send commitment to all other selected signers
        let commitment_message = WebRTCMessage::SigningCommitment {
            signing_id: signing_id.clone(),
            sender_identifier: self_identifier,
            commitment: commitments,
        };
        
        // Get identifier map to convert FROST identifiers back to device IDs
        let device_id_map: BTreeMap<Identifier<C>, String> = identifier_map
            .into_iter()
            .map(|(device_id, frost_id)| (frost_id, device_id))
            .collect();
        
        drop(guard);
        
        // Send commitments to all other selected signers
        for signer_id in selected_signers {
            if signer_id != self_identifier {
                if let Some(device_id) = device_id_map.get(&signer_id) {
                    if let Err(e) = send_webrtc_message(device_id, &commitment_message, state_clone.clone()).await {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send commitment to {}: {}",
                            device_id, e
                        ));
                    }
                }
            }
        }
        
        state_clone.lock().await.log.push(format!(
            "Sent FROST commitment for signing {} to other selected signers",
            signing_id
        ));
    });
}
