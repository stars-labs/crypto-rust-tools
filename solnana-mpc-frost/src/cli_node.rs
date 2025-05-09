use crate::protocal::signal::WebRTCMessage;
use crate::protocal::signal::WebRTCSignal;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use frost_ed25519::Ed25519Sha512;

use futures_util::{SinkExt, StreamExt};

use ratatui::{Terminal, backend::CrosstermBackend};

use std::collections::{BTreeMap, HashSet}; // Add HashSet import

// Import from lib.rs
use solnana_mpc_frost::{DkgState, InternalCommand, MeshStatus};

use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, io};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc_signal_server::{ClientMsg as SharedClientMsg, ServerMsg};
// Add display-related imports for better status handling

mod utils;
// --- Use items from modules ---
use protocal::signal::{SDPInfo, SessionInfo, SessionProposal, SessionResponse}; // Remove WebRTCMessage to avoid duplication
use utils::negotiation::initiate_offers_for_session;
use utils::peer::{
    apply_pending_candidates, create_and_setup_peer_connection, send_webrtc_message,
};
use utils::state::{AppState, ReconnectionTracker}; // Remove DkgState import

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};
// Import our new utility modules
use utils::ed25519_dkg;
// Import from our new webrtc module
use network::webrtc::{WEBRTC_API, WEBRTC_CONFIG};

mod protocal;

mod network;

// Remove the duplicate DkgState enum definition

