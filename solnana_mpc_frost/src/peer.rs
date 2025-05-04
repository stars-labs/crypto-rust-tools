use crate::signal::*;
use crate::state::{AppState, DkgState}; // Import AppState and DkgState
// Remove unused imports
// use frost_core::keys::PublicKeyPackage;
// use frost_ed25519::Ed25519Sha512;
// Use the internal ClientMsg type from cli_node

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration; // Add time imports
use tokio::sync::{Mutex as TokioMutex, mpsc};
// Add necessary WebRTC imports
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate; // Add RTCIceCandidate
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration; // Import RTCConfiguration
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
// Import InternalCommand and ClientMsg (aliased as SharedClientMsg) from lib.rs
use frost_core::keys::dkg::round2; // Add round2 import
use frost_ed25519::Ed25519Sha512;
use solnana_mpc_frost::{ClientMsg as SharedClientMsg, InternalCommand}; // Add Ed25519Sha512 import

// --- Constants ---
// Make constant public
pub const DATA_CHANNEL_LABEL: &str = "frost-dkg"; // Label for the main data channel
pub const KEEP_ALIVE_CHANNEL_LABEL: &str = "keep-alive";

// Only use one data channel per peer (DATA_CHANNEL_LABEL), and use a message envelope for all messages.

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DataChannelEnvelope {
    pub msg_type: String, // e.g. "dkg_round1", "simple", etc.
    pub payload: serde_json::Value,
}

