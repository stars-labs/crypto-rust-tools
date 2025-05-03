use crate::state::AppState;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
// Add necessary imports used in the drawing logic
use std::collections::HashSet;
// Add imports needed for key handling
use crossterm::event::{KeyCode, KeyEvent};
use solnana_mpc_frost::ClientMsg;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc; // Import ClientMsg

// Move TUI rendering and input helpers here if you have any (not the main loop, just helpers).

pub fn draw_main_ui(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &AppState, // Changed from app_guard to app to match caller
    input: &str,
    input_mode: bool,
) -> anyhow::Result<()> {
    terminal.draw(|f| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Length(5), // Peers
                Constraint::Min(5),    // Log
                Constraint::Length(3), // Session/Invites
                Constraint::Length(3), // Input Area
            ])
            .split(f.area()); // Use f.area() instead of f.size()

        let title = format!("Peer ID: {}", app.peer_id);

        // Determine which peers *should* be connected based on the session
        let session_participants: HashSet<String> = app
            .session
            .as_ref()
            .map(|s| s.participants.iter().cloned().collect())
            .unwrap_or_default();

        // Update the TUI rendering logic for peer statuses to be more accurate
        let peer_list_items = app
            .peers // Peers known via signaling
            .iter()
            .filter(|p| !p.trim().eq_ignore_ascii_case(app.peer_id.trim()))
            .map(|p| {
                let status = if session_participants.contains(p) {
                    // First check if there's an explicit status
                    if let Some(s) = app.peer_statuses.get(p) {
                        // For clarity, add connection role in the status display
                        let role_prefix = if app.peer_id < p.to_string() {
                            "→" // Initiator (outgoing connection)
                        } else {
                            "←" // Responder (incoming connection)
                        };
                        format!("{}{:?}", role_prefix, s)
                    } else {
                        // Default for session members not yet reported
                        "Pending".to_string()
                    }
                } else {
                    // If not in session, they shouldn't be connected via WebRTC
                    "N/A".to_string()
                };
                ListItem::new(format!("{} ({})", p, status))
            })
            .collect::<Vec<_>>();

        let peers_widget =
            List::new(peer_list_items) // Use the formatted list
                .block(Block::default().title("Peers").borders(Borders::ALL));

        let log = Paragraph::new(
            app.log
                .iter()
                .rev()
                .take(chunks[2].height.saturating_sub(2) as usize) // Adjust log lines based on available height
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .block(Block::default().title("Log").borders(Borders::ALL));
        let session = if let Some(sess) = &app.session {
            format!(
                "Session: {} ({} of {}, threshold {})",
                sess.session_id,
                sess.participants.len(),
                sess.total,
                sess.threshold
            )
        } else {
            "No session".to_string()
        };
        // FIX: Use is_empty()
        let invites = if !app.invites.is_empty() {
            format!(
                "Invites: {}",
                app.invites
                    .iter()
                    .map(|s| s.session_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            "No invites".to_string()
        };

        // Input box rendering based on mode
        let input_display_text = if input_mode {
            format!("> {}", input)
        } else {
            "Press 'i' to input, 'o' to accept invite, 'q' to quit".to_string()
        };
        let input_box = Paragraph::new(input_display_text)
            .block(Block::default().title("Input").borders(Borders::ALL));

        f.render_widget(
            Block::default().title(title).borders(Borders::ALL),
            chunks[0],
        );
        f.render_widget(peers_widget, chunks[1]); // Render the updated peers widget
        f.render_widget(log, chunks[2]);
        f.render_widget(
            Paragraph::new(format!("{}\n{}", session, invites)), // Keep original for now
            chunks[3],
        );
        // Render input box in its own chunk
        f.render_widget(input_box, chunks[4]);
    })?;
    Ok(())
}

