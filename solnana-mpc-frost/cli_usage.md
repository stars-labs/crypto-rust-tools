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
   - `mpc-1` will broadcast the session proposal to `mpc-2` and `mpc-3`
   - On `mpc-2` and `mpc-3`, the proposal will appear in the TUI under the "Invites:" section
   - The log will show: `Session proposal 'wallet_2of3' received from mpc-1`

4. **Accepting the Proposal:**
   - On `mpc-2`, press `o` (or type `/accept wallet_2of3`)
   - The log will show: `You accepted session proposal 'wallet_2of3'`
   - On `mpc-3`, press `o` (or type `/accept wallet_2of3`)
   - All nodes will show the session as active once all participants have accepted

5. **WebRTC Connection Establishment:**
   - Nodes will automatically exchange WebRTC signaling information via the server
   - Watch the logs for connection state changes
   - When all connections are established, each node will show all peers as "Connected"
   - Data channels will be opened between all pairs of peers

6. **Mesh Readiness:**
   - Each node automatically reports channel open status to its peers
   - When a node has all data channels open, it sends "mesh_ready" to all peers
   - When all nodes are ready, the DKG process will start automatically

7. **DKG Process:**
   - The log will show commitment messages being exchanged
   - Share distribution messages will follow
   - All nodes will calculate and verify their key shares
   - Upon successful completion, the "DKG Status" will show "Complete" and display the group public key

8. **Verification:**
   - To verify all nodes have the same group public key, check the "Group Public Key" field
   - All nodes should display the same hex value

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
