// use crate::network::websocket; // Unused
// use crate::protocal::signal::*; // Unused
use crate::protocal::signal::{WebRTCMessage, WebRTCSignal};
use crate::utils::state::{AppState, DkgState, InternalCommand};
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use frost_ed25519::Ed25519Sha512;
use webrtc_signal_server::ClientMsg as SharedClientMsg;
use crate::protocal::signal::{CandidateInfo, WebSocketMessage}; // Updated path


pub const DATA_CHANNEL_LABEL: &str = "frost-dkg"; 

pub async fn send_webrtc_message(
    target_peer_id: &str,
    message: &WebRTCMessage,
    state_log: Arc<Mutex<AppState<Ed25519Sha512>>>,
) -> Result<(), String> {
    let data_channel = {
        let guard = state_log.lock().await;
        guard.data_channels.get(target_peer_id).cloned()
    };

    if let Some(dc) = data_channel {
 
        if dc.ready_state() == RTCDataChannelState::Open {
   
            let msg_json = serde_json::to_string(&message)
                .map_err(|e| format!("Failed to serialize envelope: {}", e))?;

            if let Err(e) = dc.send_text(msg_json).await {
                state_log.lock().await.log.push(format!(
                    "Error sending message to {}: {}",
                    target_peer_id, e
                ));
                return Err(format!("Failed to send message: {}", e));
            }
            //         // udpate conncetion status
            // let mut state_guard = state_log.lock().await;
            // state_guard.peer_statuses.insert(
            //             target_peer_id.to_string(),
            //             webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected,
            // );

            state_log.lock().await.log.push(format!(
                "Successfully sent WebRTC message to {}",
                target_peer_id
            ));
            Ok(())
        } else {
            let err_msg = format!(
                "Data channel for {} is not open (state: {:?})",
                target_peer_id,
                dc.ready_state()
            );
            state_log.lock().await.log.push(err_msg.clone());
            Err(err_msg)
        }
    } else {
        let err_msg = format!("Data channel not found for peer {}", target_peer_id);
        state_log.lock().await.log.push(err_msg.clone());
        Err(err_msg)
    }
}