// 1. 新增: DKG 启动"稳定窗口"参数
const DKG_CONNECTED_STABLE_WINDOW: usize = 2; // 连续2次Connected才触发

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Get peer_id from user
    println!("Enter your peer_id:");
    let mut peer_id = String::new();
    io::stdin().read_line(&mut peer_id)?;
    let peer_id = peer_id.trim().to_string();

    // Connect to signaling server
    let ws_url = "wss://auto-life.tech";
    let (ws_stream, _) = connect_async(ws_url).await?;
    // Remove mut from ws_stream
    let (mut ws_sink, mut ws_stream) = ws_stream.split();

    // Register (Send directly, no channel needed for initial message)
    let register_msg = SharedClientMsg::Register {
        peer_id: peer_id.clone(),
    };
    ws_sink
        .send(Message::Text(serde_json::to_string(&register_msg)?.into()))
        .await?;

    // Channel for INTERNAL commands within the CLI app (uses InternalCommand from lib.rs)
    let (internal_cmd_tx, mut internal_cmd_rx) = mpsc::unbounded_channel::<InternalCommand>();

    // Shared state for TUI and network
    // Specify the Ciphersuite for AppState
    let state = Arc::new(Mutex::new(AppState::<Ed25519Sha512> {
        // TUI state uses Mutex
        peer_id: peer_id.clone(),
        peers: vec![],
        log: vec!["Registered with server".to_string()],
        log_scroll: 0, // Initialize scroll state
        session: None,
        invites: vec![],
        peer_connections: Arc::new(Mutex::new(HashMap::new())), // Use TokioMutex here
        peer_statuses: HashMap::new(),                          // Initialize peer statuses
        reconnection_tracker: ReconnectionTracker::new(),
        making_offer: HashMap::new(),
        ignore_offer: HashMap::new(),
        pending_ice_candidates: HashMap::new(),
        dkg_state: DkgState::Idle,
        identifier_map: None,
        local_dkg_part1_data: None,
        received_dkg_packages: BTreeMap::new(),
        key_package: None,
        group_public_key: None,
        data_channels: HashMap::new(),
        solana_public_key: None,
        queued_dkg_round1: vec![],
        round2_secret_package: None,
        received_dkg_round2_packages: BTreeMap::new(), // Initialize new field
        // 新增: 记录每个peer最近N次状态
        peer_connected_history: HashMap::new(), // peer_id -> Vec<RTCPeerConnectionState>
        // Add mesh ready tracking
        mesh_ready_peers: HashSet::new(),
        mesh_status: MeshStatus::Incomplete,
    }));

    // --- Spawn Peer Connection State Change Handler Task ---
    let state_for_checker = state.clone();
    let peer_connections_for_checker = state.lock().await.peer_connections.clone();
    let internal_cmd_tx_for_checker = internal_cmd_tx.clone();

    tokio::spawn(async move {
        loop {
            // Wait for any peer connection state change (simulate by polling, but you can use a notification/event if available)
            // For now, poll at a short interval (e.g., 100ms) for demonstration, but ideally, use a callback/event from your WebRTC lib.
            tokio::time::sleep(Duration::from_millis(100)).await;

            let peer_conns = {
                let lock = peer_connections_for_checker.lock().await;
                lock.clone()
            };
            for (peer_id, pc) in peer_conns.iter() {
                let current_state = pc.connection_state();
                let current_ice_state = pc.ice_connection_state();

                // Replace try_lock() with lock() to avoid Result handling issues
                // This might block but ensures we get a valid lock
                let mut guard = state_for_checker.lock().await;

                // --- Debug print for connectionState and iceConnectionState ---
                guard.log.push(format!(
                    "Peer {}: connectionState={:?}, iceConnectionState={:?}",
                    peer_id, current_state, current_ice_state
                ));
                // 记录历史状态
                let history = guard
                    .peer_connected_history
                    .entry(peer_id.clone())
                    .or_insert_with(Vec::new);
                history.push(current_state);
                if history.len() > DKG_CONNECTED_STABLE_WINDOW {
                    history.remove(0);
                }
                // --- Recovery: Detect stuck PeerConnections in New state ---
                // If the last N states are all New, forcibly close and remove the PeerConnection
                if history.len() == DKG_CONNECTED_STABLE_WINDOW
                    && history.iter().all(|s| *s == RTCPeerConnectionState::New)
                {
                    // --- Clone session participants before any mutable borrow ---
                    let participants_clone = guard.session.as_ref().map(|s| s.participants.clone());
                    let peer_id_clone = peer_id.clone();
                    let self_peer_id_clone = guard.peer_id.clone();
                    let pc_arc_clone = guard.peer_connections.clone();
                    let internal_cmd_tx_clone = internal_cmd_tx_for_checker.clone();
                    let state_clone = state_for_checker.clone();
                    let webrtc_api = &WEBRTC_API;
                    let webrtc_config = &WEBRTC_CONFIG;

                    let mut recovery_logs = Vec::new();
                    let mut removed = false;
                    {
                        let mut pc_map = guard.peer_connections.lock().await;
                        if let Some(pc_arc) = pc_map.remove(peer_id) {
                            let _ = pc_arc.close().await;
                            recovery_logs.push(format!(
                                "PeerConnection for {} closed and removed for recovery.",
                                peer_id
                            ));
                            removed = true;
                        }
                    }
                    if removed {
                        guard.peer_statuses.remove(peer_id);
                        guard.peer_connected_history.remove(peer_id);
                    }
                    recovery_logs.insert(0, format!(
                        "PeerConnection for {} stuck in New state for {} checks, forcing recreation.",
                        peer_id, DKG_CONNECTED_STABLE_WINDOW
                    ));
                    for log in recovery_logs {
                        guard.log.push(log);
                    }
                    // --- NEW: Immediately recreate PeerConnection and initiate negotiation ---
                    if let Some(participants_clone) = participants_clone {
                        // Only run recovery for peers that are not self
                        if peer_id_clone != self_peer_id_clone {
                            let is_offerer = self_peer_id_clone < peer_id_clone;
                            tokio::spawn(async move {
                                match create_and_setup_peer_connection(
                                    peer_id_clone.clone(),
                                    self_peer_id_clone.clone(),
                                    pc_arc_clone.clone(),
                                    internal_cmd_tx_clone.clone(),
                                    state_clone.clone(),
                                    webrtc_api,
                                    webrtc_config,
                                )
                                .await
                                {
                                    Ok(_) => {
                                        state_clone.lock().await.log.push(format!(
                                            "PeerConnection for {} recreated after recovery.",
                                            peer_id_clone
                                        ));
                                        if is_offerer {
                                            state_clone.lock().await.log.push(format!(
                                                "This node is offerer for {} after recovery, initiating offer...", peer_id_clone
                                            ));
                                            initiate_offers_for_session(
                                                participants_clone,
                                                self_peer_id_clone,
                                                pc_arc_clone,
                                                internal_cmd_tx_clone,
                                                state_clone,
                                            )
                                            .await;
                                        } else {
                                            state_clone.lock().await.log.push(format!(
                                                "This node is not offerer for {} after recovery, waiting for offer.", peer_id_clone
                                            ));
                                        }
                                    }
                                    Err(e) => {
                                        state_clone.lock().await.log.push(format!(
                                            "Error recreating PeerConnection for {} after recovery: {}", peer_id_clone, e
                                        ));
                                    }
                                }
                            });
                        }
                    }
                    continue;
                }
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

                // --- DKG trigger check: after updating peer_statuses ---
                if let Some(session) = &guard.session {
                    if guard.dkg_state == DkgState::Idle {
                        let all_connected = session
                            .participants
                            .iter()
                            .filter(|p| **p != guard.peer_id)
                            .all(|p_id| {
                                let history = guard.peer_connected_history.get(p_id);
                                match history {
                                    Some(hist) if hist.len() == DKG_CONNECTED_STABLE_WINDOW => {
                                        hist.iter().all(|s| *s == RTCPeerConnectionState::Connected)
                                    }
                                    _ => false,
                                }
                            });
                        if all_connected {
                            guard.log.push(
                                format!("All peers connected for {} consecutive checks, triggering DKG Round 1...", DKG_CONNECTED_STABLE_WINDOW)
                            );
                            guard.dkg_state = DkgState::Round1InProgress;
                            let _ =
                                internal_cmd_tx_for_checker.send(InternalCommand::TriggerDkgRound1);
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
    let peer_connections_arc_main_net = state.lock().await.peer_connections.clone(); // This is Arc<TokioMutex<...>>

    tokio::spawn(async move {
        // Request peer list on start
        let list_peers_msg = SharedClientMsg::ListPeers;
        let _ = ws_sink
            .send(Message::Text(
                serde_json::to_string(&list_peers_msg).unwrap().into(),
            ))
            .await;

        loop {
            tokio::select! {
                Some(cmd) = internal_cmd_rx.recv() => {
                    handle_internal_command(
                        cmd,
                        state_main_net.clone(),
                        self_peer_id_main_net.clone(),
                        internal_cmd_tx_main_net.clone(), // Pass the internal_cmd_tx to handle_internal_command
                    ).await;
                },
                maybe_msg = ws_stream.next() => {
                    match maybe_msg {
                        Some(Ok(msg)) => {
                            handle_websocket_message(
                                msg,
                                state_main_net.clone(),
                                self_peer_id_main_net.clone(),
                                internal_cmd_tx_main_net.clone(),
                                peer_connections_arc_main_net.clone(),
                                &mut ws_sink,
                            ).await;
                        },
                        Some(Err(e)) => {
                            state_main_net.lock().await.log.push(format!("WebSocket read error: {}", e));
                            break; // Exit loop on error
                        },
                        None => {
                            state_main_net.lock().await.log.push("WebSocket stream ended".to_string());
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

    loop {
        {
            let app_guard = state.lock().await;
            draw_main_ui(&mut terminal, &app_guard, &input, input_mode)?;
        }

        // Handle key events with a timeout
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    let mut app_guard = state.lock().await;
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

/// Handler for internal commands sent via MPSC channel
async fn handle_internal_command(
    cmd: InternalCommand,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Fix: add parameter here
) {
    match cmd {
        InternalCommand::SendToServer(shared_msg) => {
            // Send via the task's WebSocket sink
            if let Ok(_msg_str) = serde_json::to_string(&shared_msg) {
                // For simplicity, we'll log that we would send this message
                state
                    .lock()
                    .await
                    .log
                    .push(format!("Sending to server: {:?}", shared_msg));
                // The actual sending happens in the main task
            }
        }
        InternalCommand::SendDirect { to, message } => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                let webrtc_msg = WebRTCMessage::SimpleMessage { text: message };
                if let Err(e) = send_webrtc_message(&to, &webrtc_msg, state_clone.clone()).await {
                    state_clone
                        .lock()
                        .await
                        .log
                        .push(format!("Error sending direct message to {}: {}", to, e));
                } else {
                    state_clone
                        .lock()
                        .await
                        .log
                        .push(format!("Sent direct message to {}", to));
                }
            });
        }
        InternalCommand::ProposeSession {
            session_id,
            total,
            threshold,
            participants,
        } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            let self_peer_id_clone = self_peer_id.clone();
            tokio::spawn(async move {
                let mut state_guard = state_clone.lock().await;
                // Create the session proposal
                let session_proposal = SessionProposal {
                    session_id: session_id.clone(),
                    total,
                    threshold,
                    participants: participants.clone(),
                };
                // Log the action
                state_guard.log.push(format!(
                    "Proposing session '{}' with {} participants and threshold {}",
                    session_id, total, threshold
                ));
                // Update local session state immediately for proposer
                state_guard.session = Some(SessionInfo {
                    session_id: session_id.clone(),
                    total,
                    threshold,
                    participants: participants.clone(),
                    accepted_peers: vec![state_guard.peer_id.clone()],
                });
                // Clone peer_id before the loop to avoid borrow checker issues
                let local_peer_id_for_filter = state_guard.peer_id.clone();
                // Broadcast proposal to all participants except self
                for peer in participants
                    .iter()
                    .filter(|p| **p != local_peer_id_for_filter)
                {
                    let proposal_msg = WebRTCMessage::SessionProposal(session_proposal.clone());
                    match serde_json::to_value(proposal_msg) {
                        Ok(json_val) => {
                            let relay_msg = SharedClientMsg::Relay {
                                to: peer.clone(),
                                data: json_val,
                            };
                            // Send via relay since WebRTC might not be established yet
                            if let Err(e) =
                                internal_cmd_tx_clone.send(InternalCommand::SendToServer(relay_msg))
                            {
                                state_guard.log.push(format!(
                                    "Failed to send session proposal to {}: {}",
                                    peer, e
                                ));
                            } else {
                                state_guard
                                    .log
                                    .push(format!("Sent session proposal to {}", peer));
                            }
                        }
                        Err(e) => {
                            state_guard.log.push(format!(
                                "Error serializing session proposal for {}: {}",
                                peer, e
                            ));
                        }
                    }
                }
                drop(state_guard); // Release lock before async operation
                // Begin WebRTC connection process if not already connected
                initiate_webrtc_connections(
                    participants,
                    self_peer_id_clone,
                    state_clone.clone(),
                    internal_cmd_tx_clone,
                )
                .await;
            });
        }
        InternalCommand::AcceptSessionProposal(session_id) => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();

            tokio::spawn(async move {
                // Create a clone of the peer_id before locking state
                let mut state_guard = state_clone.lock().await;

                // Find the invite
                if let Some(invite_idx) = state_guard
                    .invites
                    .iter()
                    .position(|i| i.session_id == session_id)
                {
                    let invite = state_guard.invites.remove(invite_idx);
                    // Log acceptance
                    state_guard
                        .log
                        .push(format!("You accepted session proposal '{}'", session_id));

                    // Update local session state
                    state_guard.session = Some(SessionInfo {
                        session_id: invite.session_id.clone(),
                        total: invite.total,
                        threshold: invite.threshold,
                        participants: invite.participants.clone(),
                        accepted_peers: vec![state_guard.peer_id.clone()],
                    });

                    // Get the participants we need to notify
                    let participants_to_notify: Vec<String> = invite
                        .participants
                        .iter()
                        .filter(|p| **p != state_guard.peer_id)
                        .cloned()
                        .collect();

                    // Prepare the response once
                    let response = SessionResponse {
                        session_id: invite.session_id.clone(),
                        accepted: true,
                    };

                    // Store participants for WebRTC connection later
                    let participants = invite.participants.clone();
                    let peer_id_for_webrtc = state_guard.peer_id.clone();
                    let self_peer_id_for_logging = state_guard.peer_id.clone(); // Clone to avoid borrow issues

                    // Release lock before notification loop
                    drop(state_guard);

                    // Notify other participants of acceptance without holding lock
                    for peer in participants_to_notify {
                        let response_msg = WebRTCMessage::SessionResponse(response.clone());
                        match serde_json::to_value(response_msg) {
                            Ok(json_val) => {
                                let relay_msg = SharedClientMsg::Relay {
                                    to: peer.clone(),
                                    data: json_val,
                                };
                                // Send via channel, handle error separately to avoid borrow conflict
                                if let Err(e) = internal_cmd_tx_clone
                                    .send(InternalCommand::SendToServer(relay_msg))
                                {
                                    // Re-acquire lock just for logging
                                    let mut guard = state_clone.lock().await;
                                    guard.log.push(format!(
                                        "Failed to send acceptance to {}: {}",
                                        peer, e
                                    ));
                                } else {
                                    // Re-acquire lock just for logging
                                    let mut guard = state_clone.lock().await;
                                    guard.log.push(format!("Sent acceptance to {}", peer));
                                }
                            }
                            Err(e) => {
                                // Re-acquire lock just for logging
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
                        participants,
                        peer_id_for_webrtc,
                        state_clone.clone(),
                        internal_cmd_tx_clone,
                    )
                    .await;
                } else {
                    state_guard.log.push(format!(
                        "No pending invite found for session '{}'",
                        session_id
                    ));
                }
            });
        }
        InternalCommand::ReportChannelOpen { peer_id } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();

            tokio::spawn(async move {
                let self_peer_id_local = state_clone.lock().await.peer_id.clone();

                // Check if in a session
                if let Some(_session) = &state_clone.lock().await.session {
                    // Send channel_open notification to the peer
                    let channel_open_msg = WebRTCMessage::ChannelOpen {
                        peer_id: self_peer_id_local.clone(),
                    };

                    if let Err(e) =
                        send_webrtc_message(&peer_id, &channel_open_msg, state_clone.clone()).await
                    {
                        state_clone
                            .lock()
                            .await
                            .log
                            .push(format!("Error sending channel_open to {}: {}", peer_id, e));
                    } else {
                        state_clone
                            .lock()
                            .await
                            .log
                            .push(format!("Sent channel_open to {}", peer_id));
                    }

                    // Clone state again before passing it to check_and_send_mesh_ready to avoid borrowing issues
                    let state_for_check = state_clone.clone();
                    check_and_send_mesh_ready(state_for_check, internal_cmd_tx_clone).await;
                }
            });
        }
        InternalCommand::MeshReady => {
            let state_clone = state.clone();
            // let internal_cmd_tx_clone = internal_cmd_tx.clone(); // Not used in this spawn
            // let self_peer_id = state.lock().await.peer_id.clone(); // Re-fetch inside spawn
            tokio::spawn(async move {
                let mut state_guard = state_clone.lock().await; // state_guard for session and peer_id
                let self_peer_id_local = state_guard.peer_id.clone();
                if let Some(session) = &state_guard.session {
                    let mesh_ready_msg = WebRTCMessage::MeshReady {
                        session_id: session.session_id.clone(),
                        peer_id: self_peer_id_local,
                    };
                    let peers_to_notify: Vec<String> = session
                        .participants
                        .iter()
                        .filter(|p| **p != state_guard.peer_id)
                        .cloned()
                        .collect();
                    drop(state_guard); // Release lock before async calls in loop
                    for peer in peers_to_notify {
                        if let Err(e) =
                            send_webrtc_message(&peer, &mesh_ready_msg, state_clone.clone()).await
                        {
                            state_clone
                                .lock()
                                .await
                                .log
                                .push(format!("Error sending mesh_ready to {}: {}", peer, e));
                        } else {
                            state_clone
                                .lock()
                                .await
                                .log
                                .push(format!("Sent mesh_ready to {}", peer));
                        }
                    }
                    let mut state_guard = state_clone.lock().await; // Re-acquire lock
                    state_guard.mesh_status = MeshStatus::Ready;
                }
            });
        }
        InternalCommand::ProcessMeshReady { peer_id } => {
            let state_clone = state.clone();
            // let internal_cmd_tx_clone = internal_cmd_tx.clone(); // Not used if DKG trigger is correct
            tokio::spawn(async move {
                let mut state_guard = state_clone.lock().await;
                state_guard.mesh_ready_peers.insert(peer_id.clone());
                // Check if all peers are now ready
                if let Some(session) = &state_guard.session {
                    let total_peers = session.participants.len();
                    let ready_peers = state_guard.mesh_ready_peers.len() + 1; // +1 for self
                    state_guard.log.push(format!(
                        "Mesh readiness: {}/{} peers ready",
                        ready_peers, total_peers
                    ));
                    state_guard.mesh_status = MeshStatus::PartiallyReady(ready_peers, total_peers);
                    // If all peers ready, trigger DKG
                    if ready_peers == total_peers {
                        state_guard
                            .log
                            .push("All peers report mesh readiness! Starting DKG...".to_string());
                        state_guard.mesh_status = MeshStatus::Ready;
                        state_guard.dkg_state = DkgState::CommitmentsInProgress; // Or Round1InProgress depending on logic
                        // Drop the lock before triggering DKG
                        drop(state_guard);
                        // Trigger DKG Round 1 when mesh is complete
                        // Assuming internal_cmd_tx is the original one passed to handle_internal_command
                        let _ = internal_cmd_tx
                            .send(InternalCommand::TriggerDkgRound1)
                            .map_err(|e| {
                                // Use map_err for logging if send fails
                                tokio::spawn(async move {
                                    // Log in a new task to avoid blocking
                                    state_clone
                                        .lock()
                                        .await
                                        .log
                                        .push(format!("Failed to trigger DKG: {}", e));
                                });
                            });
                    }
                }
            });
        }
        InternalCommand::TriggerDkgRound1 => {
            let state_clone = state.clone();
            let self_peer_id_clone = self_peer_id.clone();
            tokio::spawn(async move {
                state_clone
                    .lock()
                    .await
                    .log
                    .push("Sending commitments to all peers...".to_string());
                ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
            });
        }
        InternalCommand::ProcessDkgRound1 {
            from_peer_id,
            package,
        } => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone
                    .lock()
                    .await
                    .log
                    .push(format!("Received commitment from peer {}...", from_peer_id));
                ed25519_dkg::process_dkg_round1(state_clone, from_peer_id, package).await;
            });
        }
        InternalCommand::TriggerDkgRound2 => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone
                    .lock()
                    .await
                    .log
                    .push("All commitments received. Starting share distribution...".to_string());
                state_clone.lock().await.dkg_state = DkgState::SharesInProgress;
                ed25519_dkg::handle_trigger_dkg_round2(state_clone).await;
            });
        }
        InternalCommand::ProcessDkgRound2 {
            from_peer_id,
            package,
        } => {
            let state_clone = state.clone();
            let from_peer_id_clone = from_peer_id.clone();
            tokio::spawn(async move {
                state_clone.lock().await.log.push(format!(
                    "Received share from peer {}. Verifying...",
                    from_peer_id_clone
                ));
                state_clone.lock().await.dkg_state = DkgState::VerificationInProgress;
                ed25519_dkg::process_dkg_round2(state_clone, from_peer_id, package).await;
            });
        }
        InternalCommand::FinalizeDkg => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone
                    .lock()
                    .await
                    .log
                    .push("Computing final key share...".to_string());
                ed25519_dkg::finalize_dkg(state_clone).await;
            });
        }
    }
}

/// Handler for WebSocket messages received from the server
async fn handle_websocket_message(
    msg: Message,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    ws_sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
) {
    match msg {
        Message::Text(txt) => {
            match serde_json::from_str::<ServerMsg>(&txt) {
                Ok(server_msg) => {
                    match server_msg {
                        ServerMsg::Peers { peers } => {
                            let mut state_guard = state.lock().await;
                            state_guard.peers = peers.clone();
                        }
                        ServerMsg::Error { error } => {
                            let mut state_guard = state.lock().await;
                            state_guard.log.push(format!("Error: {}", error));
                        }
                        ServerMsg::Relay { from, data } => {
                            state
                                .lock()
                                .await
                                .log
                                .push(format!("Relay from {}: {:?}", from, data));
                            match serde_json::from_value::<WebRTCSignal>(data.clone()) {
                                Ok(signal) => {
                                    state.lock().await.log.push(format!(
                                        "Parsed WebRTC signal from {}: {:?}",
                                        from, signal
                                    ));
                                    // Handle WebRTC signal in a separate function
                                    handle_webrtc_signal(
                                        from,
                                        signal,
                                        state.clone(),
                                        self_peer_id.clone(), // Pass self_peer_id
                                        internal_cmd_tx.clone(),
                                        peer_connections_arc.clone(),
                                    )
                                    .await;
                                }
                                Err(e) => {
                                    state.lock().await.log.push(format!(
                                        "Error parsing WebRTC signal from {}: {}",
                                        from, e
                                    ));
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    state
                        .lock()
                        .await
                        .log
                        .push(format!("Error parsing server message: {}", e));
                }
            }
        }
        Message::Close(_) => {
            state
                .lock()
                .await
                .log
                .push("WebSocket connection closed by server.".to_string());
        }
        Message::Ping(ping_data) => {
            let _ = ws_sink.send(Message::Pong(ping_data)).await;
        }
        Message::Pong(_) => {} // Ignore pongs
        Message::Binary(_) => {
            state
                .lock()
                .await
                .log
                .push("Received unexpected binary message.".to_string());
        }
        Message::Frame(_) => {} // Ignore frames
    }
}

/// Handler for WebRTC signaling messages
async fn handle_webrtc_signal(
    from_peer_id: String,
    signal: WebRTCSignal,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
) {
    // Clone necessary variables for the spawned task
    let from_clone = from_peer_id.clone();
    let state_log_clone = state.clone();
    let self_peer_id_clone = self_peer_id.clone();
    let internal_cmd_tx_clone = internal_cmd_tx.clone();
    let pc_arc_net_clone = peer_connections_arc.clone();
    let signal_clone = signal.clone();
    tokio::spawn(async move {
        // Get or create peer connection
        let pc_to_use_result = {
            let peer_conns_guard = pc_arc_net_clone.lock().await; // Guard for modification
            match peer_conns_guard.get(&from_clone).cloned() {
                Some(pc) => Ok(pc),
                None => {
                    // Drop guard before await
                    drop(peer_conns_guard);
                    state_log_clone.lock().await.log.push(format!(
                        "WebRTC signal from {} received, but connection object missing. Attempting creation...",
                        from_clone
                    ));
                    create_and_setup_peer_connection(
                        from_clone.clone(),
                        self_peer_id_clone.clone(),
                        pc_arc_net_clone.clone(),
                        internal_cmd_tx_clone.clone(),
                        state_log_clone.clone(),
                        &WEBRTC_API,
                        &WEBRTC_CONFIG,
                    )
                    .await
                }
            }
        };
        match pc_to_use_result {
            Ok(pc_clone) => match signal_clone {
                WebRTCSignal::Offer(offer_info) => {
                    handle_webrtc_offer(
                        &from_clone,
                        offer_info,
                        pc_clone,
                        state_log_clone.clone(),
                        self_peer_id_clone.clone(),
                        internal_cmd_tx_clone.clone(),
                    )
                    .await;
                }
                WebRTCSignal::Answer(answer_info) => {
                    handle_webrtc_answer(
                        &from_clone,
                        answer_info,
                        pc_clone,
                        state_log_clone.clone(),
                    )
                    .await;
                }
                WebRTCSignal::Candidate(candidate_info) => {
                    handle_webrtc_candidate(
                        &from_clone,
                        candidate_info,
                        pc_clone,
                        state_log_clone.clone(),
                    )
                    .await;
                }
            },
            Err(e) => {
                state_log_clone.lock().await.log.push(format!(
                    "Failed to create/retrieve connection object for {} to handle signal: {}",
                    from_clone, e
                ));
            }
        }
    });
}

/// Handler for WebRTC offer signals
async fn handle_webrtc_offer(
    from_peer_id: &str,
    offer_info: SDPInfo,
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    use webrtc::peer_connection::signaling_state::RTCSignalingState;
    // Check if we should proceed with this offer
    let proceed_with_offer;
    let mut should_abort_due_to_race = false;
    let current_signaling_state_read;
    {
        let mut state_guard = state.lock().await;
        let making = state_guard
            .making_offer
            .get(from_peer_id)
            .copied()
            .unwrap_or(false);
        let ignoring = state_guard
            .ignore_offer
            .get(from_peer_id)
            .copied()
            .unwrap_or(false);
        let is_polite = self_peer_id > from_peer_id.to_string();
        current_signaling_state_read = pc.signaling_state();
        state_guard.log.push(format!(
            "Offer from {}: making={}, ignoring={}, is_polite={}, state={:?}",
            from_peer_id, making, ignoring, is_polite, current_signaling_state_read
        ));
        let collision = making;
        proceed_with_offer = if collision && is_polite {
            true // Polite peer yields, but will process this offer
        } else if collision && !is_polite {
            false // Impolite peer ignores incoming offer during collision
        } else if ignoring {
            false // Explicitly ignoring
        } else {
            true // No collision or ignoring, proceed
        };
        if proceed_with_offer && making {
            should_abort_due_to_race = true;
        }
    }
    // Reevaluate with safeguards
    let mut final_proceed_with_offer = proceed_with_offer;
    {
        let mut state_guard = state.lock().await;
        let making = state_guard
            .making_offer
            .get(from_peer_id)
            .copied()
            .unwrap_or(false);
        let is_polite = self_peer_id > from_peer_id.to_string();
        let collision = making;
        if should_abort_due_to_race {
            state_guard.log.push(format!(
                "Aborting offer from {}: Detected making_offer=true just before async operation (Race?)",
                from_peer_id
            ));
            final_proceed_with_offer = false;
        } else if collision && is_polite {
            state_guard.log.push(format!(
                "Glare detected with {}: Polite peer yielding (state update).",
                from_peer_id
            ));
            state_guard
                .making_offer
                .insert(from_peer_id.to_string(), false);
        } else if collision && !is_polite {
            state_guard.log.push(format!(
                "Glare detected with {}: Impolite peer ignoring incoming offer (state update).",
                from_peer_id
            ));
            state_guard
                .ignore_offer
                .insert(from_peer_id.to_string(), true);
            final_proceed_with_offer = false;
        } else if state_guard
            .ignore_offer
            .get(from_peer_id)
            .copied()
            .unwrap_or(false)
        {
            state_guard.log.push(format!(
                "Ignoring offer from {} as previously decided (state update).",
                from_peer_id
            ));
            state_guard
                .ignore_offer
                .insert(from_peer_id.to_string(), false);
            final_proceed_with_offer = false;
        }
        let is_invalid_state = !(current_signaling_state_read == RTCSignalingState::Stable
            || current_signaling_state_read == RTCSignalingState::Closed);
        if final_proceed_with_offer && is_invalid_state {
            state_guard.log.push(format!(
                "Aborting offer from {}: Invalid signaling state {:?} (expected Stable or Closed)",
                from_peer_id, current_signaling_state_read
            ));
            final_proceed_with_offer = false;
        }
    }
    if final_proceed_with_offer {
        state
            .lock()
            .await
            .log
            .push(format!("Processing offer from {}...", from_peer_id));
        // Create session description from offer
        match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::offer(
            offer_info.sdp,
        ) {
            Ok(offer) => {
                // Set remote descriptions
                if let Err(e) = pc.set_remote_description(offer).await {
                    state.lock().await.log.push(format!(
                        "Error setting remote description (offer) from {}: {}",
                        from_peer_id, e
                    ));
                    return;
                }
                state.lock().await.log.push(format!(
                    "Set remote description (offer) from {}. Creating answer...",
                    from_peer_id
                ));
                // Apply any pending ICE candidates
                apply_pending_candidates(from_peer_id, pc.clone(), state.clone()).await;
                // Create data channel before answering
                process_data_channel_creation(
                    from_peer_id,
                    &pc,
                    state.clone(),
                    internal_cmd_tx.clone(),
                )
                .await;
                // Create and send answer
                create_and_send_answer(from_peer_id, &pc, state.clone(), internal_cmd_tx).await;
            }
            Err(e) => {
                state
                    .lock()
                    .await
                    .log
                    .push(format!("Error parsing offer from {}: {}", from_peer_id, e));
            }
        }
    }
}

/// Helper for creating a data channel
async fn process_data_channel_creation(
    peer_id: &str,
    pc: &Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    match pc
        .create_data_channel(utils::peer::DATA_CHANNEL_LABEL, None)
        .await
    {
        Ok(dc) => {
            let peer_id_dc = peer_id.to_string();
            let state_log_dc = state.clone();
            let cmd_tx_dc = cmd_tx.clone();
            utils::peer::setup_data_channel_callbacks(dc, peer_id_dc, state_log_dc, cmd_tx_dc)
                .await;
            state
                .lock()
                .await
                .log
                .push(format!("Created responder data channel for {}", peer_id));
        }
        Err(e) => {
            state.lock().await.log.push(format!(
                "Error creating responder data channel for {}: {} (continuing anyway)",
                peer_id, e
            ));
        }
    }
}

/// Helper for creating and sending an answer
async fn create_and_send_answer(
    peer_id: &str,
    pc: &Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    match pc.create_answer(None).await {
        Ok(answer) => {
            if let Err(e) = pc.set_local_description(answer.clone()).await {
                state.lock().await.log.push(format!(
                    "Error setting local description (answer) for {}: {}",
                    peer_id, e
                ));
                return;
            }
            state.lock().await.log.push(format!(
                "Set local description (answer) for {}. Sending answer...",
                peer_id
            ));
            let signal = WebRTCSignal::Answer(SDPInfo { sdp: answer.sdp });
            match serde_json::to_value(signal) {
                Ok(json_val) => {
                    let relay_msg = InternalCommand::SendToServer(SharedClientMsg::Relay {
                        to: peer_id.to_string(),
                        data: json_val,
                    });
                    let _ = cmd_tx.send(relay_msg);
                    state
                        .lock()
                        .await
                        .log
                        .push(format!("Sent answer to {}", peer_id));
                }
                Err(e) => {
                    state
                        .lock()
                        .await
                        .log
                        .push(format!("Error serializing answer for {}: {}", peer_id, e));
                }
            }
        }
        Err(e) => {
            state
                .lock()
                .await
                .log
                .push(format!("Error creating answer for {}: {}", peer_id, e));
        }
    }
}

/// Handler for WebRTC answer signals
async fn handle_webrtc_answer(
    from_peer_id: &str,
    answer_info: SDPInfo,
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
) {
    state
        .lock()
        .await
        .log
        .push(format!("Processing answer from {}...", from_peer_id));
    match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::answer(
        answer_info.sdp,
    ) {
        Ok(answer) => {
            if let Err(e) = pc.set_remote_description(answer).await {
                state.lock().await.log.push(format!(
                    "Error setting remote description (answer) from {}: {}",
                    from_peer_id, e
                ));
            } else {
                state.lock().await.log.push(format!(
                    "Set remote description (answer) from {}",
                    from_peer_id
                ));
                // Apply any pending ICE candidates now that the remote description is set
                apply_pending_candidates(from_peer_id, pc.clone(), state.clone()).await;
            }
        }
        Err(e) => {
            state
                .lock()
                .await
                .log
                .push(format!("Error parsing answer from {}: {}", from_peer_id, e));
        }
    }
}

/// Handler for WebRTC ICE candidate signals
async fn handle_webrtc_candidate(
    from_peer_id: &str,
    candidate_info: protocal::signal::CandidateInfo,
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
) {
    state
        .lock()
        .await
        .log
        .push(format!("Processing candidate from {}...", from_peer_id));
    let candidate_init = RTCIceCandidateInit {
        candidate: candidate_info.candidate,
        sdp_mid: candidate_info.sdp_mid,
        sdp_mline_index: candidate_info.sdp_mline_index,
        username_fragment: None,
    };
    // Check if remote description is set before adding ICE candidate
    let current_state = pc.signaling_state();
    let remote_description_set = match current_state {
        webrtc::peer_connection::signaling_state::RTCSignalingState::HaveRemoteOffer
        | webrtc::peer_connection::signaling_state::RTCSignalingState::HaveLocalPranswer
        | webrtc::peer_connection::signaling_state::RTCSignalingState::HaveRemotePranswer
        | webrtc::peer_connection::signaling_state::RTCSignalingState::Stable => true,
        _ => false,
    };
    if remote_description_set {
        state.lock().await.log.push(format!(
            "Remote description is set for {}. Adding ICE candidate now.",
            from_peer_id
        ));
        if let Err(e) = pc.add_ice_candidate(candidate_init.clone()).await {
            state.lock().await.log.push(format!(
                "Error adding ICE candidate from {}: {}",
                from_peer_id, e
            ));
        } else {
            state
                .lock()
                .await
                .log
                .push(format!("Added ICE candidate from {}", from_peer_id));
        }
    } else {
        // Store the candidate for later
        let mut state_guard = state.lock().await;
        state_guard.log.push(format!(
            "Storing ICE candidate from {} for later (remote description not set yet)",
            from_peer_id
        ));
        let candidates = state_guard
            .pending_ice_candidates
            .entry(from_peer_id.to_string())
            .or_insert_with(Vec::new);
        candidates.push(candidate_init);
        let queued_msg = format!(
            "Queued ICE candidate from {}. Total queued: {}",
            from_peer_id,
            candidates.len()
        );
        state_guard.log.push(queued_msg);
    }
}

// New helper function to check if all data channels are open and send mesh_ready if needed
async fn check_and_send_mesh_ready(
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    let mut all_channels_open = false;
    {
        let state_guard = state.lock().await;
        if let Some(session) = &state_guard.session {
            // Check if we have open data channels to all peers in the session
            let all_peers_connected = session
                .participants
                .iter()
                .filter(|p| **p != state_guard.peer_id)
                .all(|p| state_guard.data_channels.contains_key(p));
            all_channels_open = all_peers_connected;
        }
    }
    if all_channels_open {
        state
            .lock()
            .await
            .log
            .push("All data channels open! Signaling mesh readiness...".to_string());

        // Remove the `await` here and handle error synchronously
        cmd_tx.send(InternalCommand::MeshReady).unwrap_or_else(|e| {
            // Clone state before moving into the closure to avoid borrowing issues
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone
                    .lock()
                    .await
                    .log
                    .push(format!("Failed to signal mesh readiness: {}", e));
            });
        });
    }
}

// New helper function to initiate WebRTC connections with all session participants
async fn initiate_webrtc_connections(
    participants: Vec<String>,
    self_peer_id: String,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    let peer_connections_arc = state.lock().await.peer_connections.clone();
    // Create peer connections for any participants we don't have connections with yet
    for peer_id in participants.iter().filter(|p| **p != self_peer_id) {
        let peer_conns = peer_connections_arc.lock().await;
        // Remove unnecessary parentheses
        if !peer_conns.contains_key(peer_id) {
            drop(peer_conns);
            match create_and_setup_peer_connection(
                peer_id.clone(),
                self_peer_id.clone(),
                peer_connections_arc.clone(),
                internal_cmd_tx.clone(),
                state.clone(),
                &WEBRTC_API,
                &WEBRTC_CONFIG,
            )
            .await
            {
                Ok(_) => {
                    state
                        .lock()
                        .await
                        .log
                        .push(format!("Created peer connection for {}", peer_id));
                }
                Err(e) => {
                    state.lock().await.log.push(format!(
                        "Error creating peer connection for {}: {}",
                        peer_id, e
                    ));
                }
            }
        }
    }
    // Initiate offers based on peer ID ordering (to avoid both sides making offers)
    initiate_offers_for_session(
        participants,
        self_peer_id,
        peer_connections_arc,
        internal_cmd_tx,
        state,
    )
    .await;
}
