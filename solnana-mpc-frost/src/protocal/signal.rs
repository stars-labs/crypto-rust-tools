use serde::{Deserialize, Serialize};

use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
// Import the DKG Package type
use frost_core::keys::dkg::round1::Package as DkgRound1Package;
use frost_ed25519::Ed25519Sha512; // Needed for type parameter

// Import round1 and round2 packages
use frost_core::keys::dkg::round2;

// --- Session Info Struct ---
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    pub proposer_id: String, // Added field
    pub total: u16,
    pub threshold: u16,
    pub participants: Vec<String>,
    pub accepted_peers: Vec<String>, // List of peer_ids that have accepted
}

// --- WebRTC Signaling Data (sent via Relay) ---
#[derive(Serialize, Deserialize, Debug, Clone)]
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
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "websocket_msg_type")]
pub enum WebSocketMessage {
    // Relay Messages
    /// Session proposal message
    SessionProposal(SessionProposal),
    /// Session response message
    SessionResponse(SessionResponse),
    WebRTCSignal(WebRTCSignal),
}

/// Session proposal information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionProposal {
    pub session_id: String,
    pub total: u16,
    pub threshold: u16,
    pub participants: Vec<String>,
}

/// Session response information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub accepted: bool,
}

// --- Application-Level Messages (sent over established WebRTC Data Channel) ---
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "webrtc_msg_type")]
pub enum WebRTCMessage {
    // DKG Messages
    SimpleMessage {
        text: String,
    },
    DkgRound1Package {
        // Use the imported Package type directly (it derives Serialize/Deserialize)
        package: DkgRound1Package<Ed25519Sha512>,
    },
    // Add other message types as needed (e.g., for signing)
    DkgRound2Package {
        // Add this variant
        package: round2::Package<Ed25519Sha512>,
    },
    /// Data channel opened notification
    ChannelOpen {
        peer_id: String,
    },

    /// Mesh readiness notification
    MeshReady {
        session_id: String,
        peer_id: String,
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
