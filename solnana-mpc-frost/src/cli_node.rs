mod protocal;
use protocal::signal::WebRTCMessage; // Updated path
use protocal::signal::WebRTCSignal;  // Updated path
use protocal::signal::SessionResponse;

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
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
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
        ignore_offer: HashMap::new(),//,                          // Initialize peer statuses
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
        peer_connected_history: HashMap::new(), // peer_id -> Vec<RTCPeerConnectionState>
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
                        match ws_sink.send(Message::Text(msg_str.into())).await { // Added .into()
                            Ok(_) => {
                                state.lock().await.log.push(format!(
                                    "Successfully sent to server: {:?}", shared_msg
                                )); //er(shared_msg) => {
                            }, //WebSocket sink instead of creating a new connection
                            Err(e) => { //json::to_string(&shared_msg) {
                                state.lock().await.log.push(format!(
                                    "Failed to send to server: {:?} - Error: {}", shared_msg, e
                                ));
                            } //.lock().await.log.push(format!(
                        }   //"Successfully sent to server: {:?}", shared_msg
                    } else {
                        state.lock().await.log.push(format!(
                            "Failed to serialize message: {:?}", shared_msg
                        )); //te.lock().await.log.push(format!(
                    }       //"Failed to send to server: {:?} - Error: {}", shared_msg, e
                //},      //));
                //Err(e) => { // Duplicate removed
                //    state.lock().await.log.push(format!(
                //        "Failed to connect to WebSocket server for sending: {}", e
                //    )); //ock().await.log.push(format!(
                //}   //"Failed to serialize message: {:?}", shared_msg
            //}   //));
        }   //} // Duplicate removed
        InternalCommand::SendDirect { to, message } => {
            let state_clone = state.clone(); //ssage } =>:
            tokio::spawn(async move { //lone();
                let webrtc_msg = WebRTCMessage::SimpleMessage { text: message };
                if let Err(e) = send_webrtc_message(&to, &webrtc_msg, state_clone.clone()).await {
                    //state_clone send_webrtc_message(&to, &webrtc_msg, state_clone.clone()).await { // Duplicate removed
                        state_clone.lock()
                        .await
                        .log //it
                        .push(format!("Error sending direct message to {}: {}", to, e));
                } else { //.push(format!("Error sending direct message to {}: {}", to, e));
                    state_clone
                        .lock()
                        .await
                        .log //it
                        .push(format!("Sent direct message to {}", to));
                }       //.push(format!("Sent direct message to {}", to));
            }); //} // Duplicate removed
        }   //}); // Duplicate removed
        InternalCommand::ProposeSession {
            session_id, //::ProposeSession {
            total, //n_id,
            threshold,
            participants,
        } => { //rticipants,
            let state_clone = state.clone();
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            let self_peer_id_clone = self_peer_id.clone(); //one();
            tokio::spawn(async move { //self_peer_id.clone();
                let mut state_guard = state_clone.lock().await;
                // Create the session proposalone.lock().await;
                let session_proposal = SessionProposal {
                    session_id: session_id.clone(), //sal {
                    total, //n_id: session_id.clone(),
                    threshold,
                    participants: participants.clone(),
                };  //participants: participants.clone(),
                // Log the action
                state_guard.log.push(format!(
                    "Proposing session '{}' with {} participants and threshold {}",
                    session_id, total, threshold // {} participants and threshold {}",
                )); // session_id, total, threshold
                // Update local session state immediately for proposer
                state_guard.session = Some(SessionInfo { //y for proposer
                    session_id: session_id.clone(), //nfo {
                    total, //n_id: session_id.clone(),
                    threshold,
                    participants: participants.clone(),
                    accepted_peers: vec![state_guard.peer_id.clone()],
                }); // accepted_peers: vec![state_guard.peer_id.clone()],
                // Clone peer_id before the loop to avoid borrow checker issues
                let local_peer_id_for_filter = state_guard.peer_id.clone(); //sues
                // Broadcast proposal to all participants except selfone();
                for peer in participants // all participants except self
                    .iter() //participants
                    .filter(|p| **p != local_peer_id_for_filter)
                {   //.filter(|p| **p != local_peer_id_for_filter) // Duplicate removed
                    let proposal_msg = WebRTCMessage::SessionProposal(session_proposal.clone());
                    match serde_json::to_value(proposal_msg) { //roposal(session_proposal.clone());
                        Ok(json_val) => { //value(proposal_msg) {
                            let relay_msg = SharedClientMsg::Relay {
                                to: peer.clone(), //dClientMsg::Relay {
                                data: json_val, //),
                            };  //data: json_val,
                            // Send via relay since WebRTC might not be established yet
                            if let Err(e) = //ay since WebRTC might not be established yet
                                internal_cmd_tx_clone.send(InternalCommand::SendToServer(relay_msg))
                            {   //.send(InternalCommand::SendToServer(relay_msg)) // Duplicate removed
                                state_guard.log.push(format!(
                                    "Failed to send session proposal to {}: {}",
                                    peer, e // to send session proposal to {}: {}",
                                )); // peer, e
                            } else {
                                state_guard
                                    .log //ard
                                    .push(format!("Sent session proposal to {}", peer));
                            }       //.push(format!("Sent session proposal to {}", peer));
                        }   //} // Duplicate removed
                        Err(e) => {
                            state_guard.log.push(format!(
                                "Error serializing session proposal for {}: {}",
                                peer, e //serializing session proposal for {}: {}",
                            )); // peer, e
                        }   //)); // Duplicate removed
                    }   //} // Duplicate removed
                }   //} // Duplicate removed
                drop(state_guard); // Release lock before async operation
                // Begin WebRTC connection process if not already connected
                initiate_webrtc_connections( //rocess if not already connected
                    participants, //onnections(
                    self_peer_id_clone,
                    state_clone.clone(),
                    internal_cmd_tx_clone,
                )   //internal_cmd_tx_clone, // Duplicate removed
                .await;
            }); //.await; // Duplicate removed
        }   //}); // Duplicate removed
        InternalCommand::AcceptSessionProposal(session_id) => {
            let state_clone = state.clone(); //al(session_id) =>:
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            //let internal_cmd_tx_clone = internal_cmd_tx.clone(); // Duplicate removed
            tokio::spawn(async move {
                // Create a clone of the peer_id before locking state
                let mut state_guard = state_clone.lock().await; // state
                //let mut state_guard = state_clone.lock().await; // Duplicate removed
                // Find the invite
                if let Some(invite_idx) = state_guard
                    .invites //invite_idx) = state_guard // Duplicate removed
                    .iter() //s
                    .position(|i| i.session_id == session_id)
                {   //.position(|i| i.session_id == session_id) // Duplicate removed
                    let invite = state_guard.invites.remove(invite_idx);
                    // Log acceptancee_guard.invites.remove(invite_idx); // Duplicate removed
                    state_guard //ptance
                        .log //ard
                        .push(format!("You accepted session proposal '{}'", session_id));
                        //.push(format!("You accepted session proposal '{}'", session_id)); // Duplicate removed
                    // Update local session state
                    state_guard.session = Some(SessionInfo {
                        session_id: invite.session_id.clone(),
                        total: invite.total, //ession_id.clone(),
                        threshold: invite.threshold,
                        participants: invite.participants.clone(),
                        accepted_peers: vec![state_guard.peer_id.clone()],
                    }); // accepted_peers: vec![state_guard.peer_id.clone()],
                    //}); // Duplicate removed
                    // Get the participants we need to notify
                    let participants_to_notify: Vec<String> = invite
                        .participants //to_notify: Vec<String> = invite // Duplicate removed
                        .iter() //ipants
                        .filter(|p| **p != state_guard.peer_id)
                        .cloned() //p| **p != state_guard.peer_id) // Duplicate removed
                        .collect();
                        //.collect(); // Duplicate removed
                    // Prepare the response once
                    let response_payload = SessionResponse { // Renamed to avoid conflict if SessionResponse is a type name
                        session_id: invite.session_id.clone(),
                        accepted: true, //ite.session_id.clone(),
                    };  //accepted: true,
                    //}; // Duplicate removed
                    // Store participants for WebRTC connection later
                    let participants = invite.participants.clone(); //er
                    let peer_id_for_webrtc = state_guard.peer_id.clone();
                    //let peer_id_for_webrtc = state_guard.peer_id.clone(); // Duplicate removed
                    // Release lock before notification loop
                    drop(state_guard); //fore notification loop
                    //drop(state_guard); // Duplicate removed
                    // Notify other participants of acceptance without holding lock
                    for peer in participants_to_notify { //ptance without holding lock
                        let response_msg = WebRTCMessage::SessionResponse(response_payload.clone()); // Use cloned payload
                        match serde_json::to_value(response_msg) { //esponse(response.clone());
                            Ok(json_val) => { //value(response_msg) {
                                let relay_msg = SharedClientMsg::Relay {
                                    to: peer.clone(), //dClientMsg::Relay {
                                    data: json_val, //),
                                };  //data: json_val,
                                // Send via channel, handle error separately to avoid borrow conflict
                                if let Err(e) = internal_cmd_tx_clone //arately to avoid borrow conflict
                                    .send(InternalCommand::SendToServer(relay_msg))
                                {   //.send(InternalCommand::SendToServer(relay_msg)) // Duplicate removed
                                    // Re-acquire lock just for logging
                                    let mut guard = state_clone.lock().await;
                                    guard.log.push(format!( //lone.lock().await;
                                        "Failed to send acceptance to {}: {}",
                                        peer, e // to send acceptance to {}: {}",
                                    )); // peer, e
                                } else {
                                    // Re-acquire lock just for logging
                                    let mut guard = state_clone.lock().await;
                                    guard.log.push(format!("Sent acceptance to {}", peer));
                                }   //guard.log.push(format!("Sent acceptance to {}", peer)); // Duplicate removed
                            }   //} // Duplicate removed
                            Err(e) => {
                                // Re-acquire lock just for logging
                                let mut guard = state_clone.lock().await;
                                guard.log.push(format!( //lone.lock().await;
                                    "Error serializing acceptance for {}: {}",
                                    peer, e //serializing acceptance for {}: {}",
                                )); // peer, e
                            }   //)); // Duplicate removed
                        }   //} // Duplicate removed
                    }   //} // Duplicate removed
                    //} // Duplicate removed
                    // Begin WebRTC connection process
                    initiate_webrtc_connections( //rocess
                        participants, //onnections(
                        peer_id_for_webrtc,
                        state_clone.clone(),
                        internal_cmd_tx_clone,
                    )   //internal_cmd_tx_clone, // Duplicate removed
                    .await;
                } else { //it;
                    state_guard.log.push(format!(
                        "No pending invite found for session '{}'",
                        session_id //g invite found for session '{}'",
                    ));
                }
            });
        } 
        InternalCommand::ProcessSessionResponse { from_peer_id, response } => {
            let state_clone = state.clone();
            tokio::spawn(async move {
                let mut log_msgs = Vec::new();
                let mut session_cancelled = false;
                let mut session_id_to_cancel = None;
                let mut state_guard = state_clone.lock().await;
                log_msgs.push(format!(
                    "Received session response from {}: Accepted={}",
                    from_peer_id, response.accepted
                ));
                if response.accepted {
                    if let Some(session) = state_guard.session.as_mut() {
                        if session.session_id == response.session_id && !session.accepted_peers.contains(&from_peer_id) {
                            session.accepted_peers.push(from_peer_id.clone());
                            log_msgs.push(format!(
                                "Peer {} accepted session '{}'. Accepted peers: {}/{}",
                                from_peer_id, session.session_id, session.accepted_peers.len(), session.participants.len()
                            ));
                            // Optionally, trigger further actions if all participants have accepted
                        }
                    }
                } else {
                    log_msgs.push(format!(
                        "Peer {} rejected session '{}'.",
                        from_peer_id, response.session_id
                    ));
                    // Handle rejection, e.g., clear session or notify user
                    if let Some(session) = &state_guard.session {
                        if session.session_id == response.session_id {
                            session_cancelled = true;
                            session_id_to_cancel = Some(response.session_id.clone());
                        }
                    }
                }
                // End mutable borrow before pushing logs and mutating session
                drop(state_guard);
                if session_cancelled {
                    let mut guard = state_clone.lock().await;
                    guard.session = None;
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
            let state_clone = state.clone(); // peer_id } =>:
            let internal_cmd_tx_clone = internal_cmd_tx.clone();
            //let internal_cmd_tx_clone = internal_cmd_tx.clone(); // Duplicate removed
            tokio::spawn(async move {
                let self_peer_id_local = state_clone.lock().await.peer_id.clone();
                //let self_peer_id_local = state_clone.lock().await.peer_id.clone(); // Duplicate removed
                // Check if in a session
                if let Some(_session) = &state_clone.lock().await.session {
                    // Send channel_open notification to the peer.session { // Duplicate removed
                    let channel_open_msg = WebRTCMessage::ChannelOpen {
                        peer_id: self_peer_id_local.clone(), //annelOpen {
                    };
                    
                    if let Err(e) =
                        send_webrtc_message(&peer_id, &channel_open_msg, state_clone.clone()).await
                    {
                        state_clone
                            .lock()
                            .await
                            .log //it
                            .push(format!("Error sending channel_open to {}: {}", peer_id, e));
                    } else {
                        state_clone
                            .lock()
                            .await
                            .log //it
                            .push(format!("Sent channel_open to {}", peer_id));
                    }
                    let state_for_check = state_clone.clone(); //check_and_send_mesh_ready to avoid borrowing issues
                    check_and_send_mesh_ready(state_for_check, internal_cmd_tx_clone).await;
                }
            });
        }
        InternalCommand::SendOwnMeshReadySignal => { // Renamed from MeshReady
            let state_clone = state.clone();
            tokio::spawn(async move { 
                let self_peer_id_local;
                let session_id_local;
                let participants_local;
                let mut final_mesh_status_after_send_logic = MeshStatus::Incomplete; // Default
                
                { // Scope for state_guard
                    let mut state_guard = state_clone.lock().await;
                    self_peer_id_local = state_guard.peer_id.clone();

                    if let Some(session) = &state_guard.session {
                        session_id_local = session.session_id.clone();
                        participants_local = session.participants.clone();
                        let current_mesh_ready_peers_count = state_guard.mesh_ready_peers.len(); // Read before mutable borrow for log
                        let session_participants_count = session.participants.len(); // Read before mutable borrow for logic

                        state_guard.log.push(format!(
                            "Local node is mesh ready. Sending MeshReady signal to peers. Current mesh_ready_peers count: {}",
                            current_mesh_ready_peers_count
                        ));
                        state_guard.mesh_status = MeshStatus::SentSelfReady; // Mark that self has sent its signal

                        // Check if receiving MeshReady from all others already happened
                        let all_others_reported_mesh_ready = if session_participants_count > 0 {
                             current_mesh_ready_peers_count == (session_participants_count - 1)
                        } else {
                            false
                        };

                        if all_others_reported_mesh_ready {
                            state_guard.mesh_status = MeshStatus::Ready;
                            state_guard.log.push("All peers (including self) are mesh ready. Overall MeshStatus: Ready.".to_string());
                        }
                        final_mesh_status_after_send_logic = state_guard.mesh_status.clone();

                    } else {
                        state_guard.log.push("Tried to send own MeshReady signal, but no active session.".to_string());
                        return; // No session, nothing to do
                    }
                } // state_guard released

                // Now send the message
                let mesh_ready_msg = WebRTCMessage::MeshReady {
                    session_id: session_id_local.clone(), 
                    peer_id: self_peer_id_local.clone(), 
                };  
                
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
                // Log final status after sending
                 state_clone.lock().await.log.push(format!("Finished sending own MeshReady signals. Current local MeshStatus: {:?}", final_mesh_status_after_send_logic));
            }); 
        }   
        InternalCommand::ProcessMeshReady { peer_id } => {
            let state_clone = state.clone(); 
            // internal_cmd_tx_clone is not used here anymore for DKG trigger
            tokio::spawn(async move { 
                let mut final_mesh_status_after_processing = MeshStatus::Incomplete; // Default
                let mut log_messages = Vec::new();

                { // Scope for state_guard
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
                        
                        // If self has already sent its ready signal and now all others have also reported,
                        // then the overall mesh is Ready.
                        if state_guard.mesh_status == MeshStatus::SentSelfReady && all_others_reported_mesh_ready {
                            state_guard.mesh_status = MeshStatus::Ready;
                            log_messages.push(format!(
                                "All peers (including self) are mesh ready. Overall MeshStatus set to: Ready. ({} other peers ready, self was SentSelfReady)",
                                num_known_other_mesh_ready_peers
                            ));
                        } else if state_guard.mesh_status != MeshStatus::Ready { // Don't revert from Ready
                             // Update to PartiallyReady if not yet SentSelfReady or not all others reported yet
                            state_guard.mesh_status = MeshStatus::PartiallyReady(num_known_other_mesh_ready_peers + 1, total_session_participants);
                             log_messages.push(format!(
                                "Mesh status updated to PartiallyReady ({}/{} participants known ready). Self status: {:?}.",
                                num_known_other_mesh_ready_peers + 1, // +1 for self conceptually
                                total_session_participants,
                                state_guard.mesh_status // This will show PartiallyReady here
                            ));
                        }
                        final_mesh_status_after_processing = state_guard.mesh_status.clone();
                    } else {
                        log_messages.push(format!( 
                            "Received MeshReady from {} but no active session.", peer_id
                        )); 
                    }
                } // state_guard released

                // Log all messages
                let mut state_log_guard = state_clone.lock().await;
                for msg in log_messages {
                    state_log_guard.log.push(msg);
                }
                state_log_guard.log.push(format!("Finished processing MeshReady from {}. Current local MeshStatus: {:?}", peer_id, final_mesh_status_after_processing));
            }); 
        }   
        InternalCommand::TriggerDkgRound1 => {
            let state_clone = state.clone(); // {
            let self_peer_id_clone = self_peer_id.clone();
            tokio::spawn(async move { //self_peer_id.clone();
                // Include more detailed log messages as shown in documentation
                state_clone.lock().await.log.push( //ges as shown in documentation
                    "DKG Round 1: Generating and sending commitments to all peers...".to_string(),
                );  //"DKG Round 1: Generating and sending commitments to all peers...".to_string(), // Duplicate removed
                ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await;
            }); // ed25519_dkg::handle_trigger_dkg_round1(state_clone, self_peer_id_clone).await; // Duplicate removed
        }   //}); // Duplicate removed
        //} // Duplicate removed
        InternalCommand::ProcessDkgRound1 {
            from_peer_id, //ProcessDkgRound1 {
            package, //r_id,
        } => { //ckage,
            let state_clone = state.clone();
            tokio::spawn(async move { //lone();
                state_clone.lock().await.log.push(format!(
                    "DKG Round 1: Received commitment package from peer {}...",
                    from_peer_id //: Received commitment package from peer {}...",
                )); // from_peer_id
                ed25519_dkg::process_dkg_round1(state_clone, from_peer_id, package).await;
            }); // ed25519_dkg::process_dkg_round1(state_clone, from_peer_id, package).await; // Duplicate removed
        }   //}); // Duplicate removed
        //} // Duplicate removed
        InternalCommand::TriggerDkgRound2 => {
            let state_clone = state.clone(); // {
            tokio::spawn(async move { //lone();
                state_clone.lock().await.log.push(
                    "DKG Round 2: All commitments received. Generating and distributing key shares...".to_string()
                );  //"DKG Round 2: All commitments received. Generating and distributing key shares...".to_string() // Duplicate removed
                state_clone.lock().await.dkg_state = DkgState::SharesInProgress;
                ed25519_dkg::handle_trigger_dkg_round2(state_clone).await; //gress;
            }); // ed25519_dkg::handle_trigger_dkg_round2(state_clone).await; // Duplicate removed
        }   //}); // Duplicate removed
        //} // Duplicate removed
        InternalCommand::ProcessDkgRound2 {
            from_peer_id, //ProcessDkgRound2 {
            package, //r_id,
        } => { //ckage,
            let state_clone = state.clone();
            let from_peer_id_clone = from_peer_id.clone();
            tokio::spawn(async move { //from_peer_id.clone();
                state_clone.lock().await.log.push(format!(
                    "DKG Round 2: Received key share from peer {}. Verifying against commitments...",
                    from_peer_id_clone //ived key share from peer {}. Verifying against commitments...",
                )); // from_peer_id_clone
                state_clone.lock().await.dkg_state = DkgState::VerificationInProgress;
                ed25519_dkg::process_dkg_round2(state_clone, from_peer_id, package).await;
            }); // ed25519_dkg::process_dkg_round2(state_clone, from_peer_id, package).await; // Duplicate removed
        }   //}); // Duplicate removed
        //} // Duplicate removed
        InternalCommand::FinalizeDkg => {
            let state_clone = state.clone();
            tokio::spawn(async move { //lone();
                state_clone.lock().await.log.push(
                    "DKG Finalization: Computing final key share and group public key..."
                        .to_string(), //: Computing final key share and group public key..."
                );      //.to_string(), // Duplicate removed
                ed25519_dkg::finalize_dkg(state_clone).await;
            }); // ed25519_dkg::finalize_dkg(state_clone).await; // Duplicate removed
        }   //}); // Duplicate removed
    }   //} // Duplicate removed
}   //} // Duplicate removed
//} // Duplicate removed
/// Handler for WebSocket messages received from the server
async fn handle_websocket_message( // received from the server
    msg: Message, //ebsocket_message(
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String, //tate<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    ws_sink: &mut futures_util::stream::SplitSink< //, Arc<RTCPeerConnection>>>>, // Duplicate removed
        tokio_tungstenite::WebSocketStream< //itSink<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,  //tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>, // Duplicate removed
        Message,
    >,  //Message, // Duplicate removed
) { //>, // Duplicate removed
    match msg {
        Message::Text(txt) => {
            match serde_json::from_str::<ServerMsg>(&txt) {
                Ok(server_msg) => { //str::<ServerMsg>(&txt) { // Duplicate removed
                    match server_msg {
                        ServerMsg::Peers { peers } => {
                            let mut state_guard = state.lock().await;
                            state_guard.peers = peers.clone(); //.await; // Duplicate removed
                        }   //state_guard.peers = peers.clone(); // Duplicate removed
                        ServerMsg::Error { error } => {
                            let mut state_guard = state.lock().await;
                            state_guard.log.push(format!("Error: {}", error));
                        }   //state_guard.log.push(format!("Error: {}", error)); // Duplicate removed
                        ServerMsg::Relay { from, data } => {
                            state
                                .lock()
                                .await
                                .log //it
                                .push(format!("Relay from {}: {:?}", from, data.clone())); // Log data by cloning

                            // Attempt to parse as WebRTCSignal (Offer, Answer, Candidate)
                            if let Ok(signal) = serde_json::from_value::<WebRTCSignal>(data.clone()) {
                                state.lock().await.log.push(format!(
                                    "Parsed WebRTCSignal from {}: {:?}",
                                    from, signal
                                ));
                                // Handle WebRTC signaling messages (Offer, Answer, Candidate)
                                handle_webrtc_signal(
                                    from,
                                    signal, // Pass the parsed WebRTCSignal directly
                                    state.clone(),
                                    self_peer_id.clone(),
                                    internal_cmd_tx.clone(),
                                    peer_connections_arc.clone(), // Pass down the Arc<Mutex<HashMap>>
                                )
                                .await;
                            }
                            // Else, attempt to parse as WebRTCMessage (SessionProposal, SessionResponse, etc.)
                            else if let Ok(web_rtc_message) = serde_json::from_value::<protocal::signal::WebRTCMessage>(data.clone()) {
                                state.lock().await.log.push(format!(
                                    "Parsed WebRTCMessage from {}: {:?}",
                                    from, web_rtc_message
                                ));
                                // Handle application-level messages that might be relayed
                                match web_rtc_message {
                                    protocal::signal::WebRTCMessage::SessionProposal(proposal) => {
                                        let mut state_guard = state.lock().await;
                                        state_guard.log.push(format!(
                                            "Received SessionProposal from {}: ID={}, Total={}, Threshold={}, Participants={:?}",
                                            from, proposal.session_id, proposal.total, proposal.threshold, proposal.participants
                                        ));
                                        // Convert SessionProposal to SessionInfo before adding to invites
                                        let invite_info = SessionInfo {
                                            session_id: proposal.session_id.clone(),
                                            total: proposal.total,
                                            threshold: proposal.threshold,
                                            participants: proposal.participants.clone(),
                                            accepted_peers: Vec::new(), // New invites have no accepted peers yet
                                        };
                                        state_guard.invites.push(invite_info);
                                    }
                                    protocal::signal::WebRTCMessage::SessionResponse(response) => {
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
                                    // Handle other WebRTCMessage variants if they can be relayed.
                                    // Typically, DKG packages, ChannelOpen, MeshReady, SimpleMessage are sent over established data channels.
                                    protocal::signal::WebRTCMessage::DkgRound1Package { package } => {
                                        state.lock().await.log.push(format!("Received relayed DkgRound1Package from {}", from));
                                        if let Err(e) = internal_cmd_tx.send(InternalCommand::ProcessDkgRound1 { from_peer_id: from.clone(), package }) {
                                            state.lock().await.log.push(format!("Failed to send ProcessDkgRound1 command for relayed pkg: {}", e));
                                        }
                                    }
                                    protocal::signal::WebRTCMessage::DkgRound2Package { package } => {
                                        state.lock().await.log.push(format!("Received relayed DkgRound2Package from {}", from));
                                        if let Err(e) = internal_cmd_tx.send(InternalCommand::ProcessDkgRound2 { from_peer_id: from.clone(), package }) {
                                            state.lock().await.log.push(format!("Failed to send ProcessDkgRound2 command for relayed pkg: {}", e));
                                        }
                                    }
                                    protocal::signal::WebRTCMessage::ChannelOpen { peer_id: opened_by_peer_id } => {
                                        state.lock().await.log.push(format!("Received relayed ChannelOpen from {} (channel opened by {})", from, opened_by_peer_id));
                                        // Potentially dispatch an internal command or update state if needed for relayed ChannelOpen
                                    }
                                    protocal::signal::WebRTCMessage::MeshReady { session_id, peer_id: ready_peer_id } => {
                                        state.lock().await.log.push(format!("Received relayed MeshReady from {} (peer {} in session {})", from, ready_peer_id, session_id));
                                        if let Err(e) = internal_cmd_tx.send(InternalCommand::ProcessMeshReady { peer_id: ready_peer_id }) {
                                             state.lock().await.log.push(format!("Failed to send ProcessMeshReady command for relayed msg: {}", e));
                                        }
                                    }
                                    protocal::signal::WebRTCMessage::SimpleMessage { text } => {
                                        state.lock().await.log.push(format!("Received relayed SimpleMessage from {}: {}", from, text));
                                        // Handle displaying or processing the simple message
                                    }
                                }
                            }
                            // If parsing as both WebRTCSignal and WebRTCMessage fails
                            else {
                                state.lock().await.log.push(format!(
                                    "Error parsing relayed message from {} as WebRTCSignal or WebRTCMessage. Data: {:?}",
                                    from, data // data was cloned earlier, so it's safe to use here
                                ));
                            }
                        }   //} // Duplicate removed
                    }   //} // Duplicate removed
                }   //} // Duplicate removed
                Err(e) => {
                    state //{ // Duplicate removed
                        .lock()
                        .await
                        .log //it
                        .push(format!("Error parsing server message: {}", e));
                }       //.push(format!("Error parsing server message: {}", e)); // Duplicate removed
            }   //} // Duplicate removed
        }   //} // Duplicate removed
        Message::Close(_) => {
            state //Close(_) => { // Duplicate removed
                .lock()
                .await
                .log //it
                .push("WebSocket connection closed by server.".to_string());
        }       //.push("WebSocket connection closed by server.".to_string()); // Duplicate removed
        Message::Ping(ping_data) => {
            let _ = ws_sink.send(Message::Pong(ping_data)).await;
        }   //let _ = ws_sink.send(Message::Pong(ping_data)).await; // Duplicate removed
        Message::Pong(_) => {} // Ignore pongs
        Message::Binary(_) => { // Ignore pongs // Duplicate removed
            state //Binary(_) => { // Duplicate removed
                .lock()
                .await
                .log //it
                .push("Received unexpected binary message.".to_string());
        }       //.push("Received unexpected binary message.".to_string()); // Duplicate removed
        Message::Frame(_) => {} // Ignore frames
    }   //Message::Frame(_) => {} // Ignore frames // Duplicate removed
}   //} // Duplicate removed
//} // Duplicate removed
/// Handler for WebRTC signaling messages
async fn handle_webrtc_signal( //ng messages
    from_peer_id: String, //gnal(
    signal: WebRTCSignal,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String, //tate<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>, // Added parameter
) { // peer_connections_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>, // Duplicate removed
    // Clone necessary variables for the spawned task
    let from_clone = from_peer_id.clone(); //pawned task
    let state_log_clone = state.clone(); //);
    let self_peer_id_clone = self_peer_id.clone();
    let internal_cmd_tx_clone = internal_cmd_tx.clone();
    let pc_arc_net_clone = peer_connections_arc.clone(); // Use passed parameter
    let signal_clone = signal.clone(); //tions_arc.clone(); // Duplicate removed
    tokio::spawn(async move { //.clone(); // Duplicate removed
        // Get or create peer connection
        let pc_to_use_result = { //nnection
            let peer_conns_guard = pc_arc_net_clone.lock().await; // Guard for modification
            match peer_conns_guard.get(&from_clone).cloned() { //it; // Guard for modification
                Some(pc) => Ok(pc), //get(&from_clone).cloned() { // Duplicate removed
                None => { //=> Ok(pc), // Duplicate removed
                    // Drop guard before await
                    drop(peer_conns_guard); //ait
                    state_log_clone.lock().await.log.push(format!(
                        "WebRTC signal from {} received, but connection object missing. Attempting creation...",
                        from_clone //gnal from {} received, but connection object missing. Attempting creation...",
                    )); // from_clone
                    create_and_setup_peer_connection(
                        from_clone.clone(), //onnection(
                        self_peer_id_clone.clone(),
                        pc_arc_net_clone.clone(), // Pass the Arc<Mutex<HashMap>>
                        internal_cmd_tx_clone.clone(),
                        state_log_clone.clone(), //one(),
                        &WEBRTC_API, //one.clone(), // Duplicate removed
                        &WEBRTC_CONFIG,
                    )   //&WEBRTC_CONFIG, // Duplicate removed
                    .await
                }   //.await // Duplicate removed
            }   //} // Duplicate removed
        };  //} // Duplicate removed
        match pc_to_use_result {
            Ok(pc_clone) => match signal_clone {
                WebRTCSignal::Offer(offer_info) => {
                    handle_webrtc_offer( //r_info) =>:
                        &from_clone, //fer(
                        offer_info, //,
                        pc_clone, //o,
                        state_log_clone.clone(),
                        self_peer_id_clone.clone(),
                        internal_cmd_tx_clone.clone(),
                        pc_arc_net_clone.clone(), // Pass peer_connections_arc
                    )   //internal_cmd_tx_clone.clone(), // Duplicate removed
                    .await;
                }   //.await; // Duplicate removed
                WebRTCSignal::Answer(answer_info) => {
                    handle_webrtc_answer( //er_info) =>:
                        &from_clone, //swer(
                        answer_info,
                        pc_clone, //fo,
                        state_log_clone.clone(),
                    )   //state_log_clone.clone(), // Duplicate removed
                    .await;
                }   //.await; // Duplicate removed
                WebRTCSignal::Candidate(candidate_info) => {
                    handle_webrtc_candidate( //idate_info) =>:
                        &from_clone, //ndidate(
                        candidate_info,
                        pc_clone, //_info,
                        state_log_clone.clone(),
                    )   //state_log_clone.clone(), // Duplicate removed
                    .await;
                }   //.await; // Duplicate removed
            },  //} // Duplicate removed
            Err(e) => {
                state_log_clone.lock().await.log.push(format!(
                    "Failed to create/retrieve connection object for {} to handle signal: {}",
                    from_clone, e //eate/retrieve connection object for {} to handle signal: {}",
                )); // from_clone, e
            }   //)); // Duplicate removed
        }   //} // Duplicate removed
    }); //} // Duplicate removed
}   //}); // Duplicate removed
//} // Duplicate removed
/// Handler for WebRTC offer signals
async fn handle_webrtc_offer( //signals
    from_peer_id: &str, //offer(
    offer_info: SDPInfo,
    mut pc: Arc<RTCPeerConnection>, // Make pc mutable as it might be replaced
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    self_peer_id: String, //tate<Ed25519Sha512>>>,
    internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    peer_connections_map_arc: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>, // Added parameter
) { // internal_cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Duplicate removed
    use webrtc::peer_connection::signaling_state::RTCSignalingState;

    let initial_pc_state = pc.signaling_state();

    // If we receive an offer while our current PC for this peer is in a state
    // that indicates an ongoing or previous negotiation (e.g., HaveRemoteOffer, HaveLocalPranswer),
    // or generally any non-stable/new/closed state, it's often best to reset our side.
    // This handles cases where the sender might have restarted their side and sent a new offer.
    if initial_pc_state != RTCSignalingState::Stable &&
       initial_pc_state != RTCSignalingState::Closed
       
    {
        state.lock().await.log.push(format!(
            "Offer from {}: Current PC state is {:?}. Resetting PC to handle new offer cleanly.",
            from_peer_id, initial_pc_state
        ));

        // Close the existing PC
        if let Err(e) = pc.close().await {
            state.lock().await.log.push(format!(
                "Offer from {}: Error closing old PC during reset: {}. Continuing with creation of new PC.",
                from_peer_id, e
            ));
        }
        // Remove it from the central map
        {
            let mut pc_map_guard = peer_connections_map_arc.lock().await;
            pc_map_guard.remove(from_peer_id);
        }
        state.lock().await.log.push(format!(
            "Offer from {}: Old PC closed and removed. Creating new PC.",
            from_peer_id
        ));

        // Create and set up a new peer connection
        match create_and_setup_peer_connection(
            from_peer_id.to_string(),
            self_peer_id.clone(),
            peer_connections_map_arc.clone(), // Pass the Arc<Mutex<HashMap>>
            internal_cmd_tx.clone(),
            state.clone(),
            &WEBRTC_API,
            &WEBRTC_CONFIG,
        ).await {
            Ok(new_pc) => {
                pc = new_pc; // Replace the pc instance with the new one
                state.lock().await.log.push(format!(
                    "Offer from {}: New PC created. Proceeding with offer processing using new PC.",
                    from_peer_id
                ));
            }
            Err(e) => {
                state.lock().await.log.push(format!(
                    "Offer from {}: Failed to create new PC after reset: {}. Aborting offer.",
                    from_peer_id, e
                ));
                return; // Cannot proceed
            }
        }
    }
    
    // Check if we should proceed with this offer
    let proceed_with_offer: bool;
    let mut should_abort_due_to_race = false;
    
    // This block determines initial `proceed_with_offer` based on current state and politeness
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
        let is_polite = self_peer_id > from_peer_id.to_string(); // Determine politeness
        
        let current_signaling_state_for_log = pc.signaling_state(); // Get state of current pc (might be new)
        state_guard.log.push(format!(
            "Offer from {}: making_offer={}, ignore_offer={}, is_polite={}, current_pc_state={:?}",
            from_peer_id, making, ignoring, is_polite, current_signaling_state_for_log
        ));

        let collision = making; // Are we currently in the process of making an offer to this peer?
        proceed_with_offer = if collision && is_polite {
            // Polite peer yields its offer generation but processes the incoming offer.
            true
        } else if collision && !is_polite {
            // Impolite peer ignores the incoming offer during collision.
            false
        } else if ignoring {
            // Explicitly ignoring offers from this peer (likely due to previous glare).
            false
        } else {
            // No collision or explicit ignore, proceed.
            true
        };

        if proceed_with_offer && making { // If we decided to proceed BUT we were 'making' an offer
            should_abort_due_to_race = true; // Mark for potential race condition handling
        }
    } // state_guard lock released

    // Re-evaluate `proceed_with_offer` with safeguards and state updates
    let mut final_proceed_with_offer = proceed_with_offer;
    let current_pc_signaling_state = pc.signaling_state(); // Get fresh state of current pc for this check

    {
        let mut state_guard = state.lock().await;
        let making_now = state_guard // Fresh read of making_offer
            .making_offer
            .get(from_peer_id)
            .copied()
            .unwrap_or(false);
        let is_polite = self_peer_id > from_peer_id.to_string();
        let collision_now = making_now;

        if should_abort_due_to_race {
            // This case implies:
            // 1. Initially, we were `making_offer` (collision = true).
            // 2. We are `is_polite`.
            // 3. So, `proceed_with_offer` was set to `true`.
            // 4. `should_abort_due_to_race` was set to `true`.
            // Now, if this flag is true, we abort processing this incoming offer.
            // This is to ensure the polite peer, while yielding, doesn't also try to process
            // if its own offer-making process somehow didn't get cancelled cleanly.
            // It's a strong measure to prevent processing an offer if we were also making one and are polite.
            state_guard.log.push(format!(
                "Offer from {}: Aborting due to detected race (polite peer was making an offer). Making_offer flag will be reset.",
                from_peer_id
            ));
            state_guard.making_offer.insert(from_peer_id.to_string(), false); // Ensure making_offer is reset
            final_proceed_with_offer = false;
        } else if collision_now && is_polite {
            // Polite peer yields its own offer generation.
            state_guard.log.push(format!(
                "Offer from {}: Glare detected. Polite peer yielding its own offer generation for.",
                from_peer_id
            ));
            state_guard.making_offer.insert(from_peer_id.to_string(), false);
            // `final_proceed_with_offer` remains as determined by the first block (should be true here).
        } else if collision_now && !is_polite {
            // Impolite peer ignores incoming offer and continues its own.
            state_guard.log.push(format!(
                "Offer from {}: Glare detected. Impolite peer ignoring incoming offer from {}, will continue its own.",
                from_peer_id, from_peer_id
            ));
            state_guard.ignore_offer.insert(from_peer_id.to_string(), true); // Mark to ignore subsequent signals from this offer
            final_proceed_with_offer = false;
        } else if state_guard.ignore_offer.get(from_peer_id).copied().unwrap_or(false) {
            // If we are set to ignore offers (e.g., impolite peer decided this in a previous step)
            state_guard.log.push(format!(
                "Offer from {}: Ignoring offer as previously decided (ignore_offer=true). Resetting ignore_offer.",
                from_peer_id
            ));
            state_guard.ignore_offer.insert(from_peer_id.to_string(), false); // Reset after ignoring once
            final_proceed_with_offer = false;
        }

        // Final check on PC state before trying to set remote description
        // We should only proceed if the PC is in a 'Stable' state.
        // If it's 'Closed', something went wrong. Other states indicate an ongoing, possibly conflicting, process.
        let is_invalid_pc_state_for_offer = current_pc_signaling_state != RTCSignalingState::Stable;

        if final_proceed_with_offer && is_invalid_pc_state_for_offer {
            state_guard.log.push(format!(
                "Offer from {}: Aborting. PC signaling state is {:?} (expected Stable) before setting remote offer.",
                from_peer_id, current_pc_signaling_state
            ));
            final_proceed_with_offer = false;
        }
    } // state_guard lock released


    if final_proceed_with_offer {
        state
            .lock()
            .await
            .log
            .push(format!("Processing offer from {}...", from_peer_id));
        match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::offer(
            offer_info.sdp,
        ) {
            Ok(offer_sd) => { // Renamed to offer_sd to avoid conflict
                if let Err(e) = pc.set_remote_description(offer_sd).await {
                    state.lock().await.log.push(format!(
                        "Error setting remote description (offer) from {}: {}",
                        from_peer_id, e
                    ));
                    // If setting remote description fails, ensure making_offer and ignore_offer are reset for this peer
                    // to allow future attempts.
                    let mut state_guard = state.lock().await;
                    state_guard.making_offer.insert(from_peer_id.to_string(), false);
                    state_guard.ignore_offer.insert(from_peer_id.to_string(), false);
                    return;
                }
                state.lock().await.log.push(format!(
                    "Set remote description (offer) from {}. Creating answer...",
                    from_peer_id
                ));
                apply_pending_candidates(from_peer_id, pc.clone(), state.clone()).await;
                process_data_channel_creation(
                    from_peer_id,
                    &pc,
                    state.clone(),
                    internal_cmd_tx.clone(),
                )
                .await;
                create_and_send_answer(from_peer_id, &pc, state.clone(), internal_cmd_tx).await;
            }
            Err(e) => {
                state
                    .lock()
                    .await
                    .log
                    .push(format!("Error parsing offer SDP from {}: {}", from_peer_id, e));
            }
        }
    } else {
        state.lock().await.log.push(format!(
            "Offer from {}: Decided not to process this offer based on glare resolution or state.",
            from_peer_id
        ));
        // If we are not proceeding, and we were the impolite peer that set ignore_offer,
        // it's already been reset in the logic above.
        // If we were the polite peer that was making_offer, it should also have been reset.
    }
}

