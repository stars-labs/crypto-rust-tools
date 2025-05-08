use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use worker::*;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Peers {
        peers: Vec<String>,
    },
    Relay {
        from: String,
        data: serde_json::Value,
    },
    Error {
        error: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Register { peer_id: String },
    ListPeers,
    Relay { to: String, data: serde_json::Value },
}

// Durable Object for managing peers
#[durable_object]
pub struct Peers {
    peers: Rc<RefCell<HashMap<String, WebSocket>>>,
    state: Rc<State>,
}

#[durable_object]
impl DurableObject for Peers {
    fn new(state: State, _env: Env) -> Self {
        Self {
            peers: Rc::new(RefCell::new(HashMap::new())),
            state: Rc::new(state),
        }
    }

    async fn fetch(&self, req: Request) -> Result<Response> {
        let upgrade_header = match req.headers().get("Upgrade") {
            Ok(Some(h)) => h,
            Ok(None) => "".to_string(),
            Err(_) => "".to_string(),
        };
        if upgrade_header != "websocket" {
            return Response::error("Expected Upgrade: websocket", 426);
        }

        let ws_pair = WebSocketPair::new()?;
        let client = ws_pair.client;
        let server = ws_pair.server;
        server.accept()?;

        let peers = self.peers.clone();
        let state = self.state.clone();
        wasm_bindgen_futures::spawn_local({
            let server = server.clone();
            let peers = peers.clone();
            let state = state.clone();
            async move {
                let mut peer_id: Option<String> = None;
                let mut event_stream = server.events().expect("could not open stream");

                while let Some(event) = event_stream.next().await {
                    match event.expect("received error in websocket") {
                        WebsocketEvent::Message(msg) => {
                            if let Some(text) = msg.text() {
                                let parsed = serde_json::from_str::<ClientMsg>(&text);
                                match parsed {
                                    Ok(ClientMsg::Register { peer_id: reg_id }) => {
                                        // Load peer list from storage
                                        let mut peer_list: Vec<String> = state
                                            .storage()
                                            .get("peer_list")
                                            .await
                                            .unwrap_or_else(|_| Some(vec![]))
                                            .unwrap_or(vec![]);
                                        let already_registered = peer_list.contains(&reg_id);
                                        if already_registered {
                                            let err = ServerMsg::Error {
                                                error: "peer_id already registered".to_string(),
                                            };
                                            let _ = server.send_with_str(
                                                &serde_json::to_string(&err).unwrap(),
                                            );
                                            break;
                                        }
                                        peer_id = Some(reg_id.clone());
                                        peers.borrow_mut().insert(reg_id.clone(), server.clone());
                                        peer_list.push(reg_id.clone());
                                        // Save updated peer list to storage
                                        let _ = state.storage().put("peer_list", &peer_list).await;

                                        // Broadcast updated peer list to all *other* peers
                                        let msg = ServerMsg::Peers {
                                            peers: peer_list.clone(),
                                        };
                                        let msg_txt = serde_json::to_string(&msg).unwrap();
                                        for (id, ws) in peers.borrow().iter() {
                                            if id != &reg_id {
                                                let _ = ws.send_with_str(&msg_txt);
                                            }
                                        }
                                        // Optionally, send the peer list to the newly registered node as well
                                        let _ = server.send_with_str(&msg_txt);
                                    }
                                    Ok(ClientMsg::ListPeers) => {
                                        // Load peer list from storage
                                        let peer_list: Vec<String> = state
                                            .storage()
                                            .get("peer_list")
                                            .await
                                            .unwrap_or_else(|_| Some(vec![]))
                                            .unwrap_or(vec![]);
                                        let msg = ServerMsg::Peers { peers: peer_list };
                                        let _ = server
                                            .send_with_str(&serde_json::to_string(&msg).unwrap());
                                    }
                                    Ok(ClientMsg::Relay { to, data }) => {
                                        let from = peer_id.clone().unwrap_or_default();
                                        let relay = ServerMsg::Relay { from, data };
                                        let found = peers.borrow().get(&to).cloned();
                                        if let Some(ws) = found {
                                            let _ = ws.send_with_str(
                                                &serde_json::to_string(&relay).unwrap(),
                                            );
                                        } else {
                                            let err = ServerMsg::Error {
                                                error: format!("unknown peer: {}", to),
                                            };
                                            let _ = server.send_with_str(
                                                &serde_json::to_string(&err).unwrap(),
                                            );
                                        }
                                    }
                                    Err(_) => {
                                        let err = ServerMsg::Error {
                                            error: "invalid message".to_string(),
                                        };
                                        let _ = server
                                            .send_with_str(&serde_json::to_string(&err).unwrap());
                                    }
                                }
                            }
                        }
                        WebsocketEvent::Close(_event) => {
                            // Cleanup on disconnect
                            if let Some(my_id) = peer_id.clone() {
                                peers.borrow_mut().remove(&my_id);
                                // Remove from storage
                                let mut peer_list: Vec<String> = state
                                    .storage()
                                    .get("peer_list")
                                    .await
                                    .unwrap_or_else(|_| Some(vec![]))
                                    .unwrap_or(vec![]);
                                peer_list.retain(|id| id != &my_id);
                                let _ = state.storage().put("peer_list", &peer_list).await;
                                // Broadcast updated peer list
                                let msg = ServerMsg::Peers {
                                    peers: peer_list.clone(),
                                };
                                for (_id, ws) in peers.borrow().iter() {
                                    let _ = ws.send_with_str(&serde_json::to_string(&msg).unwrap());
                                }
                            }
                        }
                    }
                }
            }
        });

        Response::from_websocket(client)
    }
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    // Route all websocket requests to the Peers Durable Object
    let peers_ns = env.durable_object("Peers")?;
    let id = peers_ns.id_from_name("global")?;
    let stub = id.get_stub()?;
    stub.fetch_with_request(req).await
}
