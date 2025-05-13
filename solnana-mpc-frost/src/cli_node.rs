mod protocal;
use protocal::signal::WebRTCMessage; // Updated path
use protocal::signal::WebSocketMessage;  // Updated path
use protocal::signal::SessionResponse;

use protocal::dkg;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use frost_ed25519::Ed25519Sha512;
use frost_secp256k1::Secp256K1Sha256;
use futures_util::{SinkExt, StreamExt};

use ratatui::{Terminal, backend::CrosstermBackend};
// Remove unused import: utils::peer

use std::collections::{BTreeMap, HashSet}; // Add HashSet import

// Import from lib.rs
use utils::state::{DkgState, InternalCommand, MeshStatus}; // <-- Add SessionResponse here

use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, io};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use webrtc_signal_server::ClientMsg;
// Add display-related imports for better status handling
use frost_core::{
    Ciphersuite, Identifier,
};

mod utils;
// --- Use items from modules ---
// Updated path for all items from protocal::signal
use protocal::signal::{
 SessionInfo, SessionProposal,
};

use utils::peer::send_webrtc_message;
use utils::state::{AppState, ReconnectionTracker}; // Remove DkgState import

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};

mod network;
use clap::{Parser, ValueEnum};
use network::websocket::handle_websocket_message;
use network::webrtc::{
    initiate_webrtc_connections,
};

#[derive(Clone, Debug, ValueEnum)]
enum Curve {
    Secp256k1,
    Ed25519,
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Peer ID for this node
    #[arg(short, long)]
    peer_id: String,

    /// Curve to use for cryptographic operations
    #[arg(short, long, value_enum, default_value_t = Curve::Secp256k1)]
    curve: Curve,

    #[arg(short, long, default_value = "wss://auto-life.tech")]
    webrtc: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    // Connect to signaling server
    let ws_url = args.webrtc.clone();
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut ws_sink, ws_stream) = ws_stream.split();

    // Register (Send directly, no channel needed for initial message)
    let register_msg = ClientMsg::Register {
        peer_id: args.peer_id.clone(),
    };
    ws_sink
        .send(Message::Text(serde_json::to_string(&register_msg)?.into()))
        .await?;

    match args.curve {
        Curve::Secp256k1 => run_dkg::<Secp256K1Sha256>(args.peer_id, ws_sink, ws_stream).await?,
        Curve::Ed25519 => run_dkg::<Ed25519Sha512>(args.peer_id, ws_sink, ws_stream).await?,
    };
    Ok(())
}

