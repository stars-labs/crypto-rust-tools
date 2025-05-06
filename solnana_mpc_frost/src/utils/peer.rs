use crate::utils::signal::*;
use crate::utils::state::{AppState, DkgState};

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Mutex as TokioMutex, mpsc};

use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use frost_core::keys::dkg::round2;
use frost_ed25519::Ed25519Sha512;
use solnana_mpc_frost::{ClientMsg as SharedClientMsg, InternalCommand};

pub const DATA_CHANNEL_LABEL: &str = "frost-dkg"; 

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DataChannelEnvelope {
    pub msg_type: String,
    pub payload: serde_json::Value,
}

pub async fn send_webrtc_message(
    target_peer_id: &str,
    message: &WebRTCMessage,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
) -> Result<(), String> {
    let data_channel = {
        let guard = state_log.lock().unwrap();
        guard.data_channels.get(target_peer_id).cloned()
    };

    if let Some(dc) = data_channel {
 
        if dc.ready_state() == RTCDataChannelState::Open {
   
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
                }
            };
            let envelope = DataChannelEnvelope {
                msg_type: msg_type.to_string(),
                payload,
            };
            let msg_json = serde_json::to_string(&envelope)
                .map_err(|e| format!("Failed to serialize envelope: {}", e))?;

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

pub async fn create_and_setup_peer_connection(
    peer_id: String,
    self_peer_id: String, // Pass self_peer_id
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    api: &'static webrtc::api::API,
    config: &'static RTCConfiguration,
) -> Result<Arc<RTCPeerConnection>, String> {
    {
        let peer_conns = peer_connections_arc.lock().await;
        if let Some(existing_pc) = peer_conns.get(&peer_id) {
            state_log.lock().unwrap().log.push(format!(
                "WebRTC connection object for {} already exists. Skipping creation.",
                peer_id
            ));
            return Ok(existing_pc.clone());
        }
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

            pc_arc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                let state_log = state_log_on_state.clone();
                let peer_id = peer_id_on_state.clone();
                let cmd_tx_local = cmd_tx_on_state.clone();
                let pc_weak = Arc::downgrade(&pc_arc_for_state);

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

                            if !guard.keep_alive_peers.contains(&peer_id) {
                                guard.keep_alive_peers.insert(peer_id.clone());                                
                                // Setup keep-alive mechanism using a dedicated channel
                                let pc_weak_clone = pc_weak.clone();
                                let peer_id_clone = peer_id.clone();
                                let state_log_clone = state_log.clone();
                                let cmd_tx_clone = cmd_tx_local.clone();

                                tokio::spawn(async move {
                                    setup_robust_keepalive(
                                        pc_weak_clone,
                                        peer_id_clone,
                                        state_log_clone,
                                        cmd_tx_clone,
                                    ).await;
                                });
                            }
                        }
                    }
                    RTCPeerConnectionState::Disconnected => {
                        // Handle disconnection with more aggressive reconnection
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC DISCONNECTED with {} !!!", peer_id));
                            
                            // Remove keep-alive tracking
                            guard.keep_alive_peers.remove(&peer_id);
                            
                            // Reset DKG state if a peer disconnects during DKG
                            if guard.dkg_state != DkgState::Idle && guard.dkg_state != DkgState::Complete {
                                guard.log.push(format!("Resetting DKG state due to disconnection with {}", peer_id));
                                guard.dkg_state = DkgState::Failed(format!("Peer {} disconnected", peer_id));
                                // Clear intermediate DKG data if needed
                                guard.local_dkg_part1_data = None;
                                guard.received_dkg_packages.clear();
                            }
                            
                            // Always attempt immediate reconnection on Disconnected state
                            if let Some(current_session) = guard.session.clone() {
                                let session_id_to_rejoin = current_session.session_id;
                                guard.log.push(format!(
                                    "Attempting immediate reconnection to session '{}' due to DISCONNECTED state with {}",
                                    session_id_to_rejoin, peer_id
                                ));
                                
                                // Drop the guard before sending the command
                                drop(guard);
                                
                                // Send rejoin command without checking the reconnection tracker
                                let _ = cmd_tx_local.send(InternalCommand::SendToServer(SharedClientMsg::JoinSession {
                                    session_id: session_id_to_rejoin
                                }));
                            }
                        }
                    }
                    RTCPeerConnectionState::Failed => {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC FAILED with {} !!!", peer_id));
                            
                            // Remove keep-alive tracking
                            guard.keep_alive_peers.remove(&peer_id);
                            
                            // Reset DKG state if a peer disconnects during DKG
                            if guard.dkg_state != DkgState::Idle && guard.dkg_state != DkgState::Complete {
                                guard.log.push(format!("Resetting DKG state due to connection failure with {}", peer_id));
                                guard.dkg_state = DkgState::Failed(format!("Peer {} connection failed", peer_id));
                                guard.local_dkg_part1_data = None;
                                guard.received_dkg_packages.clear();
                            }
                            
                            // Attempt to rejoin with backoff strategy
                            if guard.reconnection_tracker.should_attempt(&peer_id) {
                                if let Some(current_session) = guard.session.clone() {
                                    let session_id_to_rejoin = current_session.session_id;
                                    guard.log.push(format!(
                                        "Attempting reconnection to session '{}' due to FAILED state with {}",
                                        session_id_to_rejoin, peer_id
                                    ));
                                    
                                    // Drop the guard before sending the command
                                    drop(guard);
                                    
                                    let _ = cmd_tx_local.send(InternalCommand::SendToServer(SharedClientMsg::JoinSession {
                                        session_id: session_id_to_rejoin
                                    }));
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
                            guard.keep_alive_peers.remove(&peer_id);
                            
                            // For explicit close, we shouldn't auto-reconnect
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
            let state_log_ice = state_log.clone();
            let peer_id_ice = peer_id.clone();
            
            pc_arc.on_ice_connection_state_change(Box::new(move |ice_state| {
                let state_log = state_log_ice.clone();
                let peer_id = peer_id_ice.clone();
                
                if let Ok(mut guard) = state_log.try_lock() {
                    guard.log.push(format!(
                        "ICE connection state with {}: {:?}",
                        peer_id, ice_state
                    ));
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

// New improved keep-alive function (updated to work with the available API)
async fn setup_robust_keepalive(
    pc_weak: std::sync::Weak<RTCPeerConnection>,
    peer_id: String,
    state_log: Arc<StdMutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    let mut failure_count = 0;
    let max_failures = 3;
    
    loop {
        interval.tick().await;
        
        // Check if PC still exists
        if let Some(pc_strong) = pc_weak.upgrade() {
            // Check connection state
            let current_state = pc_strong.connection_state();
            
            match current_state {
                RTCPeerConnectionState::Connected | RTCPeerConnectionState::Connecting => {
                    // Connection is live - create or use data channel for pinging
                    match pc_strong.create_data_channel("keep-alive", None).await {
                        Ok(dc) => {
                            // Reset failure count on successful channel creation
                            failure_count = 0;
                            
                            // Use a small scope for the ping attempt
                            {
                                if let Ok(mut guard) = state_log.try_lock() {
                                    guard.log.push(format!(
                                        "Keep-alive: Successfully created channel for peer {}",
                                        peer_id
                                    ));
                                }
                            }
                            
                            // Setup data channel state callback
                            let dc_arc = Arc::new(dc);
                            let dc_weak = Arc::downgrade(&dc_arc);
                            let peer_id_clone = peer_id.clone();
                            let state_log_clone = state_log.clone();
                            
                            dc_arc.on_open(Box::new(move || {
                                let dc_weak_ping = dc_weak.clone();
                                let peer_ping = peer_id_clone.clone();
                                let state_ping = state_log_clone.clone();
                                
                                // Log channel open
                                if let Ok(mut guard) = state_ping.try_lock() {
                                    guard.log.push(format!("Keep-alive channel open with {}", peer_ping));
                                }
                                
                                // Send a single ping and close
                                tokio::spawn(async move {
                                    if let Some(dc_strong) = dc_weak_ping.upgrade() {
                                        // Send ping
                                        if let Err(e) = dc_strong.send_text("ping").await {
                                            if let Ok(mut guard) = state_ping.try_lock() {
                                                guard.log.push(format!(
                                                    "Keep-alive failed with {}: {}",
                                                    peer_ping, e
                                                ));
                                            }
                                        }
                                        // Don't need to close explicitly - channel will be garbage collected
                                    }
                                });
                                
                                Box::pin(async {})
                            }));
                            
                            // Wait a bit to let the ping happen before recreating channel on next interval
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            failure_count += 1;
                            
                            if let Ok(mut guard) = state_log.try_lock() {
                                guard.log.push(format!(
                                    "Keep-alive: Failed to create channel for {}: {} (failure {}/{})",
                                    peer_id, e, failure_count, max_failures
                                ));
                                
                                // If we've hit max failures while connected, try to kickstart renegotiation
                                if failure_count >= max_failures && 
                                    current_state == RTCPeerConnectionState::Connected {
                                    guard.log.push(format!(
                                        "Too many keep-alive failures with {}. Triggering reconnection...",
                                        peer_id
                                    ));
                                    
                                    // Get session info for rejoin
                                    if let Some(current_session) = guard.session.clone() {
                                        let session_id_to_rejoin = current_session.session_id;
                                        drop(guard); // Drop before sending command
                                        
                                        // Request rejoin to refresh the connection
                                        let _ = cmd_tx.send(InternalCommand::SendToServer(SharedClientMsg::JoinSession {
                                            session_id: session_id_to_rejoin
                                        }));
                                        
                                        // Reset failure counter
                                        failure_count = 0;
                                    }
                                }
                            }
                        }
                    }
                }
                // For other states, don't try to keep alive - the main state handler will manage reconnection
                _ => {
                    if let Ok(mut guard) = state_log.try_lock() {
                        // Only log this occasionally to avoid spam
                        if failure_count == 0 {
                            guard.log.push(format!(
                                "Keep-alive: Connection with {} not in connectable state: {:?}",
                                peer_id, current_state
                            ));
                        }
                    }
                    // Increment failure count to reduce logging frequency
                    failure_count += 1;
                    
                    // If connection is in a bad state for too long, exit the keep-alive loop
                    if failure_count > max_failures * 2 {
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!(
                                "Keep-alive: Stopping monitor for {} due to persistent bad state: {:?}",
                                peer_id, current_state
                            ));
                        }
                        break;
                    }
                }
            }
        } else {
            // Connection was dropped, exit keep-alive loop
            if let Ok(mut guard) = state_log.try_lock() {
                guard.log.push(format!(
                    "Keep-alive: Stopping monitor for {} - connection object no longer exists",
                    peer_id
                ));
            }
            break;
        }
    }
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