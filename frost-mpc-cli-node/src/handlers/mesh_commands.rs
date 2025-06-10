use crate::utils::state::{AppState, InternalCommand, MeshStatus};
use crate::utils::peer::send_webrtc_message;
use crate::protocal::signal::WebRTCMessage;
use crate::check_and_send_mesh_ready;
use frost_core::Ciphersuite;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

/// Handles reporting that a channel is open
pub async fn handle_report_channel_open<C>(
    peer_id: String,
    state: Arc<Mutex<AppState<C>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
    self_peer_id: String,
) where
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let log_peer_id_initial_log = peer_id.clone();
    let initial_log_state_clone = state.clone();
    let initial_log_self_peer_id = self_peer_id.clone();
    
    tokio::spawn(async move {
        initial_log_state_clone.lock().await.log.push(format!(
            "[ReportChannelOpen-{}] Received for remote peer: {}.",
            initial_log_self_peer_id, log_peer_id_initial_log
        ));
    });

    let peer_id_for_main_task = peer_id.clone();
    let state_clone = state.clone();
    let internal_cmd_tx_clone = internal_cmd_tx.clone();
    let self_peer_id_for_task = self_peer_id.clone();
    
    tokio::spawn(async move {
        let local_self_peer_id = self_peer_id_for_task;
        let session_exists_at_dispatch: bool;
        let mut log_messages_for_task = Vec::new();

        {
            let mut guard = state_clone.lock().await;
            session_exists_at_dispatch = guard.session.is_some();
            log_messages_for_task.push(format!(
                "[ReportChannelOpenTask-{}] Running for remote peer: {}. Session exists at dispatch: {}",
                local_self_peer_id, peer_id_for_main_task, session_exists_at_dispatch
            ));

            guard.peer_statuses.insert(peer_id_for_main_task.clone(), RTCPeerConnectionState::Connected);
            log_messages_for_task.push(format!(
                "[ReportChannelOpenTask-{}] Set peer status for {} to Connected.",
                local_self_peer_id, peer_id_for_main_task
            ));
        }

        if session_exists_at_dispatch {
            let channel_open_msg = WebRTCMessage::ChannelOpen {
                peer_id: local_self_peer_id.clone(),
            };
            
            if let Err(e) =
                send_webrtc_message(&peer_id_for_main_task, &channel_open_msg, state_clone.clone()).await
            {
                log_messages_for_task.push(format!("[ReportChannelOpenTask-{}] Error sending channel_open to {}: {}", local_self_peer_id, peer_id_for_main_task, e));
            } else {
                log_messages_for_task.push(format!("[ReportChannelOpenTask-{}] Sent channel_open to {}", local_self_peer_id, peer_id_for_main_task));
            }
            
            log_messages_for_task.push(format!("[ReportChannelOpenTask-{}] Calling check_and_send_mesh_ready for remote peer: {}", local_self_peer_id, peer_id_for_main_task));
            
            {
                let mut guard = state_clone.lock().await;
                for msg in log_messages_for_task {
                    guard.log.push(msg);
                }
            }

            check_and_send_mesh_ready(state_clone.clone(), internal_cmd_tx_clone).await;
        } else {
            log_messages_for_task.push(format!("[ReportChannelOpenTask-{}] Session does not exist for remote peer: {}. Skipping mesh check logic.", local_self_peer_id, peer_id_for_main_task));
            
            {
                let mut guard = state_clone.lock().await;
                for msg in log_messages_for_task {
                    guard.log.push(msg);
                }
            }
        }
    });
}

/// Handles sending own mesh ready signal
pub async fn handle_send_own_mesh_ready_signal<C>(
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
        let self_peer_id_local;
        let session_id_local;
        let participants_local;
        let mut mesh_became_ready = false;
        
        { 
            let mut state_guard = state_clone.lock().await;
            self_peer_id_local = state_guard.peer_id.clone();

            if let Some(session) = &state_guard.session {
                session_id_local = session.session_id.clone();
                participants_local = session.participants.clone();
                let session_participants_count = session.participants.len();
                
                let mut current_ready_peers = match &state_guard.mesh_status {
                    MeshStatus::PartiallyReady { ready_peers, .. } => ready_peers.clone(),
                    MeshStatus::Ready => {
                        // When status is Ready, we should include all session participants as ready
                        session.participants.iter().cloned().collect()
                    },
                    _ => HashSet::new(),
                };
                
                current_ready_peers.insert(self_peer_id_local.clone());

                state_guard.log.push(format!(
                    "Local node is mesh ready. Sending MeshReady signal to peers. Current ready peers count: {}",
                    current_ready_peers.len()
                ));

                if current_ready_peers.len() == session_participants_count {
                    state_guard.mesh_status = MeshStatus::Ready;
                    mesh_became_ready = true;
                    state_guard.log.push("All peers (including self) are mesh ready. Overall MeshStatus: Ready.".to_string());
                } else {
                    state_guard.mesh_status = MeshStatus::PartiallyReady {
                        ready_peers: current_ready_peers.clone(),
                        total_peers: session_participants_count,
                    };
                }
            } else {
                state_guard.log.push("Tried to send own MeshReady signal, but no active session.".to_string());
                return; 
            }
        } 
        
        if mesh_became_ready {
            if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                 state_clone.lock().await.log.push(format!("Failed to send CheckAndTriggerDkg command: {}", e));
            }
        }

        // Send mesh ready message to all peers
        let mesh_ready_msg = WebRTCMessage::MeshReady {
            session_id: session_id_local.clone(), 
            peer_id: self_peer_id_local.clone(), 
        };  
        
        // Set the flag to true immediately when we send our mesh ready signal
        // This prevents the race condition where we receive others' signals before sending our own
        state_clone.lock().await.own_mesh_ready_sent = true;
        
        state_clone.lock().await.log.push(format!(
            "Constructed WebRTCMessage::MeshReady to send to peers: {:?}",
            mesh_ready_msg
        ));

        let peers_to_notify: Vec<String> = participants_local
            .iter() 
            .filter(|p| **p != self_peer_id_local)
            .cloned() 
            .collect();

        for peer in peers_to_notify { 
            if let Err(e) =
                send_webrtc_message(&peer, &mesh_ready_msg, state_clone.clone()).await
            {   
                state_clone
                    .lock()
                    .await
                    .log 
                    .push(format!("Error sending MeshReady signal to {}: {}", peer, e));
            } else { 
                state_clone
                    .lock()
                    .await
                    .log 
                    .push(format!("Sent MeshReady signal to {}", peer));
            }       
        }
    });
}