pub async fn create_and_setup_peer_connection(
    peer_id: String,
    self_peer_id: String, // Pass self_peer_id
    peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    state_log: Arc<Mutex<AppState<Ed25519Sha512>>>,
    api: &'static webrtc::api::API,
    config: &'static RTCConfiguration,
) -> Result<Arc<RTCPeerConnection>, String> {
    {
        let peer_conns = peer_connections_arc.lock().await;
        if let Some(existing_pc) = peer_conns.get(&peer_id) {
            state_log.lock().await.log.push(format!(
                "WebRTC connection object for {} already exists. Skipping creation.",
                peer_id
            ));
            return Ok(existing_pc.clone());
        }
    }

    state_log
        .lock()
        .await
        .log
        .push(format!("Creating WebRTC connection object for {}", peer_id));

    // Use passed-in api and config
    match api.new_peer_connection(config.clone()).await {
        Ok(pc) => {
            let pc_arc = Arc::new(pc);

            if self_peer_id < peer_id {
                match pc_arc.create_data_channel(DATA_CHANNEL_LABEL, None).await {
                    Ok(dc) => {
                        state_log.lock().await.log.push(format!(
                            "Initiator: Created data channel '{}' for {}",
                            DATA_CHANNEL_LABEL, peer_id
                        ));
                        setup_data_channel_callbacks(
                            dc,
                            peer_id.clone(),
                            state_log.clone(),
                            cmd_tx.clone(),
                        ).await;
                    }
                    Err(e) => {
                        state_log.lock().await.log.push(format!(
                            "Initiator: Failed to create data channel for {}: {}",
                            peer_id, e
                        ));
                    }
                }
            }

            let peer_id_on_ice = peer_id.clone();
            let cmd_tx_on_ice = cmd_tx.clone(); // Clones the sender for internal ClientMsg
            let state_log_on_ice = state_log.clone();
            pc_arc.on_ice_candidate(Box::new(move |candidate: Option<RTCIceCandidate>| {
                let peer_id = peer_id_on_ice.clone();
                let cmd_tx = cmd_tx_on_ice.clone();
                let state_log = state_log_on_ice.clone();
                Box::pin(async move {
                    if let Some(c) = candidate {
                        // ... existing ICE candidate sending logic ...
                        match c.to_json() {
                            Ok(init) => {
                                let signal = WebRTCSignal::Candidate(CandidateInfo {
                                    candidate: init.candidate,
                                    sdp_mid: init.sdp_mid,
                                    sdp_mline_index: init.sdp_mline_index,
                                });
                                let websocket_msg = WebSocketMessage::WebRTCSignal(signal);
                                match serde_json::to_value(websocket_msg) {
                                    Ok(json_val) => {
                                        // Wrap the Relay message inside SendToServer command
                                        let relay_cmd =
                                            InternalCommand::SendToServer(SharedClientMsg::Relay {
                                                to: peer_id.clone(),
                                                data: json_val,
                                            });
                                        let _ = cmd_tx.send(relay_cmd); // Send the internal command
                                        state_log
                                            .lock()
                                            .await
                                            .log
                                            .push(format!("Sent ICE candidate to {}", peer_id));
                                    }
                                    // FIX: Use error variable 'e'
                                    Err(e) => {
                                        state_log.lock().await.log.push(format!(
                                            "Error serializing ICE candidate for {}: {}",
                                            peer_id, e
                                        ));
                                    }
                                }
                            }
                            // FIX: Use error variable 'e'
                            Err(e) => {
                                state_log.lock().await.log.push(format!(
                                    "Error converting ICE candidate to JSON for {}: {}",
                                    peer_id, e
                                ));
                            }
                        }
                    }
                })
            }));

            // Setup state change handler with DKG trigger logic
            let state_log_on_state = state_log.clone();
            let peer_id_on_state = peer_id.clone();
            // Fix the setup_peer_connection_callbacks function
            // Clone before moving into closure
            let pc_arc_for_state = pc_arc.clone();
            pc_arc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                // Fix: Use pc_arc directly instead of undefined pc_arc_for_state
                let pc_arc = pc_arc_for_state.clone();
                
                // Log both connectionState and iceConnectionState together
                let ice_state = pc_arc.ice_connection_state();
                println!(
                    "Peer {}: connectionState={:?}, iceConnectionState={:?}",
                    peer_id, s, ice_state
                );
                if let Ok(mut app_state_guard) = state_log.try_lock() {
                    app_state_guard.peer_statuses.insert(peer_id.clone(), s);
                    app_state_guard.log.push(format!(
                        "WebRTC state with {}: {:?}, ICE: {:?}",
                        peer_id, s, ice_state
                    ));
                }

                // Handle state changes with improved logic
                match s {
                    RTCPeerConnectionState::Connected => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC CONNECTED with {} !!!", peer_id));
                            guard.reconnection_tracker.record_success(&peer_id);
                        }
                    }
                    RTCPeerConnectionState::Disconnected => {
                        // Handle disconnection with more aggressive reconnection
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC DISCONNECTED with {} !!!", peer_id));
                                                        
                            // Reset DKG state if a peer disconnects during DKG
                            if guard.dkg_state != DkgState::Idle && guard.dkg_state != DkgState::Complete {
                                guard.log.push(format!("Resetting DKG state due to disconnection with {}", peer_id));
                                guard.dkg_state = DkgState::Failed(format!("Peer {} disconnected", peer_id));
                                // Clear intermediate DKG data if needed
                                guard.dkg_part1_public_package = None;
                                guard.dkg_part1_secret_package = None;
                                guard.received_dkg_packages.clear();
                            }
                            
                            // Always attempt immediate reconnection on Disconnected state
                            if let Some(current_session) = guard.session.clone() {
                                let session_id_to_rejoin = current_session.session_id;
                                guard.log.push(format!(
                                    "Attempting immediate reconnection to session '{}' due to DISCONNECTED state with {} (no JoinSession message sent, logic removed)",
                                    session_id_to_rejoin, peer_id
                                ));
                                // Drop the guard before sending the command
                                drop(guard);
                                // No JoinSession message sent
                            }
                        }
                    }
                    RTCPeerConnectionState::Failed => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC FAILED with {} !!!", peer_id));
                            
                            // Reset DKG state if a peer disconnects during DKG
                            if guard.dkg_state != DkgState::Idle && guard.dkg_state != DkgState::Complete {
                                guard.log.push(format!("Resetting DKG state due to connection failure with {}", peer_id));
                                guard.dkg_state = DkgState::Failed(format!("Peer {} connection failed", peer_id));
                                guard.dkg_part1_public_package = None;
                                guard.dkg_part1_secret_package = None;
                                guard.received_dkg_packages.clear();
                            }
                            
                            // Attempt to rejoin with backoff strategy
                            if guard.reconnection_tracker.should_attempt(&peer_id) {
                                if let Some(current_session) = guard.session.clone() {
                                    let session_id_to_rejoin = current_session.session_id;
                                    guard.log.push(format!(
                                        "Attempting reconnection to session '{}' due to FAILED state with {} (no JoinSession message sent, logic removed)",
                                        session_id_to_rejoin, peer_id
                                    ));
                                    // Drop the guard before sending the command
                                    drop(guard);
                                    // No JoinSession message sent
                                }
                            }
                        }
                    }
                    RTCPeerConnectionState::Connecting | RTCPeerConnectionState::New => {
                        // We don't need special handling for these states,
                        // they're already logged above when updating peer_statuses
                    }
                    RTCPeerConnectionState::Closed => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("WebRTC connection CLOSED with {}", peer_id));
                        }
                    }
                    // Handle the Unspecified state to fix the compilation error
                    RTCPeerConnectionState::Unspecified => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("WebRTC in UNSPECIFIED state with {}", peer_id));
                            // No specific action needed for unspecified state
                        }
                    }
                }
                Box::pin(async {})
            }));

            // --- Setup ICE connection monitoring callback ---
            let state_log_ice = state_log_on_state.clone();
            let peer_id_ice = peer_id_on_state.clone();
            let pc_arc_for_ice = pc_arc.clone();
            pc_arc.on_ice_connection_state_change(Box::new(move |ice_state| {
                let state_log = state_log_ice.clone();
                let peer_id = peer_id_ice.clone();
                let pc_arc = pc_arc_for_ice.clone();

                // Log both connectionState and iceConnectionState together
                let conn_state = pc_arc.connection_state();
                println!(
                    "Peer {}: connectionState={:?}, iceConnectionState={:?}",
                    peer_id, conn_state, ice_state
                );
                if let Ok(mut guard) = state_log.try_lock() {
                    guard.log.push(format!(
                        "ICE connection state with {}: {:?}, PeerConnection: {:?}",
                        peer_id, ice_state, conn_state
                    ));
                }
                // No async work, just return a ready future
                Box::pin(async {})
            }));

            // --- Only set up callbacks for the main data channel (responder side) ---
            let state_log_on_data = state_log_on_state.clone();
            let peer_id_on_data = peer_id_on_state.clone();
            let cmd_tx_on_data = cmd_tx.clone();
            pc_arc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let state_log = state_log_on_data.clone();
                let peer_id = peer_id_on_data.clone();
                let cmd_tx_clone = cmd_tx_on_data.clone();

                Box::pin(async move {
                    if dc.label() == DATA_CHANNEL_LABEL {
                        state_log.lock().await.log.push(format!(
                            "Responder: Data channel '{}' opened by {}",
                            dc.label(),
                            peer_id
                        ));
                        setup_data_channel_callbacks(dc, peer_id, state_log, cmd_tx_clone).await;
                    }
                })
            }));

            // --- Store the connection object ---
            {
                let mut peer_conns = peer_connections_arc.lock().await;
                peer_conns.insert(peer_id_on_state.clone(), pc_arc.clone());
                state_log_on_state
                    .lock()
                    .await
                    .log
                    .push(format!("Stored WebRTC connection object for {}", peer_id_on_state));
            } // Drop lock

            Ok(pc_arc)
        }
        Err(e) => {
            // FIX: Add actual log message
            let err_msg = format!(
                "Error creating peer connection object for {}: {}",
                peer_id, e
            );
            state_log.lock().await.log.push(err_msg.clone());
            Err(err_msg)
        }
    }
}

