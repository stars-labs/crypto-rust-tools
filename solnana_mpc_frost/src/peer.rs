use crate::signal::*;
use crate::state::AppState; // Import AppState
use solnana_mpc_frost::ClientMsg;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration; // Add time imports
use tokio::sync::{Mutex as TokioMutex, mpsc};
// Add necessary WebRTC imports
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
// Import WEBRTC_API and WEBRTC_CONFIG if they are defined elsewhere (e.g., cli_node.rs or a new webrtc_setup.rs)
use crate::{WEBRTC_API, WEBRTC_CONFIG}; // Assuming they remain in cli_node.rs or are made public there

// Creates a single peer connection, sets up callbacks, and stores it.
pub async fn create_and_setup_peer_connection(
    peer_id: String,
    _self_peer_id: String, // FIX: Mark unused variable
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    cmd_tx: mpsc::UnboundedSender<ClientMsg>,
    state_log: Arc<StdMutex<AppState>>,
) -> Result<Arc<RTCPeerConnection>, String> {
    // Check if connection already exists before creating
    {
        let peer_conns = peer_connections_arc.lock().await;
        if let Some(existing_pc) = peer_conns.get(&peer_id) {
            // FIX: Connection already exists, log and return Ok with the existing Arc
            state_log.lock().unwrap().log.push(format!(
                "WebRTC connection object for {} already exists. Skipping creation.",
                peer_id
            ));
            return Ok(existing_pc.clone()); // Return the existing connection Arc
        }
        // Drop lock implicitly here
    }

    state_log
        .lock()
        .unwrap()
        .log
        .push(format!("Creating WebRTC connection object for {}", peer_id));

    match WEBRTC_API.new_peer_connection(WEBRTC_CONFIG.clone()).await {
        Ok(pc) => {
            let pc_arc = Arc::new(pc);

            // --- Setup Callbacks (Essential before processing any signals) ---
            let peer_id_on_ice = peer_id.clone();
            let cmd_tx_on_ice = cmd_tx.clone();
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
                                match serde_json::to_value(signal) {
                                    Ok(json_val) => {
                                        let _ = cmd_tx.send(ClientMsg::Relay {
                                            to: peer_id.clone(),
                                            data: json_val,
                                        });
                                        state_log
                                            .lock()
                                            .unwrap()
                                            .log
                                            .push(format!("Sent ICE candidate to {}", peer_id));
                                    }
                                    // FIX: Use error variable 'e'
                                    Err(e) => {
                                        state_log.lock().unwrap().log.push(format!(
                                            "Error serializing ICE candidate for {}: {}",
                                            peer_id, e
                                        ));
                                    }
                                }
                            }
                            // FIX: Use error variable 'e'
                            Err(e) => {
                                state_log.lock().unwrap().log.push(format!(
                                    "Error converting ICE candidate to JSON for {}: {}",
                                    peer_id, e
                                ));
                            }
                        }
                    }
                })
            }));

            // Setup state change handler with improved reconnection logic
            let state_log_on_state = state_log.clone();
            let peer_id_on_state = peer_id.clone();
            let cmd_tx_on_state = cmd_tx.clone();
            let pc_arc_for_state = pc_arc.clone();
            pc_arc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                let state_log = state_log_on_state.clone();
                let peer_id = peer_id_on_state.clone();
                let cmd_tx_local = cmd_tx_on_state.clone();
                let pc_weak = Arc::downgrade(&pc_arc_for_state); // Keep pc_weak for potential future use

                println!("Peer Connection State with {} has changed: {}", peer_id, s);

                // Handle state changes with improved logic
                match s {
                    RTCPeerConnectionState::Connected => {
                        // Reset reconnection tracking when connected
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC CONNECTED with {} !!!", peer_id));
                            guard.reconnection_tracker.record_success(&peer_id); // Use record_success

                            // Setup keep-alive if not already set
                            if !guard.keep_alive_peers.contains(&peer_id) {
                                guard.keep_alive_peers.insert(peer_id.clone());

                                // Setup keep-alive mechanism using a dedicated channel
                                let pc_weak_clone = pc_weak.clone();
                                let peer_id_clone = peer_id.clone();
                                let state_log_clone = state_log.clone();

                                tokio::spawn(async move {
                                    // Create a dedicated keep-alive channel
                                    if let Some(pc_strong) = pc_weak_clone.upgrade() {
                                        match pc_strong.create_data_channel("keep-alive", None).await {
                                            Ok(dc) => {
                                                let dc_arc = Arc::new(dc);

                                                // Set up on_open handler for channel
                                                let dc_weak = Arc::downgrade(&dc_arc);
                                                let peer_id_ping = peer_id_clone.clone();
                                                let state_log_ping = state_log_clone.clone();

                                                dc_arc.on_open(Box::new(move || {
                                                    if let Ok(mut guard) = state_log_ping.try_lock() {
                                                        guard.log.push(format!("Keep-alive channel open with {}", peer_id_ping));
                                                    }

                                                    // Start ping loop in a new task
                                                    let dc_weak_ping = dc_weak.clone();
                                                    let peer_ping = peer_id_ping.clone();
                                                    let state_ping = state_log_ping.clone();

                                                    tokio::spawn(async move {
                                                        let mut interval = tokio::time::interval(Duration::from_secs(10));

                                                        loop {
                                                            interval.tick().await;

                                                            if let Some(dc_strong) = dc_weak_ping.upgrade() {
                                                                if dc_strong.ready_state() != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                                                                    break;
                                                                }

                                                                if let Err(e) = dc_strong.send_text("ping").await {
                                                                    if let Ok(mut guard) = state_ping.try_lock() {
                                                                        guard.log.push(format!(
                                                                            "Keep-alive channel to {} failed: {}",
                                                                            peer_ping, e
                                                                        ));
                                                                    }
                                                                    break;
                                                                }
                                                            } else {
                                                                // Channel was dropped
                                                                break;
                                                            }
                                                        }
                                                    });

                                                    Box::pin(async {})
                                                }));

                                                if let Ok(mut guard) = state_log_clone.try_lock() {
                                                    guard.log.push(format!("Created keep-alive channel for {}", peer_id_clone));
                                                }
                                            },
                                            Err(e) => {
                                                if let Ok(mut guard) = state_log_clone.try_lock() {
                                                    guard.log.push(format!("Failed to create keep-alive channel for {}: {}",
                                                        peer_id_clone, e));
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    },
                    RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed => {
                        // Combine Disconnected and Failed handling for rejoin logic
                        if let Ok(mut guard) = state_log.try_lock() {
                            let state_name = if s == RTCPeerConnectionState::Disconnected { "DISCONNECTED" } else { "FAILED" };
                            guard.log.push(format!("!!! WebRTC {} with {} !!!", state_name, peer_id));

                            // Remove keep-alive tracking if it failed/disconnected
                            guard.keep_alive_peers.remove(&peer_id);

                            // Attempt to rejoin the current session if allowed by tracker
                            if guard.reconnection_tracker.should_attempt(&peer_id) {
                                if let Some(current_session) = guard.session.clone() {
                                    let session_id_to_rejoin = current_session.session_id;
                                    guard.log.push(format!(
                                        "Attempting automatic rejoin to session '{}' due to {} state with {}",
                                        session_id_to_rejoin, state_name, peer_id
                                    ));

                                    // Drop the guard before sending the command
                                    drop(guard);

                                    // Send JoinSession command to trigger rejoin process
                                    let _ = cmd_tx_local.send(ClientMsg::JoinSession {
                                        session_id: session_id_to_rejoin
                                    });

                                } else {
                                    // Log if no active session to rejoin
                                    guard.log.push(format!(
                                        "Cannot attempt automatic rejoin for {}: No active session.",
                                        peer_id
                                    ));
                                }
                            } else {
                                // Log if reconnection is skipped due to backoff/cooldown
                                guard.log.push(format!(
                                    "Skipping automatic rejoin attempt for {} (backoff/cooldown).",
                                    peer_id
                                ));
                            }
                        }
                    },
                    _ => {
                        // Log other states without special handling
                        if let Ok(mut app_state_guard) = state_log.try_lock() {
                            app_state_guard.log.push(format!("WebRTC state with {}: {:?}", peer_id, s));
                            // Update status map for TUI display
                            app_state_guard.peer_statuses.insert(peer_id.clone(), s);
                        }
                    }
                }

                // Also update the status in state regardless of special handling
                // Ensure the status map reflects the latest state reported by the callback
                if let Ok(mut app_state_guard) = state_log.try_lock() {
                    app_state_guard.peer_statuses.insert(peer_id.clone(), s);
                }

                Box::pin(async {})
            }));

            let state_log_on_data = state_log.clone();
            let peer_id_on_data = peer_id.clone();
            pc_arc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                // ... existing on_data_channel logic (logging, on_open, on_message) ...
                let state_log = state_log_on_data.clone();
                let peer_id = peer_id_on_data.clone();
                // FIX: Add actual log message
                state_log.lock().unwrap().log.push(format!(
                    "Receiver: Data channel '{}' opened by {}",
                    dc.label(),
                    peer_id
                ));

                let state_log_open = state_log.clone();
                let peer_id_open = peer_id.clone();
                dc.on_open(Box::new(move || {
                    // FIX: Add actual log message
                    state_log_open.lock().unwrap().log.push(format!(
                        "Receiver: Data channel open confirmed with {}",
                        peer_id_open
                    ));
                    Box::pin(async {})
                }));

                let state_log_msg = state_log.clone();
                let peer_id_msg = peer_id.clone();
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    // FIX: Add actual log message
                    let msg_str = String::from_utf8(msg.data.to_vec())
                        .unwrap_or_else(|_| "Non-UTF8 data".to_string());
                    state_log_msg.lock().unwrap().log.push(format!(
                        "Receiver: Message from {}: {}",
                        peer_id_msg, msg_str
                    ));
                    Box::pin(async {})
                }));

                Box::pin(async move {})
            }));
            // --- End Setup Callbacks ---

            // --- Store the connection object ---
            {
                let mut peer_conns = peer_connections_arc.lock().await;
                peer_conns.insert(peer_id.clone(), pc_arc.clone());
                state_log
                    .lock()
                    .unwrap()
                    .log
                    .push(format!("Stored WebRTC connection object for {}", peer_id));
            } // Drop lock

            Ok(pc_arc)
        }
        Err(e) => {
            // FIX: Add actual log message
            let err_msg = format!(
                "Error creating peer connection object for {}: {}",
                peer_id, e
            );
            state_log.lock().unwrap().log.push(err_msg.clone());
            Err(err_msg)
        }
    }
}

// Apply any pending ICE candidates for a peer
pub async fn apply_pending_candidates(
    peer_id: &str,
    pc: Arc<RTCPeerConnection>,
    state_log: Arc<StdMutex<AppState>>,
) {
    // Take the pending candidates for this peer
    let candidates = {
        let mut state_guard = state_log.lock().unwrap();
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
                    if let Ok(mut state_guard) = state_log.lock() {
                        state_guard
                            .log
                            .push(format!("Applied stored ICE candidate for {}", peer_id));
                    }
                }
                Err(e) => {
                    if let Ok(mut state_guard) = state_log.lock() {
                        state_guard.log.push(format!(
                            "Error applying stored ICE candidate for {}: {}",
                            peer_id, e
                        ));
                    }
                }
            }
        }
    }
}
