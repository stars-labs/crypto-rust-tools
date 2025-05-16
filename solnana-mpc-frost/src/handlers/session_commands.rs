use crate::utils::state::{AppState, InternalCommand};
use crate::protocal::signal::{WebSocketMessage, SessionInfo, SessionProposal, SessionResponse};
use crate::network::webrtc::initiate_webrtc_connections;
use frost_core::{Ciphersuite, Identifier};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use webrtc_signal_server::ClientMsg;

/// Handles proposing a new session
pub async fn handle_propose_session<C>(
    session_id: String,
    total: u16,
    threshold: u16,
    participants: Vec<String>,
    state: Arc<Mutex<AppState<C>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
    self_peer_id: String,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let state_clone = state.clone();
    let internal_cmd_tx_clone = internal_cmd_tx.clone();
    let self_peer_id_clone = self_peer_id.clone();
    
    tokio::spawn(async move {
        let mut state_guard = state_clone.lock().await;
        
        let session_proposal = SessionProposal {
            session_id: session_id.clone(),
            total,
            threshold,
            participants: participants.clone(),
        };
        
        state_guard.log.push(format!(
            "Proposing session '{}' with {} participants and threshold {}",
            session_id, total, threshold
        ));
        
        let current_peer_id = state_guard.peer_id.clone(); 
        state_guard.session = Some(SessionInfo {
            session_id: session_id.clone(),
            proposer_id: current_peer_id.clone(),
            total,
            threshold,
            participants: participants.clone(),
            accepted_peers: vec![current_peer_id.clone()],
        }); 
        
        let mut map_created_and_check_dkg = false;
        if participants.len() == 1 && participants.contains(&current_peer_id) {
             if state_guard.identifier_map.is_none() { 
                let mut participants_sorted = participants.clone();
                participants_sorted.sort();

                let mut new_identifier_map = BTreeMap::new();
                for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                    match Identifier::try_from((i + 1) as u16) {
                        Ok(identifier) => {
                            new_identifier_map.insert(peer_id_str.clone(), identifier);
                        }
                        Err(e) => {
                            state_guard.log.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                            state_guard.session = None;
                            state_guard.identifier_map = None;
                            return; 
                        }
                    }
                }
                state_guard.log.push(format!("Identifier map created for single-participant session: {:?}", new_identifier_map));
                state_guard.identifier_map = Some(new_identifier_map);
                map_created_and_check_dkg = true;
            }
        }
        
        let local_peer_id_for_filter = state_guard.peer_id.clone();
        drop(state_guard);

        if map_created_and_check_dkg {
            if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                state_clone.lock().await.log.push(format!(
                    "Failed to send CheckAndTriggerDkg after proposing single-user session: {}", e
                ));
            }
        }
        
        let mut state_guard_for_broadcast = state_clone.lock().await;
        for peer in participants 
            .iter() 
            .filter(|p| **p != local_peer_id_for_filter) {
        
            let proposal_msg = WebSocketMessage::SessionProposal(session_proposal.clone());
            match serde_json::to_value(proposal_msg) {
                Ok(json_val) => {
                    let relay_msg = ClientMsg::Relay {
                        to: peer.clone(), 
                        data: json_val, 
                    }; 
                    if let Err(e) = 
                        internal_cmd_tx_clone.send(InternalCommand::SendToServer(relay_msg))
                    {
                        state_guard_for_broadcast.log.push(format!(
                            "Failed to send session proposal to {}: {}",
                            peer, e
                        ));
                    } else {
                        state_guard_for_broadcast
                            .log 
                            .push(format!("Sent session proposal to {}", peer));
                    } 
                }
                Err(e) => {
                    state_guard_for_broadcast.log.push(format!(
                        "Error serializing session proposal for {}: {}",
                        peer, e
                    ));
                }
            }
        } 
        drop(state_guard_for_broadcast); 
        initiate_webrtc_connections( 
            participants, 
            self_peer_id_clone,
            state_clone.clone(),
            internal_cmd_tx_clone,
        )   
        .await;
    });
}

