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
4.  **Session/Invites/Input:**
    *   Displays information about the current MPC session if joined.
    *   Lists pending session invites by `session_id`.
    *   Shows the input prompt `>` when in input mode.

## Commands and Keybindings

### Normal Mode (Default)

*   `i`: Enter **Input Mode** to type commands.
*   `o`: Accept the *first* pending session invite listed under "Invites". If no invites are pending, a message will be logged.
*   `q`: Quit the application.

### Input Mode (Activated by `i`)

Type commands starting with `/` and press `Enter`.

*   `/list`: Manually request an updated list of peers from the server. (The list also updates periodically and on peer join/disconnect).
*   `/create <session_id> <total> <threshold> <peer1,peer2,...>`: Create a new MPC session.
    *   `<session_id>`: A unique name for the session (e.g., `mywallet`).
    *   `<total>`: The total number of participants required (e.g., `3`).
    *   `<threshold>`: The signing threshold (e.g., `2`).
    *   `<peer1,peer2,...>`: A comma-separated list of the exact `peer_id`s of all participants (including yourself).
    *   *Example:* `/create mywallet 3 2 mpc-1,mpc-2,mpc-3`
*   `/join <session_id>`: Join an existing session you were invited to.
    *   *Example:* `/join mywallet`
*   `/invite <session_id>`: (Alternative to 'o') Accept a specific pending invite by its `session_id`. If the invite is not found, a message will be logged.
    *   *Example:* `/invite mywallet`
*   `/relay <target_peer_id> <json_data>`: Send an arbitrary JSON message to another peer via the signaling server. This is a low-level command intended for implementing peer-to-peer protocols.
    *   `<target_peer_id>`: The exact `peer_id` of the recipient.
    *   `<json_data>`: A valid JSON object or value (e.g., `{"type":"hello","value":123}`). Ensure the JSON is properly formatted on a single line.
    *   *Example:* `/relay mpc-2 {"action":"ping","id":1}`
*   `/send <target_peer_id> <message>`: Send a direct WebRTC message to another peer (requires established WebRTC connection).
    *   `<target_peer_id>`: The exact `peer_id` of the recipient.
    *   `<message>`: Any text message you want to send directly to the peer.
    *   *Example:* `/send mpc-2 Hello, this is a direct message!`
    *   *Note:* This command will only work if a WebRTC connection exists with the peer, regardless of the displayed connection status.
*   `Esc`: Exit Input Mode without sending a command.
*   `Backspace`: Delete the last character typed.

## Understanding WebRTC Connection Status

After all participants have joined the session, the WebRTC connection establishment process begins automatically. In the **Peers** section, you'll see connection status indicators:

1.  **Check Connection State:** Look for state change messages in the TUI **Log** pane and the console output (where you ran `cargo run`). A successful connection between your node and another peer (e.g., `mpc-2`) will eventually result in logs like:
    *   TUI Log: `WebRTC state with mpc-2: Connected`
    *   Console: `Peer Connection State has changed: Connected`
    You should see these "Connected" messages for each peer you are supposed to connect with in the session. The state might transition through `Connecting`, `Checking`, etc., before reaching `Connected`.

2.  **Check Data Channel:** The underlying `webrtc` library often negotiates a default data channel. When this channel is successfully opened with a peer (e.g., `mpc-2`), you will see a message in the TUI **Log** pane:
    *   TUI Log: `Data channel opened with mpc-2`
    *   Console: `Peer Connection State has changed: Connected`
Seeing both the `Connected` state and the `Data channel opened` message for a specific peer indicates that the WebRTC connection mesh is successfully established with that peer, allowing for direct peer-to-peer communication. If you see `Failed` states or don't see the "Connected" / "Data channel opened" messages after a reasonable time, there might be network issues (like firewalls blocking UDP) preventing the P2P connection. In such cases, a TURN server (which relays traffic) might be necessary; this example includes a public TURN server configuration for better NAT traversal compatibility.

## Example Workflow (Creating a 2-of-3 MPC Wallet Session)

This example shows how to set up a session for 3 participants (`mpc-1`, `mpc-2`, `mpc-3`) where any 2 of them are required to sign (`threshold = 2`).

1.  **Start Server:** Run `signal_server` in one terminal.
    ```bash
    cargo run -p solnana_mpc_frost --bin signal_server
    ```

2.  **Start Nodes:** Open three separate terminals.
    *   Terminal 1: Run `cargo run -p solnana_mpc_frost --bin cli_node`, enter `mpc-1` when prompted.
    *   Terminal 2: Run `cargo run -p solnana_mpc_frost --bin cli_node`, enter `mpc-2` when prompted.
    *   Terminal 3: Run `cargo run -p solnana_mpc_frost --bin cli_node`, enter `mpc-3` when prompted.

3.  **Observe Peers:** Wait a few seconds. Each node's "Peers" list should eventually show the other two nodes connected to the server.

4.  **Create Session (e.g., from Node `mpc-1`):**
    *   On node `mpc-1`, press `i` to enter input mode.
    *   Type the command to create a session named `wallet_2of3` with 3 total participants and a threshold of 2:
        ```bash
        /create wallet_2of3 3 2 mpc-1,mpc-2,mpc-3
        ```
    *   Press `Enter`.
    *   Node `mpc-1`'s log will show session info, and the session status line will update. Node `mpc-1` is now considered "in" the session.
    *   Nodes `mpc-2` and `mpc-3` will receive an invite: "Invites: wallet_2of3" will appear, and the log will show "Session invite from mpc-1...".

5.  **Join Session (Nodes `mpc-2` and `mpc-3`):**
    *   On node `mpc-2`, press `o` to accept the first invite (or use `/join wallet_2of3`).
    *   On node `mpc-3`, press `o` to accept the first invite (or use `/join wallet_2of3`).
    *   As each node joins, all *already joined* participants (including the joiner) will receive an updated `SessionInfo` message via the server. The log will show "Session info received/updated...".
    *   Once the *last* participant joins (`mpc-3` in this case), all participants (`mpc-1`, `mpc-2`, `mpc-3`) will have the complete `SessionInfo`. At this point, the WebRTC connection process (offers, answers, candidates) should begin automatically between peers, visible in the logs.

6.  **(Optional) Relay Test Message (e.g., Node `mpc-1` to `mpc-2`):**
    *   On node `mpc-1`, press `i`.
    *   Type: `/relay mpc-2 {"msg":"hello from mpc-1"}`
    *   Press `Enter`.
    *   Node `mpc-1` logs "Relaying message to mpc-2".
    *   Node `mpc-2` logs `Received non-WebRTC signal JSON via Relay from mpc-1: ...` (or similar, as it's not a standard WebRTC signal).

7.  **Send Direct WebRTC Messages (after connections are established):**
    *   Once WebRTC connections are established (you see "Connected" and "Data channel opened" messages in the logs for the relevant peers), you can send messages directly between peers.
    *   On node `mpc-1`, press `i`.
    *   Type: `/send mpc-2 Hello, this is a direct P2P message!`
    *   Press `Enter`.
    *   Node `mpc-1` logs "尝试发送消息到 mpc-2...".
    *   Node `mpc-2` logs "Receiver: Message from mpc-1: Hello, this is a direct P2P message!" (or similar, depending on exact logging).

*(Note: This example covers session setup, signaling, basic WebRTC connection establishment, and direct peer-to-peer communication. The actual MPC protocol exchange over the established WebRTC data channels would build upon this direct communication capability.)*