async fn run_dkg<C>(peer_id: String, mut ws_sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream< 
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >, mut ws_stream: futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream< 
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >) -> anyhow::Result<()>
where 
    C: Ciphersuite + Send + Sync + 'static,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    // Channel for INTERNAL commands within the CLI app (uses InternalCommand from lib.rs)
    let (internal_cmd_tx, mut internal_cmd_rx) = mpsc::unbounded_channel::<InternalCommand<C>>();

    // Shared state for TUI and networkthin the CLI app (uses InternalCommand from lib.rs)
    // Specify the Ciphersuite for AppStaterx) = mpsc::unbounded_channel::<InternalCommand>();
    let state = Arc::new(Mutex::new(AppState {
        peer_id: peer_id.clone(),
        peers: Vec::new(),
        log: Vec::new(),
        log_scroll: 0,
        session: None,
        invites: Vec::new(),
        peer_connections: Arc::new(Mutex::new(HashMap::new())), // Use TokioMutex here
        peer_statuses: HashMap::new(),                          // Initialize peer statuses
        reconnection_tracker: ReconnectionTracker::new(),
        making_offer: HashMap::new(),//tex::new(HashMap::new())), // Use TokioMutex here
        pending_ice_candidates: HashMap::new(),//er::new(),
        dkg_state: DkgState::Idle,//(),
        identifier_map: None, //::new(),
        dkg_part1_public_package: None,
        dkg_part1_secret_package: None,
        received_dkg_packages: BTreeMap::new(),
        key_package: None,//ne,
        group_public_key: None,//one,
        data_channels: HashMap::new(),//p::new(),
        solana_public_key: None,
        etherum_public_key: None,
        round2_secret_package: None,//),
        received_dkg_round2_packages: BTreeMap::new(), // Initialize new field
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

async fn handle_internal_command<C>(
    cmd: InternalCommand<C>, //kend_mut(), LeaveAlternateScreen)?;
    state: Arc<Mutex<AppState<C>>>,
    self_peer_id: String,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>, // Fix: add parameter here
    ws_sink: &mut futures_util::stream::SplitSink< // Add ws_sink parameter
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
) where
C: Ciphersuite + Send + Sync + 'static, 
<<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync, 
<<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,     
{
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
                            let relay_msg = ClientMsg::Relay {
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
                                    let relay_msg = ClientMsg::Relay {
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
                        
                        let mut current_ready_peers = match &state_guard.mesh_status {
                            MeshStatus::PartiallyReady { ready_peers, .. } => ready_peers.clone(),
                            _ => HashSet::new(),
                        };
                        
                        current_ready_peers.insert(self_peer_id_local.clone());

                        state_guard.log.push(format!(
                            "Local node is mesh ready. Sending MeshReady signal to peers. Current ready peers count: {}",
                            current_ready_peers.len()
                        ));

                        if current_ready_peers.len() == session_participants_count {
                            state_guard.mesh_status = MeshStatus::Ready;
                            mesh_became_ready = true; // Mark that mesh became ready
                            state_guard.log.push("All peers (including self) are mesh ready. Overall MeshStatus: Ready.".to_string());
                        } else {
                            state_guard.mesh_status = MeshStatus::PartiallyReady {
                                ready_peers: current_ready_peers.clone(),
                                total_peers: session_participants_count,
                            };
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
                    
                    if let Some(session) = &state_guard.session { 
                        let total_session_participants = session.participants.len();
                        
                        let mut current_ready_peers = match &state_guard.mesh_status {
                            MeshStatus::PartiallyReady { ready_peers, .. } => ready_peers.clone(),
                            MeshStatus::Ready => { // If already Ready, it implies all peers are known
                                // We might receive a late/duplicate MeshReady, log it but don't change state from Ready
                                log_messages.push(format!("Received MeshReady from {} but mesh is already Ready. Current ready peers: all {}.", peer_id, total_session_participants));
                                final_mesh_status_after_processing = state_guard.mesh_status.clone();
                                // Push logs and return early
                                drop(state_guard);
                                let mut log_guard_early = state_clone.lock().await;
                                for msg_item in log_messages { log_guard_early.log.push(msg_item); }
                                return;
                            },
                            _ => HashSet::new(), // Incomplete or other states
                        };

                        let already_known = current_ready_peers.contains(&peer_id);
                        if !already_known {
                            current_ready_peers.insert(peer_id.clone());
                            log_messages.push(format!("Processing MeshReady from peer: {}. Added to ready set.", peer_id));
                        } else {
                            log_messages.push(format!("Received duplicate MeshReady from {}. Not changing ready set.", peer_id));
                        }
                        
                        log_messages.push(format!(
                            "Mesh readiness update: {} peers now in ready set. Total session participants: {}.",
                            current_ready_peers.len(), total_session_participants
                        ));

                        if current_ready_peers.len() == total_session_participants {
                            state_guard.mesh_status = MeshStatus::Ready;
                            mesh_became_ready = true;
                            log_messages.push(format!(
                                "All {} participants are mesh ready. Overall MeshStatus set to: Ready.",
                                total_session_participants
                            ));
                        } else {
                            state_guard.mesh_status = MeshStatus::PartiallyReady {
                                ready_peers: current_ready_peers.clone(),
                                total_peers: total_session_participants,
                            };
                             log_messages.push(format!(
                                "Mesh status updated to PartiallyReady ({}/{} participants known ready).",
                                current_ready_peers.len(), 
                                total_session_participants
                            ));
                        }
                        final_mesh_status_after_processing = state_guard.mesh_status.clone();
                    } else {
                        log_messages.push(format!( 
                            "Received MeshReady from {} but no active session.", peer_id
                        )); 
                        // Optionally set to Incomplete if desired, or leave as is
                        // state_guard.mesh_status = MeshStatus::Incomplete;
                        final_mesh_status_after_processing = state_guard.mesh_status.clone();
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
                dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
            });
        }
        InternalCommand::TriggerDkgRound2 => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                match dkg::handle_trigger_dkg_round2(state_clone.clone()).await {
                    Ok(_) => {
                        state_clone.lock().await.log.push("Successfully completed handle_trigger_dkg_round2".to_string());
                    },
                    Err(e) => {
                        state_clone.lock().await.log.push(format!(
                            "Error in handle_trigger_dkg_round2: {}", e
                        ));
                    }
                }
            });
        }
        InternalCommand::ProcessDkgRound1 {
            from_peer_id,
            package,
        } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone(); 
            tokio::spawn(async move {
                let mut guard = state_clone.lock().await;
                guard.log.push(format!(
                    "Processing DKG Round 1 package from {}",
                    from_peer_id
                ));

                // Fix borrow error by cloning the state
                let current_dkg_state = guard.dkg_state.clone();
                if current_dkg_state != DkgState::Round1InProgress {
                    guard.log.push(format!(
                        "Error: Received DKG Round 1 package from {} but DKG state is {:?}, not Round1InProgress.",
                        from_peer_id, current_dkg_state
                    ));
                    return;
                }

                // Release the lock before calling the processing function
                drop(guard);

                // Call process_dkg_round1 without matching on its return
                dkg::process_dkg_round1(
                    state_clone.clone(), 
                    from_peer_id.clone(),
                    package,
                )
                .await;

                // After processing, check if all packages are received
                let mut guard = state_clone.lock().await;
                let all_packages_received = if let Some(session) = &guard.session {
                    // Compare count of received packages to participants count
                    guard.received_dkg_packages.len() == session.participants.len()
                } else {
                    false
                };

                if all_packages_received {
                    guard.log.push(
                        "All DKG Round 1 packages received. Setting state to Round1Complete and triggering DKG Round 2."
                            .to_string(),
                    );
                    guard.dkg_state = DkgState::Round1Complete;
                    // Drop guard before sending command to avoid potential deadlock
                    drop(guard); 
                    if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::TriggerDkgRound2) {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send TriggerDkgRound2 command: {}",
                            e
                        ));
                    }
                } else {
                    guard.log.push(format!(
                        "DKG Round 1: After processing package from {}, still waiting for more packages.",
                        from_peer_id
                    ));
                }
            });
        }
        InternalCommand::ProcessDkgRound2 {
            from_peer_id, 
            package, 
        } => {
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            tokio::spawn(async move {
                let mut guard = state_clone.lock().await;
                guard.log.push(format!(
                    "Processing DKG Round 2 package from {}",
                    from_peer_id
                ));
                drop(guard);

                // Call process_dkg_round2 with explicit error handling
                match dkg::process_dkg_round2(
                    state_clone.clone(), 
                    from_peer_id.clone(),
                    package,
                ).await {
                    Ok(_) => {
                        state_clone.lock().await.log.push(format!(
                            "DKG Round 2: Successfully processed package from {}",
                            from_peer_id
                        ));
                    },
                    Err(e) => {
                        state_clone.lock().await.log.push(format!(
                            "DKG Round 2: Error processing package from {}: {}",
                            from_peer_id, e
                        ));
                        return; // Exit early on error
                    }
                }

                // After processing, check if all round 2 packages are received
                let mut guard = state_clone.lock().await;
                
                // Debug log to verify the round2 packages are being stored correctly
                // Fix borrow checker errors by extracting values before formatting
                let package_count = guard.received_dkg_round2_packages.len();
                let package_keys = guard.received_dkg_round2_packages.keys().collect::<Vec<_>>();
                let self_identifier = guard.identifier_map.as_ref()
                    .and_then(|map| map.get(&guard.peer_id)).cloned();
                
                // Create the log message first, completing the immutable borrow
                let log_message = format!(
                    "DKG Round 2: Current received_dkg_round2_packages count: {}, keys: {:?}, self identifier: {:?}",
                    package_count,
                    package_keys,
                    self_identifier
                );
                // Now perform the mutable borrow
                guard.log.push(log_message);
                
                // Check if own package is included in the count (it should be)
                let own_package_counted = if let Some(self_id) = self_identifier {
                    guard.received_dkg_round2_packages.contains_key(&self_id)
                } else {
                    false
                };
                
                guard.log.push(format!(
                    "DKG Round 2: Own package in received map: {}", own_package_counted
                ));
                
                let all_packages_received = if let Some(session) = &guard.session {
                    // Compare count of received round 2 packages to participants count
                    let current_count = guard.received_dkg_round2_packages.len();
                    let expected_count = session.participants.len() - 1;
                    let result = current_count == expected_count;
                    
                    guard.log.push(format!(
                        "DKG Round 2: Checking completion: {}/{} packages received",
                        current_count, expected_count
                    ));
                    
                    result
                } else {
                    guard.log.push("DKG Round 2: No active session found when checking for completion".to_string());
                    false
                };

                if all_packages_received {
                    guard.log.push(
                        "All DKG Round 2 packages received. Setting state to Round2Complete and triggering FinalizeDkg."
                            .to_string(),
                    );
                    guard.dkg_state = DkgState::Round2Complete;
                    drop(guard); 
                    
                    // Add more explicit logging around the FinalizeDkg command
                    state_clone.lock().await.log.push("Sending FinalizeDkg command now...".to_string());
                    
                    if let Err(e) = internal_cmd_tx_clone.send(InternalCommand::FinalizeDkg) {
                        state_clone.lock().await.log.push(format!(
                            "Failed to send FinalizeDkg command: {}",
                            e
                        ));
                    } else {
                        state_clone.lock().await.log.push("Successfully sent FinalizeDkg command".to_string());
                    }
                } else {
                    guard.log.push(format!(
                        "DKG Round 2: After processing package from {}, still waiting for more packages.",
                        from_peer_id
                    ));
                }
            });
        }
        InternalCommand::FinalizeDkg => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                let mut guard = state_clone.lock().await;
                guard.log.push("FinalizeDkg: Processing command.".to_string());

                // Clone dkg_state before using it in the format string
                let current_dkg_state = guard.dkg_state.clone();
                if current_dkg_state != DkgState::Round2Complete {
                    guard.log.push(format!(
                        "Error: Triggered FinalizeDkg but DKG state is {:?}, not Round2Complete.",
                        current_dkg_state
                    ));
                    // Optionally set to Failed or handle as appropriate
                    return;
                }
                
                guard.dkg_state = DkgState::Finalizing; // Mark as finalizing
                
                // Add enhanced logging with more detail about the state
                guard.log.push(format!(
                    "FinalizeDkg: All prerequisites met. Current state: {:?}. Moving to Finalizing state.",
                    current_dkg_state
                ));
                
                // Fix: Extract the package count before using it in format!
                let package_count = guard.received_dkg_round2_packages.len();
                guard.log.push(format!(
                    "FinalizeDkg: Preparing to finalize with {} round2 packages", 
                    package_count
                ));
                
                // Drop guard before potentially long-running async operation
                drop(guard);

                // More explicit logging
                state_clone.lock().await.log.push("FinalizeDkg: Calling ed25519_dkg::handle_finalize_dkg function...".to_string());

                // Fix: Don't try to match on the return value since it's not a Result
                dkg::handle_finalize_dkg(state_clone.clone()).await;
                
                // Add logging after the function call
                state_clone.lock().await.log.push(
                    "FinalizeDkg: Completed DKG finalization process".to_string()
                );

                // Log final status after handle_finalize_dkg has updated the state
                let mut final_guard = state_clone.lock().await;
                let final_dkg_state = final_guard.dkg_state.clone();
                final_guard.log.push(format!(
                    "FinalizeDkg: Completion attempt finished. DKG state is now: {:?}",
                    final_dkg_state
                ));
            });
        }
    }
}



async fn check_and_send_mesh_ready<C>( //all data channels are open and send mesh_ready if needed
   state: Arc<Mutex<AppState<C>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand<C>>,
) where C: Ciphersuite + Send + Sync + 'static, 
<<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync, 
<<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,     
{
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
            
            if matches!(state_guard.mesh_status, MeshStatus::PartiallyReady { .. } | MeshStatus::Ready) {
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