// Returns Ok(true) to continue, Ok(false) to quit, Err on error.
pub fn handle_key_event(
    key: KeyEvent,
    app: &mut AppState,                        // Now mutable
    input: &mut String,                        // Now mutable
    input_mode: &mut bool,                     // Now mutable
    cmd_tx: &mpsc::UnboundedSender<ClientMsg>, // Pass as reference
) -> anyhow::Result<bool> {
    if *input_mode {
        match key.code {
            KeyCode::Enter => {
                let cmd_str = input.trim().to_string();
                input.clear();
                *input_mode = false; // Exit input mode immediately
                app.log.push("Exited input mode.".to_string());

                // Parse and handle command
                if cmd_str.starts_with("/list") {
                    let _ = cmd_tx.send(ClientMsg::ListPeers);
                } else if cmd_str.starts_with("/create") {
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 5 {
                        if let (Ok(total), Ok(threshold)) = (parts[2].parse(), parts[3].parse()) {
                            let session_id = parts[1].to_string();
                            let participants = parts[4].split(',').map(|s| s.to_string()).collect();
                            let _ = cmd_tx.send(ClientMsg::CreateSession {
                                session_id,
                                total,
                                threshold,
                                participants,
                            });
                        } else {
                            app.log
                                .push("Invalid total/threshold for /create.".to_string());
                        }
                    } else {
                        app.log.push("Invalid /create format. Use: /create <id> <total> <threshold> <p1,p2,...>".to_string());
                    }
                } else if cmd_str.starts_with("/join") {
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let session_id = parts[1].to_string();
                        let _ = cmd_tx.send(ClientMsg::JoinSession { session_id });
                    } else {
                        app.log
                            .push("Invalid /join format. Use: /join <session_id>".to_string());
                    }
                } else if cmd_str.starts_with("/invite") {
                    // Check invites directly on app state
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let session_id_to_join = parts[1].to_string();
                        let invite_found = app
                            .invites
                            .iter()
                            .any(|s| s.session_id == session_id_to_join);

                        if invite_found {
                            let _ = cmd_tx.send(ClientMsg::JoinSession {
                                session_id: session_id_to_join,
                            });
                        } else {
                            app.log
                                .push(format!("Invite '{}' not found.", session_id_to_join));
                        }
                    } else {
                        app.log
                            .push("Invalid /invite format. Use: /invite <session_id>".to_string());
                    }
                } else if cmd_str.starts_with("/relay") {
                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                    if parts.len() == 3 {
                        let target_peer_id = parts[1].to_string();
                        let json_str = parts[2];
                        match serde_json::from_str::<serde_json::Value>(json_str) {
                            Ok(data) => {
                                let _ = cmd_tx.send(ClientMsg::Relay {
                                    to: target_peer_id.clone(),
                                    data,
                                });
                                app.log
                                    .push(format!("Relaying message to {}", target_peer_id));
                            }
                            Err(e) => {
                                app.log.push(format!("Invalid JSON for /relay: {}", e));
                            }
                        }
                    } else {
                        app.log.push(
                            "Invalid /relay format. Use: /relay <peer_id> <json_data>".to_string(),
                        );
                    }
                } else if cmd_str.starts_with("/send") {
                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                    if parts.len() >= 3 {
                        let target_peer_id = parts[1].to_string();
                        let message = parts[2].to_string();

                        app.log.push(format!(
                            "Attempting to send message to {}: {}", // Changed log message slightly
                            target_peer_id, message
                        ));

                        // Clone necessary things for the async task
                        // Note: We can't easily pass cmd_tx or app state mutably into the async block
                        // We need to clone Arcs or send necessary info via channels if the task needs them.
                        // For sending the message, we need the peer_connections Arc and cmd_tx.
                        let peer_connections_arc = app.peer_connections.clone();
                        let _cmd_tx_clone = cmd_tx.clone(); // Clone the sender
                        let app_log_arc = Arc::new(StdMutex::new(app.log.clone())); // Clone log for async task if needed

                        // Spawn the task to handle message sending via WebRTC
                        tokio::spawn(async move {
                            // This logic remains largely the same as before, but uses cloned Arcs/senders
                            // It needs access to peer_connections and cmd_tx
                            // It might need to log using its own cloned log Arc or send log messages back via another channel
                            // (Simplified example - omitting the full send logic for brevity,
                            // assuming it's similar to the previous version but adapted for clones)

                            let pc_result = {
                                let peer_conns = peer_connections_arc.lock().await;
                                peer_conns.get(&target_peer_id).cloned()
                            };

                            if let Some(pc) = pc_result {
                                // Simplified: Assume data channel creation and sending logic here...
                                // Use cmd_tx_clone if needed to relay messages (e.g., for reconnection)
                                // Use app_log_arc.lock().unwrap().push(...) for logging within the task
                                let channel_name = format!(
                                    "msg-{}",
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis()
                                );

                                match pc.create_data_channel(&channel_name, None).await {
                                    Ok(dc) => {
                                        let dc_arc = Arc::new(dc);
                                        let message_clone = message.clone();
                                        let target_clone = target_peer_id.clone();
                                        let log_arc_clone = app_log_arc.clone(); // Clone log Arc for on_open

                                        let dc_for_callback = dc_arc.clone();
                                        dc_arc.on_open(Box::new(move || {
                                            let dc_ref = dc_for_callback.clone(); // Clone Arc for the inner task
                                            let msg = message_clone.clone();
                                            let _target = target_clone.clone();
                                            let _log_arc_inner = log_arc_clone.clone(); // Clone log Arc for inner task

                                            tokio::spawn(async move {
                                                if let Err(_e) = dc_ref.send_text(msg.clone()).await
                                                {
                                                    // Log error using the cloned Arc
                                                    // log_arc_inner.lock().unwrap().push(...)
                                                } else {
                                                    // Log success using the cloned Arc
                                                    // log_arc_inner.lock().unwrap().push(...)
                                                }
                                                // ... rest of the on_open logic ...
                                                tokio::time::sleep(Duration::from_secs(1)).await;
                                                let _ = dc_ref.send_text("__COMPLETE__").await;
                                            });
                                            Box::pin(async {})
                                        }));
                                        // ... on_message setup ...
                                    }
                                    Err(_e) => {
                                        // Log error using the cloned Arc
                                        // app_log_arc.lock().unwrap().push(...)
                                        // Send reconnect message using cmd_tx_clone
                                        // ... reconnect logic ...
                                    }
                                }
                            } else {
                                // Log error using the cloned Arc
                                // app_log_arc.lock().unwrap().push(...)
                            }
                        }); // End of spawned task
                    } else {
                        app.log.push(
                            "Invalid /send format. Use: /send <peer_id> <message>".to_string(),
                        );
                    }
                } else if !cmd_str.is_empty() {
                    app.log.push(format!("Unknown command: {}", cmd_str));
                }
            }
            KeyCode::Char(c) => {
                input.push(c);
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Esc => {
                *input_mode = false;
                input.clear();
                app.log.push("Exited input mode (Esc).".to_string());
            }
            _ => {}
        }
    } else {
        // Not in input mode
        match key.code {
            KeyCode::Char('i') => {
                *input_mode = true;
                app.log.push("Entered input mode.".to_string());
            }
            KeyCode::Char('q') => {
                app.log.push("Quitting...".to_string());
                return Ok(false); // Signal to quit
            }
            KeyCode::Char('o') => {
                let session_to_join = app.invites.first().map(|inv| inv.session_id.clone());

                if let Some(session_id) = session_to_join {
                    app.log
                        .push("Attempting to accept first invite...".to_string());
                    let _ = cmd_tx.send(ClientMsg::JoinSession { session_id });
                } else {
                    app.log.push("No invites to accept with 'o'".to_string());
                }
            }
            _ => {}
        }
    }
    Ok(true) // Continue loop by default
}
