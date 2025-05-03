// Remove unused signal import
// use crate::signal::*; // Keep if signal types are used within state, otherwise remove
use solnana_mpc_frost::SessionInfo;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
// Add imports needed by AppState and ReconnectionTracker
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

pub struct AppState {
    pub peer_id: String,
    pub peers: Vec<String>,
    pub log: Vec<String>,
    pub session: Option<SessionInfo>,
    pub invites: Vec<SessionInfo>,
    // Use TokioMutex for async access needed by WebRTC callbacks
    pub peer_connections: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    // Store connection status for TUI display (under StdMutex)
    pub peer_statuses: HashMap<String, RTCPeerConnectionState>,
    // Flag to track if WebRTC setup has started for the current session
    pub reconnection_tracker: ReconnectionTracker,
    pub keep_alive_peers: HashSet<String>, // To track which peers are receiving keep-alives
    // Remove unused cmd_tx field
    // --- Perfect Negotiation Flags ---
    pub making_offer: HashMap<String, bool>, // Track if we are currently making an offer TO a specific peer
    pub ignore_offer: HashMap<String, bool>, // Track if we should ignore the next offer FROM a specific peer
    // Add a field to store pending ICE candidates
    pub pending_ice_candidates: HashMap<String, Vec<RTCIceCandidateInit>>,
}

// Define a struct to track reconnection attempts and prevent excessive reconnections
pub struct ReconnectionTracker {
    pub attempts: HashMap<String, usize>,
    pub timestamps: HashMap<String, Instant>,
    pub cooldown_until: HashMap<String, Option<Instant>>, // 新增冷却期字段
    pub max_sequential_attempts: usize,                   // 限制连续尝试次数
}

impl ReconnectionTracker {
    pub fn new() -> Self {
        Self {
            attempts: HashMap::new(),
            timestamps: HashMap::new(),
            cooldown_until: HashMap::new(),
            // Increase allowed attempts before cooldown
            max_sequential_attempts: 5,
        }
    }

    pub fn should_attempt(&mut self, peer_id: &str) -> bool {
        let now = Instant::now();

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
                self.cooldown_until
                    .insert(peer_id.to_string(), Some(now + Duration::from_secs(30)));
                return false;
            }

            return true;
        }

        false
    }

    // Remove unused reset method
    // pub fn reset(&mut self, peer_id: &str) {
    //     self.attempts.remove(peer_id);
    //     self.timestamps.remove(peer_id);
    //     self.cooldown_until.remove(peer_id);
    // }

    // 新增一个方法来记录重连成功，只重置尝试次数但保留时间戳
    pub fn record_success(&mut self, peer_id: &str) {
        self.attempts.insert(peer_id.to_string(), 0);
        self.cooldown_until.insert(peer_id.to_string(), None);
    }
}