/// Helper for creating a data channel
async fn process_data_channel_creation(
    peer_id: &str, //ata_channel_creation(
    pc: &Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) { // cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Duplicate removed
    match pc
        .create_data_channel(utils::peer::DATA_CHANNEL_LABEL, None)
        .await //e_data_channel(utils::peer::DATA_CHANNEL_LABEL, None) // Duplicate removed
    {   //.await // Duplicate removed
        Ok(dc) => {
            let peer_id_dc = peer_id.to_string();
            let state_log_dc = state.clone(); //g();
            let cmd_tx_dc = cmd_tx.clone(); //);
            utils::peer::setup_data_channel_callbacks(dc, peer_id_dc, state_log_dc, cmd_tx_dc)
                .await; //::setup_data_channel_callbacks(dc, peer_id_dc, state_log_dc, cmd_tx_dc) // Duplicate removed
            state //await; // Duplicate removed
                .lock()
                .await
                .log //it
                .push(format!("Created responder data channel for {}", peer_id));
        }       //.push(format!("Created responder data channel for {}", peer_id)); // Duplicate removed
        Err(e) => {
            state.lock().await.log.push(format!(
                "Error creating responder data channel for {}: {} (continuing anyway)",
                peer_id, e //ating responder data channel for {}: {} (continuing anyway)",
            )); // peer_id, e
        }   //)); // Duplicate removed
    }
}

