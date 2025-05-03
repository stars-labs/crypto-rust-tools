use std::sync::{Arc, Mutex as StdMutex}; // Use StdMutex for TUI state
use std::time::{Duration, SystemTime, UNIX_EPOCH}; // Add SystemTime and UNIX_EPOCH
// Add HashSet import
use std::{
    collections::{HashMap, HashSet},
    io,
};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::{SinkExt, StreamExt};
use lazy_static::lazy_static;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use serde::{Deserialize, Serialize}; // Add serde traits
use tokio::sync::{Mutex as TokioMutex, mpsc}; // Use TokioMutex for async WebRTC state
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
// Uncomment imports
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate};
// Correct the import path for RTCIceCredentialType
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
// Remove unused import RTCSessionDescription
// use webrtc::peer_connection::sdp::session_description::RTCSessionDescription; // Import RTCDataChannel // Import DataChannelMessage

// Add these imports for WebRTC policies
use webrtc::peer_connection::policy::{
    bundle_policy::RTCBundlePolicy, ice_transport_policy::RTCIceTransportPolicy,
    rtcp_mux_policy::RTCRtcpMuxPolicy,
};

// Import shared types from the library crate
use solnana_mpc_frost::{ClientMsg, ServerMsg, SessionInfo};

// --- WebRTC Signaling Structures ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum WebRTCSignal {
    Offer(SDPInfo),
    Answer(SDPInfo),
    Candidate(CandidateInfo),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SDPInfo {
    sdp: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CandidateInfo {
    candidate: String,
    #[serde(rename = "sdpMid")]
    sdp_mid: Option<String>,
    #[serde(rename = "sdpMLineIndex")]
    sdp_mline_index: Option<u16>,
}
// --- End WebRTC Signaling Structures ---

struct AppState {
    peer_id: String,
    peers: Vec<String>,
    log: Vec<String>,
    session: Option<SessionInfo>,
    invites: Vec<SessionInfo>,
    // Use TokioMutex for async access needed by WebRTC callbacks
    peer_connections: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    // Store connection status for TUI display (under StdMutex)
    peer_statuses: HashMap<String, RTCPeerConnectionState>,
    // Flag to track if WebRTC setup has started for the current session
    reconnection_tracker: ReconnectionTracker,
    keep_alive_peers: HashSet<String>, // To track which peers are receiving keep-alives
    // 添加一个可选的命令发送器字段
    cmd_tx: Option<mpsc::UnboundedSender<ClientMsg>>,
    // --- Perfect Negotiation Flags ---
    making_offer: HashMap<String, bool>, // Track if we are currently making an offer TO a specific peer
    ignore_offer: HashMap<String, bool>, // Track if we should ignore the next offer FROM a specific peer
    // Add a field to store pending ICE candidates
    pending_ice_candidates: HashMap<String, Vec<webrtc::ice_transport::ice_candidate::RTCIceCandidateInit>>,
}

// --- WebRTC API Setup ---
lazy_static! {
    static ref WEBRTC_CONFIG: RTCConfiguration = RTCConfiguration {
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
    static ref WEBRTC_API: webrtc::api::API = {
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

// Define a struct to track reconnection attempts and prevent excessive reconnections
struct ReconnectionTracker {
    attempts: std::collections::HashMap<String, usize>,
    timestamps: std::collections::HashMap<String, std::time::Instant>,
    cooldown_until: std::collections::HashMap<String, Option<std::time::Instant>>, // 新增冷却期字段
    max_sequential_attempts: usize, // 限制连续尝试次数
}

impl ReconnectionTracker {
    fn new() -> Self {
        Self {
            attempts: std::collections::HashMap::new(),
            timestamps: std::collections::HashMap::new(),
            cooldown_until: std::collections::HashMap::new(),
            // Increase allowed attempts before cooldown
            max_sequential_attempts: 5, 
        }
    }

    fn should_attempt(&mut self, peer_id: &str) -> bool {
        let now = std::time::Instant::now();
        
        // 检查是否在冷却期
        if let Some(Some(cooldown_time)) = self.cooldown_until.get(peer_id) {
            if now < *cooldown_time {
                return false; // 仍在冷却期，不尝试重连
            } else {
                // 冷却期结束，重置计数
                self.cooldown_until.insert(peer_id.to_string(), None);
                self.attempts.insert(peer_id.to_string(), 0);
            }
        }
        
        let attempt_count = self.attempts.entry(peer_id.to_string()).or_insert(0);
        let last_attempt = self.timestamps.entry(peer_id.to_string()).or_insert(now);

        // 指数退避策略 - 随着尝试次数增加而延长等待时间
        let wait_duration = if *attempt_count == 0 {
            Duration::from_millis(500) // 首次尝试更快
        } else {
            // Reduce the cap for exponential backoff (max wait ~15 seconds)
            Duration::from_millis((500 * (1 << std::cmp::min(*attempt_count, 5))) as u64) 
        };

        // 如果已经过了足够的时间
        if now.duration_since(*last_attempt) > wait_duration {
            *last_attempt = now;
            *attempt_count += 1;
            
            // 检查是否需要进入冷却期
            if *attempt_count > self.max_sequential_attempts {
                // Reduce cooldown duration to 30 seconds
                self.cooldown_until.insert(
                    peer_id.to_string(), 
                    Some(now + Duration::from_secs(30)) 
                );
                return false;
            }
            
            return true;
        }
        
        false
    }

    fn reset(&mut self, peer_id: &str) {
        self.attempts.remove(peer_id);
        self.timestamps.remove(peer_id);
        self.cooldown_until.remove(peer_id);
    }
    
    // 新增一个方法来记录重连成功，只重置尝试次数但保留时间戳
    fn record_success(&mut self, peer_id: &str) {
        self.attempts.insert(peer_id.to_string(), 0);
        self.cooldown_until.insert(peer_id.to_string(), None);
    }
}

// --- NEW Helper Function: Create Peer Connection Object ---
// Creates a single peer connection, sets up callbacks, and stores it.
async fn create_and_setup_peer_connection(
    peer_id: String,
    _self_peer_id: String, // FIX: Mark unused variable
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    cmd_tx: mpsc::UnboundedSender<ClientMsg>,
    state_log: Arc<StdMutex<AppState>>,
) -> Result<Arc<RTCPeerConnection>, String> {
    // Check if connection already exists before creating
    {
        let peer_conns = peer_connections_arc.lock().await;
        if let Some(existing_pc) = peer_conns.get(&peer_id) {
            // FIX: Connection already exists, log and return Ok with the existing Arc
            state_log.lock().unwrap().log.push(format!(
                "WebRTC connection object for {} already exists. Skipping creation.",
                peer_id
            ));
            return Ok(existing_pc.clone()); // Return the existing connection Arc
        }
        // Drop lock implicitly here
    }

    state_log
        .lock()
        .unwrap()
        .log
        .push(format!("Creating WebRTC connection object for {}", peer_id));

    match WEBRTC_API.new_peer_connection(WEBRTC_CONFIG.clone()).await {
        Ok(pc) => {
            let pc_arc = Arc::new(pc);

            // --- Setup Callbacks (Essential before processing any signals) ---
            let peer_id_on_ice = peer_id.clone();
            let cmd_tx_on_ice = cmd_tx.clone();
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
                                        let _ = cmd_tx.send(ClientMsg::Relay {
                                            to: peer_id.clone(),
                                            data: json_val,
                                        });
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

            // Setup state change handler with improved reconnection logic
            let state_log_on_state = state_log.clone();
            let peer_id_on_state = peer_id.clone();
            let cmd_tx_on_state = cmd_tx.clone();
            let pc_arc_for_state = pc_arc.clone();
            pc_arc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
                let state_log = state_log_on_state.clone();
                let peer_id = peer_id_on_state.clone();
                let cmd_tx_local = cmd_tx_on_state.clone();
                let pc_weak = Arc::downgrade(&pc_arc_for_state); // Keep pc_weak for potential future use

                println!("Peer Connection State with {} has changed: {}", peer_id, s);

                // Handle state changes with improved logic
                match s {
                    RTCPeerConnectionState::Connected => {
                        // Reset reconnection tracking when connected
                        if let Ok(mut guard) = state_log.try_lock() {
                            guard.log.push(format!("!!! WebRTC CONNECTED with {} !!!", peer_id));
                            guard.reconnection_tracker.reset(&peer_id);
                            
                            // Setup keep-alive if not already set
                            if !guard.keep_alive_peers.contains(&peer_id) {
                                guard.keep_alive_peers.insert(peer_id.clone());
                                
                                // Setup keep-alive mechanism using a dedicated channel
                                let pc_weak_clone = pc_weak.clone();
                                let peer_id_clone = peer_id.clone();
                                let state_log_clone = state_log.clone();
                                
                                tokio::spawn(async move {
                                    // Create a dedicated keep-alive channel
                                    if let Some(pc_strong) = pc_weak_clone.upgrade() {
                                        match pc_strong.create_data_channel("keep-alive", None).await {
                                            Ok(dc) => {
                                                let dc_arc = Arc::new(dc);
                                                
                                                // Set up on_open handler for channel
                                                let dc_weak = Arc::downgrade(&dc_arc);
                                                let peer_id_ping = peer_id_clone.clone();
                                                let state_log_ping = state_log_clone.clone();
                                                
                                                dc_arc.on_open(Box::new(move || {
                                                    if let Ok(mut guard) = state_log_ping.try_lock() {
                                                        guard.log.push(format!("Keep-alive channel open with {}", peer_id_ping));
                                                    }
                                                    
                                                    // Start ping loop in a new task
                                                    let dc_weak_ping = dc_weak.clone();
                                                    let peer_ping = peer_id_ping.clone();
                                                    let state_ping = state_log_ping.clone();
                                                    
                                                    tokio::spawn(async move {
                                                        let mut interval = tokio::time::interval(Duration::from_secs(10));
                                                        
                                                        loop {
                                                            interval.tick().await;
                                                            
                                                            if let Some(dc_strong) = dc_weak_ping.upgrade() {
                                                                if dc_strong.ready_state() != webrtc::data_channel::data_channel_state::RTCDataChannelState::Open {
                                                                    break;
                                                                }
                                                                
                                                                if let Err(e) = dc_strong.send_text("ping").await {
                                                                    if let Ok(mut guard) = state_ping.try_lock() {
                                                                        guard.log.push(format!(
                                                                            "Keep-alive channel to {} failed: {}",
                                                                            peer_ping, e
                                                                        ));
                                                                    }
                                                                    break;
                                                                }
                                                            } else {
                                                                // Channel was dropped
                                                                break;
                                                            }
                                                        }
                                                    });
                                                    
                                                    Box::pin(async {})
                                                }));
                                                
                                                if let Ok(mut guard) = state_log_clone.try_lock() {
                                                    guard.log.push(format!("Created keep-alive channel for {}", peer_id_clone));
                                                }
                                            },
                                            Err(e) => {
                                                if let Ok(mut guard) = state_log_clone.try_lock() {
                                                    guard.log.push(format!("Failed to create keep-alive channel for {}: {}", 
                                                        peer_id_clone, e));
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    },
                    RTCPeerConnectionState::Disconnected | RTCPeerConnectionState::Failed => {
                        // Combine Disconnected and Failed handling for rejoin logic
                        if let Ok(mut guard) = state_log.try_lock() {
                            let state_name = if s == RTCPeerConnectionState::Disconnected { "DISCONNECTED" } else { "FAILED" };
                            guard.log.push(format!("!!! WebRTC {} with {} !!!", state_name, peer_id));
                            
                            // Remove keep-alive tracking if it failed/disconnected
                            guard.keep_alive_peers.remove(&peer_id);

                            // Attempt to rejoin the current session if allowed by tracker
                            if guard.reconnection_tracker.should_attempt(&peer_id) {
                                if let Some(current_session) = guard.session.clone() {
                                    let session_id_to_rejoin = current_session.session_id;
                                    guard.log.push(format!(
                                        "Attempting automatic rejoin to session '{}' due to {} state with {}",
                                        session_id_to_rejoin, state_name, peer_id
                                    ));
                                    
                                    // Drop the guard before sending the command
                                    drop(guard); 
                                    
                                    // Send JoinSession command to trigger rejoin process
                                    let _ = cmd_tx_local.send(ClientMsg::JoinSession { 
                                        session_id: session_id_to_rejoin 
                                    });

                                } else {
                                    // Log if no active session to rejoin
                                    guard.log.push(format!(
                                        "Cannot attempt automatic rejoin for {}: No active session.",
                                        peer_id
                                    ));
                                }
                            } else {
                                // Log if reconnection is skipped due to backoff/cooldown
                                guard.log.push(format!(
                                    "Skipping automatic rejoin attempt for {} (backoff/cooldown).",
                                    peer_id
                                ));
                            }
                        }
                    },
                    _ => {
                        // Log other states without special handling
                        if let Ok(mut app_state_guard) = state_log.try_lock() {
                            app_state_guard.log.push(format!("WebRTC state with {}: {:?}", peer_id, s));
                            // Update status map for TUI display
                            app_state_guard.peer_statuses.insert(peer_id.clone(), s); 
                        }
                    }
                }
                
                // Also update the status in state regardless of special handling
                // Ensure the status map reflects the latest state reported by the callback
                if let Ok(mut app_state_guard) = state_log.try_lock() {
                    app_state_guard.peer_statuses.insert(peer_id.clone(), s);
                }
                
                Box::pin(async {})
            }));

            let state_log_on_data = state_log.clone();
            let peer_id_on_data = peer_id.clone();
            pc_arc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
                // ... existing on_data_channel logic (logging, on_open, on_message) ...
                let state_log = state_log_on_data.clone();
                let peer_id = peer_id_on_data.clone();
                // FIX: Add actual log message
                state_log.lock().unwrap().log.push(format!(
                    "Receiver: Data channel '{}' opened by {}",
                    dc.label(),
                    peer_id
                ));

                let state_log_open = state_log.clone();
                let peer_id_open = peer_id.clone();
                dc.on_open(Box::new(move || {
                    // FIX: Add actual log message
                    state_log_open.lock().unwrap().log.push(format!(
                        "Receiver: Data channel open confirmed with {}",
                        peer_id_open
                    ));
                    Box::pin(async {})
                }));

                let state_log_msg = state_log.clone();
                let peer_id_msg = peer_id.clone();
                dc.on_message(Box::new(move |msg: DataChannelMessage| {
                    // FIX: Add actual log message
                    let msg_str = String::from_utf8(msg.data.to_vec())
                        .unwrap_or_else(|_| "Non-UTF8 data".to_string());
                    state_log_msg.lock().unwrap().log.push(format!(
                        "Receiver: Message from {}: {}",
                        peer_id_msg, msg_str
                    ));
                    Box::pin(async {})
                }));

                Box::pin(async move {})
            }));
            // --- End Setup Callbacks ---

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

// --- REFACTORED Helper Function: Initiate Offers ---
// Implements Perfect Negotiation logic (initiator side)
async fn initiate_offers_for_session(
    participants: Vec<String>,
    self_peer_id: String,
    peer_connections_arc: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    cmd_tx: mpsc::UnboundedSender<ClientMsg>,
    state_log: Arc<StdMutex<AppState>>, // Use state_log which is Arc<StdMutex<AppState>>
) {
    state_log
        .lock()
        .unwrap()
        .log
        .push("Initiating WebRTC offers check...".to_string()); // Renamed log

    // Lock connections once
    let peer_conns = peer_connections_arc.lock().await;
    state_log
        .lock()
        .unwrap()
        .log
        .push(format!("Found {} peer connection objects.", peer_conns.len()));


    for peer_id in participants {
        if peer_id == self_peer_id { // Removed unnecessary parentheses
            continue;
        }

        // --- Perfect Negotiation Role Check ---
        let should_initiate = self_peer_id < peer_id; // Impolite peer initiates

        state_log.lock().unwrap().log.push(format!(
            "Checking peer {}: Should initiate? {}", peer_id, should_initiate
        ));

        if should_initiate {
            if let Some(pc_arc) = peer_conns.get(&peer_id) {
                state_log.lock().unwrap().log.push(format!(
                    "Found PC object for peer {}", peer_id
                ));
                let current_state = pc_arc.connection_state();
                let signaling_state = pc_arc.signaling_state();

                // Check if negotiation is needed based on state
                let negotiation_needed = match current_state {
                    RTCPeerConnectionState::New |
                    RTCPeerConnectionState::Closed |
                    RTCPeerConnectionState::Disconnected |
                    RTCPeerConnectionState::Failed => true, // Need to negotiate if not connected/connecting
                    _ => match signaling_state {
                        webrtc::peer_connection::signaling_state::RTCSignalingState::Stable => false, // Assume stable means no immediate need
                        _ => false, // If already negotiating, don't start another one
                    }
                };

                state_log.lock().unwrap().log.push(format!(
                    "Peer {}: Negotiation needed? {} (State: {:?}/{:?})",
                    peer_id, negotiation_needed, current_state, signaling_state
                ));


                if !negotiation_needed {
                    // Log already exists, skipping continue
                    continue;
                }

                // --- Check making_offer flag ---
                let is_already_making_offer = state_log.lock().unwrap()
                                            .making_offer.get(&peer_id).copied().unwrap_or(false);

                state_log.lock().unwrap().log.push(format!(
                    "Peer {}: Already making offer? {}", peer_id, is_already_making_offer
                ));


                if is_already_making_offer {
                    // Log already exists, skipping continue
                    continue;
                }

                // --- Proceed to make offer ---
                state_log.lock().unwrap().log.push(format!(
                    "Proceeding to spawn offer task for peer {}", peer_id // Added log
                ));
                let pc_arc_clone = pc_arc.clone();
                let peer_id_clone = peer_id.clone();
                let state_log_clone = state_log.clone(); // Clone the Arc<StdMutex<AppState>>
                let cmd_tx_clone = cmd_tx.clone();

                tokio::spawn(async move { // Spawn task for actual offer creation
                    // --- Set making_offer flag ---
                    state_log_clone.lock().unwrap().making_offer.insert(peer_id_clone.clone(), true);
                    state_log_clone.lock().unwrap().log.push(format!("Set making_offer=true for {}", peer_id_clone));


                    // Prefix offer_result with _ to mark it as intentionally unused
                    let offer_result = async { // Wrap offer logic in an async block for easier cleanup
                        // ... (Create Data Channel logic remains the same) ...
                        // Add log before creating data channel
                        state_log_clone.lock().unwrap().log.push(format!(
                            "Offer Task [{}]: Creating data channel...", peer_id_clone
                        ));
                        // Example: Create a default data channel if none exists
                        // Note: This might interfere if the other side also tries to create it.
                        // Consider creating channels only *after* connection is stable, or use negotiation.
                        // For now, let's assume a default channel is needed for testing.
                        match pc_arc_clone.create_data_channel("data", None).await {
                            Ok(dc) => {
                                state_log_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Created data channel '{}'. Setting up callbacks.", peer_id_clone, dc.label()
                                ));
                                let dc_arc = Arc::new(dc);
                                // Setup basic logging callbacks for the created channel
                                let state_log_dc = state_log_clone.clone();
                                let peer_id_dc = peer_id_clone.clone();
                                dc_arc.on_open(Box::new(move || {
                                    state_log_dc.lock().unwrap().log.push(format!(
                                        "Offer Task [{}]: Data channel opened!", peer_id_dc
                                    ));
                                    Box::pin(async {})
                                }));
                                let state_log_dc_msg = state_log_clone.clone();
                                let peer_id_dc_msg = peer_id_clone.clone();
                                dc_arc.on_message(Box::new(move |msg: DataChannelMessage| {
                                     let msg_str = String::from_utf8(msg.data.to_vec()).unwrap_or_else(|_| "Non-UTF8 data".to_string());
                                     state_log_dc_msg.lock().unwrap().log.push(format!(
                                         "Offer Task [{}]: Received message: {}", peer_id_dc_msg, msg_str
                                     ));
                                    Box::pin(async {})
                                }));
                            }
                            Err(e) => {
                                 state_log_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Error creating data channel: {}", peer_id_clone, e
                                )); // Fixed: added missing closing parenthesis
                                // Decide if this is fatal for the offer attempt
                                // return Err(()); // Maybe don't fail the whole offer just for the data channel?
                            }
                        }


                        // --- Create and Send Offer ---
                        state_log_clone.lock().unwrap().log.push(format!(
                            "Offer Task [{}]: Creating offer...", peer_id_clone // Added log
                        ));
                        match pc_arc_clone.create_offer(None).await {
                            Ok(offer) => {
                                state_log_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Created offer. Setting local description...", peer_id_clone // Added log
                                ));
                                // Fix error logging for setting local description
                                if let Err(e) = pc_arc_clone.set_local_description(offer.clone()).await {
                                    state_log_clone.lock().unwrap().log.push(format!(
                                        "Offer Task [{}]: Error setting local description (offer): {}", // Restored log
                                        peer_id_clone, e
                                    ));
                                    return Err(()); // Indicate failure
                                }
                                state_log_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Set local description (offer). Serializing and sending...", // Restored log
                                    peer_id_clone
                                ));

                                let signal = WebRTCSignal::Offer(SDPInfo { sdp: offer.sdp });
                                match serde_json::to_value(signal) {
                                    Ok(json_val) => {
                                        // Fix ClientMsg::Relay initialization
                                        let _ = cmd_tx_clone.send(ClientMsg::Relay {
                                            to: peer_id_clone.clone(), // Restored field
                                            data: json_val,            // Restored field
                                        });
                                        state_log_clone
                                            .lock()
                                            .unwrap()
                                            .log
                                            .push(format!("Offer Task [{}]: Sent offer.", peer_id_clone)); // Restored log
                                    }
                                    // Fix error logging for offer serialization
                                    Err(e) => {
                                        state_log_clone.lock().unwrap().log.push(format!(
                                            "Offer Task [{}]: Error serializing offer: {}", peer_id_clone, e
                                        ));
                                        return Err(()); // Indicate failure
                                    }
                                }
                            }
                            // Fix error logging for offer creation
                            Err(e) => {
                                state_log_clone
                                    .lock()
                                    .unwrap()
                                    .log
                                    .push(format!("Offer Task [{}]: Error creating offer: {}", peer_id_clone, e)); // Restored log
                                return Err(()); // Indicate failure
                            }
                        }
                        Ok(()) // Indicate success
                    }.await;

                    // --- Clear making_offer flag regardless of success/failure ---
                    // Log success/failure based on offer_result
                    let outcome = if offer_result.is_ok() { "succeeded" } else { "failed" };
                    state_log_clone.lock().unwrap().log.push(format!(
                        "Offer Task [{}] {} negotiation.", peer_id_clone, outcome
                    ));
                    state_log_clone.lock().unwrap().making_offer.insert(peer_id_clone.clone(), false);
                    state_log_clone.lock().unwrap().log.push(format!("Set making_offer=false for {}", peer_id_clone));

                }); // End of spawned task

            } else {
                state_log.lock().unwrap().log.push(format!(
                    "Should initiate offer to {}, but connection object not found!", peer_id // Log remains valid
                ));
            }
        }
    }
    // peer_conns lock dropped here
    state_log
        .lock()
        .unwrap()
        .log
        .push("Finished WebRTC offers check.".to_string()); // Added log
}

// Apply any pending ICE candidates for a peer
async fn apply_pending_candidates(
    peer_id: &str,
    pc: Arc<RTCPeerConnection>,
    state_log: Arc<StdMutex<AppState>>,
) {
    // Take the pending candidates for this peer
    let candidates = {
        let mut state_guard = state_log.lock().unwrap();
        let pending = state_guard.pending_ice_candidates.remove(peer_id);
        if let Some(candidates) = &pending {
            if !candidates.is_empty() {
                state_guard.log.push(format!(
                    "Applying {} stored ICE candidate(s) for {}",
                    candidates.len(), peer_id
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
                        state_guard.log.push(format!("Applied stored ICE candidate for {}", peer_id));
                    }
                }
                Err(e) => {
                    if let Ok(mut state_guard) = state_log.lock() {
                        state_guard.log.push(format!(
                            "Error applying stored ICE candidate for {}: {}", peer_id, e
                        ));
                    }
                }
            }
        }
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
        cmd_tx: Some(cmd_tx.clone()), // 存储一个命令发送器副本
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
                                    peer_id, old_status.unwrap_or(RTCPeerConnectionState::New), current_state
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
                                            // ...existing log...
                                            match serde_json::from_value::<WebRTCSignal>(data.clone()) {
                                                Ok(signal) => {
                                                    // ...existing log...
                                                    // Clone necessary Arcs and data FOR the spawned task
                                                    let pc_arc_net_clone = peer_connections_arc_net.clone();
                                                    let from_clone = from.clone();
                                                    let cmd_tx_clone = cmd_tx_net.clone();
                                                    let state_log_clone = state_net.clone();
                                                    let signal_clone = signal.clone();
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
                                                                        let candidate_init = webrtc::ice_transport::ice_candidate::RTCIceCandidateInit {
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
                        _ => {} // Ignore other events like Mouse, Resize etc.
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
        let app_guard = state.lock().unwrap();
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(3), // Title
                    Constraint::Length(5), // Peers
                    Constraint::Min(5),    // Log
                    Constraint::Length(3), // Session/Invites
                    Constraint::Length(3), // Input Area
                ])
                .split(f.area());

            let title = format!("Peer ID: {}", app_guard.peer_id);

            // Determine which peers *should* be connected based on the session
            let session_participants: HashSet<String> = app_guard
                .session
                .as_ref()
                .map(|s| s.participants.iter().cloned().collect())
                .unwrap_or_default();

            // Update the TUI rendering logic for peer statuses to be more accurate
            let peer_list_items = app_guard
                .peers // Peers known via signaling
                .iter()
                .filter(|p| !p.trim().eq_ignore_ascii_case(app_guard.peer_id.trim()))
                .map(|p| {
                    let status = if session_participants.contains(p) {
                        // First check if there's an explicit status
                        if let Some(s) = app_guard.peer_statuses.get(p) {
                            // For clarity, add connection role in the status display
                            let role_prefix = if app_guard.peer_id < p.to_string() {
                                "→" // Initiator (outgoing connection)
                            } else {
                                "←" // Responder (incoming connection)
                            };
                            format!("{}{:?}", role_prefix, s)
                        } else {
                            // Default for session members not yet reported
                            "Pending".to_string()
                        }
                    } else {
                        // If not in session, they shouldn't be connected via WebRTC
                        "N/A".to_string()
                    };
                    ListItem::new(format!("{} ({})", p, status))
                })
                .collect::<Vec<_>>();

            let peers_widget =
                List::new(peer_list_items) // Use the formatted list
                    .block(Block::default().title("Peers").borders(Borders::ALL));

            let log = Paragraph::new(
                app_guard
                    .log
                    .iter()
                    .rev()
                    .take(chunks[2].height.saturating_sub(2) as usize) // Adjust log lines based on available height
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .block(Block::default().title("Log").borders(Borders::ALL));
            let session = if let Some(sess) = &app_guard.session {
                format!(
                    "Session: {} ({} of {}, threshold {})",
                    sess.session_id,
                    sess.participants.len(),
                    sess.total,
                    sess.threshold
                )
            } else {
                "No session".to_string()
            };
            // FIX: Use is_empty()
            let invites = if !app_guard.invites.is_empty() {
                format!(
                    "Invites: {}",
                    app_guard
                        .invites
                        .iter()
                        .map(|s| s.session_id.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                "No invites".to_string()
            };

            // Input box rendering based on mode
            let input_display_text = if input_mode {
                format!("> {}", input)
            } else {
                "Press 'i' to input, 'o' to accept invite, 'q' to quit".to_string()
            };
            let input_box = Paragraph::new(input_display_text)
                .block(Block::default().title("Input").borders(Borders::ALL));

            f.render_widget(
                Block::default().title(title).borders(Borders::ALL),
                chunks[0],
            );
            f.render_widget(peers_widget, chunks[1]); // Render the updated peers widget
            f.render_widget(log, chunks[2]);
            f.render_widget(
                Paragraph::new(format!("{}\n{}", session, invites)), // Keep original for now
                chunks[3],
            );
            // Render input box in its own chunk
            f.render_widget(input_box, chunks[4]);
        })?;
        // Drop the drawing lock before handling input
        drop(app_guard);

        // Handle input
        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Lock TUI state for input handling
                    let mut app_guard = state.lock().unwrap();
                    if input_mode {
                        match key.code {
                            KeyCode::Enter => {
                                let cmd_str = input.trim().to_string();
                                input.clear();
                                input_mode = false; // Exit input mode immediately
                                app_guard.log.push("Exited input mode.".to_string());
                                drop(app_guard); // Drop lock before potentially sending command

                                // Parse and handle command
                                // FIX: Use starts_with()
                                if cmd_str.starts_with("/list") {
                                    let _ = cmd_tx.send(ClientMsg::ListPeers);
                                // FIX: Use starts_with()
                                } else if cmd_str.starts_with("/create") {
                                    // ... existing /create logic ...
                                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                                    if parts.len() == 5 {
                                        if let (Ok(total), Ok(threshold)) =
                                            (parts[2].parse(), parts[3].parse())
                                        {
                                            let session_id = parts[1].to_string();
                                            let participants = parts[4]
                                                .split(',')
                                                .map(|s| s.to_string())
                                                .collect();
                                            let _ = cmd_tx.send(ClientMsg::CreateSession {
                                                session_id,
                                                total,
                                                threshold,
                                                participants,
                                            });
                                        } else {
                                            // Re-acquire lock to log error
                                            state.lock().unwrap().log.push(
                                                "Invalid total/threshold for /create.".to_string(),
                                            );
                                        }
                                    } else {
                                        // Re-acquire lock to log error
                                        state.lock().unwrap().log.push("Invalid /create format. Use: /create <id> <total> <threshold> <p1,p2,...>".to_string());
                                    }
                                // FIX: Use starts_with()
                                } else if cmd_str.starts_with("/join") {
                                    // ... existing /join logic ...
                                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                                    if parts.len() == 2 {
                                        let session_id = parts[1].to_string();
                                        let _ = cmd_tx.send(ClientMsg::JoinSession { session_id });
                                    } else {
                                        // Re-acquire lock to log error
                                        state.lock().unwrap().log.push(
                                            "Invalid /join format. Use: /join <session_id>"
                                                .to_string(),
                                        );
                                    }
                                // FIX: Use starts_with()
                                } else if cmd_str.starts_with("/invite") {
                                    // Need to re-acquire lock to check invites
                                    let app_guard_check = state.lock().unwrap();
                                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                                    if parts.len() == 2 {
                                        let session_id_to_join = parts[1].to_string();
                                        let invite_found = app_guard_check
                                            .invites
                                            .iter()
                                            .any(|s| s.session_id == session_id_to_join);
                                        drop(app_guard_check); // Drop lock before sending

                                        if invite_found {
                                            let _ = cmd_tx.send(ClientMsg::JoinSession {
                                                session_id: session_id_to_join,
                                            });
                                        } else {
                                            // Re-acquire lock to log error
                                            state.lock().unwrap().log.push(format!(
                                                "Invite '{}' not found.",
                                                session_id_to_join
                                            ));
                                        }
                                    } else {
                                        // Re-acquire lock to log error
                                        state.lock().unwrap().log.push(
                                            "Invalid /invite format. Use: /invite <session_id>"
                                                .to_string(),
                                        );
                                    }
                                // FIX: Use starts_with()
                                } else if cmd_str.starts_with("/relay") {
                                    // ... existing /relay logic ...
                                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                                    if parts.len() == 3 {
                                        let target_peer_id = parts[1].to_string();
                                        let json_str = parts[2];
                                        match serde_json::from_str::<serde_json::Value>(json_str) {
                                            Ok(data) => {
                                                let _ = cmd_tx.send(ClientMsg::Relay {
                                                    to: target_peer_id.clone(),
                                                    data,
                                                });
                                                // Re-acquire lock to log
                                                state.lock().unwrap().log.push(format!(
                                                    "Relaying message to {}",
                                                    target_peer_id
                                                ));
                                            }
                                            Err(e) => {
                                                // Re-acquire lock to log error
                                                state.lock().unwrap().log.push(format!(
                                                    "Invalid JSON for /relay: {}",
                                                    e
                                                ));
                                            }
                                        }
                                    } else {
                                        // Re-acquire lock to log error
                                        state.lock().unwrap().log.push("Invalid /relay format. Use: /relay <peer_id> <json_data>".to_string());
                                    }
                                // FIX: Use starts_with()
                                } else if cmd_str.starts_with("/send") {
                                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                                    if parts.len() >= 3 {
                                        let target_peer_id = parts[1].to_string();
                                        let message = parts[2].to_string();
                                        
                                        // 记录发送消息的尝试
                                        state.lock().unwrap().log.push(format!(
                                            "尝试发送消息到 {}: {}", 
                                            target_peer_id, message
                                        ));
                                        
                                        let state_clone = state.clone();
                                        let peer_connections_arc = state.lock().unwrap().peer_connections.clone();
                                        
                                        // 在单独的任务中发送消息
                                        tokio::spawn(async move {
                                            let pc_result = {
                                                let peer_conns = peer_connections_arc.lock().await;
                                                peer_conns.get(&target_peer_id).cloned()
                                            };
                                            
                                            if let Some(pc) = pc_result {
                                                // 检查连接状态
                                                let conn_state = pc.connection_state();
                                                
                                                if conn_state != RTCPeerConnectionState::Connected {
                                                    if let Ok(mut guard) = state_clone.try_lock() {
                                                        guard.log.push(format!(
                                                            "警告：与 {} 的连接处于 {:?} 状态，可能无法发送消息",
                                                            target_peer_id, conn_state
                                                        ));
                                                    }
                                                }
                                                
                                                // 改进方法: 直接创建一个专用的消息通道
                                                let channel_name = format!("msg-{}", SystemTime::now()
                                                    .duration_since(UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_millis());
                                                
                                                match pc.create_data_channel(&channel_name, None).await {
                                                    Ok(dc) => {
                                                        let dc_arc = Arc::new(dc);
                                                        let message_clone = message.clone();
                                                        let target_clone = target_peer_id.clone();
                                                        let state_log_clone = state_clone.clone();
                                                        
                                                        // 在on_open前克隆dc_arc以避免所有权问题
                                                        let dc_for_callback = dc_arc.clone();
                                                        
                                                        // 设置on_open处理器来发送消息
                                                        dc_arc.on_open(Box::new(move || {
                                                            let dc_ref = dc_for_callback;
                                                            let msg = message_clone.clone();
                                                            let target = target_clone.clone();
                                                            let state_log = state_log_clone.clone();
                                                            
                                                            tokio::spawn(async move {
                                                                if let Err(e) = dc_ref.send_text(msg.clone()).await {
                                                                    if let Ok(mut guard) = state_log.try_lock() {
                                                                        guard.log.push(format!(
                                                                            "发送消息到 {} 失败: {}",
                                                                            target, e
                                                                        ));
                                                                    }
                                                                } else {
                                                                    if let Ok(mut guard) = state_log.try_lock() {
                                                                        guard.log.push(format!("已成功发送消息到 {}: {}", target, msg));
                                                                    }
                                                                }
                                                                
                                                                // 消息发送后等待一会儿再关闭通道
                                                                tokio::time::sleep(Duration::from_secs(1)).await;
                                                                
                                                                // 通知远端我们已经完成消息发送
                                                                let _ = dc_ref.send_text("__COMPLETE__").await;
                                                            });
                                                            
                                                            Box::pin(async {})
                                                        }));
                                                        
                                                        // 添加消息接收处理以便记录确认等
                                                        dc_arc.on_message(Box::new(move |msg: DataChannelMessage| {
                                                            let msg_str = String::from_utf8(msg.data.to_vec())
                                                                .unwrap_or_else(|_| "Non-UTF8 data".to_string());
                                                            
                                                            if let Ok(mut guard) = state_clone.try_lock() {
                                                                guard.log.push(format!(
                                                                    "收到 {} 的消息通道回应: {}",
                                                                    target_peer_id, msg_str
                                                                ));
                                                            }
                                                            
                                                            Box::pin(async {})
                                                        }));
                                                    },
                                                    Err(e) => {
                                                        if let Ok(mut guard) = state_clone.try_lock() {
                                                            guard.log.push(format!(
                                                                "为发送到 {} 的消息创建数据通道出错: {}", 
                                                                target_peer_id, e
                                                            ));
                                                            guard.log.push("尝试重新建立连接...".to_string());
                                                        }
                                                        
                                                        // 如果创建通道失败，尝试发送重连请求
                                                        let timestamp = SystemTime::now()
                                                            .duration_since(UNIX_EPOCH)
                                                            .unwrap_or_default()
                                                            .as_secs();
                                                        
                                                        let recon_msg = serde_json::json!({
                                                            "type": "reconnect_request",
                                                            "timestamp": timestamp,
                                                            "from_send_command": true
                                                        });
                                                        
                                                        // 回到主线程发送重连请求
                                                        let state_clone_for_task = state_clone.clone();
                                                        tokio::spawn(async move {
                                                            let state_guard = state_clone_for_task.lock().unwrap();
                                                            let cmd_tx_clone = state_guard.cmd_tx.clone();
                                                            drop(state_guard); // Drop lock after modifying input
                                                            
                                                            if let Some(cmd_tx) = cmd_tx_clone {
                                                                let _ = cmd_tx.send(ClientMsg::Relay {
                                                                    to: target_peer_id.clone(),
                                                                    data: recon_msg,
                                                                });
                                                            }
                                                        });
                                                    }
                                                }
                                            } else {
                                                if let Ok(mut guard) = state_clone.try_lock() {
                                                    guard.log.push(format!(
                                                        "没有与 {} 的WebRTC连接存在",
                                                        target_peer_id
                                                    ));
                                                }
                                            }
                                        });
                                    } else {
                                        // Re-acquire lock to log error
                                        state.lock().unwrap().log.push("无效的 /send 格式。请使用: /send <peer_id> <message>".to_string());
                                    }
                                } else if !cmd_str.is_empty() {
                                    // Re-acquire lock to log unknown command
                                    state.lock().unwrap().log.push(format!(
                                        "Unknown command: {}", cmd_str
                                    ));
                                }
                            }
                            KeyCode::Char(c) => {
                                input.push(c);
                                drop(app_guard); // Drop lock after modifying input
                            }
                            KeyCode::Backspace => {
                                input.pop();
                                drop(app_guard); // Drop lock after modifying input
                            }
                            KeyCode::Esc => {
                                input_mode = false;
                                input.clear();
                                app_guard.log.push("Exited input mode (Esc).".to_string());
                                drop(app_guard); // Drop lock
                            }
                            _ => {
                                drop(app_guard);
                            } // Drop lock if no action taken
                        }
                    } else {
                        // Not in input mode
                        match key.code {
                            KeyCode::Char('i') => {
                                input_mode = true;
                                app_guard.log.push("Entered input mode.".to_string());
                                drop(app_guard); // Drop lock
                            }
                            KeyCode::Char('q') => {
                                app_guard.log.push("Quitting...".to_string());
                                drop(app_guard); // Drop lock
                                break; // Exit loop
                            }
                            KeyCode::Char('o') => {
                                // Lock only to check invites
                                let session_to_join = {
                                    // app_guard is already held
                                    app_guard.invites.first().map(|inv| inv.session_id.clone())
                                };

                                if let Some(session_id) = session_to_join {
                                    app_guard
                                        .log
                                        .push("Attempting to accept first invite...".to_string());
                                    drop(app_guard); // Drop lock before sending command
                                    let _ = cmd_tx.send(ClientMsg::JoinSession { session_id });
                                } else {
                                    app_guard
                                        .log
                                        .push("No invites to accept with 'o'".to_string());
                                    drop(app_guard); // Drop lock
                                }
                            }
                            _ => {
                                drop(app_guard);
                            } // Drop lock if no action taken
                        }
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