pub async fn setup_data_channel_callbacks(
    dc: Arc<RTCDataChannel>,
    peer_id: String,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    // Update the sender type here
    cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Use InternalCommand
) {
    let dc_arc = dc.clone(); // Clone the Arc for the data channel

    // Only store and set up callbacks for the main data channel
    if dc_arc.label() == DATA_CHANNEL_LABEL {
        let mut guard = state.lock().await;
        guard.data_channels.insert(peer_id.clone(), dc_arc.clone());
        guard
            .log
            .push(format!("Data channel for {} stored in app state", peer_id));
    }

    let state_log_open = state.clone();
    let peer_id_open = peer_id.clone();
    let dc_clone = dc_arc.clone();
    dc_arc.on_open(Box::new(move || {
        let state_log_open = state_log_open.clone();
        let peer_id_open = peer_id_open.clone();
        let dc_clone = dc_clone.clone();
        Box::pin(async move {
            state_log_open.lock().await.log.push(format!(
                "Data channel '{}' open confirmed with {}",
                dc_clone.label(),
                peer_id_open
            ));
        })
    }));

    let state_log_msg = state.clone();
    let peer_id_msg = peer_id.clone();
    let cmd_tx_msg = cmd_tx.clone(); // Clone internal cmd_tx for on_message
    let dc_arc_msg = dc_arc.clone(); // Clone for use inside async block
    dc_arc.on_message(Box::new(move |msg: DataChannelMessage| {
        let peer_id = peer_id_msg.clone();
        let state_log = state_log_msg.clone();
        let cmd_tx = cmd_tx_msg.clone();
        let dc_arc = dc_arc_msg.clone(); // Use a clone inside the async block

        Box::pin(async move {
            // Only process messages if this is the main frost-dkg channel
            if dc_arc.label() != DATA_CHANNEL_LABEL {
                return;
            }

            if let Ok(text) = String::from_utf8(msg.data.to_vec()) {
                // Parse envelope
                match serde_json::from_str::<WebRTCMessage>(&text) {
                    Ok(envelope) => {
                        match envelope {
                            WebRTCMessage::DkgRound1Package { package } => {
                                    let _ = cmd_tx.send(InternalCommand::ProcessDkgRound1 {
                                        from_peer_id: peer_id.clone(),
                                        package,
                                    });
                            }
                            WebRTCMessage::DkgRound2Package { package } => {
                                // FIX: Add type annotation for from_value
                                    state_log.lock().await.log.push(format!(
                                        "Received DKG Round 2 package from {}",
                                        peer_id
                                    ));
                                    let _ = cmd_tx.send(InternalCommand::ProcessDkgRound2 {
                                        from_peer_id: peer_id.clone(),
                                        package,
                                    });
                            }
                            WebRTCMessage::SimpleMessage { text } => {
                                    state_log.lock().await.log.push(format!(
                                        "Receiver: Message from {}: {}",
                                        peer_id, text
                                    ));
                            },
                            WebRTCMessage::ChannelOpen { peer_id: _ } => {
                                // FIX: Add type annotation for from_value
                                state_log.lock().await.log.push(format!(
                                    "Data channel opened with {}",
                                    peer_id
                                ));
                                let _ = cmd_tx.send(InternalCommand::ReportChannelOpen {
                                    peer_id: peer_id.clone(),
                                });
                            },
                            WebRTCMessage::MeshReady { session_id, peer_id } => {
                                state_log.lock().await.log.push(format!(
                                    "Mesh ready notification from {}: session_id: {}, peer_id: {}",
                                    peer_id, session_id, peer_id
                                ));
                                let _ = cmd_tx.send(InternalCommand::ProcessMeshReady {
                                    peer_id: peer_id.clone(),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        state_log
                            .lock()
                            .await
                            .log
                            .push(format!("Failed to parse envelope from {}: {}", peer_id, e));
                    }
                }
            } else {
                state_log
                    .lock()
                    .await
                    .log
                    .push(format!("Received non-UTF8 data from {}", peer_id));
            }
        })
    }));

    let state_log_close = state.clone();
    let peer_id_close = peer_id.clone();
    dc.on_close(Box::new(move || {
        let state_log_close = state_log_close.clone();
        let peer_id_close = peer_id_close.clone();
        Box::pin(async move {
            state_log_close.lock().await.log.push(format!(
                "Data channel '{}' closed with {}",
                DATA_CHANNEL_LABEL, peer_id_close
            ));
        })
    }));

    let state_log_error = state.clone();
    let peer_id_error = peer_id.clone();
    dc.on_error(Box::new(move |e| {
        let state_log_error = state_log_error.clone();
        let peer_id_error = peer_id_error.clone();
        Box::pin(async move {
            state_log_error.lock().await.log.push(format!(
                "Data channel '{}' error with {}: {}",
                DATA_CHANNEL_LABEL, peer_id_error, e
            ));
        })
    }));
}

// Apply any pending ICE candidates for a peer
pub async fn apply_pending_candidates(
    peer_id: &str,
    pc: Arc<RTCPeerConnection>,
    state_log: Arc<Mutex<AppState<Ed25519Sha512>>>,
) {
    // Take the pending candidates for this peer
    let candidates = {
        let mut state_guard = state_log.lock().await;
        let pending = state_guard.pending_ice_candidates.remove(peer_id);
        if let Some(candidates) = &pending {
            if !candidates.is_empty() {
                state_guard.log.push(format!(
                    "Applying {} stored ICE candidate(s) for {}",
                    candidates.len(),
                    peer_id
                ));
            }
        }
        pending
    };

    // If there are pending candidates, apply them
    if let Some(candidates) = candidates {
        // Apply each candidate
        for candidate in candidates {
            match pc.add_ice_candidate(candidate.clone()).await {
                Ok(_) => {
                    let mut state_guard = state_log.lock().await;
                    state_guard
                        .log
                        .push(format!("Applied stored ICE candidate for {}", peer_id));
                    // apply candidate to the peer connection
                    
                }
                Err(e) => {
                    let mut state_guard = state_log.lock().await;
                    state_guard.log.push(format!(
                            "Error applying stored ICE candidate for {}: {}",
                            peer_id, e
                        ));                
                }
            }
        }
    }
}