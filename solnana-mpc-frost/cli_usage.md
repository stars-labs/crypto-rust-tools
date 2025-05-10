# MPC CLI Node Usage Guide

This guide explains how to use the `cli_node` application for participating in MPC wallet creation sessions via a signaling server.

## Running the CLI Node

1.  **Start the Signaling Server:** Ensure the `signal_server` is running.
    ```bash
    cargo run -p solnana_mpc_frost --bin signal_server
    ```

2.  **Run the CLI Node:** Open a new terminal for each participant and run the following command. You will be prompted to enter a unique `peer_id` for each node.
    ```bash
    cargo run -p solnana_mpc_frost --bin cli_node
    ```
    Enter a unique ID when prompted, e.g., `mpc-1`, `mpc-2`, etc.

## TUI Interface

The CLI node presents a terminal user interface (TUI) with the following sections:

1.  **Peer ID:** Displays the unique ID you entered for this node.
2.  **Peers:** Lists other peers currently connected to the signaling server (excluding yourself).
3.  **Log:** Shows recent events, received messages (including raw JSON for debugging), and errors.
4.  **Session/Invites/DKG/Input:**
    *   Displays information about the current MPC session if joined.
    *   Lists pending session invites by `session_id`.
    *   Shows the current status of the Distributed Key Generation (DKG) process.
    *   Shows the input prompt `>` when in input mode.

## Commands and Keybindings

### Normal Mode (Default)

*   `i`: Enter **Input Mode** to type commands.
*   `o`: Accept the *first* pending session invite listed under "Invites". If no invites are pending, a message will be logged.
*   `q`: Quit the application.

### Input Mode (Activated by `i`)

Type commands starting with `/` and press `Enter`.

*   `/list`: Manually request an updated list of peers from the server. The list also updates periodically and on peer join/disconnect.
*   `/propose <session_id> <total> <threshold> <peer1,peer2,...>`: Propose a new MPC session.
    *   `<session_id>`: A unique name for the session (e.g., `mywallet`).
    *   `<total>`: The total number of participants required (e.g., `3`).
    *   `<threshold>`: The signing threshold (e.g., `2`).
    *   `<peer1,peer2,...>`: A comma-separated list of the exact `peer_id`s of all participants (including yourself).
    *   *Example:* `/propose mywallet 3 2 mpc-1,mpc-2,mpc-3`
*   `/join <session_id>`: Join an existing session you were invited to.
    *   *Example:* `/join mywallet`
*   `/accept <session_id>`: (Alternative to 'o') Accept a specific pending session proposal by its `session_id`.
    *   *Example:* `/accept mywallet`
*   `/relay <target_peer_id> <json_data>`: Send an arbitrary JSON message to another peer via the signaling server.
    *   `<target_peer_id>`: The exact `peer_id` of the recipient.
    *   `<json_data>`: A valid JSON object or value (e.g., `{"type":"hello","value":123}`).
    *   *Example:* `/relay mpc-2 {"type":"ping","payload":{"id":1}}`
*   `/send <target_peer_id> <message>`: Send a direct WebRTC message to another peer.
    *   `<target_peer_id>`: The exact `peer_id` of the recipient.
    *   `<message>`: Any text message you want to send directly to the peer.
    *   *Example:* `/send mpc-2 Hello, this is a direct message!`
*   `/status`: Show detailed information about the current session and mesh state.
*   `/mesh_ready`: Manually indicate this node is ready with all WebRTC connections established.
*   `Esc`: Exit Input Mode without sending a command.
*   `Backspace`: Delete the last character typed.

## Understanding WebRTC Connection Status

After participants have agreed to join a session, the WebRTC connection establishment process begins:

1. **Signaling Exchange:** Peers exchange WebRTC signaling data (SDP offers/answers, ICE candidates) via the signaling server.
   - The log will show messages about offers, answers, and ICE candidates being sent and received.

2. **Connection States:** In the **Peers** section, you'll see connection status indicators:
   - `New`: Initial state
   - `Connecting`: Connection attempt in progress
   - `Connected`: WebRTC connection established
   - `Failed`: Connection attempt failed
   - `Disconnected`: Connection was established but then lost

3. **Data Channel Status:** For successful MPC communication, data channels must be opened.
   - TUI Log will show: `Data channel opened with mpc-2`
   - Console may show: `WebRTC data channel state change: open`

4. **Mesh Readiness:** A complete mesh is formed when all participants have established WebRTC connections with each other.
   - Each node automatically sends a `channel_open` message when a data channel is successfully opened
   - When all required connections are established, a node automatically signals `mesh_ready` to all peers
   - Manual intervention with `/mesh_ready` command is only needed if the automatic process fails
   - The TUI will show "Mesh Status: Ready (3/3)" when all peers report readiness
   - When all peers report mesh readiness, the MPC protocol proceeds automatically

## Distributed Key Generation (DKG)

Once the full WebRTC mesh is established and all participants have signaled readiness, the DKG process begins:

1. **Commitment Exchange:** Each node sends its cryptographic commitments to all others.
   - Log will show: `Sending commitments to all peers...`
   
2. **Share Distribution:** Nodes exchange encrypted key shares with each participant.
   - Log will show: `Sending share to peer mpc-2...`
   
3. **Verification:** Each node verifies the received shares against the commitments.
   - Log will show: `Verifying share from peer mpc-2...`
   
