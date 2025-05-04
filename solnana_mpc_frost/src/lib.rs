use frost_core::keys::dkg::{round1, round2}; // Add round2 import
use frost_ed25519::Ed25519Sha512;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub total: usize,
    pub threshold: usize,
    pub participants: Vec<String>,
    // Note: 'joined' field is only used server-side, so it's not included here
    // If needed client-side later, it can be added optionally.
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Peers {
        peers: Vec<String>,
    },
    SessionInfo {
        session_id: String,
        total: usize,
        threshold: usize,
        participants: Vec<String>,
    },
    SessionInvite {
        session_id: String,
        from: String,
        total: usize,
        threshold: usize,
        participants: Vec<String>,
    },
    Relay {
        from: String,
        data: serde_json::Value,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Register {
        peer_id: String,
    },
    ListPeers,
    CreateSession {
        session_id: String,
        total: usize,
        threshold: usize,
        participants: Vec<String>,
    },
    JoinSession {
        session_id: String,
    },
    Relay {
        to: String,
        data: serde_json::Value,
    },
}

// --- Define NEW internal command enum ---
// This enum is used for commands passed *within* the cli_node application,
// not for messages sent over the network via WebSocket.
#[derive(Debug)]
pub enum InternalCommand {
    SendDirect {
        // For sending via WebRTC
        to: String,
        message: String,
    },
    SendToServer(ClientMsg), // Wrap the message intended for the server
    TriggerDkgRound1,
    ProcessDkgRound1 {
        from_peer_id: String,
        package: round1::Package<Ed25519Sha512>, // Ensure DkgRound1Package is imported
    },
    ProcessDkgRound2 {
        from_peer_id: String,
        package: round2::Package<Ed25519Sha512>,
    },
    // Add other internal commands if needed
}
