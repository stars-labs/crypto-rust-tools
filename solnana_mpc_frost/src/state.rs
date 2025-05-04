use crate::signal::SessionInfo;
use frost_core::{
    Ciphersuite, Identifier,
    keys::{
        KeyPackage,
        PublicKeyPackage,
        dkg::{round1, round2}, // Import the specific DKG types
    },
};
use frost_ed25519::Ed25519Sha512; // Keep this for AppState generic default if needed elsewhere, or specifically in cli_node.rs
use std::sync::Arc; // Use TokioMutex for peer_connections
use std::time::{Duration, Instant}; // Import Duration and Instant
use tokio::sync::{Mutex as TokioMutex, mpsc}; // Use TokioMutex for async WebRTC state
use webrtc::{
    data_channel::RTCDataChannel, ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::RTCPeerConnection,
    peer_connection::peer_connection_state::RTCPeerConnectionState,
}; // Keep SessionInfo import

use std::{
    collections::{BTreeMap, HashMap, HashSet}, // Keep BTreeMap
                                               // Remove Arc import from here if only used for peer_connections
};

// --- DKG State Enum ---
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DkgState {
    Idle,
    Round1InProgress,
    Round1Complete, // All Round 1 packages received
    Round2InProgress,
    Complete,
    Failed(String),
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
    pub keep_alive_peers: HashSet<String>,
    // Perfect Negotiation Flags
    pub making_offer: HashMap<String, bool>,
    pub ignore_offer: HashMap<String, bool>,
    pub pending_ice_candidates: HashMap<String, Vec<RTCIceCandidateInit>>,
    // --- DKG State ---
    pub dkg_state: DkgState,
    pub identifier_map: Option<BTreeMap<String, Identifier<Ed25519Sha512>>>, // peer_id -> FROST Identifier
    // pub identifier_to_index_map: Option<BTreeMap<Identifier<C>, u16>>, // Removed field
    // Fix: Use proper round1::SecretPackage and round1::Package types
    pub local_dkg_part1_data: Option<(round1::SecretPackage<C>, round1::Package<C>)>,
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
    // Fix: Use proper round1::Package type
    pub queued_dkg_round1: Vec<(String, round1::Package<C>)>,
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
            cooldown: Duration::from_secs(10), // Cooldown period
            max_attempts: 5,                   // Max attempts before longer cooldown
        }
    }

    pub fn should_attempt(&mut self, peer_id: &str) -> bool {
        let now = Instant::now();
        let attempts = self.attempts.entry(peer_id.to_string()).or_insert(0);
        let last = self
            .last_attempt
            .entry(peer_id.to_string())
            .or_insert_with(|| now - self.cooldown * 2); // Ensure first attempt is allowed

        if *attempts >= self.max_attempts {
            // Implement exponential backoff or longer fixed cooldown after max attempts
            if now.duration_since(*last) < self.cooldown * (*attempts as u32) {
                return false; // Still in cooldown
            }
        } else if now.duration_since(*last) < self.cooldown {
            return false; // Still in cooldown
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

impl<C: Ciphersuite> AppState<C> {
    // If you have a `new` function, initialize the new field:
    // pub fn new(peer_id: String, /* other args */) -> Self {
    //     Self {
    //         // ... other initializations ...
    //         received_dkg_round2_packages: BTreeMap::new(),
    //         // ... other initializations ...
    //     }
    // }

    // Add helper if needed, e.g., to clear DKG state
    pub fn clear_dkg_state(&mut self) {
        self.dkg_state = DkgState::Idle;
        self.identifier_map = None;
        self.local_dkg_part1_data = None;
        self.received_dkg_packages.clear();
        self.round2_secret_package = None;
        self.received_dkg_round2_packages.clear(); // Clear the new field
        self.key_package = None;
        self.group_public_key = None;
        self.queued_dkg_round1.clear();
    }
}
