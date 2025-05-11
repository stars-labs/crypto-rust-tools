use crate::protocal::signal::SessionInfo;
use frost_core::{
    Ciphersuite, Identifier,
    keys::{
        KeyPackage,
        PublicKeyPackage,
        dkg::{round1, round2}, // Import the specific DKG types
    },
};

use std::sync::Arc; // Use TokioMutex for peer_connections
use std::time::{Duration, Instant}; // Import Duration and Instant
use tokio::sync::Mutex as TokioMutex; // Use TokioMutex for async WebRTC state
use webrtc::{
    data_channel::RTCDataChannel, ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::RTCPeerConnection,
    peer_connection::peer_connection_state::RTCPeerConnectionState,
}; // Keep SessionInfo import

use std::{
    collections::{BTreeMap, HashMap, HashSet}, // Keep BTreeMap
                                               // Remove Arc import from here if only used for peer_connections
};

use frost_ed25519::Ed25519Sha512;
use webrtc_signal_server::ClientMsg as SharedClientMsg;
// Add this import

use crate::protocal::signal::SessionResponse;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum InternalCommand {
    /// Send a message to the signaling server
    SendToServer(SharedClientMsg),

    /// Send a direct WebRTC message to a peer
    SendDirect {
        to: String,
        message: String,
    },

    /// Propose a new MPC session (replacing the old CreateSession)
    ProposeSession {
        session_id: String,
        total: u16,
        threshold: u16,
        participants: Vec<String>,
    },

    /// Accept a session proposal by session ID
    AcceptSessionProposal(String),

    /// Process a session response from a peer
    ProcessSessionResponse {
        from_peer_id: String,
        response: SessionResponse,
    },

    /// Report that a data channel has been opened with a peer
    ReportChannelOpen {
        peer_id: String,
    },

    // MeshReady, // This variant is redundant and has been removed. Use SendOwnMeshReadySignal.
    SendOwnMeshReadySignal,
    /// Process mesh ready notification from a peer
    ProcessMeshReady {
        peer_id: String,
    },

    /// Check if conditions are met to trigger DKG and do so if appropriate
    CheckAndTriggerDkg,

    /// Trigger DKG Round 1 (Commitments)
    TriggerDkgRound1,

    /// Process DKG Round 1 data from a peer
    ProcessDkgRound1 {
        from_peer_id: String,
        package: round1::Package<Ed25519Sha512>,
    },

    /// Trigger DKG Round 2 (Shares)
    TriggerDkgRound2,

    /// Process DKG Round 2 data from a peer
    ProcessDkgRound2 {
        from_peer_id: String,
        package: round2::Package<Ed25519Sha512>,
    },

    /// Finalize the DKG process
    FinalizeDkg,
}

/// DKG status tracking enum
#[derive(Debug, PartialEq, Clone)]
pub enum DkgState {
    Idle,
    Round1InProgress, // Same as CommitmentsInProgress but with naming used in other files
    Round1Complete,   // All Round 1 packages received
    Round2InProgress, // Same as SharesInProgress but with naming used in other files
    Round2Complete,   // All Round 2 packages received
    Finalizing,
    Complete,
    Failed(String),
}

/// Mesh status tracking enum
#[derive(Debug, PartialEq, Clone)]
pub enum MeshStatus {
    Incomplete,
    PartiallyReady {
        ready_peers: HashSet<String>,
        total_peers: usize,
    },
    Ready,
}

// DkgStateDisplay trait - defines display behavior for DkgState
pub trait DkgStateDisplay {
    fn display_status(&self) -> String;
    fn is_active(&self) -> bool;
    fn is_completed(&self) -> bool;
}

// Implement the trait for the imported DkgState
impl DkgStateDisplay for DkgState {
    fn display_status(&self) -> String {
        match self {
            DkgState::Idle => "Idle".to_string(),
            DkgState::Round1InProgress => "Round 1 In Progress".to_string(),
            DkgState::Round1Complete => "Round 1 Complete".to_string(),
            DkgState::Round2InProgress => "Round 2 In Progress".to_string(),
            DkgState::Round2Complete => "Round 2 Complete".to_string(),
            DkgState::Finalizing => "Finalizing".to_string(),
            DkgState::Complete => "DKG Complete".to_string(),
            DkgState::Failed(reason) => format!("Failed: {}", reason),
        }
    }

