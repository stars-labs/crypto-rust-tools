mod protocal;
use protocal::signal::WebRTCMessage; // Updated path
use protocal::signal::WebSocketMessage;  // Updated path
use protocal::signal::SessionResponse;
use protocal::signal::WebRTCSignal;
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
use utils::state::{DkgState, InternalCommand, MeshStatus}; // <-- Add SessionResponse here

use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, io};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::RTCPeerConnection;

use webrtc_signal_server::{ClientMsg as SharedClientMsg, ServerMsg};
// Add display-related imports for better status handling

mod utils;
// --- Use items from modules ---
// Updated path for all items from protocal::signal
use protocal::signal::{
    CandidateInfo, SDPInfo, SessionInfo, SessionProposal,
};
use utils::negotiation::initiate_offers_for_session;
use utils::peer::{
    apply_pending_candidates, create_and_setup_peer_connection, send_webrtc_message,
};
use utils::state::{AppState, ReconnectionTracker}; // Remove DkgState import

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};

use utils::ed25519_dkg;

use network::webrtc::{WEBRTC_API, WEBRTC_CONFIG};

mod network;

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

    // Shared state for TUI and networkthin the CLI app (uses InternalCommand from lib.rs)
    // Specify the Ciphersuite for AppStaterx) = mpsc::unbounded_channel::<InternalCommand>();
    let state = Arc::new(Mutex::new(AppState::<Ed25519Sha512> {
        // TUI state uses Mutex network
        peer_id: peer_id.clone(),//r AppState
        peers: vec![],//ew(Mutex::new(AppState::<Ed25519Sha512> {
        log: vec!["Registered with server".to_string()], // Corrected typo
        log_scroll: 0, // Initialize scroll state
        session: None,
        invites: vec![],//tered with server".to_string()],
        peer_connections: Arc::new(Mutex::new(HashMap::new())), // Use TokioMutex here
        peer_statuses: HashMap::new(),                          // Initialize peer statuses
        reconnection_tracker: ReconnectionTracker::new(),
        making_offer: HashMap::new(),//tex::new(HashMap::new())), // Use TokioMutex here
        pending_ice_candidates: HashMap::new(),//er::new(),
        dkg_state: DkgState::Idle,//(),
        identifier_map: None, //::new(),
        local_dkg_part1_data: None,//hMap::new(),
        received_dkg_packages: BTreeMap::new(),
        key_package: None,//ne,
        group_public_key: None,//one,
        data_channels: HashMap::new(),//p::new(),
        solana_public_key: None,
        queued_dkg_round1: vec![],
        round2_secret_package: None,//),
        received_dkg_round2_packages: BTreeMap::new(), // Initialize new field
        mesh_ready_peers: HashSet::new(),
        mesh_status: MeshStatus::Incomplete,//(), // peer_id -> Vec<RTCPeerConnectionState>
    }));
    let state_main_net = state.clone();
    let self_peer_id_main_net = peer_id.clone(); //mmunication + Internal Commands) ---
    let internal_cmd_tx_main_net = internal_cmd_tx.clone();
    let peer_connections_arc_main_net = state.lock().await.peer_connections.clone(); // This is Arc<TokioMutex<...>>

    tokio::spawn(async move {
        loop {
            tokio::select! { //to_string(&list_peers_msg).unwrap().into(),
                Some(cmd) = internal_cmd_rx.recv() => {
                    handle_internal_command(
                        cmd,
                        state_main_net.clone(),
                        self_peer_id_main_net.clone(),
                        internal_cmd_tx_main_net.clone(),
                        &mut ws_sink,
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
                        Some(Err(e)) => { //n_net.clone(),
                            state_main_net.lock().await.log.push(format!("WebSocket read error: {}", e));
                            break;
                        },
                        None => {
                            state_main_net.lock().await.log.push("WebSocket stream ended".to_string());
                            break;
                        } 
                    }     
                }         
            }           
        }               
    });
       
    // TUI setup        //}
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?; // Keep mut for terminal.backend_mut() later

    let mut input = String::new(); // Need mut for editing input
    let mut input_mode = false; // Need mut for toggling input mode
    //let mut stdout = io::stdout(); // Duplicate removed
    loop { //e!(stdout, EnterAlternateScreen)?;
        { //ackend = CrosstermBackend::new(stdout);
            let app_guard = state.lock().await; // Keep mut for terminal.backend_mut() later
            draw_main_ui(&mut terminal, &app_guard, &input, input_mode)?;
        } //ut input = String::new(); // Need mut for editing input
    //let mut input_mode = false; // Need mut for toggling input mode // Duplicate removed
        // Handle key events with a timeout
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => { //ck().await;
                    let mut app_guard = state.lock().await; // input_mode)?;
                    let continue_loop = handle_key_event(
                        key,
                        &mut app_guard, //eout
                        &mut input, //om_millis(100))? {
                        &mut input_mode,
                        &internal_cmd_tx,
                    )?; // mut app_guard = state.lock().await;
                    if !continue_loop { // handle_key_event(
                        break;
                    }   //&mut app_guard,
                }       //&mut input,
                _ => {} // Ignore other events like Mouse, Resize etc.
            }           //&internal_cmd_tx,
        }           //)?;
        // No sleep needed - event::poll has a timeout already
    }                   //break;
                    //} // Duplicate removed
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?; //esize etc.
    terminal.show_cursor()?;
    Ok(())
}       // No sleep needed - event::poll has a timeout already
    //} // Duplicate removed