4. **Finalization:** Nodes complete the DKG process by computing their final key shares.
   - Log will show: `Computing final key share...`

The "DKG Status" in the TUI will update as the process progresses through these phases:
- `Idle` → `CommitmentsInProgress` → `CommitmentsComplete` → `SharesInProgress` → `VerificationInProgress` → `Complete` or `Failed`

## WebRTC Mesh Formation

Before DKG can begin, all participating nodes must establish a full peer-to-peer mesh network via WebRTC:

1. **WebRTC Data Channel Establishment:**
   - For each node pair, WebRTC connections must be established through the signaling server
   - When a data channel successfully opens, nodes automatically exchange `channel_open` messages:
     ```json
     { "type": "channel_open", "payload": { "peer_id": "<peer_id>" } }
     ```
   - Each node tracks its connected peers against the session participant list

2. **Mesh Readiness Notification:**
   - When a node has open channels to all participants, it broadcasts a `mesh_ready` message:
     ```json
     { "type": "mesh_ready", "payload": { "session_id": "<session_id>", "peer_id": "<sender_peer_id>" } }
     ```
   - DKG begins only when all nodes have confirmed mesh readiness
   - The TUI displays "Mesh Status: Ready (3/3)" when the mesh is complete

## Example Workflow (Creating a 2-of-3 MPC Wallet Session)

This example shows how to set up a session for 3 participants (`mpc-1`, `mpc-2`, `mpc-3`) where any 2 are required to sign (`threshold = 2`).

1. **Start Server:**
   ```bash
   cargo run -p solnana_mpc_frost --bin signal_server
   ```

2. **Start Nodes:** In three separate terminals:
   - Terminal 1: Run node with peer ID `mpc-1`
   - Terminal 2: Run node with peer ID `mpc-2`
   - Terminal 3: Run node with peer ID `mpc-3`

3. **Session Proposal:**
   - On `mpc-1`, enter input mode (`i`) and type:
   ```
   /propose wallet_2of3 3 2 mpc-1,mpc-2,mpc-3
   ```
   - This creates a `session_proposal` message that is relayed to other participants:
     ```json
     {
       "type": "session_proposal", 
       "payload": { 
         "session_id": "wallet_2of3", 
         "total": 3, 
         "threshold": 2, 
         "participants": ["mpc-1", "mpc-2", "mpc-3"] 
       }
     }
     ```
   - On `mpc-2` and `mpc-3`, the proposal will appear in the TUI under the "Invites:" section

4. **Accepting the Proposal:**
   - On `mpc-2` and `mpc-3`, press `o` (or type `/accept wallet_2of3`)
   - Each accepting node sends an acceptance message to the proposer

5. **WebRTC Connection Establishment:**
   - Nodes automatically exchange WebRTC signaling information via the server:
     - SDP Offers/Answers: Initial connection parameters
     - ICE Candidates: Network path information
   - These messages use the following format through the signaling server:
     ```json
     { "type": "relay", "to": "<peer_id>", "data": { "type": "offer|answer|candidate", ... } }
     ```

6. **Mesh Readiness:**
   - As each data channel opens, nodes exchange `channel_open` messages
   - When all required connections are established, nodes send `mesh_ready` messages
   - The TUI will show "Mesh Status: Ready (3/3)" when all peers report readiness

7. **DKG Process:**
   - **Commitments:** Each node broadcasts its commitments to all peers:
     ```json
     { "type": "commitment", "payload": { "data": "<hex-encoded-commitment>" } }
     ```
   - **Shares:** Each node sends encrypted key shares to each participant:
     ```json
     { "type": "share", "payload": { "data": "<hex-encoded-share>" } }
     ```
   - **Verification:** Each node verifies received shares against commitments
   - **Completion:** When verification completes, the final key shares are computed

8. **Verification:**
   - Upon successful completion, all nodes will display the same group public key
   - Nodes may also exchange a final status message:
     ```json
     { "type": "wallet_created", "payload": { "session_id": "wallet_2of3", "pubkey": "<hex-group-pubkey>" } }
     ```

## Protocol Messaging Flow

Behind the scenes, the CLI node implements the following protocol flow:

1. **Registration:** Each node registers with the signaling server using a unique `peer_id`
2. **Peer Discovery:** Nodes request lists of available peers from the server
3. **Session Negotiation:** Nodes exchange session proposals and acceptances
4. **WebRTC Signaling:** Peers exchange offers, answers, and ICE candidates via the server
5. **Mesh Formation:** Nodes establish direct WebRTC connections and report readiness
6. **DKG Protocol:** Once the mesh is complete, the DKG protocol executes over WebRTC
7. **Wallet Creation:** The MPC wallet is created when DKG completes successfully

Each of these steps involves specific message types defined in the protocol documentation.

## Troubleshooting

* **Peer not showing in list:** Try using `/list` to refresh the peer list.
* **Failed WebRTC connections:** Check your network settings. WebRTC may be blocked by some firewalls.
* **DKG fails to complete:** Ensure all nodes have proper WebRTC connections before starting DKG.
* **Mesh formation issues:** If the mesh isn't completing automatically, check the logs for connection errors.
* **Message not received:** Verify that all participants have joined the same session ID.
* **State synchronization issues:** Sometimes restarting the affected nodes may help resolve state inconsistencies.

For persistent issues, consult the protocol documentation for more detailed message flow information.
