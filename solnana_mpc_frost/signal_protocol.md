# Signal and WebRTC Message Types

This document defines the JSON message types and protocol flow for negotiating and creating an MPC wallet using a signaling server. Nodes may run as CLI applications (Linux) or as Chrome extensions. The signaling server coordinates peer discovery and WebRTC connection setup; all MPC protocol messages are exchanged over WebRTC.

---

## Protocol Overview

1. **Node Registration:**  
   Each node (CLI or Chrome extension) connects to the signaling server via WebSocket and registers with a unique `peer_id`.

2. **Discovery:**  
   Nodes query the signaling server for available peers or a specific MPC session.

3. **Session Negotiation:**  
   Nodes agree on session parameters (e.g., total participants, threshold, session ID).

4. **Signaling Exchange:**  
   Nodes exchange WebRTC signaling data (SDP offers/answers, ICE candidates) via the signaling server to establish direct peer-to-peer connections.

5. **MPC Wallet Creation:**  
   Once WebRTC connections are established, nodes exchange MPC protocol messages (commitments, shares, etc.) directly.

---

## WebSocket (Signaling Server) Message Types

### 1. Registration

**Client → Server**
```json
{ "register": "<peer_id>" }
```
Registers the client with the signaling server using a unique `peer_id`.

---

### 2. Peer Discovery

**Client → Server**
```json
{ "list_peers": true }
```
Requests a list of currently registered peers.

**Server → Client**
```json
{ "peers": ["peer1", "peer2", ...] }
```
Returns the list of available peers.

---

### 3. Session Negotiation

**Client → Server**
```json
{ "create_session": { "session_id": "<id>", "total": 3, "threshold": 2, "participants": ["peer1", "peer2", "peer3"] } }
```
Creates a new MPC session.

**Client → Server**
```json
{ "join_session": { "session_id": "<id>" } }
```
Joins an existing session.

**Server → Client**
```json
{ "session_info": { "session_id": "<id>", "total": 3, "threshold": 2, "participants": ["peer1", "peer2", "peer3"] } }
```
Confirms session creation or join.

---

### 4. Signaling Relay

**Client → Server**
```json
{ "to": "<peer_id>", "data": { ... } }
```
Sends signaling data (SDP offer/answer, ICE candidate, etc.) to another peer via the server.

**Server → Client**
```json
{ "from": "<peer_id>", "data": { ... } }
```
Relays signaling data from another peer.

---

### 5. Error

**Server → Client**
```json
{ "error": "<description>" }
```
Sent if an error occurs (e.g., unknown peer, session error).

---

## WebRTC (Peer-to-Peer) Message Types

Once a direct WebRTC connection is established, nodes exchange application-level messages for the MPC protocol.

### 1. Application Message

**Peer ↔ Peer**
```json
{ "type": "<msg_type>", "payload": { ... } }
```
- `type`: String describing the message purpose (e.g., `"commitment"`, `"share"`, `"ready"`, `"session_info"`, etc.).
- `payload`: Message-specific data.

**Examples:**
```json
{ "type": "commitment", "payload": { "data": "<hex-encoded-commitment>" } }
{ "type": "share", "payload": { "data": "<hex-encoded-share>" } }
{ "type": "ready", "payload": { "session_id": "<id>" } }
```

---

### 2. Acknowledgement

**Peer ↔ Peer**
```json
{ "type": "ack", "payload": { "msg_id": "<id>" } }
```
Used to acknowledge receipt of a message, if needed.

---

## Protocol Flow

### 1. Registration & Discovery

- Each node connects to the signaling server and registers with a unique `peer_id`.
- Nodes may request a list of available peers or sessions.

### 2. Session Negotiation

- One node (initiator) creates a session, specifying `session_id`, `total`, `threshold`, and participant list.
- Other nodes join the session using `session_id`.
- The server confirms session creation/join and notifies all participants.

### 3. WebRTC Signaling

