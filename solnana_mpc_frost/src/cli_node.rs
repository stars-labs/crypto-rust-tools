use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use frost_core::Identifier;
use frost_core::keys::dkg::{part1, part2, part3, round1}; // Remove unused round2
use frost_ed25519::Ed25519Sha512; // Keep this for type parameters
use frost_ed25519::rand_core::OsRng;
use futures_util::{SinkExt, StreamExt};
use lazy_static::lazy_static;
use ratatui::{Terminal, backend::CrosstermBackend};
use serde::Serialize; // Add Serialize
use sha2::{Digest, Sha256}; // Add Sha256 and Digest
use std::collections::BTreeMap; // Import BTreeMap
use std::convert::TryFrom; // Import TryFrom
use std::sync::{Arc, Mutex as StdMutex}; // Use StdMutex for TUI state
use std::time::Duration; // Add SystemTime and UNIX_EPOCH
use std::{
    collections::{HashMap, HashSet},
    io,
};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit; // Keep RTCIceCandidateInit here if used directly
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState; // Use TokioMutex for async WebRTC state
// Add these imports for WebRTC policies
use webrtc::peer_connection::policy::{
    bundle_policy::RTCBundlePolicy, ice_transport_policy::RTCIceTransportPolicy,
    rtcp_mux_policy::RTCRtcpMuxPolicy,
};
// Import shared types from the library crate
// Use ClientMsg as SharedClientMsg, and import the new InternalCommand
use solnana_mpc_frost::{ClientMsg as SharedClientMsg, InternalCommand, ServerMsg};
// Fix WebRTCMessage import
// Remove unused SessionInfo import
use crate::signal::WebRTCMessage;
// --- Declare Modules ---
mod negotiation;
mod peer;
mod signal;
mod state;
mod tui;

// --- Use items from modules ---
use negotiation::initiate_offers_for_session;
// Import send_webrtc_message
use peer::{apply_pending_candidates, create_and_setup_peer_connection, send_webrtc_message};
use signal::{SDPInfo, WebRTCSignal};
// Import DkgState
use state::{AppState, DkgState, ReconnectionTracker};
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

