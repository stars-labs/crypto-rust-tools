mod protocal;
mod keystore;

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

use std::collections::BTreeMap; // Add HashSet import

// Import from lib.rs
use utils::state::{DkgState, InternalCommand, MeshStatus, SigningState}; // <-- Add SessionResponse here

use std::sync::Arc;
use std::time::Duration;
use std::{collections::HashMap, io};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use webrtc_signal_server::ClientMsg;
// Add display-related imports for better status handling
use frost_core::Ciphersuite;

mod utils;

use utils::state::{AppState, ReconnectionTracker}; // Remove DkgState import

mod ui;
use ui::tui::{draw_main_ui, handle_key_event};

mod network;
use clap::{Parser, ValueEnum};
use network::websocket::handle_websocket_message;

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
        pending_mesh_ready_signals: Vec::new(), // Initialize the buffer
        own_mesh_ready_sent: false, // Initialize to false - this node hasn't sent its mesh ready signal yet
        signing_state: SigningState::Idle, // Initialize signing state to idle
        keystore: None, // Initialize keystore to None
        current_wallet_id: None, // Initialize current wallet ID to None
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

mod handlers;
use handlers::*;

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
            handle_send_to_server(shared_msg, state, ws_sink).await;
        }
        InternalCommand::SendDirect { to, message } => {
            handle_send_direct(to, message, state).await;
        }
        InternalCommand::ProposeSession {
            session_id,
            total, 
            threshold,
            participants,
        } => {
            handle_propose_session(session_id, total, threshold, participants, state, internal_cmd_tx, self_peer_id).await;
        }
        InternalCommand::AcceptSessionProposal(session_id) => {
            handle_accept_session_proposal(session_id, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSessionResponse { from_peer_id, response } => {
            handle_process_session_response(from_peer_id, response, state, internal_cmd_tx).await;
        }
        InternalCommand::ReportChannelOpen { peer_id } => {
            handle_report_channel_open(peer_id, state, internal_cmd_tx, self_peer_id).await;
        }
        InternalCommand::SendOwnMeshReadySignal => { 
            handle_send_own_mesh_ready_signal(state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessMeshReady { peer_id } => {
            handle_process_mesh_ready(peer_id, state, internal_cmd_tx).await;
        }
        InternalCommand::CheckAndTriggerDkg => {
            handle_check_and_trigger_dkg(state, internal_cmd_tx).await;
        }
        InternalCommand::TriggerDkgRound1 => {
            handle_trigger_dkg_round1(state, self_peer_id).await;
        }
        InternalCommand::TriggerDkgRound2 => {
            handle_trigger_dkg_round2(state).await;
        }
        InternalCommand::ProcessDkgRound1 {
            from_peer_id,
            package,
        } => {
            handle_process_dkg_round1(from_peer_id, package, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessDkgRound2 {
            from_peer_id, 
            package, 
        } => {
            handle_process_dkg_round2(from_peer_id, package, state, internal_cmd_tx).await;
        }
        // --- DKG Commands ---
        InternalCommand::FinalizeDkg => {
            handle_finalize_dkg(state).await;
        }
        
        // --- Signing Command Handlers ---
        InternalCommand::InitiateSigning { transaction_data } => {
            handle_initiate_signing(transaction_data, state, internal_cmd_tx).await;
        }
        InternalCommand::AcceptSigning { signing_id } => {
            handle_accept_signing(signing_id, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSigningRequest { from_peer_id, signing_id, transaction_data, timestamp } => {
            handle_process_signing_request(from_peer_id, signing_id, transaction_data, timestamp, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSigningAcceptance { from_peer_id, signing_id, timestamp } => {
            handle_process_signing_acceptance(from_peer_id, signing_id, timestamp, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSigningCommitment { from_peer_id, signing_id, commitment } => {
            handle_process_signing_commitment(from_peer_id, signing_id, commitment, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSignatureShare { from_peer_id, signing_id, share } => {
            handle_process_signature_share(from_peer_id, signing_id, share, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessAggregatedSignature { from_peer_id, signing_id, signature } => {
            handle_process_aggregated_signature(from_peer_id, signing_id, signature, state, internal_cmd_tx).await;
        }
        InternalCommand::ProcessSignerSelection { from_peer_id, signing_id, selected_signers } => {
            handle_process_signer_selection(from_peer_id, signing_id, selected_signers, state, internal_cmd_tx).await;
        }
        InternalCommand::InitiateFrostRound1 { signing_id, transaction_data, selected_signers } => {
            handle_initiate_frost_round1(signing_id, transaction_data, selected_signers, state, internal_cmd_tx).await;
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
    let mut all_responses_received_debug = false;
    let mut already_sent_own_ready_debug = false;
    let mut session_exists_debug = false;
    let mut participants_to_check_debug: Vec<String> = Vec::new();
    let mut data_channels_keys_debug: Vec<String> = Vec::new();
    let self_peer_id_debug: String;
    let current_mesh_status_debug: MeshStatus;

    {
        let mut state_guard = state.lock().await;
        self_peer_id_debug = state_guard.peer_id.clone();
        current_mesh_status_debug = state_guard.mesh_status.clone(); // Clone for logging
        if let Some(session) = &state_guard.session {
            session_exists_debug = true;
            let peer_id_clone = state_guard.peer_id.clone(); // Clone to avoid borrow issues
            participants_to_check_debug = session
                .participants
                .iter()
                .filter(|p| **p != peer_id_clone)
                .cloned()
                .collect();
            
            data_channels_keys_debug = state_guard.data_channels.keys().cloned().collect();

            all_channels_open_debug = participants_to_check_debug
                .iter()
                .all(|p| state_guard.data_channels.contains_key(p));

            // Check if all session responses received (all participants accepted)
            all_responses_received_debug = session.accepted_peers.len() == session.participants.len();
            
            // Clone values before the match to avoid borrowing conflicts
            let current_session_size = session.participants.len();
            
            // Check if we've already sent our own mesh ready signal using explicit tracking
            // This replaces the flawed logic that incorrectly inferred from mesh status
            already_sent_own_ready_debug = state_guard.own_mesh_ready_sent;
        }
    } // state_guard is dropped

    // Log outside the lock to minimize lock holding time
    let mut log_guard = state.lock().await;
    log_guard.log.push(format!(
        "[MeshCheck-{}] Status: {:?}, SessionExists: {}, ParticipantsToCheck: {:?}, OpenDCKeys: {:?}, AllOpenCalc: {}, AllResponsesReceivedCalc: {}, AlreadySentCalc: {}",
        self_peer_id_debug,
        current_mesh_status_debug, // Log current status
        session_exists_debug,
        participants_to_check_debug,
        data_channels_keys_debug,
        all_channels_open_debug,
        all_responses_received_debug,
        already_sent_own_ready_debug
    ));
    drop(log_guard);


    if session_exists_debug && all_channels_open_debug && all_responses_received_debug && !already_sent_own_ready_debug {
        // Re-acquire lock for the specific log message and subsequent command sending
        state 
            .lock()
            .await
            .log
            .push(format!("[MeshCheck-{}] All local data channels open AND all session responses received! Signaling to process own mesh readiness...", self_peer_id_debug));
        
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
        } else if !all_responses_received_debug {
            final_log_guard.log.push(format!("[MeshCheck-{}] Not all session responses received yet, cannot send SendOwnMeshReadySignal.", self_peer_id_debug));
        } else if already_sent_own_ready_debug {
            final_log_guard.log.push(format!("[MeshCheck-{}] Already sent own ready signal (Status: {:?}), not sending again.", self_peer_id_debug, current_mesh_status_debug));
        }
    }
}