/// Handler for internal commands sent via MPSC channel
use frost_core::Identifier; // Add this import for Identifier::try_from
async fn handle_internal_command(
    cmd: InternalCommand, //kend_mut(), LeaveAlternateScreen)?;
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Fix: add parameter here
    ws_sink: &mut futures_util::stream::SplitSink< // Add ws_sink parameter
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
) {
    match cmd { // internal commands sent via MPSC channel
        InternalCommand::SendToServer(shared_msg) => {
                    if let Ok(msg_str) = serde_json::to_string(&shared_msg) {
                        match ws_sink.send(Message::Text(msg_str.into())).await {
                            Ok(_) => {
                                state.lock().await.log.push(format!(
                                    "Successfully sent to server: {:?}", shared_msg
                                ));
                            },
                            Err(e) => {
                                state.lock().await.log.push(format!(
                                    "Failed to send to server: {:?} - Error: {}", shared_msg, e
                                ));
                            }
                        }  
                    } else {
                        state.lock().await.log.push(format!(
                            "Failed to serialize message: {:?}", shared_msg
                        ));
                    } 
        }
        InternalCommand::SendDirect { to, message } => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                let webrtc_msg = WebRTCMessage::SimpleMessage { text: message };
                if let Err(e) = send_webrtc_message(&to, &webrtc_msg, state_clone.clone()).await {
                    
                        state_clone.lock()
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
                
                let session_proposal = SessionProposal {
                    session_id: session_id.clone(),
                    total,
                    threshold,
                    participants: participants.clone(),
                };
                
                state_guard.log.push(format!(
                    "Proposing session '{}' with {} participants and threshold {}",
                    session_id, total, threshold
                ));
                
                let current_peer_id = state_guard.peer_id.clone(); 
                state_guard.session = Some(SessionInfo {
                    session_id: session_id.clone(),
                    proposer_id: current_peer_id.clone(), // Set proposer_id
                    total,
                    threshold,
                    participants: participants.clone(),
                    accepted_peers: vec![current_peer_id.clone()],
                }); 
                
                let mut map_created_and_check_dkg = false;
                if participants.len() == 1 && participants.contains(&current_peer_id) {
                     if state_guard.identifier_map.is_none() { 
                        let mut participants_sorted = participants.clone();
                        participants_sorted.sort();

                        let mut new_identifier_map = BTreeMap::new();
                        for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                            match Identifier::try_from((i + 1) as u16) {
                                Ok(identifier) => {
                                    new_identifier_map.insert(peer_id_str.clone(), identifier);
                                }
                                Err(e) => {
                                    state_guard.log.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                                    state_guard.session = None;
                                    state_guard.identifier_map = None;
                                    return; 
                                }
                            }
                        }
                        state_guard.log.push(format!("Identifier map created for single-participant session: {:?}", new_identifier_map));
                        state_guard.identifier_map = Some(new_identifier_map);
                        map_created_and_check_dkg = true;
                    }
                }
                
                // Clone peer_id before the loop to avoid borrow checker issues
                let local_peer_id_for_filter = state_guard.peer_id.clone();
                // Broadcast proposal to all participants except self
                // Release lock before async operations or sending internal commands
                drop(state_guard);

                if map_created_and_check_dkg {
                    if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                        // Log error or handle, need to re-acquire lock for logging
                        state_clone.lock().await.log.push(format!("Failed to send CheckAndTriggerDkg after proposing single-user session: {}", e));
                    }
                }
                
                // Re-acquire lock for logging within the loop if necessary, or collect messages
                let mut state_guard_for_broadcast = state_clone.lock().await;
                for peer in participants 
                    .iter() 
                    .filter(|p| **p != local_peer_id_for_filter) {
                
                    let proposal_msg = WebSocketMessage::SessionProposal(session_proposal.clone());
                    match serde_json::to_value(proposal_msg) {
                        Ok(json_val) => {
                            let relay_msg = SharedClientMsg::Relay {
                                to: peer.clone(), 
                                data: json_val, 
                            }; 
                            if let Err(e) = 
                                internal_cmd_tx_clone.send(InternalCommand::SendToServer(relay_msg))
                            {
                                state_guard_for_broadcast.log.push(format!(
                                    "Failed to send session proposal to {}: {}",
                                    peer, e
                                ));
                            } else {
                                state_guard_for_broadcast
                                    .log 
                                    .push(format!("Sent session proposal to {}", peer));
                            } 
                        }
                        Err(e) => {
                            state_guard_for_broadcast.log.push(format!(
                                "Error serializing session proposal for {}: {}",
                                peer, e
                            ));
                        }
                    }
                } 
                drop(state_guard_for_broadcast); 
                initiate_webrtc_connections( 
                    participants, 
                    self_peer_id_clone, // Use the cloned self_peer_id from the start of the task
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
                let mut map_created_and_check_dkg = false;
                let mut participants_for_webrtc: Vec<String> = Vec::new(); 
                let mut peer_id_for_webrtc: String = String::new();    
                let mut participants_for_map_creation_in_accept: Option<Vec<String>> = None;

                { 
                    let mut state_guard = state_clone.lock().await; 

                    if let Some(invite_idx) = state_guard
                        .invites
                        .iter()
                        .position(|i| i.session_id == session_id)
                    {   
                        let invite = state_guard.invites.remove(invite_idx);
                        let current_peer_id = state_guard.peer_id.clone();
                        
                        state_guard
                            .log
                            .push(format!("You accepted session proposal '{}'", session_id));

                        let mut combined_accepted_peers = HashSet::new();
                        combined_accepted_peers.insert(current_peer_id.clone()); // Self
                        combined_accepted_peers.insert(invite.proposer_id.clone()); // Proposer
                        for early_accepter in &invite.accepted_peers { // Peers who responded before local accept
                            combined_accepted_peers.insert(early_accepter.clone());
                        }

                        let final_accepted_peers: Vec<String> = combined_accepted_peers.into_iter().collect();

                        state_guard.session = Some(SessionInfo {
                            session_id: invite.session_id.clone(),
                            proposer_id: invite.proposer_id.clone(), 
                            total: invite.total,
                            threshold: invite.threshold,
                            participants: invite.participants.clone(),
                            accepted_peers: final_accepted_peers.clone(), 
                        });
                        
                        peer_id_for_webrtc = current_peer_id.clone(); 
                        participants_for_webrtc = invite.participants.clone(); 

                        // Check if all participants have accepted *now* that we've processed our own acceptance
                        // and combined early responses.
                        if final_accepted_peers.len() == invite.participants.len() {
                            if state_guard.identifier_map.is_none() {
                                state_guard.log.push(format!(
                                    "All {} participants now confirmed for session '{}' upon local acceptance. Preparing identifier map.",
                                    invite.participants.len(), invite.session_id
                                ));
                                // Use invite.participants for map creation as it's the definitive list
                                participants_for_map_creation_in_accept = Some(invite.participants.clone());
                            }
                        }
                        
                        // This specific single-user map creation might be redundant if the above general check works,
                        // but keeping it doesn't harm.
                        if invite.participants.len() == 1 && invite.participants.contains(&current_peer_id) {
                            if state_guard.identifier_map.is_none() {
                                // Ensure participants_for_map_creation_in_accept is set if not already
                                if participants_for_map_creation_in_accept.is_none() {
                                     participants_for_map_creation_in_accept = Some(invite.participants.clone());
                                }
                            }
                        }

                        if let Some(participants_list) = participants_for_map_creation_in_accept {
                             // This check for identifier_map.is_none() is important to avoid overwriting
                            if state_guard.identifier_map.is_none() {
                                let mut participants_sorted = participants_list;
                                participants_sorted.sort();
                                let mut new_identifier_map = BTreeMap::new();
                                for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                                    match Identifier::try_from((i + 1) as u16) {
                                        Ok(identifier) => {
                                            new_identifier_map.insert(peer_id_str.clone(), identifier);
                                        }
                                        Err(e) => {
                                            state_guard.log.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                                            state_guard.session = None;
                                            state_guard.identifier_map = None;
                                            // Early return if map creation fails
                                            return; 
                                        }
                                    }
                                }
                                state_guard.log.push(format!("Identifier map created (on accept): {:?}", new_identifier_map));
                                state_guard.identifier_map = Some(new_identifier_map);
                                map_created_and_check_dkg = true;
                            }
                        }

                        let participants_to_notify: Vec<String> = invite
                            .participants
                            .iter()
                            .filter(|p| **p != current_peer_id) // Use current_peer_id from state
                            .cloned()
                            .collect();

                        let response_payload = SessionResponse {
                            session_id: invite.session_id.clone(),
                            accepted: true,
                        };
                        
                        // Release lock before notification loop and other async operations
                        drop(state_guard); 

                        if map_created_and_check_dkg {
                             if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                                state_clone.lock().await.log.push(format!("Failed to send CheckAndTriggerDkg after accepting single-user session: {}", e));
                            }
                        }

                        for peer in participants_to_notify {
                            let response_msg = WebSocketMessage::SessionResponse(response_payload.clone()); 
                            match serde_json::to_value(response_msg) {
                                Ok(json_val) => {
                                    let relay_msg = SharedClientMsg::Relay {
                                        to: peer.clone(),
                                        data: json_val,
                                    };  
                                    if let Err(e) = internal_cmd_tx_clone
                                        .send(InternalCommand::SendToServer(relay_msg))
                                    {
                                        let mut guard = state_clone.lock().await;
                                        guard.log.push(format!(
                                            "Failed to send acceptance to {}: {}",
                                            peer, e
                                        ));
                                    } else {
                                        let mut guard = state_clone.lock().await;
                                        guard.log.push(format!("Sent acceptance to {}", peer));
                                    }   
                                }
                                Err(e) => {
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
                            participants_for_webrtc, 
                            peer_id_for_webrtc, // Use the captured peer_id
                            state_clone.clone(),
                            internal_cmd_tx_clone,
                        )   
                        .await;
                        return; // Successfully processed
                    } else { 
                        // This block is inside the if let Some(invite_idx)
                        // If invite not found, this log is correct.
                        state_guard.log.push(format!(
                            "No pending invite found for session '{}'",
                            session_id 
                        ));
                    }
                } // Lock is released here if invite was not found or if already released above
            });
        } 
        InternalCommand::ProcessSessionResponse { from_peer_id, response } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone(); 
            tokio::spawn(async move {
                let mut log_msgs = Vec::new();
                let mut session_cancelled = false;
                let mut session_id_to_cancel = None;
                let mut map_created_and_check_dkg = false;
                let mut participants_for_map_creation: Option<Vec<String>> = None;

                { 
                    let mut state_guard = state_clone.lock().await;
                    log_msgs.push(format!(
                        "Received session response from {}: Accepted={}",
                        from_peer_id, response.accepted
                    ));
                    if response.accepted {
                        let mut handled_in_active_session = false;
                        if let Some(session) = state_guard.session.as_mut() {
                            if session.session_id == response.session_id {
                                handled_in_active_session = true; // Mark that we found a matching active session
                                if !session.accepted_peers.contains(&from_peer_id) {
                                    session.accepted_peers.push(from_peer_id.clone());
                                    log_msgs.push(format!(
                                        "Peer {} accepted session '{}'. Active session accepted peers: {}/{}",
                                        from_peer_id, session.session_id, session.accepted_peers.len(), session.participants.len()
                                    ));
                                    if session.accepted_peers.len() == session.participants.len() {
                                        log_msgs.push(format!(
                                            "All {} participants accepted session '{}' (active session). Preparing identifier map.",
                                            session.participants.len(), session.session_id
                                        ));
                                        participants_for_map_creation = Some(session.participants.clone());
                                    }
                                } else {
                                    log_msgs.push(format!(
                                        "Peer {} already recorded in active session '{}' accepted_peers.",
                                        from_peer_id, session.session_id
                                    ));
                                }
                            }
                        } 
                        
                        // If not handled in active session (either no active session, or different session_id)
                        // then check invites.
                        if !handled_in_active_session {
                            if let Some(invite_to_update) = state_guard.invites.iter_mut().find(|i| i.session_id == response.session_id) {
                                if !invite_to_update.accepted_peers.contains(&from_peer_id) {
                                    invite_to_update.accepted_peers.push(from_peer_id.clone());
                                    log_msgs.push(format!(
                                        "Recorded early acceptance from {} for pending invite '{}'. Invite accepted_peers count: {}.",
                                        from_peer_id, invite_to_update.session_id, invite_to_update.accepted_peers.len()
                                    ));
                                } else {
                                     log_msgs.push(format!(
                                        "Peer {} already recorded in pending invite '{}' accepted_peers.",
                                        from_peer_id, invite_to_update.session_id
                                    ));
                                }
                            } else {
                                log_msgs.push(format!(
                                    "No active session or pending invite found for session ID '{}' from peer {}.",
                                    response.session_id, from_peer_id
                                ));
                            }
                        }
                        
                        // Map creation logic based on `participants_for_map_creation` which is set if an *active* session became full.
                        if let Some(participants_list) = participants_for_map_creation {
                            if state_guard.identifier_map.is_none() { 
                                let mut participants_sorted = participants_list; 
                                participants_sorted.sort(); 

                                let mut new_identifier_map = BTreeMap::new();
                                for (i, peer_id_str) in participants_sorted.iter().enumerate() {
                                    match Identifier::try_from((i + 1) as u16) {
                                        Ok(identifier) => {
                                            new_identifier_map.insert(peer_id_str.clone(), identifier);
                                        }
                                        Err(e) => {
                                            log_msgs.push(format!("Error creating identifier for {}: {}", peer_id_str, e));
                                            state_guard.session = None; 
                                            state_guard.identifier_map = None;
                                            // Push logs and return
                                            for msg_item in log_msgs { state_guard.log.push(msg_item); }
                                            return; 
                                        }
                                    }
                                }
                                log_msgs.push(format!("Identifier map created: {:?}", new_identifier_map));
                                state_guard.identifier_map = Some(new_identifier_map);
                                map_created_and_check_dkg = true;
                            }
                        }
                    } else {
                        log_msgs.push(format!(
                            "Peer {} rejected session '{}'.",
                            from_peer_id, response.session_id
                        ));
                        if let Some(session) = &state_guard.session { // Immutable borrow here is fine
                            if session.session_id == response.session_id {
                                session_cancelled = true;
                                session_id_to_cancel = Some(response.session_id.clone());
                            }
                        }
                    }
                } // state_guard lock released
                
                if map_created_and_check_dkg {
                    if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::CheckAndTriggerDkg) {
                        log_msgs.push(format!("Failed to send CheckAndTriggerDkg command: {}", e));
                    }
                }

                if session_cancelled {
                    let mut guard = state_clone.lock().await;
                    guard.session = None;
                    guard.identifier_map = None; 
                    if let Some(sid) = session_id_to_cancel {
                        log_msgs.push(format!("Session '{}' cancelled due to rejection by {}.", sid, from_peer_id));
                    }
                }
                
                let mut guard = state_clone.lock().await;
                for msg in log_msgs {
                    guard.log.push(msg);
                }
            });
        }
        InternalCommand::ReportChannelOpen { peer_id } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            // Clone self_peer_id for use in the spawned task to avoid capturing the original self_peer_id from the outer function
            let self_peer_id_for_task = self_peer_id.clone(); 

            // Log immediately upon receiving the command, outside the main processing task
            // to ensure this log appears even if the task has issues.
            // We need to acquire a lock briefly for this initial log.
            let log_peer_id_initial_log = peer_id.clone(); // Clone for initial log
            let initial_log_state_clone = state.clone();
            let initial_log_self_peer_id = self_peer_id.clone();
            tokio::spawn(async move {
                initial_log_state_clone.lock().await.log.push(format!(
                    "[ReportChannelOpen-{}] Received for remote peer: {}.",
                    initial_log_self_peer_id, log_peer_id_initial_log
                ));
            });

            // Clone peer_id again for the main processing task as it's moved into the first spawn
            let peer_id_for_main_task = peer_id.clone();
            tokio::spawn(async move {
                let local_self_peer_id = self_peer_id_for_task; // Use the cloned self_peer_id for this task
                let session_exists_at_dispatch: bool;
                let mut log_messages_for_task = Vec::new();

                {
                    let mut guard = state_clone.lock().await; // Make guard mutable
                    session_exists_at_dispatch = guard.session.is_some();
                    log_messages_for_task.push(format!(
                        "[ReportChannelOpenTask-{}] Running for remote peer: {}. Session exists at dispatch: {}",
                        local_self_peer_id, peer_id_for_main_task, session_exists_at_dispatch
                    ));

                    // Update peer status to Connected as data channel is reported open
                    guard.peer_statuses.insert(peer_id_for_main_task.clone(), webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected);
                    log_messages_for_task.push(format!(
                        "[ReportChannelOpenTask-{}] Set peer status for {} to Connected.",
                        local_self_peer_id, peer_id_for_main_task
                    ));
                } // Lock released

                if session_exists_at_dispatch {
                    // Send channel_open notification to the remote peer
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
                    
                    // Acquire lock to push messages before calling the next async function
                    {
                        let mut guard = state_clone.lock().await;
                        for msg in log_messages_for_task {
                            guard.log.push(msg);
                        }
                    } // Lock released

                    check_and_send_mesh_ready(state_clone.clone(), internal_cmd_tx_clone).await;
                } else {
                    log_messages_for_task.push(format!("[ReportChannelOpenTask-{}] Session does not exist for remote peer: {}. Skipping mesh check logic.", local_self_peer_id, peer_id_for_main_task));
                    // Acquire lock to push final log messages
                    {
                        let mut guard = state_clone.lock().await;
                        for msg in log_messages_for_task {
                            guard.log.push(msg);
                        }
                    } // Lock released
                }
            });
        }
        InternalCommand::SendOwnMeshReadySignal => { 
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone(); // For CheckAndTriggerDkg
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
                        let current_mesh_ready_peers_count = state_guard.mesh_ready_peers.len(); 

                        state_guard.log.push(format!(
                            "Local node is mesh ready. Sending MeshReady signal to peers. Current mesh_ready_peers count: {}",
                            current_mesh_ready_peers_count
                        ));
                        state_guard.mesh_status = MeshStatus::SentSelfReady; 

                        let all_others_reported_mesh_ready = if session_participants_count > 0 {
                             current_mesh_ready_peers_count == (session_participants_count - 1)
                        } else {
                            false
                        };

                        if all_others_reported_mesh_ready {
                            state_guard.mesh_status = MeshStatus::Ready;
                            mesh_became_ready = true; // Mark that mesh became ready
                            state_guard.log.push("All peers (including self) are mesh ready. Overall MeshStatus: Ready.".to_string());
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

                let mesh_ready_msg = WebRTCMessage::MeshReady {
                    session_id: session_id_local.clone(), 
                    peer_id: self_peer_id_local.clone(), 
                };  
                
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
        InternalCommand::ProcessMeshReady { peer_id } => {
            let state_clone = state.clone(); 
            let internal_cmd_tx_clone = internal_cmd_tx.clone(); // For CheckAndTriggerDkg
            tokio::spawn(async move { 
                let mut final_mesh_status_after_processing = MeshStatus::Incomplete; 
                let mut log_messages = Vec::new();
                let mut mesh_became_ready = false;

                { 
                    let mut state_guard = state_clone.lock().await;
                    let already_known = state_guard.mesh_ready_peers.contains(&peer_id);
                    state_guard.mesh_ready_peers.insert(peer_id.clone());
                    
                    if already_known {
                        log_messages.push(format!("Received duplicate MeshReady from {}. Ignoring.", peer_id));
                    } else {
                        log_messages.push(format!("Processing MeshReady from peer: {}", peer_id));
                    }

                    if let Some(session) = &state_guard.session { 
                        let total_session_participants = session.participants.len();
                        let num_known_other_mesh_ready_peers = state_guard.mesh_ready_peers.len();

                        log_messages.push(format!(
                            "Mesh readiness update: {} other peers reported MeshReady. Total session participants: {}.",
                            num_known_other_mesh_ready_peers, total_session_participants
                        ));

                        let all_others_reported_mesh_ready = if total_session_participants > 0 {
                            num_known_other_mesh_ready_peers == (total_session_participants - 1)
                        } else {
                            false 
                        };
                        
                        if state_guard.mesh_status == MeshStatus::SentSelfReady && all_others_reported_mesh_ready {
                            state_guard.mesh_status = MeshStatus::Ready;
                            mesh_became_ready = true; // Mark that mesh became ready
                            log_messages.push(format!(
                                "All peers (including self) are mesh ready. Overall MeshStatus set to: Ready. ({} other peers ready, self was SentSelfReady)",
                                num_known_other_mesh_ready_peers
                            ));
                        } else if state_guard.mesh_status != MeshStatus::Ready { 
                            state_guard.mesh_status = MeshStatus::PartiallyReady(num_known_other_mesh_ready_peers + 1, total_session_participants);
                             log_messages.push(format!(
                                "Mesh status updated to PartiallyReady ({}/{} participants known ready). Self status: {:?}.",
                                num_known_other_mesh_ready_peers + 1, 
                                total_session_participants,
                                state_guard.mesh_status 
                            ));
                        }
                        final_mesh_status_after_processing = state_guard.mesh_status.clone();
                    } else {
                        log_messages.push(format!( 
                            "Received MeshReady from {} but no active session.", peer_id
                        )); 
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
                state_log_guard.log.push(format!("Finished processing MeshReady from {}. Current local MeshStatus: {:?}", peer_id, final_mesh_status_after_processing));
            }); 
        }   
        InternalCommand::CheckAndTriggerDkg => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            tokio::spawn(async move {
                let mut guard = state_clone.lock().await;
                let peer_id_for_log = guard.peer_id.clone(); // Clone peer_id for logging

                let mesh_ready = guard.mesh_status == MeshStatus::Ready;
                let map_exists = guard.identifier_map.is_some();
                let session_active = guard.session.is_some();
                let dkg_idle = guard.dkg_state == DkgState::Idle;

                if mesh_ready && map_exists && session_active && dkg_idle {
                    guard.log.push(format!(
                        "[CheckAndTriggerDkg-{}] All conditions met. Triggering DKG Round 1.",
                        peer_id_for_log
                    ));
                    if internal_cmd_tx_clone.send(InternalCommand::TriggerDkgRound1).is_ok() {
                        guard.dkg_state = DkgState::Round1InProgress; 
                    } else {
                        guard.log.push(format!(
                            "[CheckAndTriggerDkg-{}] Failed to send TriggerDkgRound1 command.",
                             peer_id_for_log
                        ));
                    }
                } else {
                    guard.log.push(format!(
                        "[CheckAndTriggerDkg-{}] Conditions not met. MeshReady: {}, IdentifiersMapped: {}, SessionActive: {}, DkgIdle: {}",
                        peer_id_for_log,
                        mesh_ready,
                        map_exists,
                        session_active,
                        dkg_idle
                    ));
                }
            });
        }
        InternalCommand::TriggerDkgRound1 => {
            let state_clone = state.clone();
            let self_peer_id_clone = self_peer_id.clone();
            tokio::spawn(async move {
                // Include more detailed log messages as shown in documentation
                state_clone.lock().await.log.push(
                    "DKG Round 1: Generating and sending commitments to all peers...".to_string(),
                );
                ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
            });
        }
        InternalCommand::ProcessDkgRound1 {
            from_peer_id,
            package,
        } => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone.lock().await.log.push(format!(
                    "DKG Round 1: Received commitment package from peer {}...",
                    from_peer_id
                ));
                ed25519_dkg::process_dkg_round1(state_clone, from_peer_id, package).await;
            });
        }
        InternalCommand::TriggerDkgRound2 => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone.lock().await.log.push(
                    "DKG Round 2: All commitments received. Generating and distributing key shares...".to_string()
                );
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
                    "DKG Round 2: Received key share from peer {}. Verifying against commitments...",
                    from_peer_id_clone
                ));
                ed25519_dkg::process_dkg_round2(state_clone, from_peer_id, package).await;
            });
        }
        InternalCommand::FinalizeDkg => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                state_clone.lock().await.log.push(
                    "DKG Finalization: Computing final key share and group public key..."
                        .to_string(), //: Computing final key share and group public key..."
                );
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
                                .push(format!("Relay from {}: {:?}", from, data.clone())); // Log data by cloning
    
                                match serde_json::from_value::<protocal::signal::WebSocketMessage>(data.clone()) {
                                    Ok(protocal::signal::WebSocketMessage::WebRTCSignal(signal)) => {
                                        handle_webrtc_signal(
                                            from,
                                            signal,
                                            state.clone(),
                                            self_peer_id.clone(),
                                            internal_cmd_tx.clone(),
                                            peer_connections_arc.clone(),
                                        )
                                        .await;
                                    }

                                    Ok(protocal::signal::WebSocketMessage::SessionProposal(proposal)) => {
                                        let mut state_guard = state.lock().await;
                                        state_guard.log.push(format!(
                                            "Received SessionProposal from {}: ID={}, Total={}, Threshold={}, Participants={:?}",
                                            from, proposal.session_id, proposal.total, proposal.threshold, proposal.participants
                                        ));
                                        
                                        let invite_info = SessionInfo {
                                            session_id: proposal.session_id.clone(),
                                            proposer_id: from.clone(), // Store the proposer's ID
                                            total: proposal.total,
                                            threshold: proposal.threshold,
                                            participants: proposal.participants.clone(),
                                            accepted_peers: Vec::new(), 
                                        };
                                        state_guard.invites.push(invite_info);
                                    }
                                    Ok(protocal::signal::WebSocketMessage::SessionResponse(response))    => {
                                        state.lock().await.log.push(format!("Received SessionResponse from {}: {:?}", from, response.clone()));
                                        // Convert to the internal SessionResponse type if needed
                                        let internal_response = SessionResponse {
                                            session_id: response.session_id.clone(),
                                            accepted: response.accepted,
                                        };
                                        if let Err(e) = internal_cmd_tx.send(InternalCommand::ProcessSessionResponse { from_peer_id: from.clone(), response: internal_response }) {
                                            state.lock().await.log.push(format!("Failed to send ProcessSessionResponse command: {}", e));
                                        }
                                    }
                                    Err(e) => {
                                        state.lock().await.log.push(format!("Error parsing WebSocketMessage: {}", e));
                                        state.lock().await.log.push(format!("Error parsing WebSocketMessage: {}", data.clone()));
                                    }
                                }
                            }}
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
        Message::Pong(_) => {}
        Message::Binary(_) => {
            state
                .lock()
                .await
                .log
                .push("Received unexpected binary message.".to_string());
        }
        Message::Frame(_) => {}
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
    let pc_arc_net_clone = peer_connections_arc.clone(); // Use passed parameter
    let signal_clone = signal.clone();
    tokio::spawn(async move {
        // Get or create peer connection
        let pc_to_use_result = {
            let peer_conns_guard = pc_arc_net_clone.lock().await; // Guard for modification
            match peer_conns_guard.get(&from_clone).cloned() {
                Some(pc) => Ok(pc),
                None => {
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
                        offer_info, // Pass offer_info here
                        pc_clone,
                        state_log_clone.clone(),
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

async fn handle_webrtc_offer(
    from_peer_id: &str,
    offer_info: SDPInfo, // Add offer_info parameter
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {    
    let pc_to_use = pc.clone(); // Start with the assumption we use the passed pc

    match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::offer(
        offer_info.sdp, // Use offer_info.sdp here
    ) {
        Ok(offer_sdp) => {
            if let Err(e) = pc_to_use.set_remote_description(offer_sdp).await {
                state.lock().await.log.push(format!(
                    "Offer from {}: Error setting remote description (offer): {}",
                    from_peer_id, e
                ));
                return;
            }
            state.lock().await.log.push(format!(
                "Offer from {}: Remote description (offer) set successfully.",
                from_peer_id
            ));

            // Apply any pending candidates that might have arrived before the offer was set
            apply_pending_candidates(from_peer_id, pc_to_use.clone(), state.clone()).await;

            // Create Answer
            match pc_to_use.create_answer(None).await {
                Ok(answer) => {
                    // Clone answer for set_local_description and for sending
                    let answer_to_send = answer.clone();
                    if let Err(e) = pc_to_use.set_local_description(answer).await {
                        state.lock().await.log.push(format!(
                            "Offer from {}: Error setting local description (answer): {}",
                            from_peer_id, e
                        ));
                        return;
                    }
                    state.lock().await.log.push(format!(
                        "Offer from {}: Local description (answer) created and set.",
                        from_peer_id
                    ));

                    let answer_info_to_send = SDPInfo {
                        sdp: answer_to_send.sdp,                    
                    };
                    // Wrap the WebRTCSignal in WebSocketMessage
                    let webrtc_signal = WebRTCSignal::Answer(answer_info_to_send);
                    let websocket_message = WebSocketMessage::WebRTCSignal(webrtc_signal);

                    match serde_json::to_value(websocket_message) { // Serialize the WebSocketMessage
                        Ok(json_val) => {
                            let relay_msg = SharedClientMsg::Relay {
                                to: from_peer_id.to_string(),
                                data: json_val,
                            };
                            if let Err(e) = internal_cmd_tx.send(InternalCommand::SendToServer(relay_msg)) {
                                state.lock().await.log.push(format!(
                                    "Offer from {}: Failed to send answer to server: {}",
                                    from_peer_id, e
                                ));
                            } else {
                                state.lock().await.log.push(format!(
                                    "Offer from {}: Answer sent to {}",
                                    from_peer_id, from_peer_id
                                ));
                            }
                        }
                        Err(e) => {
                            state.lock().await.log.push(format!(
                                "Offer from {}: Error serializing answer: {}",
                                from_peer_id, e
                            ));
                        }
                    }
                }
                Err(e) => {
                    state.lock().await.log.push(format!(
                        "Offer from {}: Error creating answer: {}",
                        from_peer_id, e
                    ));
                }
            }
        }
        Err(e) => {
            state.lock().await.log.push(format!(
                "Offer from {}: Error parsing offer SDP: {}",
                from_peer_id, e
            ));
        }
    }
}


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
    candidate_info: CandidateInfo,
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
) {
    state
        .lock()
        .await
        .log //it
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
        | webrtc::peer_connection::signaling_state::RTCSignalingState::Stable => true, //wer
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


async fn check_and_send_mesh_ready( //all data channels are open and send mesh_ready if needed
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    let mut all_channels_open_debug = false;
    let mut already_sent_own_ready_debug = false;
    let mut session_exists_debug = false;
    let mut participants_to_check_debug: Vec<String> = Vec::new();
    let mut data_channels_keys_debug: Vec<String> = Vec::new();
    let self_peer_id_debug: String;
    let current_mesh_status_debug: MeshStatus;

    {
        let state_guard = state.lock().await;
        self_peer_id_debug = state_guard.peer_id.clone();
        current_mesh_status_debug = state_guard.mesh_status.clone(); // Clone for logging
        if let Some(session) = &state_guard.session {
            session_exists_debug = true;
            participants_to_check_debug = session
                .participants
                .iter()
                .filter(|p| **p != state_guard.peer_id)
                .cloned()
                .collect();
            
            data_channels_keys_debug = state_guard.data_channels.keys().cloned().collect();

            all_channels_open_debug = participants_to_check_debug
                .iter()
                .all(|p| state_guard.data_channels.contains_key(p));
            
            if matches!(state_guard.mesh_status, MeshStatus::SentSelfReady | MeshStatus::Ready) {
                already_sent_own_ready_debug = true;
            }
        }
    } // state_guard is dropped

    // Log outside the lock to minimize lock holding time
    let mut log_guard = state.lock().await;
    log_guard.log.push(format!(
        "[MeshCheck-{}] Status: {:?}, SessionExists: {}, ParticipantsToCheck: {:?}, OpenDCKeys: {:?}, AllOpenCalc: {}, AlreadySentCalc: {}",
        self_peer_id_debug,
        current_mesh_status_debug, // Log current status
        session_exists_debug,
        participants_to_check_debug,
        data_channels_keys_debug,
        all_channels_open_debug,
        already_sent_own_ready_debug
    ));
    drop(log_guard);


    if session_exists_debug && all_channels_open_debug && !already_sent_own_ready_debug {
        // Re-acquire lock for the specific log message and subsequent command sending
        state 
            .lock()
            .await
            .log
            .push(format!("[MeshCheck-{}] All local data channels open! Signaling to process own mesh readiness...", self_peer_id_debug));
        
        if let Err(e) = cmd_tx.send(InternalCommand::SendOwnMeshReadySignal) {
            // Clone necessary items for the async logging task
            let state_clone_for_err = state.clone(); 
            let self_peer_id_err_clone = self_peer_id_debug.clone();
            tokio::spawn(async move { 
                state_clone_for_err
                    .lock()
                    .await
                    .log
                    .push(format!("[MeshCheck-{}] Failed to send SendOwnMeshReadySignal command: {}", self_peer_id_err_clone, e));
            });   
        }
    } else {
        // Log reason for not sending, re-acquiring lock briefly
        let mut final_log_guard = state.lock().await;
        if !session_exists_debug {
            final_log_guard.log.push(format!("[MeshCheck-{}] No active session, cannot send SendOwnMeshReadySignal.", self_peer_id_debug));
        } else if !all_channels_open_debug {
            final_log_guard.log.push(format!("[MeshCheck-{}] Not all channels open yet (expected {:?}, have {:?}), cannot send SendOwnMeshReadySignal.", self_peer_id_debug, participants_to_check_debug, data_channels_keys_debug));
        } else if already_sent_own_ready_debug {
            final_log_guard.log.push(format!("[MeshCheck-{}] Already sent own ready signal (Status: {:?}), not sending again.", self_peer_id_debug, current_mesh_status_debug));
        }
    }
}

async fn initiate_webrtc_connections( //bRTC connections with all session participants
    participants: Vec<String>,
    self_peer_id: String,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) {
    let peer_connections_arc = state.lock().await.peer_connections.clone();

    // Step 1: Ensure RTCPeerConnection objects exist for all other participants.
    // This is necessary for both sending and receiving offers.
    for peer_id_str in participants.iter().filter(|p| **p != self_peer_id) {
        let needs_creation;
        { // Scope for the lock guard
            let peer_conns_guard = peer_connections_arc.lock().await;
            needs_creation = !peer_conns_guard.contains_key(peer_id_str);
        }

        if needs_creation {
            match create_and_setup_peer_connection(
                peer_id_str.clone(),
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
                        .push(format!("Created peer connection for {}", peer_id_str));
                }
                Err(e) => {
                    state.lock().await.log.push(format!(
                        "Error creating peer connection for {}: {}",
                        peer_id_str, e
                    ));
                }
            }
        }
    }

    // Step 2: Filter participants to whom self should make an offer (politeness rule).
    let peers_to_offer_to: Vec<String> = participants
        .iter()
        .filter(|p_id| **p_id != self_peer_id && *self_peer_id < ***p_id) // Offer if self_peer_id is smaller
        .cloned()
        .collect();

    if !peers_to_offer_to.is_empty() {
        state.lock().await.log.push(format!(
            "Initiating offers to peers based on ID ordering: {:?}",
            peers_to_offer_to
        ));
        initiate_offers_for_session(
            peers_to_offer_to, // Pass only the filtered list of peers to offer to
            self_peer_id.clone(),
            peer_connections_arc.clone(),
            internal_cmd_tx.clone(),
            state.clone(),
        )
        .await;
    } else {
        state.lock().await.log.push(
            "No peers to initiate offers to based on ID ordering. Waiting for incoming offers.".to_string()
        );
    }
}