/// Handles processing a mesh ready message from another peer
pub async fn handle_process_mesh_ready<C>(
    peer_id: String,
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
        let mut log_messages = Vec::new();
        let mut mesh_became_ready = false;

        { 
            let mut state_guard = state_clone.lock().await;
            
            if let Some(session) = &state_guard.session { 
                let total_session_participants = session.participants.len();
                
                // Check if all session responses have been received
                let all_session_responses_received = session.accepted_peers.len() == session.participants.len();
                
                if !all_session_responses_received {
                    log_messages.push(format!(
                        "Received MeshReady from {} but not all session responses received yet ({}/{} responses). Buffering for later processing.",
                        peer_id, session.accepted_peers.len(), session.participants.len()
                    ));
                    // Buffer the signal for later processing when all session responses are received
                    state_guard.pending_mesh_ready_signals.push(peer_id.clone());
                } else {
                    // All session responses received, process mesh ready normally
                    let mut current_ready_peers = match &state_guard.mesh_status {
                    MeshStatus::PartiallyReady { ready_peers, .. } => ready_peers.clone(),
                    MeshStatus::Ready => { 
                        log_messages.push(format!("Received MeshReady from {} but mesh is already Ready. Current ready peers: all {}.", peer_id, total_session_participants));
                        drop(state_guard);
                        let mut log_guard_early = state_clone.lock().await;
                        for msg_item in log_messages { log_guard_early.log.push(msg_item); }
                        return;
                    },
                    _ => {
                        // When status is Incomplete, check if current node has already sent mesh ready
                        // by looking at data channels - if we have channels to all peers, we should include ourselves
                        let mut initial_set = HashSet::new();
                        let session_peers_except_self: Vec<String> = session
                            .participants
                            .iter()
                            .filter(|p| **p != state_guard.peer_id)
                            .cloned()
                            .collect();
                        
                        let self_has_sent_mesh_ready = session_peers_except_self.iter()
                            .all(|peer_id| state_guard.data_channels.contains_key(peer_id));
                        
                        if self_has_sent_mesh_ready {
                            initial_set.insert(state_guard.peer_id.clone());
                            log_messages.push(format!("Status is Incomplete but current node has data channels to all peers, including self in ready count."));
                        }
                        initial_set
                    },
                };

                let already_known = current_ready_peers.contains(&peer_id);
                if !already_known {
                    current_ready_peers.insert(peer_id.clone());
                    log_messages.push(format!("Processing MeshReady from peer: {}. Added to ready set.", peer_id));
                } else {
                    log_messages.push(format!("Received duplicate MeshReady from {}. Not changing ready set.", peer_id));
                }
                
                log_messages.push(format!(
                    "Mesh readiness update: {} peers now in ready set. Total session participants: {}.",
                    current_ready_peers.len(), total_session_participants
                ));

                if current_ready_peers.len() == total_session_participants {
                    state_guard.mesh_status = MeshStatus::Ready;
                    mesh_became_ready = true;
                    log_messages.push(format!(
                        "All {} participants are mesh ready. Overall MeshStatus set to: Ready.",
                        total_session_participants
                    ));
                } else {
                    state_guard.mesh_status = MeshStatus::PartiallyReady {
                        ready_peers: current_ready_peers.clone(),
                        total_peers: total_session_participants,
                    };
                     log_messages.push(format!(
                        "Mesh status updated to PartiallyReady ({}/{} participants known ready).",
                        current_ready_peers.len(), 
                        total_session_participants
                    ));
                }
                } // End of else block for all_session_responses_received
            } else {
                log_messages.push(format!( 
                    "Received MeshReady from {} but no active session. Buffering for later processing.", peer_id
                ));
                // Buffer the signal for later processing when session becomes active
                state_guard.pending_mesh_ready_signals.push(peer_id.clone());
            }
        } 
        
        if mesh_became_ready {
            if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                log_messages.push(format!("Failed to send CheckAndTriggerDkg command: {}", e));
            }
        }
        
        let mut state_log_guard = state_clone.lock().await;
        for msg in log_messages {
            state_log_guard.log.push(msg);
        }
        state_log_guard.log.push(format!("Finished processing MeshReady from {}.", peer_id));
    });
}
