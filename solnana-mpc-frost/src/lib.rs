use frost_core::keys::dkg::{round1, round2}; // Add round2 import
use frost_ed25519::Ed25519Sha512;
use webrtc_signal_server::ClientMsg as SharedClientMsg;

#[derive(Debug)]
pub enum InternalCommand {
    /// Send a message to the signaling server
    SendToServer(SharedClientMsg),

    /// Send a direct WebRTC message to a peer
    SendDirect { to: String, message: String },

    /// Propose a new MPC session (replacing the old CreateSession)
    ProposeSession {
        session_id: String,
        total: u16,
        threshold: u16,
        participants: Vec<String>,
    },

    /// Accept a session proposal by session ID
    AcceptSessionProposal(String),

    /// Report that a data channel has been opened with a peer
    ReportChannelOpen { peer_id: String },

    /// Signal mesh readiness to all peers
    MeshReady,

    /// Process mesh ready notification from a peer
    ProcessMeshReady { peer_id: String },

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
    CommitmentsInProgress, // Keep original variants for backward compatibility
    CommitmentsComplete,
    SharesInProgress,
    VerificationInProgress,
    Complete,
    Failed(String),
}

/// Mesh status tracking enum
#[derive(Debug, PartialEq, Clone)]
pub enum MeshStatus {
    Incomplete,
    PartiallyReady(usize, usize), // (ready_count, total_count)
    Ready,
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