async fn create_and_send_answer( //ing an answer
    peer_id: &str, //d_send_answer(
    pc: &Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
) { // cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Duplicate removed
    match pc.create_answer(None).await {
        Ok(answer) => { //wer(None).await { // Duplicate removed
            if let Err(e) = pc.set_local_description(answer.clone()).await {
                state.lock().await.log.push(format!( //(answer.clone()).await { // Duplicate removed
                    "Error setting local description (answer) for {}: {}",
                    peer_id, e //ting local description (answer) for {}: {}",
                )); // peer_id, e
                return;
            }   //return; // Duplicate removed
            state.lock().await.log.push(format!(
                "Set local description (answer) for {}. Sending answer...",
                peer_id //cal description (answer) for {}. Sending answer...",
            )); // peer_id
            let signal = WebRTCSignal::Answer(SDPInfo { sdp: answer.sdp });
            match serde_json::to_value(signal) { //PInfo { sdp: answer.sdp }); // Duplicate removed
                Ok(json_val) => { //value(signal) { // Duplicate removed
                    let relay_msg = InternalCommand::SendToServer(SharedClientMsg::Relay {
                        to: peer_id.to_string(), //and::SendToServer(SharedClientMsg::Relay { // Duplicate removed
                        data: json_val, //string(), // Duplicate removed
                    }); // data: json_val, // Duplicate removed
                    let _ = cmd_tx.send(relay_msg);
                    state //= cmd_tx.send(relay_msg); // Duplicate removed
                        .lock()
                        .await
                        .log //it
                        .push(format!("Sent answer to {}", peer_id));
                }       //.push(format!("Sent answer to {}", peer_id)); // Duplicate removed
                Err(e) => {
                    state //{ // Duplicate removed
                        .lock()
                        .await
                        .log //it
                        .push(format!("Error serializing answer for {}: {}", peer_id, e));
                }       //.push(format!("Error serializing answer for {}: {}", peer_id, e)); // Duplicate removed
            }   //} // Duplicate removed
        }   //} // Duplicate removed
        Err(e) => {
            state //{ // Duplicate removed
                .lock()
                .await
                .log //it
                .push(format!("Error creating answer for {}: {}", peer_id, e));
        }
    } 
}