- Each node exchanges signaling messages (`offer`, `answer`, `ice-candidate`) via the signaling server to establish direct WebRTC connections with other participants.

### 4. MPC Wallet Creation

- Once WebRTC connections are established, nodes exchange MPC protocol messages:
  - **Commitments:** Each node broadcasts its commitment.
  - **Shares:** Each node sends shares to others.
  - **Ready:** Each node signals readiness to proceed.
  - **Other MPC messages:** As required by the protocol.

- All MPC messages are sent as JSON objects over the WebRTC data channel.

### 5. Completion

- When the protocol completes, nodes may exchange a final message (e.g., `wallet_created`) and disconnect.

---

## Summary Table

| Context    | Direction         | Message Example                                 | Purpose                      |
|------------|-------------------|------------------------------------------------|------------------------------|
| WebSocket  | Client → Server   | `{ "register": "peer1" }`                      | Register with signal server  |
| WebSocket  | Client → Server   | `{ "list_peers": true }`                       | List available peers         |
| WebSocket  | Client → Server   | `{ "create_session": { ... } }`                | Create MPC session           |
| WebSocket  | Client → Server   | `{ "join_session": { ... } }`                  | Join MPC session             |
| WebSocket  | Server → Client   | `{ "session_info": { ... } }`                  | Session info/confirmation    |
| WebSocket  | Client ↔ Server   | `{ "to": "peer2", "data": { ... } }`           | Relay signaling data         |
| WebSocket  | Server → Client   | `{ "from": "peer2", "data": { ... } }`         | Receive signaling data       |
| WebRTC     | Peer ↔ Peer       | `{ "type": "commitment", "payload": { ... } }` | Commitment message           |
| WebRTC     | Peer ↔ Peer       | `{ "type": "share", "payload": { ... } }`      | Share message                |
| WebRTC     | Peer ↔ Peer       | `{ "type": "ready", "payload": { ... } }`      | Ready message                |
| WebRTC     | Peer ↔ Peer       | `{ "type": "ack", "payload": { "msg_id": "1" } }` | Acknowledgement           |

---

## Simulation Run Example

Below is a step-by-step simulation of an MPC wallet creation session involving three nodes:  
- `cli1` (Linux CLI)  
- `cli2` (Linux CLI)  
- `chrome1` (Chrome extension)

### 1. Registration

Each node connects to the signaling server and registers:

**cli1 → server**
```json
{ "register": "cli1" }
```
**cli2 → server**
```json
{ "register": "cli2" }
```
**chrome1 → server**
```json
{ "register": "chrome1" }
```

---

### 2. Peer Discovery

Each node requests the list of available peers:

**cli1 → server**
```json
{ "list_peers": true }
```
**server → cli1**
```json
{ "peers": ["cli1", "cli2", "chrome1"] }
```

---

### 3. Session Negotiation

**cli1** creates a session and invites the others:

**cli1 → server**
```json
{
  "create_session": {
    "session_id": "session123",
    "total": 3,
    "threshold": 2,
    "participants": ["cli1", "cli2", "chrome1"]
  }
}
```
**server → cli1**
```json
{
  "session_info": {
    "session_id": "session123",
    "total": 3,
    "threshold": 2,
    "participants": ["cli1", "cli2", "chrome1"]
  }
}
```
**Session Invitation Notification**

When a session is created and participants are specified, the server should proactively notify all listed participants (except the creator) about the invitation. This allows clients to display a prompt or automatically join.

**server → cli2**
```json
{ "session_invite": { "session_id": "session123", "from": "cli1", "total": 3, "threshold": 2, "participants": ["cli1", "cli2", "chrome1"] } }
```
**server → chrome1**
```json
{ "session_invite": { "session_id": "session123", "from": "cli1", "total": 3, "threshold": 2, "participants": ["cli1", "cli2", "chrome1"] } }

```