// --- Helper function to hash serializable data ---
fn hash_data<T: Serialize>(data: &T) -> String {
    match bincode::serde::encode_to_vec(data, bincode::config::standard()) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        }
        Err(_) => "SerializationError".to_string(),
    }
}

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
                                        });  // Add semicolon here
                                    }
                                    InternalCommand::TriggerDkgRound1 => { // Handle internal DKG trigger
                                        // --- Start DKG Round 1 ---
                                        let state_clone = state_main_net.clone();

                                        let self_peer_id_clone = self_peer_id_main_net.clone();
                                        // No need to clone internal_cmd_tx here unless the spawned task sends more internal commands

                                        tokio::spawn(async move {
                                            // --- Extract data under lock ---
                                            let extracted_data = { // Scope for guard
                                                let mut guard = state_clone.lock().unwrap();
                                                if guard.dkg_state != DkgState::Round1InProgress {
                                                    guard.log.push("DKG Round 1 already started or not ready.".to_string());
                                                    return; // Exit task
                                                }
                                                guard.log.push("Starting DKG Round 1 logic...".to_string());

                                                let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
                                                    (Some(s), Some(m)) => (s.clone(), m.clone()),
                                                    _ => {
                                                        guard.log.push("Error: Session or identifier map not ready for DKG Round 1.".to_string());
                                                        guard.dkg_state = DkgState::Failed("Session/Map not ready".to_string());
                                                        return; // Exit task
                                                    }
                                                };
                                                let my_identifier = match identifier_map.get(&self_peer_id_clone) {
                                                    Some(id) => *id,
                                                    None => {
                                                         guard.log.push(format!("Error: Own identifier not found in map for peer_id {}", self_peer_id_clone));
                                                         guard.dkg_state = DkgState::Failed("Identifier not found".to_string());
                                                         return; // Exit task
                                                    }
                                                };

                                                // --- Call frost_core::keys::dkg::part1 ---
                                                let mut rng = OsRng;
                                                let part1_result = part1::<Ed25519Sha512, _>(
                                                    my_identifier,
                                                    session.total,
                                                    session.threshold,
                                                    &mut rng,
                                                );

                                                match part1_result {
                                                    Ok((secret_package, public_package)) => {
                                                        guard.log.push(format!("DKG Part 1 successful for identifier {:?}", my_identifier));
                                                        // --- Log Hash of Generated Part 1 Package ---
                                                        let package_hash = hash_data(&public_package);
                                                        guard.log.push(format!("DEBUG: Generated Part1 Package Hash: {}", package_hash));
                                                        // --- End Log ---
                                                        guard.local_dkg_part1_data = Some((secret_package, public_package.clone()));
                                                        let dkg_msg = WebRTCMessage::DkgRound1Package { package: public_package };
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
                                                    if target_peer_id != &self_peer_id_clone { // Compare with reference
                                                        if let Err(e) = send_webrtc_message(target_peer_id, &dkg_msg, state_clone.clone()).await {
                                                            // Log error without holding lock for long
                                                            state_clone.lock().unwrap().log.push(format!(
                                                                "Error sending DKG Round 1 package to {}: {}", target_peer_id, e
                                                            ));
                                                        } else {
                                                            sent_count += 1;
                                                        }
                                                    }
                                                }
                                                state_clone.lock().unwrap().log.push(format!("Broadcasted DKG Round 1 package to {} peers.", sent_count));
                                            }
                                        });
                                    }
                                    InternalCommand::ProcessDkgRound1 { from_peer_id, package } => {
                                        // Handle internal DKG processing (synchronous within this task)
                                        let mut guard = state_main_net.lock().unwrap(); // Use state_main_net

                                        // Ensure session and identifier map are ready before processing
                                        let (session, identifier_map) = match (&guard.session, &guard.identifier_map) {
                                            (Some(s), Some(m)) => (s.clone(), m.clone()),
                                            _ => {
                                                guard.log.push(format!(
                                                    "DKG Round 1: Session/Map not ready. Queuing package from {}.",
                                                    from_peer_id
                                                ));
                                                guard.queued_dkg_round1.push((from_peer_id, package));
                                                continue; // Skip processing for now
                                            }
                                        };

                                        guard.log.push(format!("Processing DKG Round 1 package from {}", from_peer_id));
                                        // --- Log Hash of Received Part 1 Package ---
                                        let package_hash = hash_data(&package);
                                        guard.log.push(format!("DEBUG: Received Part1 Package Hash from {}: {}", from_peer_id, package_hash));
                                        // --- End Log ---

                                        let sender_identifier = match identifier_map.get(&from_peer_id) {
                                            Some(id) => *id,
                                            None => {
                                                 guard.log.push(format!("Error: Identifier not found for sender {}", from_peer_id));
                                                 continue; // Skip processing
                                            }
                                        };

                                        // Insert the received package
                                        guard.received_dkg_packages.insert(sender_identifier, package);

                                        // --- FIX: Ensure our own package is present ---
                                        let local_package_data_clone = guard.local_dkg_part1_data.clone();
                                        let own_peer_id_clone = guard.peer_id.clone();
                                        if let Some((_, local_public_package)) = local_package_data_clone {
                                            if let Some(my_identifier) = identifier_map.get(&own_peer_id_clone) {
                                                // Simpler check: if my identifier is not in the map, insert it.
                                                if !guard.received_dkg_packages.contains_key(my_identifier) {
                                                    guard.log.push(format!("Adding own DKG Round 1 package for {:?} to received map.", my_identifier));
                                                    guard.received_dkg_packages.insert(*my_identifier, local_public_package);
                                                }
                                            } else {
                                                 guard.log.push(format!("Error: Own identifier not found in map for peer_id {}", own_peer_id_clone));
                                                 guard.dkg_state = DkgState::Failed("Own identifier missing".to_string());
                                                 continue;
                                            }
                                        } else {
                                            // This case might happen if ProcessDkgRound1 is called before TriggerDkgRound1 finishes
                                            guard.log.push("Warning: Local DKG Part 1 data not yet available while processing received package.".to_string());
                                            // We don't fail here, just wait for the local data to be added later or by another ProcessDkgRound1 call
                                        }

                                        let current_package_count = guard.received_dkg_packages.len();
                                        let expected_packages = session.total as usize;
                                        guard.log.push(format!(
                                            "DKG Round 1: Received {}/{} packages.",
                                            current_package_count,
                                            expected_packages
                                        ));

                                        // --- Check for Round 1 completion and Trigger Round 2 ---
                                        if current_package_count == expected_packages
                                            && guard.dkg_state == DkgState::Round1InProgress // Only proceed if Round 1 is in progress
                                        {
                                            guard.log.push("All DKG Round 1 packages received. Proceeding to Round 2...".to_string());
                                            guard.dkg_state = DkgState::Round1Complete; // Mark Round 1 as complete

                                            // --- Execute DKG Part 2 ---
                                            let local_secret_package_opt: Option<round1::SecretPackage<Ed25519Sha512>> =
                                                guard.local_dkg_part1_data.as_ref().map(|(s, _)| s.clone());
                                            let received_packages_clone = guard.received_dkg_packages.clone(); // Clone for part2
                                            let my_identifier_opt = guard.identifier_map.as_ref().and_then(|map| map.get(&guard.peer_id).copied()); // Get own identifier

                                            let local_secret_package = match local_secret_package_opt {
                                                Some(secret) => secret,
                                                None => {
                                                    guard.log.push("Error: Local DKG Part 1 secret share missing for Round 2.".to_string());
                                                    guard.dkg_state = DkgState::Failed("Missing local secret share".to_string());
                                                    continue; // Skip
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
                                                let mut guard = state_main_net.lock().unwrap();
                                                guard.log.push("Error: Could not find own identifier to remove package for part2.".to_string());
                                                guard.dkg_state = DkgState::Failed("Own identifier missing for part2".to_string());
                                                continue; // Skip part2 call
                                            }
                                            let actual_part2_input_count = packages_for_part2.len(); // Actual count after removal

                                            // --- Log Hashes and Counts before Part 2 ---
                                            let mut guard = state_main_net.lock().unwrap();
                                            guard.log.push("DEBUG: Preparing for Part 2:".to_string());
                                            guard.log.push(format!("  My Identifier: {:?}", my_identifier_opt));
                                            guard.log.push(format!("  Expected input package count (N-1): {}", expected_part2_input_count));
                                            guard.log.push(format!("  Actual input package count: {}", actual_part2_input_count));
                                            guard.log.push("  Input Package Hashes (should be N-1):".to_string());
                                            for (id, pkg) in &packages_for_part2 {
                                                 guard.log.push(format!("    {:?}: {}", id, hash_data(pkg)));
                                            }
                                            // --- End Log ---
                                            drop(guard); // Drop guard before check and call

                                            // --- Strict Check: Ensure correct number of packages for part2 ---
                                            if actual_part2_input_count != expected_part2_input_count {
                                                 let mut guard = state_main_net.lock().unwrap();
                                                 guard.log.push(format!(
                                                     "Error: Incorrect number of packages for part2. Expected {}, Got {}. Aborting.",
                                                     expected_part2_input_count, actual_part2_input_count
                                                 ));
                                                 guard.dkg_state = DkgState::Failed("Part 2 package count mismatch".to_string());
                                                 continue; // Skip part2 call
                                            }

                                            let part2_result = part2::<Ed25519Sha512>(
                                                local_secret_package,
                                                &packages_for_part2, // Use the map *without* own package (N-1 entries)
                                            );

                                            // Re-acquire lock to update state and broadcast
                                            let mut guard = state_main_net.lock().unwrap();

                                            match part2_result {
                                                Ok((round2_secret, round2_packages_to_send)) => {
                                                    guard.log.push(format!(
                                                        "DKG Part 2 successful for identifier {:?}. Storing local Round 2 secret.",
                                                        round2_secret.identifier()
                                                    ));
                                                    guard.round2_secret_package = Some(round2_secret); // Store the secret needed for part3

                                                    // --- Broadcast Round 2 Packages ---
                                                    guard.log.push("Broadcasting DKG Round 2 packages...".to_string());
                                                    let identifier_map_clone = guard.identifier_map.clone().unwrap(); // Assumed safe due to earlier checks
                                                    let state_clone = state_main_net.clone();

                                                    let self_peer_id_clone = guard.peer_id.clone(); // Use guard.peer_id

                                                    let mut broadcast_count = 0;
                                                    for (target_identifier, round2_package) in round2_packages_to_send {
                                                        // Find the peer_id corresponding to the target_identifier
                                                        let target_peer_id = identifier_map_clone.iter().find_map(|(peer_id, &id)| {
                                                            if id == target_identifier { Some(peer_id.clone()) } else { None }
                                                        });

                                                        if let Some(target_peer_id) = target_peer_id {
                                                            // Don't send to self
                                                            if target_peer_id != self_peer_id_clone {
                                                                // --- Log Hash of Generated Part 2 Package ---
                                                                let package_hash = hash_data(&round2_package);
                                                                guard.log.push(format!("DEBUG: Generated Part2 Package Hash for {:?}: {}", target_identifier, package_hash));
                                                                // --- End Log ---
                                                                let dkg_msg = WebRTCMessage::DkgRound2Package { package: round2_package };
                                                                let state_task_clone = state_clone.clone();

                                                                let target_peer_id_task = target_peer_id.clone();

                                                                tokio::spawn(async move {
                                                                    if let Err(e) = send_webrtc_message(
                                                                        &target_peer_id_task,
                                                                        &dkg_msg,
                                                                        state_task_clone.clone(),
                                                                    ).await {
                                                                        state_task_clone.lock().unwrap().log.push(format!(
                                                                            "Error sending DKG Round 2 package to {}: {}",
                                                                            target_peer_id_task, e
                                                                        ));
                                                                    } else {
                                                                        // Log success inside task if needed, or count outside
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
                                                    guard.log.push(format!("Sent DKG Round 2 packages to {} peers.", broadcast_count));
            guard.dkg_state = DkgState::Round2InProgress; // Update state
                                                }
                                                Err(e) => {
                                                    guard.log.push(format!("DKG Part 2 failed: {:?}", e));
                                                    guard.dkg_state = DkgState::Failed(format!("DKG Part 2 Error: {:?}", e));
                                                }
                                            }
                                        } else if current_package_count == expected_packages && guard.dkg_state != DkgState::Round1InProgress {
                                            // FIX: Read dkg_state before the log.push call
                                            let current_dkg_state = guard.dkg_state.clone();
                                            // Log if all packages arrived but state wasn't Round1InProgress (e.g., already completed/failed)
                                            guard.log.push(format!(
                                                "DKG Round 1: Received all packages, but DKG state is {:?}. No action taken.",
                                                current_dkg_state // Use the cloned state here
                                            ));
                                        }
                                    }
                                    InternalCommand::ProcessDkgRound2 { from_peer_id, package } => {
                                        let mut guard = state_main_net.lock().unwrap();

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
                                                continue; // Skip processing
                                            }
                                        };

                                        // Only process if Round 2 is in progress
                                        if guard.dkg_state != DkgState::Round2InProgress {
                                            let current_dkg_state = guard.dkg_state.clone();
                                            guard.log.push(format!(
                                                "DKG Round 2: Received package from {}, but DKG state is {:?}. Ignoring.",
                                                from_peer_id, current_dkg_state
                                            ));
                                            continue;
                                        }

                                        guard.log.push(format!("Processing DKG Round 2 package from {}", from_peer_id));
                                        // --- Log Hash of Received Part 2 Package ---
                                        let package_hash = hash_data(&package);
                                        guard.log.push(format!("DEBUG: Received Part2 Package Hash from {}: {}", from_peer_id, package_hash));
                                        // --- End Log ---


                                        let sender_identifier = match identifier_map.get(&from_peer_id) {
                                            Some(id) => *id,
                                            None => {
                                                 guard.log.push(format!("Error: Identifier not found for sender {} in Round 2.", from_peer_id));
                                                 guard.dkg_state = DkgState::Failed(format!("Identifier missing for {}", from_peer_id));
                                                 continue; // Skip processing
                                            }
                                        };

                                        // Store the received package
                                        guard.received_dkg_round2_packages.insert(sender_identifier, package);

                                        // --- FIX: Only count packages from other participants (exclude self) ---
                                        let my_identifier = match identifier_map.get(&guard.peer_id) {
                                            Some(id) => *id,
                                            None => {
                                                guard.log.push("Error: Own identifier not found in identifier_map during DKG Round 2.".to_string());
                                                guard.dkg_state = DkgState::Failed("Own identifier missing".to_string());
                                                continue;
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
                                            filtered_count,
                                            expected_packages
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

                                            // --- Previous DEBUG logs (keep them) ---
                                            // ... existing DEBUG logs ...

                                            // --- Add More Detailed Logging Right Before part3 ---
                                            guard.log.push("--- Pre-part3 Input Verification ---".to_string());
                                            guard.log.push(format!(
                                                "  My Identifier: {:?}",
                                                my_identifier
                                            ));
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
                                            guard.log.push("--- End Pre-part3 Input Verification ---".to_string());
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

                                            guard.log.push("--- Pre-part3 Type/Count Verification (Refs) ---".to_string());
                                            if let Some(secret_ref) = round2_secret_ref_opt {
                                                 guard.log.push(format!(
                                                     "  Type R2SecretRef: {}",
                                                     std::any::type_name::<&frost_core::keys::dkg::round2::SecretPackage<Ed25519Sha512>>()
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
                                                std::any::type_name::<&BTreeMap<frost_core::Identifier<Ed25519Sha512>, frost_core::keys::dkg::round1::Package<Ed25519Sha512>>>()
                                            ));
                                            guard.log.push(format!(
                                                "  Count R1MapRef: {} (Expected: {})",
                                                round1_packages_ref.len(), expected_part3_round1_count // Log expected N-1
                                            ));
                                             guard.log.push(format!(
                                                "  Type R2MapRef: {}",
                                                std::any::type_name::<&BTreeMap<frost_core::Identifier<Ed25519Sha512>, frost_core::keys::dkg::round2::Package<Ed25519Sha512>>>()
                                            ));
                                            guard.log.push(format!(
                                                "  Count R2MapRef: {} (Expected: {})",
                                                round2_packages_ref.len(), expected_part3_round2_count // Log expected N-1
                                            ));
                                            guard.log.push("--- End Pre-part3 Type/Count Verification (Refs) ---".to_string());
                                            // --- End Type/Count Logging ---

                                            // --- Strict Check: Ensure correct number of packages for part3 ---
                                            if round1_packages_ref.len() != expected_part3_round1_count {
                                                 guard.log.push(format!(
                                                     "Error: Incorrect number of Round 1 packages for part3. Expected {}, Got {}. Aborting.",
                                                     expected_part3_round1_count, round1_packages_ref.len()
                                                 ));
                                                 guard.dkg_state = DkgState::Failed("Part 3 Round 1 package count mismatch".to_string()); // Corrected string
                                                 continue; // Skip part3 call
                                            }
                                            if round2_packages_ref.len() != expected_part3_round2_count {
                                                 guard.log.push(format!(
                                                     "Error: Incorrect number of Round 2 packages for part3. Expected {}, Got {}. Aborting.",
                                                     expected_part3_round2_count, round2_packages_ref.len()
                                                 ));
                                                 guard.dkg_state = DkgState::Failed("Part 3 Round 2 package count mismatch".to_string()); // Corrected string
                                                 continue; // Skip part3 call
                                            }
                                            // --- End Strict Check ---


                                            let round2_secret_package = match round2_secret_package_opt {
                                                Some(secret) => secret,
                                                None => {
                                                    guard.log.push("Error: Local DKG Round 2 secret share missing for Part 3.".to_string());
                                                    guard.dkg_state = DkgState::Failed("Missing Round 2 secret share".to_string()); // Corrected string
                                                    continue; // Skip
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
                                            let mut guard = state_main_net.lock().unwrap();

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

                                                    // --- Generate and store Solana public key ---
                                                    let verifying_key = group_public_key.verifying_key();
                                                    match verifying_key.serialize() {
                                                        Ok(pk_bytes) => {
                                                            let solana_pubkey_str = bs58::encode(pk_bytes).into_string();
                                                            guard.log.push(format!("Generated Solana Public Key: {}", solana_pubkey_str));
                                                            guard.solana_public_key = Some(solana_pubkey_str);
                                                        }
                                                        Err(e) => {
                                                            guard.log.push(format!("Error serializing group verifying key: {:?}", e));
                                                            // Handle error appropriately, maybe set DKG state back to failed?
                                                        }
                                                    }
                                                    // --- End Solana public key generation ---


                                                    // Optionally clear intermediate DKG data now
                                                    guard.local_dkg_part1_data = None;
                                                    guard.received_dkg_packages.clear();
                                                    guard.round2_secret_package = None;
                                                    guard.received_dkg_round2_packages.clear();
                                                    guard.queued_dkg_round1.clear();

                                                    guard.log.push("DKG process completed successfully.".to_string());
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
                                                                    state_guard.invites.push(crate::signal::SessionInfo { session_id: session_id.clone(), total: total as u16, threshold: threshold as u16, participants: participants.clone() });
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
                                                                    let new_session = crate::signal::SessionInfo {
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
                                                                                                        match pc_clone.create_data_channel(peer::DATA_CHANNEL_LABEL, None).await {
                                                                                                            Ok(dc) => {
                                                                                                                // Don't wrap in Arc::new() again
                                                                                                                let peer_id_dc = from_clone.clone();
                                                                                                                let state_log_dc = state_log_clone.clone();
                                                                                                                // Pass INTERNAL cmd_tx
                                                                                                                let cmd_tx_dc = internal_cmd_tx_clone.clone(); // Pass internal cmd_tx

                                                                                                                // Setup callbacks for the created channel
                                                                                                                // Pass dc directly
                                                                                                                peer::setup_data_channel_callbacks(dc, peer_id_dc, state_log_dc, cmd_tx_dc);

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