// --- Helper Function to Send WebRTC Messages ---
pub async fn send_webrtc_message(
    target_peer_id: &str,
    message: &WebRTCMessage,
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
) -> Result<(), String> {
    // Check if data channel exists in app state
    let data_channel = {
        let guard = state_log.lock().unwrap();
        guard.data_channels.get(target_peer_id).cloned()
    };

    if let Some(dc) = data_channel {
        // Check if data channel is open
        if dc.ready_state() == RTCDataChannelState::Open {
            // Wrap message in envelope
            let (msg_type, payload) = match message {
                WebRTCMessage::DkgRound1Package { package } => (
                    "dkg_round1",
                    serde_json::to_value(package).unwrap_or(serde_json::Value::Null),
                ),
                WebRTCMessage::DkgRound2Package { package } => (
                    "dkg_round2",
                    serde_json::to_value(package).unwrap_or(serde_json::Value::Null),
                ),
                WebRTCMessage::SimpleMessage { text } => {
                    ("simple", serde_json::json!({ "text": text }))
                } // ...add more variants as needed...
            };
            let envelope = DataChannelEnvelope {
                msg_type: msg_type.to_string(),
                payload,
            };
            let msg_json = serde_json::to_string(&envelope)
                .map_err(|e| format!("Failed to serialize envelope: {}", e))?;

            // Send the message
            if let Err(e) = dc.send_text(msg_json).await {
                state_log.lock().unwrap().log.push(format!(
                    "Error sending message to {}: {}",
                    target_peer_id, e
                ));
                return Err(format!("Failed to send message: {}", e));
            }

            state_log.lock().unwrap().log.push(format!(
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
            state_log.lock().unwrap().log.push(err_msg.clone());
            Err(err_msg)
        }
    } else {
        let err_msg = format!("Data channel not found for peer {}", target_peer_id);
        state_log.lock().unwrap().log.push(err_msg.clone());
        Err(err_msg)
    }
}

// Creates a single peer connection, sets up callbacks, and stores it.
pub async fn create_and_setup_peer_connection(
    peer_id: String,
    self_peer_id: String, // Pass self_peer_id
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    // Update the sender type to use the new InternalCommand
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    // Add WebRTC API and Config as arguments
    api: &'static webrtc::api::API,
    config: &'static RTCConfiguration,
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

    // Use passed-in api and config
    match api.new_peer_connection(config.clone()).await {
        Ok(pc) => {
            let pc_arc = Arc::new(pc);

            // --- Only create one data channel (initiator side) ---
            if self_peer_id < peer_id {
                match pc_arc.create_data_channel(DATA_CHANNEL_LABEL, None).await {
                    Ok(dc) => {
                        state_log.lock().unwrap().log.push(format!(
                            "Initiator: Created data channel '{}' for {}",
                            DATA_CHANNEL_LABEL, peer_id
                        ));
                        setup_data_channel_callbacks(
                            dc,
                            peer_id.clone(),
                            state_log.clone(),
                            cmd_tx.clone(),
                        );
                    }
                    Err(e) => {
                        state_log.lock().unwrap().log.push(format!(
                            "Initiator: Failed to create data channel for {}: {}",
                            peer_id, e
                        ));
                    }
                }
            }

            // --- Setup Callbacks (Essential before processing any signals) ---
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
                                match serde_json::to_value(signal) {
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

            // Setup state change handler with DKG trigger logic
            let state_log_on_state = state_log.clone();
            let peer_id_on_state = peer_id.clone();
            let cmd_tx_on_state = cmd_tx.clone(); // Clones the sender for internal ClientMsg
            let pc_arc_for_state = pc_arc.clone();
            let self_peer_id_on_state = self_peer_id.clone(); // Capture self_peer_id
            pc_arc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                let state_log = state_log_on_state.clone();
                let peer_id = peer_id_on_state.clone();
                let cmd_tx_local = cmd_tx_on_state.clone();
                let pc_weak = Arc::downgrade(&pc_arc_for_state);
                let self_peer_id = self_peer_id_on_state.clone();

                println!("Peer Connection State with {} has changed: {}", peer_id, s);

                // Update status map first
                if let Ok(mut app_state_guard) = state_log.try_lock() {
                    app_state_guard.peer_statuses.insert(peer_id.clone(), s);
                    app_state_guard
                        .log
                        .push(format!("WebRTC state with {}: {:?}", peer_id, s));
                }

                // Handle state changes with improved logic
                match s {
                    RTCPeerConnectionState::Connected => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC CONNECTED with {} !!!", peer_id));
                            guard.reconnection_tracker.record_success(&peer_id);

                            // --- REMOVED DKG Check ---
                            // The DKG trigger logic is moved to the SessionInfo handler in cli_node.rs
                            // to ensure session data is ready before starting.

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
                    }
                    RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed => {
                        // Combine Disconnected and Failed handling for rejoin logic
                        if let Ok(mut guard) = state_log.try_lock() {
                            let state_name = if s == RTCPeerConnectionState::Disconnected { "DISCONNECTED" } else { "FAILED" };
                            guard.log.push(format!("!!! WebRTC {} with {} !!!", state_name, peer_id));

                            // Remove keep-alive tracking if it failed/disconnected
                            guard.keep_alive_peers.remove(&peer_id);

                            // Reset DKG state if a peer disconnects during DKG
                            if guard.dkg_state != DkgState::Idle && guard.dkg_state != DkgState::Complete {
                                guard.log.push(format!("Resetting DKG state due to disconnection with {}", peer_id));
                                guard.dkg_state = DkgState::Failed(format!("Peer {} disconnected", peer_id));
                                // Clear intermediate DKG data if needed
                                guard.local_dkg_part1_data = None;
                                guard.received_dkg_packages.clear();
                            }

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

                                    // Send the *shared* message type via the internal sender
                                    let _ = cmd_tx_local.send(InternalCommand::SendToServer(SharedClientMsg::JoinSession {
                                        session_id: session_id_to_rejoin
                                    }));

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
                    }
                    _ => {
                        // Log other states without special handling
                        // Already logged above when status map is updated
                    }
                }

                Box::pin(async {})
            }));

            // --- Only set up callbacks for the main data channel (responder side) ---
            let state_log_on_data = state_log.clone();
            let peer_id_on_data = peer_id.clone();
            let cmd_tx_on_data = cmd_tx.clone();
            pc_arc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                let state_log = state_log_on_data.clone();
                let peer_id = peer_id_on_data.clone();
                let cmd_tx_clone = cmd_tx_on_data.clone();

                if dc.label() == DATA_CHANNEL_LABEL {
                    state_log.lock().unwrap().log.push(format!(
                        "Responder: Data channel '{}' opened by {}",
                        dc.label(),
                        peer_id
                    ));
                    setup_data_channel_callbacks(dc, peer_id, state_log, cmd_tx_clone);
                }
                Box::pin(async move {})
            }));

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

