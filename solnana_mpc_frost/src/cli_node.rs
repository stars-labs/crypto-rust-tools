use std::sync::{Arc, Mutex as StdMutex}; // Use StdMutex for TUI state
// Remove unused SystemTime, UNIX_EPOCH
use std::time::Duration; // Add SystemTime and UNIX_EPOCH
// Add HashSet import
use std::{
    collections::{HashMap, HashSet},
    io,
};

use crossterm::{
    // Remove unused KeyCode
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::{SinkExt, StreamExt};
use lazy_static::lazy_static;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{Mutex as TokioMutex, mpsc}; // Use TokioMutex for async WebRTC state
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
// Uncomment imports
// Remove unused DataChannelMessage import
// use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit; // Keep RTCIceCandidateInit here if used directly
// Correct the import path for RTCIceCredentialType
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

// Add these imports for WebRTC policies
use webrtc::peer_connection::policy::{
    bundle_policy::RTCBundlePolicy, ice_transport_policy::RTCIceTransportPolicy,
    rtcp_mux_policy::RTCRtcpMuxPolicy,
};
// Import shared types from the library crate
use solnana_mpc_frost::{ClientMsg, ServerMsg, SessionInfo};

// --- Declare Modules ---
mod negotiation;
mod peer;
mod signal;
mod state;
mod tui;

// --- Use items from modules ---
use negotiation::initiate_offers_for_session;
use peer::{apply_pending_candidates, create_and_setup_peer_connection};
use signal::{SDPInfo, WebRTCSignal};
use state::{AppState, ReconnectionTracker};
// Import the new handler function
use tui::{draw_main_ui, handle_key_event};

// --- WebRTC API Setup (Keep lazy_static here or move to a dedicated module) ---
lazy_static! {
    // Make these public if peer.rs or negotiation.rs need direct access
    // Or pass WEBRTC_API as an argument if preferred
    pub static ref WEBRTC_CONFIG: RTCConfiguration = RTCConfiguration {
        ice_servers: vec![
            // Primary STUN servers - using multiple to increase reliability
            RTCIceServer {
                urls: vec![
                    "stun:stun.l.google.com:19302".to_owned(),
                    "stun:stun1.l.google.com:19302".to_owned(),
                    "stun:stun2.l.google.com:19302".to_owned(),
                    "stun:stun3.l.google.com:19302".to_owned(),
                    "stun:stun4.l.google.com:19302".to_owned(),
                ],
                ..Default::default()
            },
            // 添加更多可靠的TURN服务器 - 改善NAT穿透
            RTCIceServer {
                urls: vec!["turn:numb.viagenie.ca".to_owned()],
                username: "muazkh".to_owned(),
                credential: "webrtc@live.com".to_owned(),
            },
            // 备用公共TURN服务器
            RTCIceServer {
                urls: vec!["turn:openrelay.metered.ca:80".to_owned()],
                username: "openrelayproject".to_owned(),
                credential: "openrelayproject".to_owned(),
            },
        ],
        ice_transport_policy: RTCIceTransportPolicy::All,
        bundle_policy: RTCBundlePolicy::MaxBundle,
        rtcp_mux_policy: RTCRtcpMuxPolicy::Require,
        ice_candidate_pool_size: 10, // 增加候选池大小以提高连接成功率
        ..Default::default()
    };
    pub static ref WEBRTC_API: webrtc::api::API = {
        let mut m = MediaEngine::default();
        // NOTE: Registering codecs is required for audio/video, but not for data channels.
        // m.register_default_codecs().unwrap();
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m).unwrap();
        APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build()
    };
}
// --- End WebRTC API Setup ---

