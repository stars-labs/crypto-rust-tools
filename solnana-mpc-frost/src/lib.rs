use frost_core::keys::dkg::{round1, round2}; // Add round2 import
use frost_ed25519::Ed25519Sha512;

use webrtc_signal_server::ClientMsg;
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
}

pub enum NodeState {
    DisconnectedFromSignalServer,
    ConnectedToSignalServer,
    InSession,
    AllPeersConnected,
    DkgStarted,
    DkgCompleted,
    WaitingForSign,
}