// Helper function to set up callbacks for a data channel (created or received)
// Make function public
pub fn setup_data_channel_callbacks(
    dc: Arc<RTCDataChannel>,
    peer_id: String,
    state: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    // Update the sender type here
    cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Use InternalCommand
) {
    let dc_arc = dc.clone(); // Clone the Arc for the data channel

    // Only store and set up callbacks for the main data channel
    if dc_arc.label() == DATA_CHANNEL_LABEL {
        let mut guard = state.lock().unwrap();
        guard.data_channels.insert(peer_id.clone(), dc_arc.clone());
        guard
            .log
            .push(format!("Data channel for {} stored in app state", peer_id));
    }

    let state_log_open = state.clone();
    let peer_id_open = peer_id.clone();
    let dc_clone = dc_arc.clone();
    dc_arc.on_open(Box::new(move || {
        state_log_open.lock().unwrap().log.push(format!(
            "Data channel '{}' open confirmed with {}",
            dc_clone.label(),
            peer_id_open
        ));
        Box::pin(async {})
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
                match serde_json::from_str::<DataChannelEnvelope>(&text) {
                    Ok(envelope) => {
                        match envelope.msg_type.as_str() {
                            "dkg_round1" => {
                                if let Ok(package) = serde_json::from_value(envelope.payload) {
                                    state_log.lock().unwrap().log.push(format!(
                                        "Received DKG Round 1 package from {}",
                                        peer_id
                                    ));
                                    let _ = cmd_tx.send(InternalCommand::ProcessDkgRound1 {
                                        from_peer_id: peer_id.clone(),
                                        package,
                                    });
                                } else {
                                    state_log.lock().unwrap().log.push(format!(
                                        "Failed to parse DKG Round 1 payload from {}",
                                        peer_id
                                    ));
                                }
                            }
                            "dkg_round2" => {
                                // FIX: Add type annotation for from_value
                                if let Ok(package) =
                                    serde_json::from_value::<round2::Package<Ed25519Sha512>>(
                                        envelope.payload,
                                    )
                                {
                                    state_log.lock().unwrap().log.push(format!(
                                        "Received DKG Round 2 package from {}",
                                        peer_id
                                    ));
                                    let _ = cmd_tx.send(InternalCommand::ProcessDkgRound2 {
                                        from_peer_id: peer_id.clone(),
                                        package,
                                    });
                                } else {
                                    state_log.lock().unwrap().log.push(format!(
                                        "Failed to parse DKG Round 2 payload from {}",
                                        peer_id
                                    ));
                                }
                            }
                            "simple" => {
                                if let Some(text) =
                                    envelope.payload.get("text").and_then(|v| v.as_str())
                                {
                                    state_log.lock().unwrap().log.push(format!(
                                        "Receiver: Message from {}: {}",
                                        peer_id, text
                                    ));
                                } else {
                                    state_log
                                        .lock()
                                        .unwrap()
                                        .log
                                        .push(format!("Malformed simple message from {}", peer_id));
                                }
                            }
                            _ => {
                                state_log.lock().unwrap().log.push(format!(
                                    "Unknown message type '{}' from {}",
                                    envelope.msg_type, peer_id
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        state_log
                            .lock()
                            .unwrap()
                            .log
                            .push(format!("Failed to parse envelope from {}: {}", peer_id, e));
                    }
                }
            } else {
                state_log
                    .lock()
                    .unwrap()
                    .log
                    .push(format!("Received non-UTF8 data from {}", peer_id));
            }
        })
    }));

    let state_log_close = state.clone();
    let peer_id_close = peer_id.clone();
    dc.on_close(Box::new(move || {
        state_log_close.lock().unwrap().log.push(format!(
            "Data channel '{}' closed with {}",
            DATA_CHANNEL_LABEL, peer_id_close
        ));
        Box::pin(async {})
    }));

    let state_log_error = state.clone();
    let peer_id_error = peer_id.clone();
    dc.on_error(Box::new(move |e| {
        state_log_error.lock().unwrap().log.push(format!(
            "Data channel '{}' error with {}: {}",
            DATA_CHANNEL_LABEL, peer_id_error, e
        ));
        Box::pin(async {})
    }));
}

// Apply any pending ICE candidates for a peer
pub async fn apply_pending_candidates(
    peer_id: &str,
    pc: Arc<RTCPeerConnection>,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
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