/// Handles accepting a session proposal
pub async fn handle_accept_session_proposal<C>(
    session_id: String,
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
        let mut map_created_and_check_dkg = false;
        let mut participants_for_webrtc: Vec<String> = Vec::new();
        let mut peer_id_for_webrtc: String = String::new();    
        let mut participants_for_map_creation_in_accept: Option<Vec<String>> = None;

        { 
            let mut state_guard = state_clone.lock().await; 

            if let Some(invite_idx) = state_guard
                .invites
                .iter()
                .position(|i| i.session_id == session_id)
            {   
                // ... existing code ...
                let invite = state_guard.invites.remove(invite_idx);
                let current_peer_id = state_guard.peer_id.clone();
                
                state_guard
                    .log
                    .push(format!("You accepted session proposal '{}'", session_id));

                let mut combined_accepted_peers = HashSet::new();
                combined_accepted_peers.insert(current_peer_id.clone()); // Self
                combined_accepted_peers.insert(invite.proposer_id.clone()); // Proposer
                for early_accepter in &invite.accepted_peers {
                    combined_accepted_peers.insert(early_accepter.clone());
                }

                let final_accepted_peers: Vec<String> = combined_accepted_peers.into_iter().collect();

                state_guard.session = Some(SessionInfo {
                    session_id: invite.session_id.clone(),
                    proposer_id: invite.proposer_id.clone(), 
                    total: invite.total,
                    threshold: invite.threshold,
                    participants: invite.participants.clone(),
                    accepted_peers: final_accepted_peers.clone(), 
                });
                
                peer_id_for_webrtc = current_peer_id.clone(); 
                participants_for_webrtc = invite.participants.clone(); 

                // Check if all participants have accepted
                if final_accepted_peers.len() == invite.participants.len() {
                    if state_guard.identifier_map.is_none() {
                        state_guard.log.push(format!(
                            "All {} participants now confirmed for session '{}' upon local acceptance. Preparing identifier map.",
                            invite.participants.len(), invite.session_id
                        ));
                        participants_for_map_creation_in_accept = Some(invite.participants.clone());
                    }
                }
                
                // Handle single-user case
                if invite.participants.len() == 1 && invite.participants.contains(&current_peer_id) {
                    if state_guard.identifier_map.is_none() {
                        if participants_for_map_creation_in_accept.is_none() {
                             participants_for_map_creation_in_accept = Some(invite.participants.clone());
                        }
                    }
                }

                // Create identifier map if needed
                if let Some(participants_list) = participants_for_map_creation_in_accept {
                    if state_guard.identifier_map.is_none() {
                        let mut participants_sorted = participants_list;
                        participants_sorted.sort();
                        let mut new_identifier_map = BTreeMap::new();
                        for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                            match Identifier::try_from((i + 1) as u16) {
                                Ok(identifier) => {
                                    new_identifier_map.insert(peer_id_str.clone(), identifier);
                                }
                                Err(e) => {
                                    state_guard.log.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                                    state_guard.session = None;
                                    state_guard.identifier_map = None;
                                    return; 
                                }
                            }
                        }
                        state_guard.log.push(format!("Identifier map created (on accept): {:?}", new_identifier_map));
                        state_guard.identifier_map = Some(new_identifier_map);
                        map_created_and_check_dkg = true;
                    }
                }

                let participants_to_notify: Vec<String> = invite
                    .participants
                    .iter()
                    .filter(|p| **p != current_peer_id)
                    .cloned()
                    .collect();

                let response_payload = SessionResponse {
                    session_id: invite.session_id.clone(),
                    accepted: true,
                };
                
                drop(state_guard); 

                if map_created_and_check_dkg {
                     if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                        state_clone.lock().await.log.push(format!("Failed to send CheckAndTriggerDkg after accepting single-user session: {}", e));
                    }
                }

                // Notify other participants
                for peer in participants_to_notify {
                    let response_msg = WebSocketMessage::SessionResponse(response_payload.clone()); 
                    match serde_json::to_value(response_msg) {
                        Ok(json_val) => {
                            let relay_msg = ClientMsg::Relay {
                                to: peer.clone(),
                                data: json_val,
                            };  
                            if let Err(e) = internal_cmd_tx_clone
                                .send(InternalCommand::SendToServer(relay_msg))
                            {
                                let mut guard = state_clone.lock().await;
                                guard.log.push(format!(
                                    "Failed to send acceptance to {}: {}",
                                    peer, e
                                ));
                            } else {
                                let mut guard = state_clone.lock().await;
                                guard.log.push(format!("Sent acceptance to {}", peer));
                            }   
                        }
                        Err(e) => {
                            let mut guard = state_clone.lock().await;
                            guard.log.push(format!( 
                                "Error serializing acceptance for {}: {}",
                                peer, e 
                            ));
                        }
                    }
                }
                
                // Begin WebRTC connection process
                initiate_webrtc_connections( 
                    participants_for_webrtc, 
                    peer_id_for_webrtc,
                    state_clone.clone(),
                    internal_cmd_tx_clone,
                )   
                .await;
                return; // Successfully processed
            } else { 
                state_guard.log.push(format!(
                    "No pending invite found for session '{}'",
                    session_id 
                ));
            }
        }
    });
}

