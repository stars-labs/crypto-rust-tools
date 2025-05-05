use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};
use tokio::sync::{Mutex as TokioMutex, mpsc};
use webrtc::peer_connection::RTCPeerConnection;

// Import InternalCommand and ClientMsg (aliased as SharedClientMsg) from lib.rs
use crate::signal::{SDPInfo, WebRTCSignal};
use crate::state::AppState;
use frost_ed25519::Ed25519Sha512;
use solnana_mpc_frost::{ClientMsg as SharedClientMsg, InternalCommand};
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState; // Import the specific Ciphersuite

// Implements Perfect Negotiation logic (initiator side)
pub async fn initiate_offers_for_session(
    participants: Vec<String>,
    self_peer_id: String,
    peer_connections: Arc<TokioMutex<HashMap<String, Arc<RTCPeerConnection>>>>,
    // Sender type is already InternalCommand
    cmd_tx: mpsc::UnboundedSender<InternalCommand>,
    state: Arc<StdMutex<AppState<Ed25519Sha512>>>,
) {
    state
        .lock()
        .unwrap()
        .log
        .push("Initiating WebRTC offers check...".to_string()); // Renamed log

    // Lock connections once
    let peer_conns = peer_connections.lock().await;
    state.lock().unwrap().log.push(format!(
        "Found {} peer connection objects.",
        peer_conns.len()
    ));

    for peer_id in participants {
        if peer_id == self_peer_id {
            // Removed unnecessary parentheses
            continue;
        }

        // --- Perfect Negotiation Role Check ---
        let should_initiate = self_peer_id < peer_id; // Impolite peer initiates

        state.lock().unwrap().log.push(format!(
            "Checking peer {}: Should initiate? {}",
            peer_id, should_initiate
        ));

        if should_initiate {
            if let Some(pc_arc) = peer_conns.get(&peer_id) {
                state
                    .lock()
                    .unwrap()
                    .log
                    .push(format!("Found PC object for peer {}", peer_id));
                let current_state = pc_arc.connection_state();
                let signaling_state = pc_arc.signaling_state();

                // Check if negotiation is needed based on state
                let negotiation_needed = match current_state {
                    RTCPeerConnectionState::New
                    | RTCPeerConnectionState::Closed
                    | RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Failed => true, // Need to negotiate if not connected/connecting
                    _ => match signaling_state {
                        webrtc::peer_connection::signaling_state::RTCSignalingState::Stable => {
                            false
                        } // Assume stable means no immediate need
                        _ => false, // If already negotiating, don't start another one
                    },
                };

                state.lock().unwrap().log.push(format!(
                    "Peer {}: Negotiation needed? {} (State: {:?}/{:?})",
                    peer_id, negotiation_needed, current_state, signaling_state
                ));

                if !negotiation_needed {
                    // Log already exists, skipping continue
                    continue;
                }

                // --- Check making_offer flag ---
                let is_already_making_offer = state
                    .lock()
                    .unwrap()
                    .making_offer
                    .get(&peer_id)
                    .copied()
                    .unwrap_or(false);

                state.lock().unwrap().log.push(format!(
                    "Peer {}: Already making offer? {}",
                    peer_id, is_already_making_offer
                ));

                if is_already_making_offer {
                    // Log already exists, skipping continue
                    continue;
                }

                // --- Proceed to make offer ---
                state.lock().unwrap().log.push(format!(
                    "Proceeding to spawn offer task for peer {}",
                    peer_id // Added log
                ));
                let pc_arc_clone = pc_arc.clone();
                let peer_id_clone = peer_id.clone();
                let state_clone = state.clone(); // Clone the Arc<StdMutex<AppState>>
                let cmd_tx_clone = cmd_tx.clone();

                tokio::spawn(async move {
                    // Spawn task for actual offer creation
                    // --- Set making_offer flag ---
                    state_clone
                        .lock()
                        .unwrap()
                        .making_offer
                        .insert(peer_id_clone.clone(), true);
                    state_clone
                        .lock()
                        .unwrap()
                        .log
                        .push(format!("Set making_offer=true for {}", peer_id_clone));

                    // Prefix offer_result with _ to mark it as intentionally unused
                    let offer_result = async { // Wrap offer logic in an async block for easier cleanup
                        // ... (Create Data Channel logic remains the same) ...
                        // Add log before creating data channel
                        state_clone.lock().unwrap().log.push(format!(
                            "Offer Task [{}]: Creating data channel...", peer_id_clone
                        ));
                        // Example: Create a default data channel if none exists
                        // Note: This might interfere if the other side also tries to create it.
                        // Consider creating channels only *after* connection is stable, or use negotiation.
                        // For now, let's assume a default channel is needed for testing.
                        match pc_arc_clone.create_data_channel("data", None).await {
                            Ok(dc) => {
                                state_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Created data channel '{}'. Setting up callbacks.", peer_id_clone, dc.label()
                                ));
                                let dc_arc = Arc::new(dc);
                                // Setup basic logging callbacks for the created channel
                                let state_log_dc = state_clone.clone();
                                let peer_id_dc = peer_id_clone.clone();
                                dc_arc.on_open(Box::new(move || {
                                    state_log_dc.lock().unwrap().log.push(format!(
                                        "Offer Task [{}]: Data channel opened!", peer_id_dc
                                    ));
                                    Box::pin(async {})
                                }));
                                let state_log_dc_msg = state_clone.clone();
                                let peer_id_dc_msg = peer_id_clone.clone();
                                dc_arc.on_message(Box::new(move |msg: DataChannelMessage| {
                                     let msg_str = String::from_utf8(msg.data.to_vec()).unwrap_or_else(|_| "Non-UTF8 data".to_string());
                                     state_log_dc_msg.lock().unwrap().log.push(format!(
                                         "Offer Task [{}]: Received message: {}", peer_id_dc_msg, msg_str
                                     ));
                                    Box::pin(async {})
                                }));
                            }
                            Err(e) => {
                                 state_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Error creating data channel: {}", peer_id_clone, e
                                )); // Fixed: added missing closing parenthesis
                                // Decide if this is fatal for the offer attempt
                                // return Err(()); // Maybe don't fail the whole offer just for the data channel?
                            }
                        }


                        // --- Create and Send Offer ---
                        state_clone.lock().unwrap().log.push(format!(
                            "Offer Task [{}]: Creating offer...", peer_id_clone // Added log
                        ));
                        match pc_arc_clone.create_offer(None).await {
                            Ok(offer) => {
                                state_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Created offer. Setting local description...", peer_id_clone // Added log
                                ));
                                // Fix error logging for setting local description
                                if let Err(e) = pc_arc_clone.set_local_description(offer.clone()).await {
                                    state_clone.lock().unwrap().log.push(format!(
                                        "Offer Task [{}]: Error setting local description (offer): {}", // Restored log
                                        peer_id_clone, e
                                    ));
                                    return Err(()); // Indicate failure
                                }
                                state_clone.lock().unwrap().log.push(format!(
                                    "Offer Task [{}]: Set local description (offer). Serializing and sending...", // Restored log
                                    peer_id_clone
                                ));

                                let signal = WebRTCSignal::Offer(SDPInfo { sdp: offer.sdp });
                                match serde_json::to_value(signal) {
                                    Ok(json_val) => {
                                        // Wrap the Relay message inside SendToServer command
                                        let relay_cmd = InternalCommand::SendToServer(SharedClientMsg::Relay {
                                            to: peer_id_clone.clone(),
                                            data: json_val,
                                        });
                                        let _ = cmd_tx_clone.send(relay_cmd); // Send the internal command
                                        state_clone
                                            .lock()
                                            .unwrap()
                                            .log
                                            .push(format!("Offer Task [{}]: Sent offer.", peer_id_clone)); // Restored log
                                    }
                                    // Fix error logging for offer serialization
                                    Err(e) => {
                                        state_clone.lock().unwrap().log.push(format!(
                                            "Offer Task [{}]: Error serializing offer: {}", peer_id_clone, e
                                        ));
                                        return Err(()); // Indicate failure
                                    }
                                }
                            }
                            // Fix error logging for offer creation
                            Err(e) => {
                                state_clone
                                    .lock()
                                    .unwrap()
                                    .log
                                    .push(format!("Offer Task [{}]: Error creating offer: {}", peer_id_clone, e)); // Restored log
                                return Err(()); // Indicate failure
                            }
                        }
                        Ok(()) // Indicate success
                    }.await;

                    // --- Clear making_offer flag regardless of success/failure ---
                    // Log success/failure based on offer_result
                    let outcome = if offer_result.is_ok() {
                        "succeeded"
                    } else {
                        "failed"
                    };
                    state_clone.lock().unwrap().log.push(format!(
                        "Offer Task [{}] {} negotiation.",
                        peer_id_clone, outcome
                    ));
                    state_clone
                        .lock()
                        .unwrap()
                        .making_offer
                        .insert(peer_id_clone.clone(), false);
                    state_clone
                        .lock()
                        .unwrap()
                        .log
                        .push(format!("Set making_offer=false for {}", peer_id_clone));
                }); // End of spawned task
            } else {
                state.lock().unwrap().log.push(format!(
                    "Should initiate offer to {}, but connection object not found!",
                    peer_id // Log remains valid
                ));
            }
        }
    }
    // peer_conns lock dropped here
    state
        .lock()
        .unwrap()
        .log
        .push("Finished WebRTC offers check.".to_string()); // Added log
}
