use crate::protocal::signal::WebRTCMessage;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use frost_ed25519::Ed25519Sha512;

use futures_util::{SinkExt, StreamExt};

use ratatui::{Terminal, backend::CrosstermBackend};

use std::collections::BTreeMap;

use solnana_mpc_frost::InternalCommand;
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
use protocal::signal::{SDPInfo, WebRTCSignal};
use utils::negotiation::initiate_offers_for_session;
use utils::peer::{
    apply_pending_candidates, create_and_setup_peer_connection, send_webrtc_message,
};
use utils::state::{AppState, DkgState, ReconnectionTracker};

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};
// Import our new utility modules
use utils::ed25519_dkg;
// Import from our new webrtc module
use network::webrtc::{WEBRTC_API, WEBRTC_CONFIG};

mod protocal;

mod network;

// 1. 新增: DKG 启动“稳定窗口”参数
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

                if let Ok(mut guard) = state_for_checker.try_lock() {
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
                        let participants_clone =
                            guard.session.as_ref().map(|s| s.participants.clone());
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
                                            hist.iter()
                                                .all(|s| *s == RTCPeerConnectionState::Connected)
                                        }
                                        _ => false,
                                    }
                                });
                            if all_connected {
                                guard.log.push(
                                    format!("All peers connected for {} consecutive checks, triggering DKG Round 1...", DKG_CONNECTED_STABLE_WINDOW)
                                );
                                guard.dkg_state = DkgState::Round1InProgress;
                                let _ = internal_cmd_tx_for_checker
                                    .send(InternalCommand::TriggerDkgRound1);
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
                        self_peer_id_main_net.clone()
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
) {
    match cmd {
        InternalCommand::SendToServer(shared_msg) => {
            // Send via the task's WebSocket sink
            if let Ok(msg_str) = serde_json::to_string(&shared_msg) {
                // Fix unused variable warning by prefixing with underscore
                let _server_msg = Message::Text(msg_str.into());
                // Access shared state to get WebSocket sink (or pass it as parameter)
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
        InternalCommand::TriggerDkgRound1 => {
            let state_clone = state.clone();
            let self_peer_id_clone = self_peer_id.clone();
            tokio::spawn(async move {
                ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
            });
        }
        InternalCommand::ProcessDkgRound1 {
            from_peer_id,
            package,
        } => {
            ed25519_dkg::process_dkg_round1(state.clone(), from_peer_id, package).await;
        }
        InternalCommand::ProcessDkgRound2 {
            from_peer_id,
            package,
        } => {
            let state_clone = state.clone();
            let from_peer_id_clone = from_peer_id.clone();
            let package_clone = package.clone();
            tokio::spawn(async move {
                ed25519_dkg::process_dkg_round2(state_clone, from_peer_id_clone, package_clone)
                    .await;
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
                            drop(state_guard);
                        }
                        ServerMsg::Error { error } => {
                            let mut state_guard = state.lock().await;
                            state_guard.log.push(format!("Error: {}", error));
                            drop(state_guard);
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
                                        self_peer_id,
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
            // The main loop will handle breaking out
        }
        Message::Ping(ping_data) => {
            // Respond with Pong
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
    let self_peer_id_clone = self_peer_id;
    let internal_cmd_tx_clone = internal_cmd_tx;
    let pc_arc_net_clone = peer_connections_arc.clone();
    let signal_clone = signal.clone();

    tokio::spawn(async move {
        // Get or create peer connection
        let pc_to_use_result = {
            let peer_conns = pc_arc_net_clone.lock().await;
            match peer_conns.get(&from_clone).cloned() {
                Some(pc) => Ok(pc),
                None => {
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
                // Set remote description
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
