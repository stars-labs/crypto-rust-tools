use futures_util::{SinkExt, StreamExt};
// Remove serde imports if no longer directly used here for defining types
// use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::{accept_async, tungstenite::Message};

// Import shared types from the library crate
use solnana_mpc_frost::{ClientMsg, ServerMsg};

type PeerSender = mpsc::UnboundedSender<Message>;
type PeerMap = Arc<Mutex<HashMap<String, PeerSender>>>;

// Define an internal struct for server-side session management
// that includes the 'joined' field.
#[derive(Clone, Debug)]
struct SessionInfoInternal {
    session_id: String,
    total: usize,
    threshold: usize,
    participants: Vec<String>,
    joined: HashSet<String>,
}

// Use the internal struct for the SessionMap
type SessionMap = Arc<Mutex<HashMap<String, SessionInfoInternal>>>;

#[tokio::main]
async fn main() {
    let peers: PeerMap = Arc::new(Mutex::new(HashMap::new()));
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let listener = TcpListener::bind("0.0.0.0:9000").await.unwrap();
    println!("Signal server listening on 0.0.0.0:9000");

    while let Ok((stream, _)) = listener.accept().await {
        let peers = peers.clone();
        let sessions = sessions.clone();
        tokio::spawn(async move {
            let ws_stream = accept_async(stream).await.unwrap();
            let (mut ws_sink, mut ws_stream) = ws_stream.split();
            let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
            let mut peer_id: Option<String> = None;

            // Task to forward messages from rx to ws_sink
            let ws_sink_task = tokio::spawn(async move {
                while let Some(msg) = rx.recv().await {
                    if ws_sink.send(msg).await.is_err() {
                        break;
                    }
                }
            });

            loop {
                tokio::select! {
                    Some(msg) = ws_stream.next() => {
                        let msg = match msg {
                            Ok(m) if m.is_text() => m.into_text().unwrap(),
                            Ok(m) if m.is_close() => break,
                            _ => continue,
                        };

                        let parsed: Result<ClientMsg, _> = serde_json::from_str(&msg);

                        match parsed {
                            Ok(ClientMsg::Register { peer_id: reg_id }) => {
                                let mut peers_guard = peers.lock().unwrap();
                                if peers_guard.contains_key(&reg_id) {
                                    let err = ServerMsg::Error { error: "peer_id already registered".to_string() };
                                    let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                                    break;
                                }
                                peer_id = Some(reg_id.clone());
                                peers_guard.insert(reg_id.clone(), tx.clone());
                                println!("Registered peer: {}", reg_id);

                                // Broadcast updated peer list to all peers (owned Vec)
                                let peer_list: Vec<String> = peers_guard.keys().cloned().collect();
                                let msg = ServerMsg::Peers { peers: peer_list.clone() };
                                let msg_txt = serde_json::to_string(&msg).unwrap();
                                for (_id, ptx) in peers_guard.iter() {
                                    let _ = ptx.send(Message::Text(msg_txt.clone()));
                                }
                            }
                            Ok(ClientMsg::ListPeers) => {
                                let peers_guard = peers.lock().unwrap();
                                let peer_list: Vec<String> = peers_guard.keys().cloned().collect();
                                let msg = ServerMsg::Peers { peers: peer_list };
                                let _ = tx.send(Message::Text(serde_json::to_string(&msg).unwrap()));
                            }
                            Ok(ClientMsg::CreateSession { session_id, total, threshold, participants }) => {
                                let mut sessions_guard = sessions.lock().unwrap();
                                if sessions_guard.contains_key(&session_id) {
                                    let err = ServerMsg::Error { error: "session_id already exists".to_string() };
                                    let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                                    continue;
                                }
                                let mut joined = HashSet::new();
                                if let Some(pid) = &peer_id {
                                    joined.insert(pid.clone());
                                }
                                // Use the internal struct here
                                let info = SessionInfoInternal {
                                    session_id: session_id.clone(),
                                    total,
                                    threshold,
                                    participants: participants.clone(),
                                    joined,
                                };
                                sessions_guard.insert(session_id.clone(), info.clone());
                                // Notify creator using the public SessionInfo structure
                                let msg = ServerMsg::SessionInfo {
                                    session_id: session_id.clone(),
                                    total,
                                    threshold,
                                    participants: participants.clone(),
                                };
                                let _ = tx.send(Message::Text(serde_json::to_string(&msg).unwrap()));
                                // Notify other participants
                                let peers_guard = peers.lock().unwrap();
                                for p in &participants {
                                    if Some(p) != peer_id.as_ref() {
                                        if let Some(p_tx) = peers_guard.get(p) {
                                            let invite = ServerMsg::SessionInvite {
                                                session_id: session_id.clone(),
                                                from: peer_id.as_deref().unwrap_or_default().to_string(),
                                                total,
                                                threshold,
                                                participants: participants.clone(),
                                            };
                                            let _ = p_tx.send(Message::Text(serde_json::to_string(&invite).unwrap()));
                                        }
                                    }
                                }
                            }
                            Ok(ClientMsg::JoinSession { session_id }) => {
                                let mut sessions_guard = sessions.lock().unwrap();
                                if let Some(info) = sessions_guard.get_mut(&session_id) {
                                    let current_peer_id = peer_id.clone().unwrap_or_default();
                                    let joined_successfully = info.participants.contains(&current_peer_id);

                                    if joined_successfully {
                                        info.joined.insert(current_peer_id.clone());
                                        println!("Peer {} joined session {}", current_peer_id, session_id);

                                        // Create the SessionInfo message payload once
                                        let session_info_msg = ServerMsg::SessionInfo {
                                            session_id: info.session_id.clone(),
                                            total: info.total,
                                            threshold: info.threshold,
                                            participants: info.participants.clone(), // Send the full intended participant list
                                        };
                                        let msg_text = serde_json::to_string(&session_info_msg).unwrap();

                                        // Notify all currently joined participants (including the new one)
                                        let peers_guard = peers.lock().unwrap();
                                        for joined_peer_id in info.joined.iter() {
                                            if let Some(peer_tx) = peers_guard.get(joined_peer_id) {
                                                 println!("Notifying peer {} about session update for {}", joined_peer_id, session_id);
                                                let _ = peer_tx.send(Message::Text(msg_text.clone()));
                                            } else {
                                                // This might happen if a peer disconnected abruptly after joining
                                                println!("Warning: Could not find sender for joined peer {}", joined_peer_id);
                                            }
                                        }
                                        // Drop guards explicitly before potential async operations or further logic if needed
                                        drop(peers_guard);


                                    } else {
                                         println!("Peer {} failed to join session {} (not in participant list)", current_peer_id, session_id);
                                         let err = ServerMsg::Error { error: "not invited to this session".to_string() };
                                         let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                                    }


                                } else {
                                    println!("Peer {} failed to join session {} (not found)", peer_id.as_deref().unwrap_or("unknown"), session_id);
                                    let err = ServerMsg::Error { error: "session_id not found".to_string() };
                                    let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                                }
                                // Explicitly drop the lock
                                drop(sessions_guard);
                            }
                            Ok(ClientMsg::Relay { to, data }) => {
                                let peers_guard = peers.lock().unwrap();
                                if let Some(peer_tx) = peers_guard.get(&to) {
                                    let relay = ServerMsg::Relay {
                                        from: peer_id.as_deref().unwrap_or_default().to_string(),
                                        data: data.clone(), // Clone data for the message
                                    };
                                    // Log the relay action
                                    println!("Relaying message from {} to {}: {:?}", peer_id.as_deref().unwrap_or("unknown"), to, data);
                                    let _ = peer_tx.send(Message::Text(serde_json::to_string(&relay).unwrap()));
                                } else {
                                    println!("Relay failed: unknown peer {}", to);
                                    let err = ServerMsg::Error { error: format!("unknown peer: {}", to) };
                                    let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                                }
                                // Explicitly drop the lock
                                drop(peers_guard);
                            }
                            Err(_) => {
                                let err = ServerMsg::Error { error: "invalid message".to_string() };
                                let _ = tx.send(Message::Text(serde_json::to_string(&err).unwrap()));
                            }
                        }
                    }
                    else => break,
                }
            }

            // Cleanup on disconnect
            if let Some(my_id) = peer_id {
                let mut peers_guard = peers.lock().unwrap();
                peers_guard.remove(&my_id);
                println!("Peer {} disconnected", my_id);

                // Broadcast updated peer list to all peers (owned Vec)
                let peer_list: Vec<String> = peers_guard.keys().cloned().collect();
                let msg = ServerMsg::Peers {
                    peers: peer_list.clone(),
                };
                let msg_txt = serde_json::to_string(&msg).unwrap();
                for (_id, ptx) in peers_guard.iter() {
                    let _ = ptx.send(Message::Text(msg_txt.clone()));
                }
            }
            ws_sink_task.abort();
        });
    }
}