// --- State, Signal, Peer, Negotiation structs/enums/functions moved to respective modules ---

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Get peer_id from user
    println!("Enter your peer_id:");
    let mut peer_id = String::new();
    io::stdin().read_line(&mut peer_id)?;
    let peer_id = peer_id.trim().to_string();

    // Connect to signaling server
    let url = url::Url::parse("ws://127.0.0.1:9000").unwrap();
    let (ws_stream, _) = connect_async(url).await?;
    // Remove mut from ws_stream
    let (mut ws_sink, ws_stream) = ws_stream.split();

    // Register
    ws_sink
        .send(Message::Text(serde_json::to_string(
            &ClientMsg::Register {
                peer_id: peer_id.clone(),
            },
        )?))
        .await?;

    // Channel for TUI to send commands to network
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClientMsg>();

    // Shared state for TUI and network
    let state = Arc::new(StdMutex::new(AppState {
        // TUI state uses StdMutex
        peer_id: peer_id.clone(),
        peers: vec![],
        log: vec!["Registered with server".to_string()],
        session: None,
        invites: vec![],
        peer_connections: Arc::new(TokioMutex::new(HashMap::new())), // WebRTC state uses TokioMutex
        peer_statuses: HashMap::new(),                               // Initialize peer statuses
        reconnection_tracker: ReconnectionTracker::new(),
        keep_alive_peers: HashSet::new(),
        // Remove cmd_tx field as it's unused in AppState
        // cmd_tx: Some(cmd_tx.clone()), // 存储一个命令发送器副本
        // --- Initialize Perfect Negotiation Flags ---
        making_offer: HashMap::new(),
        ignore_offer: HashMap::new(),
        // Initialize the pending_ice_candidates field
        pending_ice_candidates: HashMap::new(),
    }));

    // Spawn network task
    let state_net = state.clone();
    let self_peer_id = peer_id.clone();
    let cmd_tx_net = cmd_tx.clone(); // Clone cmd_tx for the network task
    // Clone peer_connections Arc for the network task
    let peer_connections_arc_net = state.lock().unwrap().peer_connections.clone();

    // Add periodic connection status checker task
    let state_for_checker = state.clone();
    let peer_connections_for_checker = peer_connections_arc_net.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            interval.tick().await;

            // Check and update all connection statuses
            let peer_conns = {
                let lock = peer_connections_for_checker.lock().await;
                lock.clone()
            };

            // Check each connection and update status
            for (peer_id, pc) in peer_conns.iter() {
                let current_state = pc.connection_state();

                // Update the status in AppState
                if let Ok(mut guard) = state_for_checker.try_lock() {
                    // Only update if we're in a session with this peer
                    if let Some(session) = &guard.session {
                        if session.participants.contains(peer_id) {
                            let old_status = guard.peer_statuses.get(peer_id).cloned();

                            // Update status
                            guard.peer_statuses.insert(peer_id.clone(), current_state);

                            // Log significant status changes
                            if old_status != Some(current_state) {
                                guard.log.push(format!(
                                    "Status updated for {}: {:?} → {:?}",
                                    peer_id,
                                    old_status.unwrap_or(RTCPeerConnectionState::New),
                                    current_state
                                ));
                            }

                            // For Connected status, perform an extra check for data channels
                            if current_state == RTCPeerConnectionState::Connected {
                                // This connection is functional - ensure we're not showing misleading status
                                guard.reconnection_tracker.record_success(peer_id);
                            }
                        }
                    }
                }
            }
        }
    });

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        // Request peer list on start
        let _ = ws_sink
            .send(Message::Text(
                serde_json::to_string(&ClientMsg::ListPeers).unwrap(),
            ))
            .await;
        // Need mut here because cmd_rx.recv() takes &mut self
        let mut cmd_rx = cmd_rx;
        // Need mut here because ws_stream.next() takes &mut self
        let mut ws_stream = ws_stream;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Periodically request peer list to check if all participants have joined
                    let _ = ws_sink.send(Message::Text(serde_json::to_string(&ClientMsg::ListPeers).unwrap())).await;
                }
                Some(cmd) = cmd_rx.recv() => {
                    let _ = ws_sink.send(Message::Text(serde_json::to_string(&cmd).unwrap())).await;
                }
                Some(msg) = ws_stream.next() => {
                    match msg {
                        Ok(Message::Text(txt)) => {
                            match serde_json::from_str::<ServerMsg>(&txt) {
                                Ok(server_msg) => {
                                    // Handle simple state updates first, keep lock brief
                                    match server_msg { // Restore missing arms here
                                        ServerMsg::Peers { ref peers } => {
                                            // --- Lock StdMutex ---
                                            let mut state_guard = state_net.lock().unwrap();
                                            state_guard.peers = peers.clone();
                                            // FIX: Remove offer initiation logic from here
                                            drop(state_guard); // Drop lock
                                        }
                                        ServerMsg::SessionInvite { ref session_id, ref from, total, threshold, ref participants } => {
                                            // ... (SessionInvite handling remains the same) ...
                                            let mut state_guard = state_net.lock().unwrap();
                                            // FIX: Remove unnecessary parentheses
                                            if !state_guard.invites.iter().any(|inv| inv.session_id == *session_id) {
                                                state_guard.invites.push(SessionInfo { session_id: session_id.clone(), total, threshold, participants: participants.clone() });
                                                state_guard.log.push(format!("Session invite from {} for session {}", from, session_id));
                                            } else {
                                                 state_guard.log.push(format!("Duplicate invite from {} for session {}", from, session_id));
                                            }
                                            // Drop guard immediately
                                            drop(state_guard);
                                        }
                                        ServerMsg::Error { ref error } => {
                                            // ... (Error handling remains the same) ...
                                            let mut state_guard = state_net.lock().unwrap();
                                            state_guard.log.push(format!("Error: {}", error));
                                            // Drop guard immediately
                                            drop(state_guard);
                                        }
                                        // Handle complex cases involving async operations separately
                                        ServerMsg::SessionInfo { session_id, total, threshold, participants } => {
                                            // --- Lock StdMutex, update state, drop lock ---
                                            // ... (existing SessionInfo handling) ...
                                            { // New scope for StdMutexGuard
                                                let mut state_guard = state_net.lock().unwrap();
                                                let session_info = SessionInfo { session_id: session_id.clone(), total, threshold, participants: participants.clone() };
                                                state_guard.session = Some(session_info.clone());
                                                state_guard.log.push(format!("Session info received/updated: {}. Waiting for all participants.", session_info.session_id));
                                                // Reset the flag as we have new session info (or updated participants)
                                                // FIX: Selectively update peer_statuses instead of clearing
                                                let new_participants_set: HashSet<String> = participants.iter().cloned().collect();
                                                state_guard.peer_statuses.retain(|peer_id, _| new_participants_set.contains(peer_id));
                                                // Add default status for new participants not already in the map
                                                for p_id in &participants {
                                                    if *p_id != state_guard.peer_id && !state_guard.peer_statuses.contains_key(p_id) {
                                                        // Add a default status, e.g., Connecting or New. Let's use Connecting.
                                                        state_guard.peer_statuses.insert(p_id.clone(), RTCPeerConnectionState::Connecting);
                                                        state_guard.log.push(format!("Set initial status for new participant {} to Connecting", p_id));
                                                    }
                                                }
                                            } // Drop StdMutexGuard

                                            // --- Spawn Task to Create PeerConnection Objects for NEW Peers ---
                                            // ... (existing connection object creation logic) ...
                                            let pc_arc_clone = peer_connections_arc_net.clone();
                                            let cmd_tx_clone = cmd_tx_net.clone();
                                            let state_log_clone = state_net.clone();
                                            let self_peer_id_clone = self_peer_id.clone();
                                            let participants_clone = participants.clone(); // Clone participants for the task
                                            let session_id_clone = session_id.clone(); // Clone session_id for logging

                                            tokio::spawn(async move {
                                                // ... (existing peer connection setup and offer initiation logic) ...
                                                state_log_clone.lock().unwrap().log.push(format!(
                                                    "SessionInfo Task: Creating peer connections for session {}...",
                                                    session_id_clone
                                                ));
                                                // FIX: Iterate over a slice to avoid moving participants_clone
                                                for peer_id in &participants_clone {
                                                    if *peer_id != self_peer_id_clone {
                                                        // Use function from peer module
                                                        let _ = create_and_setup_peer_connection(
                                                            peer_id.clone(),
                                                            self_peer_id_clone.clone(),
                                                            pc_arc_clone.clone(),
                                                            cmd_tx_clone.clone(),
                                                            state_log_clone.clone(),
                                                        ).await;
                                                    }
                                                }
                                                // Initiate offers after creating connections
                                                // Use function from negotiation module
                                                initiate_offers_for_session(
                                                    participants_clone, // Pass the original Vec here
                                                    self_peer_id_clone,
                                                    pc_arc_clone,
                                                    cmd_tx_clone,
                                                    state_log_clone,
                                                ).await;
                                            }); // End of spawned task
                                        }
                                        ServerMsg::Relay { ref from, ref data } => {
                                            state_net.lock().unwrap().log.push(format!("Relay from {}: {:?}", from, data)); // Log raw data
                                            match serde_json::from_value::<WebRTCSignal>(data.clone()) {
                                                Ok(signal) => {
                                                    state_net.lock().unwrap().log.push(format!("Parsed WebRTC signal from {}: {:?}", from, signal)); // Log parsed signal
                                                    // Clone necessary Arcs and data FOR the spawned task
                                                    let pc_arc_net_clone = peer_connections_arc_net.clone();
                                                    let from_clone = from.clone();
                                                    let cmd_tx_clone = cmd_tx_net.clone();
                                                    let state_log_clone = state_net.clone();
                                                    let signal_clone = signal.clone(); // Clone the parsed signal
                                                    let self_peer_id_clone = self_peer_id.clone();

                                                    tokio::spawn(async move { // Spawn signal handling
                                                        // --- Get or Create Peer Connection ---
                                                        let pc_result = { // Scope for lock
                                                            let peer_conns = pc_arc_net_clone.lock().await;
                                                            peer_conns.get(&from_clone).cloned() // Try to get existing
                                                        };

                                                        // Restructure: Define the result explicitly first
                                                        let pc_to_use_result: Result<Arc<RTCPeerConnection>, String> = match pc_result {
                                                            Some(pc) => Ok(pc), // Use existing
                                                            None => {
                                                                // If not found, try to create it now
                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                    "WebRTC signal from {} received, but connection object missing. Attempting creation...",
                                                                    from_clone
                                                                ));
                                                                // Call create_and_setup_peer_connection. It handles "already exists" internally.
                                                                // Use function from peer module
                                                                create_and_setup_peer_connection(
                                                                    from_clone.clone(),
                                                                    self_peer_id_clone.clone(), // Pass self_peer_id
                                                                    pc_arc_net_clone.clone(),   // Pass Arc again
                                                                    cmd_tx_clone.clone(),
                                                                    state_log_clone.clone(),
                                                                ).await // Await the result
                                                            }
                                                        };
                                                        // --- End Get or Create Peer Connection ---


                                                        // --- Process Signal if PC is available ---
                                                        // Match on the explicitly defined result
                                                        match pc_to_use_result {
                                                            Ok(pc_clone) => { // Use the obtained (existing or newly created) Arc
                                                                match signal_clone {
                                                                    WebRTCSignal::Offer(offer_info) => {
                                                                        use webrtc::peer_connection::signaling_state::RTCSignalingState;

                                                                        // --- Perfect Negotiation & State Check (Read Only under Lock) ---
                                                                        // FIX: Remove mut and initial assignment for proceed_with_offer
                                                                        let proceed_with_offer;
                                                                        let mut should_abort_due_to_race = false;
                                                                        // FIX: Remove is_stable_or_closed variable declaration here
                                                                        let current_signaling_state_read;

                                                                        { // Scope for AppState lock (read-only phase)
                                                                            let mut state_guard = state_log_clone.lock().unwrap();
                                                                            let making = state_guard.making_offer.get(&from_clone).copied().unwrap_or(false);
                                                                            let ignoring = state_guard.ignore_offer.get(&from_clone).copied().unwrap_or(false);
                                                                            let is_polite = self_peer_id_clone > from_clone;
                                                                            current_signaling_state_read = pc_clone.signaling_state();

                                                                            state_guard.log.push(format!(
                                                                                "Offer from {}: making={}, ignoring={}, is_polite={}, state={:?}",
                                                                                from_clone, making, ignoring, is_polite, current_signaling_state_read
                                                                            ));

                                                                            // --- Glare Handling logic ---
                                                                            let collision = making;
                                                                            // FIX: Calculate proceed_with_offer directly here
                                                                            proceed_with_offer = if collision && is_polite {
                                                                                true // Polite peer yields, but will process this offer
                                                                            } else if collision && !is_polite {
                                                                                false // Impolite peer ignores incoming offer during collision
                                                                            } else if ignoring {
                                                                                false // Explicitly ignoring
                                                                            } else {
                                                                                true // No collision or ignoring, proceed
                                                                            };


                                                                            // --- Abort Check (Safeguard) ---
                                                                            // FIX: Check against the calculated proceed_with_offer
                                                                            if proceed_with_offer && making {
                                                                                should_abort_due_to_race = true;
                                                                                // proceed_with_offer = false; // Let the next block handle this
                                                                            }

                                                                        } // --- AppState lock dropped ---

                                                                        // --- Update State based on decisions (Brief lock) ---
                                                                        // FIX: Re-evaluate proceed_with_offer based on state checks inside this block
                                                                        let mut final_proceed_with_offer = proceed_with_offer; // Start with initial decision
                                                                        {
                                                                            let mut state_guard = state_log_clone.lock().unwrap();
                                                                            let making = state_guard.making_offer.get(&from_clone).copied().unwrap_or(false);
                                                                            let is_polite = self_peer_id_clone > from_clone;
                                                                            let collision = making;

                                                                            if should_abort_due_to_race {
                                                                                 state_guard.log.push(format!(
                                                                                    "Aborting offer from {}: Detected making_offer=true just before async operation (Race?)",
                                                                                    from_clone
                                                                                ));
                                                                                final_proceed_with_offer = false; // Abort
                                                                            } else if collision && is_polite {
                                                                                state_guard.log.push(format!("Glare detected with {}: Polite peer yielding (state update).", from_clone));
                                                                                state_guard.making_offer.insert(from_clone.clone(), false);
                                                                                // final_proceed_with_offer remains true (polite peer processes)
                                                                            } else if collision && !is_polite {
                                                                                state_guard.log.push(format!("Glare detected with {}: Impolite peer ignoring incoming offer (state update).", from_clone));
                                                                                state_guard.ignore_offer.insert(from_clone.clone(), true);
                                                                                final_proceed_with_offer = false; // Impolite peer ignores
                                                                            } else if state_guard.ignore_offer.get(&from_clone).copied().unwrap_or(false) {
                                                                                state_guard.log.push(format!("Ignoring offer from {} as previously decided (state update).", from_clone));
                                                                                state_guard.ignore_offer.insert(from_clone.clone(), false); // Reset ignore flag
                                                                                final_proceed_with_offer = false; // Ignore this time
                                                                            }

                                                                            // FIX: Perform state check directly here using current_signaling_state_read
                                                                            let is_invalid_state = !(current_signaling_state_read == RTCSignalingState::Stable || current_signaling_state_read == RTCSignalingState::Closed);
                                                                            if final_proceed_with_offer && is_invalid_state {
                                                                                state_guard.log.push(format!(
                                                                                    "Aborting offer from {}: Invalid signaling state {:?} (expected Stable or Closed)",
                                                                                    from_clone, current_signaling_state_read
                                                                                ));
                                                                                final_proceed_with_offer = false; // Cannot proceed in this state
                                                                            }
                                                                        } // --- AppState lock dropped ---

                                                                        // --- Perform Async Operations (No AppState lock held) ---
                                                                        // FIX: Use final_proceed_with_offer for the decision
                                                                        if final_proceed_with_offer {
                                                                            state_log_clone.lock().unwrap().log.push(format!(
                                                                                "Processing offer from {}...", from_clone
                                                                            ));
                                                                            // FIX: Use RTCSessionDescription::offer
                                                                            match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::offer(
                                                                                offer_info.sdp.clone()
                                                                            ) {
                                                                                Ok(offer) => {
                                                                                    if let Err(e) = pc_clone.set_remote_description(offer).await {
                                                                                        state_log_clone.lock().unwrap().log.push(format!(
                                                                                            "Error setting remote description (offer) from {}: {}", from_clone, e
                                                                                        ));
                                                                                        return;
                                                                                    }
                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                        "Set remote description (offer) from {}. Creating answer...", from_clone
                                                                                    ));
                                                                                    // Apply any pending ICE candidates now that the remote description is set
                                                                                    // Use function from peer module
                                                                                    apply_pending_candidates(&from_clone, pc_clone.clone(), state_log_clone.clone()).await;

                                                                                    // Create data channel before answering (optional)
                                                                                    // This helps ensure both sides have a data channel
                                                                                    match pc_clone.create_data_channel("respond-data", None).await {
                                                                                        Ok(dc) => {
                                                                                            let dc_arc = Arc::new(dc);
                                                                                            let peer_id_dc = from_clone.clone();
                                                                                            let state_log_dc = state_log_clone.clone();

                                                                                            dc_arc.on_open(Box::new(move || {
                                                                                                state_log_dc.lock().unwrap().log.push(format!(
                                                                                                    "Responder: Data channel opened with {}", peer_id_dc
                                                                                                ));
                                                                                                Box::pin(async {})
                                                                                            }));

                                                                                            state_log_clone.lock().unwrap().log.push(format!(
                                                                                                "Created responder data channel for {}", from_clone
                                                                                            ));
                                                                                        },
                                                                                        Err(e) => {
                                                                                            state_log_clone.lock().unwrap().log.push(format!(
                                                                                                "Error creating responder data channel for {}: {} (continuing anyway)",
                                                                                                from_clone, e
                                                                                            ));
                                                                                            // Don't return/fail here, as we can still answer without our own channel
                                                                                        }
                                                                                    }

                                                                                    match pc_clone.create_answer(None).await {
                                                                                        Ok(answer) => {
                                                                                            if let Err(e) = pc_clone.set_local_description(answer.clone()).await {
                                                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                                                    "Error setting local description (answer) for {}: {}", from_clone, e
                                                                                                ));
                                                                                                return;
                                                                                            }
                                                                                            state_log_clone.lock().unwrap().log.push(format!(
                                                                                                "Set local description (answer) for {}. Sending answer...", from_clone
                                                                                            ));
                                                                                            let signal = WebRTCSignal::Answer(SDPInfo { sdp: answer.sdp });
                                                                                            match serde_json::to_value(signal) {
                                                                                                Ok(json_val) => {
                                                                                                    let _ = cmd_tx_clone.send(ClientMsg::Relay {
                                                                                                        to: from_clone.clone(),
                                                                                                        data: json_val,
                                                                                                    });
                                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                                        "Sent answer to {}", from_clone
                                                                                                    ));
                                                                                                },
                                                                                                Err(e) => {
                                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                                        "Error serializing answer for {}: {}", from_clone, e
                                                                                                    ));
                                                                                                }
                                                                                            }
                                                                                        },
                                                                                        Err(e) => {
                                                                                            state_log_clone.lock().unwrap().log.push(format!(
                                                                                                "Error creating answer for {}: {}", from_clone, e
                                                                                            ));
                                                                                        }
                                                                                    }
                                                                                },
                                                                                Err(e) => {
                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                        "Error parsing offer from {}: {}", from_clone, e
                                                                                    ));
                                                                                }
                                                                            }
                                                                        }
                                                                    }
                                                                    WebRTCSignal::Answer(answer_info) => {

                                                                        state_log_clone.lock().unwrap().log.push(format!(
                                                                            "Processing answer from {}...", from_clone
                                                                        ));
                                                                        // FIX: Use RTCSessionDescription::answer
                                                                        match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::answer(
                                                                            answer_info.sdp.clone()
                                                                        ) {
                                                                            Ok(answer) => {
                                                                                if let Err(e) = pc_clone.set_remote_description(answer).await {
                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                        "Error setting remote description (answer) from {}: {}", from_clone, e
                                                                                    ));
                                                                                } else {
                                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                                        "Set remote description (answer) from {}", from_clone
                                                                                    ));
                                                                                    // Apply any pending ICE candidates now that the remote description is set
                                                                                    // Use function from peer module
                                                                                    apply_pending_candidates(&from_clone, pc_clone.clone(), state_log_clone.clone()).await;
                                                                                }
                                                                            },
                                                                            Err(e) => {
                                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                                    "Error parsing answer from {}: {}", from_clone, e
                                                                                ));
                                                                            }
                                                                        }
                                                                    }
                                                                    WebRTCSignal::Candidate(candidate_info) => {
                                                                        // ... (existing Candidate handling) ...
                                                                        state_log_clone.lock().unwrap().log.push(format!(
                                                                            "Processing candidate from {}...", from_clone
                                                                        ));
                                                                        // FIX: Use RTCIceCandidateInit directly with add_ice_candidate
                                                                        let candidate_init = RTCIceCandidateInit { // Use imported RTCIceCandidateInit
                                                                            candidate: candidate_info.candidate,
                                                                            sdp_mid: candidate_info.sdp_mid,
                                                                            sdp_mline_index: candidate_info.sdp_mline_index,
                                                                            username_fragment: None, // Add username_fragment field
                                                                        };

                                                                        // Check if remote description is set before adding ICE candidate
                                                                        let current_state = pc_clone.signaling_state();
                                                                        let remote_description_set = match current_state {
                                                                            webrtc::peer_connection::signaling_state::RTCSignalingState::HaveRemoteOffer |
                                                                            webrtc::peer_connection::signaling_state::RTCSignalingState::HaveLocalPranswer |
                                                                            webrtc::peer_connection::signaling_state::RTCSignalingState::HaveRemotePranswer |
                                                                            webrtc::peer_connection::signaling_state::RTCSignalingState::Stable => true,
                                                                            _ => false,
                                                                        };

                                                                        if remote_description_set {
                                                                            // If remote description is set, add the candidate directly
                                                                            if let Err(e) = pc_clone.add_ice_candidate(candidate_init.clone()).await {
                                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                                    "Error adding ICE candidate from {}: {}", from_clone, e
                                                                                ));
                                                                            } else {
                                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                                    "Added ICE candidate from {}", from_clone
                                                                                ));
                                                                            }
                                                                        } else {
                                                                            // If remote description is not set, store the candidate for later
                                                                            let mut state_guard = state_log_clone.lock().unwrap();
                                                                            state_guard.log.push(format!(
                                                                                "Storing ICE candidate from {} for later (remote description not set yet)",
                                                                                from_clone
                                                                            ));
                                                                            let candidates = state_guard.pending_ice_candidates
                                                                                .entry(from_clone.clone())
                                                                                .or_insert_with(Vec::new);
                                                                            candidates.push(candidate_init);
                                                                            drop(state_guard);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            Err(e) => {
                                                                // Log the error from pc_to_use_result
                                                                state_log_clone.lock().unwrap().log.push(format!(
                                                                    "Failed to create/retrieve connection object for {} to handle signal: {}",
                                                                    from_clone, e
                                                                ));
                                                            }
                                                        }
                                                    }); // End of spawned task for signal handling
                                                }
                                                Err(e) => {
                                                    // ... (existing Err handling for WebRTCSignal parsing) ...
                                                    state_net.lock().unwrap().log.push(format!(
                                                        "Error parsing WebRTC signal from {}: {}", from, e
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    // ... (existing Err handling for websocket message reading) ...
                                    state_net.lock().unwrap().log.push(format!(
                                        "Error reading websocket message: {}", e
                                    ));
                                }
                            }
                        }
                        // FIX: Handle other message types like Close, Ping, Pong if necessary
                        Ok(Message::Close(_)) => {
                            state_net.lock().unwrap().log.push("WebSocket connection closed by server.".to_string());
                            break; // Exit the network loop
                        }
                        Ok(Message::Ping(ping_data)) => {
                            // Respond with Pong
                            let _ = ws_sink.send(Message::Pong(ping_data)).await;
                        }
                        Ok(Message::Pong(_)) => {
                            // Pong received, maybe reset a keep-alive timer if you implement one
                        }
                        Ok(Message::Binary(_)) => {
                             state_net.lock().unwrap().log.push("Received unexpected binary message.".to_string());
                        }
                        // Add catch-all for Frame and potential future variants
                        Ok(Message::Frame(_)) => {
                            // Low-level frames are usually handled by the library, ignore them.
                            // state_net.lock().unwrap().log.push("Received WebSocket frame.".to_string());
                        }
                        Err(e) => {
                             state_net.lock().unwrap().log.push(format!("WebSocket read error: {}", e));
                             break; // Exit loop on error
                        }
                    }
                }
            }
        }
    }); // End of main network task

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut input = String::new();
    let mut input_mode = false;

    loop {
        // Lock state for drawing (Use StdMutex for TUI state)
        {
            // Scope for the lock guard
            let app_guard = state.lock().unwrap();
            // Use function from tui module
            draw_main_ui(&mut terminal, &app_guard, &input, input_mode)?;
        } // Drop the drawing lock before handling input

        // Handle input
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Lock TUI state for input handling
                    let mut app_guard = state.lock().unwrap();
                    // Call the handler function from the tui module
                    let continue_loop = handle_key_event(
                        key,
                        &mut app_guard,  // Pass mutable reference to state
                        &mut input,      // Pass mutable reference to input
                        &mut input_mode, // Pass mutable reference to input_mode
                        &cmd_tx,         // Pass command sender
                    )?;
                    // No need to explicitly drop app_guard here, it goes out of scope

                    if !continue_loop {
                        break; // Exit loop if handler signals quit
                    }
                }
                _ => {} // Ignore other events like Mouse, Resize etc.
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