**cli2 → server**
```json
{ "join_session": { "session_id": "session123" } }
```
**chrome1 → server**
```json
{ "join_session": { "session_id": "session123" } }
```
**server → cli2**
```json
{
  "session_info": {
    "session_id": "session123",
    "total": 3,
    "threshold": 2,
    "participants": ["cli1", "cli2", "chrome1"]
  }
}
```
**server → chrome1**
```json
{
  "session_info": {
    "session_id": "session123",
    "total": 3,
    "threshold": 2,
    "participants": ["cli1", "cli2", "chrome1"]
  }
}
```

### 4. WebRTC Signaling

Each node exchanges signaling messages to establish direct connections.

**cli1 → server**
```json
{ "to": "cli2", "data": { "type": "offer", "sdp": "<sdp-offer>" } }
```
**server → cli2**
```json
{ "from": "cli1", "data": { "type": "offer", "sdp": "<sdp-offer>" } }
```
**cli2 → server**
```json
{ "to": "cli1", "data": { "type": "answer", "sdp": "<sdp-answer>" } }
```
**server → cli1**
```json
{ "from": "cli2", "data": { "type": "answer", "sdp": "<sdp-answer>" } }
```
*...similar signaling for cli1 ↔ chrome1, cli2 ↔ chrome1, including ICE candidates...*

---

#### How do we know the P2P connection mesh is created?

Each node should track the state of its WebRTC data channels with all other participants. The mesh is considered established when every node has an open and ready data channel to every other participant.

**Recommended approach:**

- When a WebRTC data channel is successfully opened to a peer, send a message:
    ```json
    { "type": "channel_open", "payload": { "peer_id": "<peer_id>" } }
    ```
- Each node maintains a set of connected peer IDs.
- When the set of connected peers matches the expected participant list (excluding self), the mesh is complete for that node.
- Optionally, after all channels are open, each node can send a "mesh_ready" message to all peers:
    ```json
    { "type": "mesh_ready", "payload": { "session_id": "<session_id>" } }
    ```
- When a node receives "mesh_ready" from all other participants, it can proceed to the MPC protocol.

**Example:**
```json
{ "type": "channel_open", "payload": { "peer_id": "cli2" } }
{ "type": "channel_open", "payload": { "peer_id": "chrome1" } }
{ "type": "mesh_ready", "payload": { "session_id": "session123" } }
```

This handshake ensures all nodes are aware that the full P2P mesh is established before starting MPC.

---

### 5. MPC Wallet Creation (over WebRTC)

Once WebRTC connections are established, nodes exchange MPC protocol messages directly.

**cli1 → cli2**
```json
{ "type": "commitment", "payload": { "data": "<hex-commitment-cli1>" } }
```
**cli2 → chrome1**
```json
{ "type": "commitment", "payload": { "data": "<hex-commitment-cli2>" } }
```
**chrome1 → cli1**
```json
{ "type": "commitment", "payload": { "data": "<hex-commitment-chrome1>" } }
```

**cli1 → cli2**
```json
{ "type": "share", "payload": { "data": "<hex-share-cli1-to-cli2>" } }
```
**cli2 → chrome1**
```json
{ "type": "share", "payload": { "data": "<hex-share-cli2-to-chrome1>" } }
```
*...and so on for all shares...*

**cli1 → cli2**
```json
{ "type": "ready", "payload": { "session_id": "session123" } }
```
*...all nodes send "ready" when prepared to proceed...*

---

### 6. Completion

When the protocol completes, nodes may send a final message:

**cli1 → cli2**
```json
{ "type": "wallet_created", "payload": { "session_id": "session123", "pubkey": "<hex-group-pubkey>" } }
```

---

## Notes

- **Peer IDs:** Should be unique per node (e.g., UUID, public key, or user-chosen name).
- **Session IDs:** Should be unique per MPC wallet creation session.
- **Security:** All sensitive data should be encrypted as appropriate for your application.
- **Extensibility:** You may add additional message types as needed for your MPC protocol.

---
