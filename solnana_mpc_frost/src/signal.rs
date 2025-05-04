use serde::{Deserialize, Serialize};
use serde_json::Value;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
// Import the DKG Package type
use frost_core::keys::dkg::round1::Package as DkgRound1Package;
use frost_ed25519::Ed25519Sha512; // Needed for type parameter

// Import round1 and round2 packages
use frost_core::keys::dkg::round2;

// --- Client -> Server Messages ---
#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMsg {
    Register {
        peer_id: String,
    },
    ListPeers,
    CreateSession {
        session_id: String,
        total: u16,
        threshold: u16,
        participants: Vec<String>,
    },
    JoinSession {
        session_id: String,
    },
    Relay {
        to: String,
        data: Value, // Can be WebRTCSignal or other JSON
    },
    // Add a message for direct WebRTC sending (though handled differently)
    SendDirect {
        to: String,
        message: String, // Or a more structured message type
    },
}

// --- Server -> Client Messages ---
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ServerMsg {
    Peers {
        peers: Vec<String>,
    },
    SessionInfo {
        session_id: String,
        total: u16,
        threshold: u16,
        participants: Vec<String>,
    },
    SessionInvite {
        session_id: String,
        from: String,
        total: u16,
        threshold: u16,
        participants: Vec<String>,
    },
    Relay {
        from: String,
        data: Value,
    },
    Error {
        error: String,
    },
}

// --- Session Info Struct ---
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub total: u16,
    pub threshold: u16,
    pub participants: Vec<String>,
}

// --- WebRTC Signaling Data (sent via Relay) ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "signal")]
pub enum WebRTCSignal {
    Offer(SDPInfo),
    Answer(SDPInfo),
    Candidate(CandidateInfo),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SDPInfo {
    pub sdp: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CandidateInfo {
    pub candidate: String,
    #[serde(rename = "sdpMid")]
    pub sdp_mid: Option<String>,
    #[serde(rename = "sdpMLineIndex")]
    pub sdp_mline_index: Option<u16>,
}

// --- Application-Level Messages (sent over established WebRTC Data Channel) ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "msg_type")]
pub enum WebRTCMessage {
    // DKG Messages
    DkgRound1Package {
        // Use the imported Package type directly (it derives Serialize/Deserialize)
        package: DkgRound1Package<Ed25519Sha512>,
    },
    // Add other message types as needed (e.g., for signing)
    SimpleMessage {
        text: String,
    },
    DkgRound2Package { // Add this variant
        package: round2::Package<Ed25519Sha512>,
    },
}

// Helper to convert RTCIceCandidate to CandidateInfo
impl From<RTCIceCandidateInit> for CandidateInfo {
    fn from(init: RTCIceCandidateInit) -> Self {
        CandidateInfo {
            candidate: init.candidate,
            sdp_mid: init.sdp_mid,
            sdp_mline_index: init.sdp_mline_index,
        }
    }
}

// Helper to convert RTCSessionDescription to SDPInfo
impl From<RTCSessionDescription> for SDPInfo {
    fn from(desc: RTCSessionDescription) -> Self {
        SDPInfo { sdp: desc.sdp }
    }
}
