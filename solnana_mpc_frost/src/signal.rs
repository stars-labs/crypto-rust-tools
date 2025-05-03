use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
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
