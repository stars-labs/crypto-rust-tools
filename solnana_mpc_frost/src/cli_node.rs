use crate::utils::signal::WebRTCMessage;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use frost_core::Identifier;

use frost_ed25519::Ed25519Sha512;

use futures_util::{SinkExt, StreamExt};

use ratatui::{Terminal, backend::CrosstermBackend};

use solnana_mpc_frost::{ClientMsg as SharedClientMsg, InternalCommand, ServerMsg};
use std::collections::BTreeMap;
use std::convert::TryFrom;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    io,
};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

mod utils;
// --- Use items from modules ---
use utils::negotiation::initiate_offers_for_session;
use utils::peer::{
    apply_pending_candidates, create_and_setup_peer_connection, send_webrtc_message,
};
use utils::signal::{SDPInfo, WebRTCSignal};
use utils::state::{AppState, DkgState, ReconnectionTracker};

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};
// Import our new utility modules
use utils::ed25519_dkg;
// Import from our new webrtc module
use utils::webrtc::{WEBRTC_API, WEBRTC_CONFIG};

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
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Register (Send directly, no channel needed for initial message)
    let register_msg = SharedClientMsg::Register {
        peer_id: peer_id.clone(),
    };
    ws_sink
        .send(Message::Text(serde_json::to_string(&register_msg)?))
        .await?;

    // Channel for INTERNAL commands within the CLI app (uses InternalCommand from lib.rs)
    let (internal_cmd_tx, mut internal_cmd_rx) = mpsc::unbounded_channel::<InternalCommand>();

    // Shared state for TUI and network
    // Specify the Ciphersuite for AppState
    let state = Arc::new(StdMutex::new(AppState::<Ed25519Sha512> {
        // TUI state uses StdMutex
        peer_id: peer_id.clone(),
        peers: vec![],
        log: vec!["Registered with server".to_string()],
        log_scroll: 0, // Initialize scroll state
        session: None,
        invites: vec![],
        peer_connections: Arc::new(tokio::sync::Mutex::new(HashMap::new())), // Use TokioMutex here
        peer_statuses: HashMap::new(), // Initialize peer statuses
        reconnection_tracker: ReconnectionTracker::new(),
        keep_alive_peers: HashSet::new(),
        // Remove cmd_tx field as it's unused in AppState
        // cmd_tx: Some(cmd_tx.clone()), // 存储一个命令发送器副本
        // --- Initialize Perfect Negotiation Flags ---
        making_offer: HashMap::new(),
        ignore_offer: HashMap::new(),
        // Initialize the pending_ice_candidates field
        pending_ice_candidates: HashMap::new(),
        // --- Initialize DKG State ---
        dkg_state: DkgState::Idle,
        identifier_map: None,
        // identifier_to_index_map: None, // Removed initialization
        local_dkg_part1_data: None,
        received_dkg_packages: BTreeMap::new(),
        key_package: None,
        group_public_key: None,
        // Add missing fields:
        data_channels: HashMap::new(),
        solana_public_key: None,
        queued_dkg_round1: vec![],
        round2_secret_package: None,
        received_dkg_round2_packages: BTreeMap::new(), // Initialize new field
    }));

    // --- Spawn Periodic Connection Status Checker Task ---
    let state_for_checker = state.clone();
    let peer_connections_for_checker = state.lock().unwrap().peer_connections.clone();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            // Check and update all connection statuses
            let peer_conns = {
                let lock = peer_connections_for_checker.lock().await; // Use TokioMutex and .await
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

    // --- Spawn Main Network Task (WebSocket Communication + Internal Commands) ---
    // Clone needed variables for the main network task
    let state_main_net = state.clone();
    let self_peer_id_main_net = peer_id.clone();
    let internal_cmd_tx_main_net = internal_cmd_tx.clone();
    let peer_connections_arc_main_net = state.lock().unwrap().peer_connections.clone(); // This is Arc<TokioMutex<...>>

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        // Request peer list on start
        let list_peers_msg = SharedClientMsg::ListPeers;
        let _ = ws_sink
            .send(Message::Text(
                serde_json::to_string(&list_peers_msg).unwrap(),
            ))
            .await;

        loop {
            tokio::select! {
            _ = interval.tick() => {
                // Periodic peer list request
                let list_peers_msg = SharedClientMsg::ListPeers;
                let _ = ws_sink.send(Message::Text(serde_json::to_string(&list_peers_msg).unwrap())).await;
            },
            Some(cmd) = internal_cmd_rx.recv() => {
                match cmd {
                    InternalCommand::SendToServer(shared_msg) => {
                        // Send shared messages received via internal channel to WebSocket
                        let _ = ws_sink.send(Message::Text(serde_json::to_string(&shared_msg).unwrap())).await;
                    }
                    InternalCommand::SendDirect { to, message } => {
                        // Handle sending direct WebRTC message
                        let state_clone = state_main_net.clone();

                        tokio::spawn(async move {
                            let webrtc_msg = WebRTCMessage::SimpleMessage { text: message };
                            if let Err(e) = send_webrtc_message(&to, &webrtc_msg,state_clone.clone()).await {
                                 state_clone.lock().unwrap().log.push(format!("Error sending direct message to {}: {}", to, e));
                            } else {
                                 state_clone.lock().unwrap().log.push(format!("Sent direct message to {}", to));
                            }
                        });
                    }
                    InternalCommand::TriggerDkgRound1 => {
                        // Use our new ed25519_dkg module
                        let state_clone = state_main_net.clone();
                        let self_peer_id_clone = self_peer_id_main_net.clone();

                        tokio::spawn(async move {
                            ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
                        });
                    }
                    InternalCommand::ProcessDkgRound1 { from_peer_id, package } => {
                        // Use our new ed25519_dkg module
                        ed25519_dkg::process_dkg_round1(
                            state_main_net.clone(),
                            from_peer_id,
                            package,
                        );
                    }
                    InternalCommand::ProcessDkgRound2 { from_peer_id, package } => {
                        // Use our new ed25519_dkg module
                        let state_clone = state_main_net.clone();
                        let from_peer_id_clone = from_peer_id.clone();
                        let package_clone = package.clone();

                        tokio::spawn(async move {
                            ed25519_dkg::process_dkg_round2(state_clone, from_peer_id_clone, package_clone).await;
                        });
                    }
                }
            },
            maybe_msg = ws_stream.next() => {
                match maybe_msg {
                    Some(Ok(msg)) => {
                        // Renamed 'msg' variable inside the block to avoid conflict
                        let current_msg = msg;
                        // Match directly on Message variants, not Ok(Message::...)
                        match current_msg {
                            Message::Text(txt) => { // Remove Ok()
                                match serde_json::from_str::<ServerMsg>(&txt) {
                                    Ok(server_msg) => {
                                        // Handle simple state updates first, keep lock brief
                                        match server_msg { // Restore missing arms here
                                            ServerMsg::Peers { ref peers } => {
                                                // --- Lock StdMutex ---
                                                let mut state_guard = state_main_net.lock().unwrap();
                                                state_guard.peers = peers.clone();
                                                // FIX: Remove offer initiation logic from here
                                                drop(state_guard); // Drop lock
                                            }
                                            ServerMsg::SessionInvite { ref session_id, ref from, total, threshold, ref participants } => {
                                                let mut state_guard = state_main_net.lock().unwrap();
                                                // Use crate::signal::SessionInfo
                                                // Remove unnecessary parentheses
                                                if !state_guard.invites.iter().any(|inv| inv.session_id == *session_id) {
                                                    state_guard.invites.push(crate::utils::signal::SessionInfo { session_id: session_id.clone(), total: total as u16, threshold: threshold as u16, participants: participants.clone() });
                                                    state_guard.log.push(format!("Session invite from {} for session {}", from, session_id));
                                                } else {
                                                     state_guard.log.push(format!("Duplicate invite from {} for session {}", from, session_id));
                                                }
                                                drop(state_guard);
                                            }
                                            ServerMsg::Error { ref error } => {
                                                // ... (Error handling remains the same) ...
                                                let mut state_guard = state_main_net.lock().unwrap();
                                                state_guard.log.push(format!("Error: {}", error));
                                                // Drop guard immediately
                                                drop(state_guard);
                                            }
                                            // Handle complex cases involving async operations separately
                                            ServerMsg::SessionInfo { session_id, total, threshold, participants } => {
                                                let queued_packages_to_process = { // Scope for guard
                                                    let mut guard = state_main_net.lock().unwrap();
                                                    guard.log.push(format!(
                                                        "Session info received/updated: {}. Waiting for all participants.",
                                                        session_id
                                                    ));
                                                    let new_session = crate::utils::signal::SessionInfo {
                                                        session_id: session_id.clone(),
                                                        total: total as u16,
                                                        threshold: threshold as u16,
                                                        participants: participants.clone(),
                                                    };
                                                    guard.session = Some(new_session);

                                                    // Create or update identifier map
                                                    guard.log.push("Session participants updated. Creating identifier map...".to_string());
                                                    let mut id_map = BTreeMap::new();
                                                    // let mut id_to_index_map = BTreeMap::new(); // Removed map for reverse lookup
                                                    for (i, p_id) in participants.iter().enumerate() {
                                                        let index = (i + 1) as u16; // 1-based index
                                                        match Identifier::<Ed25519Sha512>::try_from(index) {
                                                            Ok(identifier) => {
                                                                id_map.insert(p_id.clone(), identifier);
                                                                // id_to_index_map.insert(identifier, index); // Removed reverse mapping
                                                                guard.log.push(format!("Mapped {} -> {:?}", p_id, identifier)); // Log Debug format
                                                            }
                                                            Err(e) => {
                                                                guard.log.push(format!("Error creating identifier for {}: {:?}", p_id, e));
                                                                // Handle error appropriately, maybe fail DKG
                                                            }
                                                        }
                                                    }
                                                    guard.identifier_map = Some(id_map);
                                                    // guard.identifier_to_index_map = Some(id_to_index_map); // Removed storing the reverse map

                                                    // Take queued packages for processing outside the lock
                                                    let queued = std::mem::take(&mut guard.queued_dkg_round1);
                                                    if !queued.is_empty() {
                                                        guard.log.push(format!(
                                                            "Processing {} queued DKG Round 1 packages after session/identifier_map ready.",
                                                            queued.len()
                                                        ));
                                                    }
                                                    queued // Return the queued packages
                                                }; // guard dropped here

                                                // Process queued packages by sending internal commands
                                                for (from_peer_id, package) in queued_packages_to_process {
                                                    let _ = internal_cmd_tx_main_net.send(InternalCommand::ProcessDkgRound1 {
                                                        from_peer_id,
                                                        package,
                                                    });
                                                }

                                                // --- Spawn Task to Create PeerConnection Objects ---
                                                let pc_arc_clone = peer_connections_arc_main_net.clone();
                                                let internal_cmd_tx_clone = internal_cmd_tx_main_net.clone();
                                                let state_log_clone = state_main_net.clone();
                                                let self_peer_id_clone = self_peer_id_main_net.clone();
                                                let participants_clone = participants.clone(); // Clone participants for the task
                                                let session_id_clone = session_id.clone(); // Clone session_id for logging

                                                tokio::spawn(async move {
                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                        "SessionInfo Task: Creating peer connections for session {}...",
                                                        session_id_clone // Use cloned session_id for logging
                                                    ));
                                                    for p_id in participants_clone.iter() { // Iterate over cloned participants
                                                        if *p_id != self_peer_id_clone {
                                                            // Pass WEBRTC_API and WEBRTC_CONFIG directly
                                                            match create_and_setup_peer_connection(
                                                                p_id.clone(),
                                                                self_peer_id_clone.clone(),
                                                                pc_arc_clone.clone(), // Use Arc<TokioMutex<...>>
                                                                internal_cmd_tx_clone.clone(),
                                                                state_log_clone.clone(),
                                                                &WEBRTC_API, // Pass API reference
                                                                &WEBRTC_CONFIG, // Pass Config reference
                                                            )
                                                            .await
                                                            {
                                                                Ok(_) => { /* Connection object created or existed */ }
                                                                Err(e) => {
                                                                    state_log_clone.lock().unwrap().log.push(format!(
                                                                        "Error setting up peer connection for {}: {}",
                                                                        p_id, e
                                                                    ));
                                                                }
                                                            }
                                                        }
                                                    }
                                                    // Initiate offers *after* creating/setting up connections
                                                    // FIX: Pass arguments in the correct order and remove borrow from self_peer_id
                                                    initiate_offers_for_session(
                                                        participants_clone, // Pass the Vec<String> of participants
                                                        self_peer_id_clone, // Pass String, not &String
                                                        pc_arc_clone.clone(), // Use Arc<TokioMutex<...>>
                                                        internal_cmd_tx_clone.clone(), // Pass command sender
                                                        state_log_clone.clone(), // Pass state log
                                                    )
                                                    .await;
                                                });

                                                // --- Check and Trigger DKG Round 1 ---
                                                { // Scope for guard
                                                    let mut guard = state_main_net.lock().unwrap();
                                                    if let Some(session) = &guard.session {
                                                        // Check if DKG is idle *and* session matches the received info
                                                        if guard.dkg_state == DkgState::Idle && session.session_id == session_id {
                                                            let all_connected = session
                                                                .participants
                                                                .iter()
                                                                .filter(|p| **p != self_peer_id_main_net) // Exclude self
                                                                .all(|p_id| {
                                                                    guard.peer_statuses.get(p_id)
                                                                        == Some(&RTCPeerConnectionState::Connected)
                                                                });

                                                            if all_connected {
                                                                guard.log.push(
                                                                    "All peers connected and session ready! Triggering DKG Round 1...".to_string(),
                                                                );
                                                                guard.dkg_state = DkgState::Round1InProgress; // Set state BEFORE sending command
                                                                // Send internal command
                                                                let _ = internal_cmd_tx_main_net.send(InternalCommand::TriggerDkgRound1);
                                                            } else {
                                                                // Collect detailed log messages without holding the mutable borrow during iteration
                                                                let mut waiting_logs = Vec::new();
                                                                for p_id in session.participants.iter().filter(|p| **p != self_peer_id_main_net) {
                                                                    let status = guard.peer_statuses.get(p_id);
                                                                    if status != Some(&RTCPeerConnectionState::Connected) {
                                                                        waiting_logs.push(format!(
                                                                            "DKG Wait: Peer {} status is {:?} (Expected Connected)",
                                                                            p_id, status // Log the actual status found in the map
                                                                        ));
                                                                    }
                                                                }
                                                                // Log the initial waiting message *after* the loop using `session`
                                                                guard.log.push(
                                                                    "Session ready, waiting for all peers to connect before starting DKG...".to_string(),
                                                                );
                                                                // Now push the collected detailed logs
                                                                for log_msg in waiting_logs {
                                                                    guard.log.push(log_msg);
                                                                }
                                                            }
                                                        }
                                                    }
                                                } // guard dropped here
                                            }
                                            ServerMsg::Relay { ref from, ref data } => {
                                                state_main_net.lock().unwrap().log.push(format!("Relay from {}: {:?}", from, data)); // Log raw data
                                                match serde_json::from_value::<WebRTCSignal>(data.clone()) {
                                                    Ok(signal) => {
                                                        state_main_net.lock().unwrap().log.push(format!("Parsed WebRTC signal from {}: {:?}", from, signal)); // Log parsed signal
                                                        // Clone necessary Arcs and data FOR the spawned task
                                                        let pc_arc_net_clone = peer_connections_arc_main_net.clone();
                                                        let from_clone = from.clone();
                                                        // Pass INTERNAL cmd_tx
                                                        let internal_cmd_tx_clone = internal_cmd_tx_main_net.clone();
                                                        let state_log_clone = state_main_net.clone();
                                                        let signal_clone = signal.clone(); // Clone the parsed signal
                                                        let self_peer_id_clone = self_peer_id_main_net.clone();

                                                        tokio::spawn(async move { // Spawn signal handling
                                                            // --- Get or Create Peer Connection ---
                                                            let pc_result = { // Scope for lock
                                                                let peer_conns = pc_arc_net_clone.lock().await; // Use TokioMutex and .await
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
                                                                    // Call create_and_setup_peer_connection. It handles "already exists" internally. // Corrected comment
                                                                    // Pass INTERNAL cmd_tx AND WebRTC statics
                                                                    create_and_setup_peer_connection(
                                                                        from_clone.clone(),
                                                                        self_peer_id_clone.clone(), // Pass self_peer_id
                                                                        pc_arc_net_clone.clone(),   // Pass Arc<TokioMutex<...>>
                                                                        internal_cmd_tx_clone.clone(), // Pass internal cmd_tx
                                                                        state_log_clone.clone(),
                                                                        &WEBRTC_API, // Pass static ref
                                                                        &WEBRTC_CONFIG, // Pass static ref
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
                                                                                        apply_pending_candidates(&from_clone, pc_clone.clone(), state_log_clone.clone()).await;

                                                                                        // Create data channel before answering
                                                                                        match pc_clone.create_data_channel(utils::peer::DATA_CHANNEL_LABEL, None).await {
                                                                                            Ok(dc) => {
                                                                                                // Don't wrap in Arc::new() again
                                                                                                let peer_id_dc = from_clone.clone();
                                                                                                let state_log_dc = state_log_clone.clone();
                                                                                                // Pass INTERNAL cmd_tx
                                                                                                let cmd_tx_dc = internal_cmd_tx_clone.clone(); // Pass internal cmd_tx

                                                                                                // Setup callbacks for the created channel
                                                                                                // Pass dc directly
                                                                                                utils::peer::setup_data_channel_callbacks(dc, peer_id_dc, state_log_dc, cmd_tx_dc);

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
                                                                                                        // Send Relay message via INTERNAL channel
                                                                                                        let relay_msg = InternalCommand::SendToServer(SharedClientMsg::Relay {
                                                                                                            to: from_clone.clone(),
                                                                                                            data: json_val,
                                                                                                        });
                                                                                                        // Use the correct sender clone
                                                                                                        let _ = internal_cmd_tx_clone.send(relay_msg);
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
                                                                            // ... existing Candidate handling ...
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
                                                                    });  // Add semicolon here
                                                                }
                                                                Err(e) => {
                                                                    // ... existing Err handling for WebRTCSignal parsing ...
                                                                    state_main_net.lock().unwrap().log.push(format!(
                                                                        "Error parsing WebRTC signal from {}: {}", from, e
                                                                    ));
                                                                }
                                                                }
                                                            }
                                                        }
                                                    }
                                                Err(e) => {
                                                    // ... existing Err handling for websocket message reading ...
                                                    state_main_net.lock().unwrap().log.push(format!(
                                                        "Error reading websocket message: {}", e
                                                    ));
                                                }
                                            }
                                        }
                                        // FIX: Handle other message types like Close, Ping, Pong if necessary
                                        Message::Close(_) => { // Remove Ok()
                                            state_main_net.lock().unwrap().log.push("WebSocket connection closed by server.".to_string());
                                            break; // Exit the network loop
                                        }
                                        Message::Ping(ping_data) => { // Remove Ok()
                                            // Respond with Pong
                                            let _ = ws_sink.send(Message::Pong(ping_data)).await;
                                        }
                                        Message::Pong(_) => {} // Remove Ok()
                                        Message::Binary(_) => { // Remove Ok()
                                             state_main_net.lock().unwrap().log.push("Received unexpected binary message.".to_string());
                                        }
                                        // Add catch-all for Frame and potential future variants
                                        Message::Frame(_) => {} // Remove Ok()
                                    }
                                }
                                Some(Err(e)) => {
                                    // Handle error from ws_stream.next()
                                    state_main_net.lock().unwrap().log.push(format!("WebSocket read error: {}", e));
                                    break; // Exit loop on error
                                }
                                None => {
                                    // Stream ended
                                    state_main_net.lock().unwrap().log.push("WebSocket stream ended".to_string());
                                    break; // Exit loop
                    }
                }
            }
                    }
        }
    });

    // TUI setup
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?; // Keep mut for terminal.backend_mut() later

    let mut input = String::new(); // Need mut for editing input
    let mut input_mode = false; // Need mut for toggling input mode

    // Enable the TUI loop - uncomment this section
    loop {
        {
            // Scope for drawing lock
            let app_guard = state.lock().unwrap();
            draw_main_ui(&mut terminal, &app_guard, &input, input_mode)?;
        }

        // Handle key events with a timeout
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    let mut app_guard = state.lock().unwrap();
                    let continue_loop = handle_key_event(
                        key,
                        &mut app_guard,
                        &mut input,
                        &mut input_mode,
                        &internal_cmd_tx,
                    )?;
                    if !continue_loop {
                        break;
                    }
                }
                _ => {} // Ignore other events like Mouse, Resize etc.
            }
        }

        // No sleep needed - event::poll has a timeout already
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