    fn is_active(&self) -> bool {
        matches!(
            self,
            DkgState::Round1InProgress
                | DkgState::Round1Complete
                | DkgState::Round2InProgress
                | DkgState::Round2Complete
                | DkgState::Finalizing
        )
    }

    fn is_completed(&self) -> bool {
        matches!(self, DkgState::Complete)
    }
}

// --- AppState Struct ---
#[derive(Clone)]
pub struct AppState<C: Ciphersuite> {
    pub peer_id: String,
    pub peers: Vec<String>,
    pub log: Vec<String>,
    pub log_scroll: u16, // Add scroll state for the log
    pub session: Option<SessionInfo>,
    pub invites: Vec<SessionInfo>, // Store full SessionInfo for invites
    // WebRTC related state (needs TokioMutex for async access)
    pub peer_connections: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    // TUI related state (can use StdMutex)
    pub peer_statuses: HashMap<String, RTCPeerConnectionState>,
    pub reconnection_tracker: ReconnectionTracker,
    // Perfect Negotiation Flags
    pub making_offer: HashMap<String, bool>,
    pub pending_ice_candidates: HashMap<String, Vec<RTCIceCandidateInit>>,
    // --- DKG State ---
    pub dkg_state: DkgState,
    pub identifier_map: Option<BTreeMap<String, Identifier<C>>>, // peer_id -> FROST Identifier
    // pub identifier_to_index_map: Option<BTreeMap<Identifier<C>, u16>>, // Removed field
    // Fix: Use proper round1::SecretPackage and round1::Package types
    pub dkg_part1_public_package: Option<round1::Package<C>>,
    pub dkg_part1_secret_package: Option<round1::SecretPackage<C>>,
    // Fix: Store received round1 packages with correct type
    pub received_dkg_packages: BTreeMap<Identifier<C>, round1::Package<C>>,
    pub round2_secret_package: Option<round2::SecretPackage<C>>, // Secret needed for Part 3
    // Fix: Store received round2 packages with correct type
    pub received_dkg_round2_packages: BTreeMap<Identifier<C>, round2::Package<C>>,
    pub key_package: Option<KeyPackage<C>>,
    pub group_public_key: Option<PublicKeyPackage<C>>, // Use PublicKeyPackage from frost_core
    // Add data channels mapping
    pub data_channels: HashMap<String, Arc<RTCDataChannel>>,
    // Add Solana public key
    pub solana_public_key: Option<String>,
    pub mesh_status: MeshStatus,
}

// --- Reconnection Tracker ---
#[derive(Debug, Clone)]
pub struct ReconnectionTracker {
    attempts: HashMap<String, usize>,
    last_attempt: HashMap<String, Instant>,
    cooldown: Duration,
    max_attempts: usize,
}

impl ReconnectionTracker {
    pub fn new() -> Self {
        ReconnectionTracker {
            attempts: HashMap::new(),
            last_attempt: HashMap::new(),
            cooldown: Duration::from_secs(5), // Reduced from 10 to 5 seconds for faster recovery
            max_attempts: 10, // Increased from 5 to 10 for more persistent reconnection
        }
    }

    pub fn should_attempt(&mut self, peer_id: &str) -> bool {
        let now = Instant::now();
        let attempts = self.attempts.entry(peer_id.to_string()).or_insert(0);
        let last = self
            .last_attempt
            .entry(peer_id.to_string())
            .or_insert_with(|| now - self.cooldown * 2); // Ensure first attempt is allowed

        // For first few attempts, retry quickly
        if *attempts < 3 {
            // Almost no cooldown for the first few attempts
            if now.duration_since(*last) < Duration::from_millis(500) {
                return false;
            }
        } else if *attempts >= self.max_attempts {
            // Use exponential backoff with a cap after max attempts
            let backoff = self
                .cooldown
                .mul_f32(1.5_f32.powi(*attempts as i32 - self.max_attempts as i32));
            let capped_backoff = std::cmp::min(backoff, Duration::from_secs(60)); // Cap at 1 minute

            if now.duration_since(*last) < capped_backoff {
                return false; // Still in cooldown
            }
        } else {
            // Linear backoff between the first few attempts and max attempts
            if now.duration_since(*last) < self.cooldown.mul_f32(*attempts as f32 / 2.0) {
                return false; // Still in cooldown
            }
        }

        *attempts += 1;
        *last = now;
        true
    }

    pub fn record_success(&mut self, peer_id: &str) {
        self.attempts.remove(peer_id);
        self.last_attempt.remove(peer_id);
    }
}
