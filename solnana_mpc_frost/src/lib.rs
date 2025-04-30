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