/// Handles processing a session response
pub async fn handle_process_session_response<C>(
    from_peer_id: String,
    response: SessionResponse,
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
        let mut log_msgs = Vec::new();
        let mut session_cancelled = false;
        let mut session_id_to_cancel = None;
        let mut map_created_and_check_dkg = false;
        let mut participants_for_map_creation: Option<Vec<String>> = None;

        { 
            let mut state_guard = state_clone.lock().await;
            log_msgs.push(format!(
                "Received session response from {}: Accepted={}",
                from_peer_id, response.accepted
            ));
            
            if response.accepted {
                // ... existing code ...
                let mut handled_in_active_session = false;
                if let Some(session) = state_guard.session.as_mut() {
                    if session.session_id == response.session_id {
                        handled_in_active_session = true;
                        if !session.accepted_peers.contains(&from_peer_id) {
                            session.accepted_peers.push(from_peer_id.clone());
                            log_msgs.push(format!(
                                "Peer {} accepted session '{}'. Active session accepted peers: {}/{}",
                                from_peer_id, session.session_id, session.accepted_peers.len(), session.participants.len()
                            ));
                            if session.accepted_peers.len() == session.participants.len() {
                                log_msgs.push(format!(
                                    "All {} participants accepted session '{}' (active session). Preparing identifier map.",
                                    session.participants.len(), session.session_id
                                ));
                                participants_for_map_creation = Some(session.participants.clone());
                            }
                        } else {
                            log_msgs.push(format!(
                                "Peer {} already recorded in active session '{}' accepted_peers.",
                                from_peer_id, session.session_id
                            ));
                        }
                    }
                }
                
                // Check pending invites if not handled in active session
                if !handled_in_active_session {
                    // ... existing code ...
                    if let Some(invite_to_update) = state_guard.invites.iter_mut().find(|i| i.session_id == response.session_id) {
                        if !invite_to_update.accepted_peers.contains(&from_peer_id) {
                            invite_to_update.accepted_peers.push(from_peer_id.clone());
                            log_msgs.push(format!(
                                "Recorded early acceptance from {} for pending invite '{}'. Invite accepted_peers count: {}.",
                                from_peer_id, invite_to_update.session_id, invite_to_update.accepted_peers.len()
                            ));
                        } else {
                             log_msgs.push(format!(
                                "Peer {} already recorded in pending invite '{}' accepted_peers.",
                                from_peer_id, invite_to_update.session_id
                            ));
                        }
                    } else {
                        log_msgs.push(format!(
                            "No active session or pending invite found for session ID '{}' from peer {}.",
                            response.session_id, from_peer_id
                        ));
                    }
                }
                
                // Create identifier map if all participants have accepted
                if let Some(participants_list) = participants_for_map_creation {
                    if state_guard.identifier_map.is_none() { 
                        // ... existing code ...
                        let mut participants_sorted = participants_list; 
                        participants_sorted.sort(); 

                        let mut new_identifier_map = BTreeMap::new();
                        for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                            match Identifier::try_from((i + 1) as u16) {
                                Ok(identifier) => {
                                    new_identifier_map.insert(peer_id_str.clone(), identifier);
                                }
                                Err(e) => {
                                    log_msgs.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                                    state_guard.session = None; 
                                    state_guard.identifier_map = None;
                                    // Push logs and return
                                    for msg_item in log_msgs { state_guard.log.push(msg_item); }
                                    return; 
                                }
                            }
                        }
                        log_msgs.push(format!("Identifier map created: {:?}", new_identifier_map));
                        state_guard.identifier_map = Some(new_identifier_map);
                        map_created_and_check_dkg = true;
                    }
                }
            } else {
                // Handle rejection
                log_msgs.push(format!(
                    "Peer {} rejected session '{}'.",
                    from_peer_id, response.session_id
                ));
                if let Some(session) = &state_guard.session {
                    if session.session_id == response.session_id {
                        session_cancelled = true;
                        session_id_to_cancel = Some(response.session_id.clone());
                    }
                }
            }
        }
        
        // Trigger DKG if needed
        if map_created_and_check_dkg {
            if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                log_msgs.push(format!("Failed to send CheckAndTriggerDkg command: {}", e));
            }
        }

        // Cancel session if rejected
        if session_cancelled {
            let mut guard = state_clone.lock().await;
            guard.session = None;
            guard.identifier_map = None; 
            if let Some(sid) = session_id_to_cancel {
                log_msgs.push(format!("Session '{}' cancelled due to rejection by {}.", sid, from_peer_id));
            }
        }
        
        // Log all messages
        let mut guard = state_clone.lock().await;
        for msg in log_msgs {
            guard.log.push(msg);
        }
    });
}
