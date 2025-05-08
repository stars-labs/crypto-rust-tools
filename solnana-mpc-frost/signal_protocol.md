# Signal and WebRTC Message Types

This document defines the JSON message types and protocol flow for negotiating and creating an MPC wallet using a signaling server. Nodes may run as CLI applications (Linux) or as Chrome extensions. The signaling server coordinates peer discovery and WebRTC connection setup; all MPC protocol messages are exchanged over WebRTC.

---

## Protocol Overview

1. **Node Registration:**  
   Each node (CLI or Chrome extension) connects to the signaling server via WebSocket and registers with a unique `peer_id`.

2. **Discovery:**  
   Nodes query the signaling server for available peers.

3. **Session Negotiation & Mesh Formation:**  
   Nodes coordinate session parameters (e.g., total participants, threshold, session ID) and build the mesh themselves. The signaling server does **not** store or manage session state.  
   Each node maintains its own session state and uses a spinning tree or broadcast approach to sense and build the mesh.

4. **Signaling Exchange:**  
   Nodes exchange WebRTC signaling data (SDP offers/answers, ICE candidates) via the signaling server to establish direct peer-to-peer connections.

5. **MPC Wallet Creation:**  
   Once WebRTC connections are established, nodes exchange MPC protocol messages (commitments, shares, etc.) directly.

---

## WebSocket (Signaling Server) Message Types

### 1. Registration

**Client → Server**
```json
{ "type": "register", "peer_id": "<peer_id>" }
```
Registers the client with the signaling server using a unique `peer_id`.

---

### 2. Peer Discovery

**Client → Server**
```json
{ "type": "list_peers" }
```
Requests a list of currently registered peers.

**Server → Client**
```json
{ "type": "peers", "peers": ["peer1", "peer2", ...] }
```
Returns the list of available peers.

---

### 3. Session Negotiation & Mesh Formation

**Note:**  
The signaling server does **not** store or manage session state.  
Nodes must coordinate session creation, joining, and participant lists among themselves.  
A spinning tree or broadcast protocol is used by nodes to sense the network and build the mesh.

- Nodes broadcast their intent to participate in a session (e.g., via WebRTC or relayed messages).
- Each node maintains its own session state and tracks the mesh topology.
- Nodes use peer discovery and direct communication to build the full mesh.

---

### 4. Signaling Relay

**Client → Server**
```json
{ "type": "relay", "to": "<peer_id>", "data": { ... } }
```
Sends signaling data (SDP offer/answer, ICE candidate, etc.) to another peer via the server.

**Server → Client**
```json
{ "type": "relay", "from": "<peer_id>", "data": { ... } }
```
Relays signaling data from another peer.

---

### 5. Error

**Server → Client**
```json
{ "type": "error", "error": "<description>" }
```
Sent if an error occurs (e.g., unknown peer).

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
- Nodes may request a list of available peers.

### 2. Session Negotiation & Mesh Formation

- One node (initiator) proposes a session by directly communicating with other nodes (using WebRTC or relayed messages).
- All nodes maintain their own session state and participant list.
- Nodes use a spinning tree or broadcast protocol to sense the network and build the mesh, without relying on the signaling server to store session info.
- Each node attempts to connect to all other participants, forming a full mesh.

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
| WebSocket  | Client → Server   | `{ "type": "register", "peer_id": "peer1" }`   | Register with signal server  |
| WebSocket  | Client → Server   | `{ "type": "list_peers" }`                     | List available peers         |
| WebSocket  | Client ↔ Server   | `{ "type": "relay", "to": "peer2", "data": { ... } }` | Relay signaling data   |
| WebSocket  | Server → Client   | `{ "type": "relay", "from": "peer2", "data": { ... } }` | Receive signaling data |
| WebSocket  | Server → Client   | `{ "type": "peers", "peers": ["peer1", "peer2"] }` | List of peers           |
| WebSocket  | Server → Client   | `{ "type": "error", "error": "description" }`  | Error message                |
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
{ "type": "register", "peer_id": "cli1" }
```
**cli2 → server**
```json
{ "type": "register", "peer_id": "cli2" }
```
**chrome1 → server**
```json
{ "type": "register", "peer_id": "chrome1" }
```

---

### 2. Peer Discovery

Each node requests the list of available peers:

**cli1 → server**
```json
{ "type": "list_peers" }
```
**server → cli1**
```json
{ "type": "peers", "peers": ["cli1", "cli2", "chrome1"] }
```

---

### 3. Session Negotiation & Mesh Formation

**cli1** proposes a session by directly communicating with the other nodes (using WebRTC or relayed messages):

**cli1 → cli2 (via relay or WebRTC)**
```json
{ "type": "session_proposal", "payload": { "session_id": "session123", "total": 3, "threshold": 2, "participants": ["cli1", "cli2", "chrome1"] } }
```
**cli1 → chrome1 (via relay or WebRTC)**
```json
{ "type": "session_proposal", "payload": { "session_id": "session123", "total": 3, "threshold": 2, "participants": ["cli1", "cli2", "chrome1"] } }
```
**cli2** and **chrome1** respond and coordinate directly with each other and cli1 to agree on the session.

Each node maintains its own session state and attempts to connect to all other participants, using a spinning tree or broadcast protocol to sense and build the mesh.

---

### 4. WebRTC Signaling

Each node exchanges signaling messages to establish direct connections.

**cli1 → server**
```json
{ "type": "relay", "to": "cli2", "data": { "type": "offer", "sdp": "<sdp-offer>" } }
```
**server → cli2**
```json
{ "type": "relay", "from": "cli1", "data": { "type": "offer", "sdp": "<sdp-offer>" } }
```
**cli2 → server**
```json
{ "type": "relay", "to": "cli1", "data": { "type": "answer", "sdp": "<sdp-answer>" } }
```
**server → cli1**
```json
{ "type": "relay", "from": "cli2", "data": { "type": "answer", "sdp": "<sdp-answer>" } }
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
- **Session State:** The signaling server does **not** store or manage session state. All session coordination and mesh building is handled by the nodes themselves.

---