async fn handle_webrtc_answer( //signals
    from_peer_id: &str, //answer(
    answer_info: SDPInfo,
    pc: Arc<RTCPeerConnection>,
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
) { // state: Arc<Mutex<AppState<Ed25519Sha512>>>, // Duplicate removed
    state
        .lock()
        .await
        .log //it
        .push(format!("Processing answer from {}...", from_peer_id));
    match webrtc::peer_connection::sdp::session_description::RTCSessionDescription::answer(
        answer_info.sdp, //onnection::sdp::session_description::RTCSessionDescription::answer( // Duplicate removed
    ) { // answer_info.sdp, // Duplicate removed
        Ok(answer) => {
            if let Err(e) = pc.set_remote_description(answer).await {
                state.lock().await.log.push(format!( //(answer).await { // Duplicate removed
                    "Error setting remote description (answer) from {}: {}",
                    from_peer_id, e //remote description (answer) from {}: {}",
                )); // from_peer_id, e
            } else {
                state.lock().await.log.push(format!(
                    "Set remote description (answer) from {}",
                    from_peer_id //description (answer) from {}",
                )); // from_peer_id
                // Apply any pending ICE candidates now that the remote description is set
                apply_pending_candidates(from_peer_id, pc.clone(), state.clone()).await; //et
            }   //apply_pending_candidates(from_peer_id, pc.clone(), state.clone()).await; // Duplicate removed
        }   //} // Duplicate removed
        Err(e) => {
            state //{ // Duplicate removed
                .lock()
                .await
                .log //it
                .push(format!("Error parsing answer from {}: {}", from_peer_id, e));
        }       //.push(format!("Error parsing answer from {}: {}", from_peer_id, e)); // Duplicate removed
    }   //} // Duplicate removed
}   //} // Duplicate removed
//} // Duplicate removed
/// Handler for WebRTC ICE candidate signals
async fn handle_webrtc_candidate( //ate signals
    from_peer_id: &str, //candidate(
    candidate_info: CandidateInfo, // Updated to use imported CandidateInfo
    pc: Arc<RTCPeerConnection>, //ignal::CandidateInfo, // Duplicate removed
    state: Arc<Mutex<AppState<Ed25519Sha512>>>,
) { // state: Arc<Mutex<AppState<Ed25519Sha512>>>, // Duplicate removed
    state
        .lock()
        .await
        .log //it
        .push(format!("Processing candidate from {}...", from_peer_id));
    let candidate_init = RTCIceCandidateInit { //om {}...", from_peer_id)); // Duplicate removed
        candidate: candidate_info.candidate, //{ // Duplicate removed
        sdp_mid: candidate_info.sdp_mid, //ate, // Duplicate removed
        sdp_mline_index: candidate_info.sdp_mline_index,
        username_fragment: None, //te_info.sdp_mline_index, // Duplicate removed
    };  //username_fragment: None, // Duplicate removed
    // Check if remote description is set before adding ICE candidate
    let current_state = pc.signaling_state(); //ore adding ICE candidate
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
            state.lock().await.log.push(format!( //didate_init.clone()).await { // Duplicate removed
                "Error adding ICE candidate from {}: {}",
                from_peer_id, e
            ));
        } else {
            state
                .lock()
                .await
                .log //it
                .push(format!("Added ICE candidate from {}", from_peer_id));
        }   
    } else {
        let mut state_guard = state.lock().await;
        state_guard.log.push(format!( //ock().await; // Duplicate removed
            "Storing ICE candidate from {} for later (remote description not set yet)",
            from_peer_id 
        ));
        let candidates = state_guard
            .pending_ice_candidates //d
            .entry(from_peer_id.to_string())
            .or_insert_with(Vec::new); //ing())
        candidates.push(candidate_init);
        let queued_msg = format!( //_init); // Duplicate removed
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
) { // cmd_tx: mpsc::UnboundedSender<InternalCommand>, // Duplicate removed
    let mut all_channels_open = false;
    let mut already_sent_own_ready = false;
    { //et mut all_channels_open = false; // Duplicate removed
        let state_guard = state.lock().await;
        if let Some(session) = &state_guard.session {
            // Check if we have open data channels to all peers in the session
            let all_peers_connected_via_dc = session //nnels to all peers in the session
                .participants //nected = session // Duplicate removed
                .iter() //ipants
                .filter(|p| **p != state_guard.peer_id)
                .all(|p| state_guard.data_channels.contains_key(p));
            all_channels_open = all_peers_connected_via_dc; //ontains_key(p)); // Duplicate removed
            
            // Check if this node has already processed its own mesh readiness
            if matches!(state_guard.mesh_status, MeshStatus::SentSelfReady | MeshStatus::Ready) {
                already_sent_own_ready = true;
            }
        }   //all_channels_open = all_peers_connected; // Duplicate removed
    }   //} // Duplicate removed
    if all_channels_open && !already_sent_own_ready {
        state //annels_open { // Duplicate removed
            .lock()
            .await
            .log //it
            .push("All local data channels open! Signaling to process own mesh readiness...".to_string());
            //.push("All data channels open! Signaling mesh readiness...".to_string()); // Duplicate removed
        // Send command to handle this node's own readiness
        cmd_tx.send(InternalCommand::SendOwnMeshReadySignal).unwrap_or_else(|e| {
            // Clone state before moving into the closure to avoid borrowing issues
            let state_clone = state.clone(); //o the closure to avoid borrowing issues
            tokio::spawn(async move { //lone(); // Duplicate removed
                state_clone //ync move { // Duplicate removed
                    .lock()
                    .await
                    .log //it
                    .push(format!("Failed to send SendOwnMeshReadySignal command: {}", e));
            });   
        });
    } 
}

async fn initiate_webrtc_connections( //bRTC connections with all session participants
    participants: Vec<String>, //ctions(
    self_peer_id: String, //ing>,
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
        } // Lock guard (peer_conns_guard) is dropped here

        if needs_creation {
            match create_and_setup_peer_connection(
                peer_id_str.clone(),
                self_peer_id.clone(),
                peer_connections_arc.clone(), // Pass the Arc<Mutex<HashMap>>
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

