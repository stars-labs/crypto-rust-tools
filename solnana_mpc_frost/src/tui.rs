use crate::state::{AppState, DkgState};
use crate::{InternalCommand, SharedClientMsg}; // Import necessary types
use crossterm::event::{KeyCode, KeyEvent};
use frost_ed25519::Ed25519Sha512; // Keep for AppState generic
use ratatui::{
    Frame, // Use Frame directly
    Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap}, // Keep Wrap
};
use std::collections::HashSet;
use std::io;
use tokio::sync::mpsc; // For command channel

pub fn draw_main_ui<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &AppState<Ed25519Sha512>,
    input: &str,
    input_mode: bool,
) -> io::Result<()> {
    terminal.draw(|f| {
        // Main layout: Title, Peers, Log, Status, Input
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Title area
                Constraint::Length(5), // Peers area
                Constraint::Min(5),    // Log area (flexible height)
                Constraint::Length(8), // Status area (increased height for wrapping)
                Constraint::Length(3), // Input area
            ])
            .split(f.area());

        // --- Title Widget ---
        let title_block = Block::default()
            .title(format!(" Peer ID: {} ", app.peer_id)) // Add spacing
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded); // Use rounded borders
        f.render_widget(title_block, main_chunks[0]);

        // --- Peers Widget ---
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
                let status_str = if session_participants.contains(p) {
                    // First check if there's an explicit status
                    if let Some(s) = app.peer_statuses.get(p) {
                        // For clarity, add connection role in the status display
                        let role_prefix = if app.peer_id < *p { "→" } else { "←" }; // Simplified comparison
                        format!("{}{:?}", role_prefix, s)
                    } else {
                        // Default for session members not yet reported
                        "Pending".to_string()
                    }
                } else {
                    // If not in session, they shouldn't be connected via WebRTC
                    "N/A".to_string()
                };
                // Add color based on status
                let style = match app.peer_statuses.get(p) {
                    Some(webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected) => Style::default().fg(Color::Green),
                    Some(webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connecting) => Style::default().fg(Color::Yellow),
                    Some(webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Failed) |
                    Some(webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Disconnected) |
                    Some(webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Closed) => Style::default().fg(Color::Red),
                    _ => Style::default(),
                };

                ListItem::new(format!("{} ({})", p, status_str)).style(style)
            })
            .collect::<Vec<_>>();

        let peers_widget =
            List::new(peer_list_items) // Use the formatted list
                .block(Block::default().title(" Peers (Signaling) ").borders(Borders::ALL));
        f.render_widget(peers_widget, main_chunks[1]);

        // --- Log Widget ---
        let log_text: Vec<Line> = app.log.iter().map(|l| Line::from(l.clone())).collect();
        let log_widget = Paragraph::new(log_text)
            .block(Block::default().title(" Log (Scroll: Up/Down) ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }) // Enable wrapping
            .scroll((app.log_scroll, 0)); // Apply vertical scroll offset
        f.render_widget(log_widget, main_chunks[2]);

        // --- Status Widget ---
        let session_line = if let Some(sess) = &app.session {
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
        let invites_line = if !app.invites.is_empty() {
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

        let dkg_status_line = match &app.dkg_state {
            DkgState::Idle => "DKG Status: Idle".to_string(),
            DkgState::Round1InProgress => "DKG Status: Round 1 (Sending Packages...)".to_string(),
            DkgState::Round1Complete => "DKG Status: Round 1 Complete (Waiting for Round 2...)".to_string(),
            DkgState::Round2InProgress => "DKG Status: Round 2 (Processing...)".to_string(),
            DkgState::Complete => "DKG Status: COMPLETE!".to_string(),
            DkgState::Failed(reason) => format!("DKG Status: FAILED ({})", reason),
        };
        let dkg_style = match app.dkg_state {
             DkgState::Complete => Style::default().fg(Color::Green),
             DkgState::Failed(_) => Style::default().fg(Color::Red),
             _ => Style::default().fg(Color::Yellow),
        };

        // Create lines for the keys
        let key_package_line = match &app.key_package {
            Some(kp) => {
                let id = kp.identifier();
                // Format ID using Debug, then truncate
                let id_str = format!("{:?}", id); // e.g., "Identifier(\"0100...\")"
                let display_id = id_str.chars().skip(12).take(2).collect::<String>(); // Skip "Identifier(\"" and take "01"

                // Convert signing share scalar to bytes and then hex encode (full)
                let share_bytes = kp.signing_share().to_scalar().to_bytes();
                let share_hex = hex::encode(share_bytes);
                // Use the truncated ID for display
                format!("Key Share: ID {} (Share: {})", display_id, share_hex)
            },
            None => "Key Share: None".to_string(),
        };

        let group_key_line = match &app.group_public_key {
            Some(pk) => {
                // FIX: Remove .verifying_key() call, just use .serialize()
                // Use serialize directly on PublicKeyPackage for Ed25519
                match pk.serialize() {
                    Ok(pk_bytes) => {
                        let pk_hex = hex::encode(pk_bytes);
                        // Show only a portion of the key for brevity
                        format!("Group Public Key: {}...", pk_hex.chars().take(32).collect::<String>())
                    },
                    Err(_) => "Group Public Key: Error serializing".to_string(),
                }
            },
            None => "Group Public Key: None".to_string(),
        };

        let solana_key_line = match &app.solana_public_key {
            Some(key) => format!("Solana Public Key: {}", key), // Assuming this is already a string
            None => "Solana Public Key: None".to_string(),
        };

        let status_paragraph = Paragraph::new(vec![
            Line::from(session_line),
            Line::from(invites_line),
            Line::from(dkg_status_line).style(dkg_style),
            Line::from(key_package_line).style(Style::default().fg(Color::Cyan)),
            Line::from(group_key_line).style(Style::default().fg(Color::Green)),
            Line::from(solana_key_line).style(Style::default().fg(Color::Magenta)),
        ])
        .block(Block::default().title(" Status ").borders(Borders::ALL))
        .wrap(Wrap { trim: true }); // Apply wrapping, trim leading/trailing whitespace
        f.render_widget(status_paragraph, main_chunks[3]);

        // --- Input Widget ---
        let input_title = if input_mode { " Input (Esc to cancel) " } else { " Help " };
        let input_display_text = if input_mode {
            format!("> {}", input)
        } else {
            "Scroll Log: ↑/↓ | Input: i | Accept Invite: o | Quit: q".to_string() // Update help text
        };
        let input_box = Paragraph::new(input_display_text)
            .style(if input_mode { Style::default().fg(Color::Yellow) } else { Style::default() })
            .block(Block::default().title(input_title).borders(Borders::ALL));
        f.render_widget(input_box, main_chunks[4]);

        // --- Cursor for Input Mode ---
        if input_mode {
            // Calculate cursor position based on input text length
            // Add 2 for the "> " prefix
            let cursor_x = main_chunks[4].x + input.chars().count() as u16 + 2;
            let cursor_y = main_chunks[4].y + 1; // Inside the input box border
            f.set_cursor(cursor_x, cursor_y);
        }
    })?;
    Ok(())
}

// --- Helper functions to draw UI components ---

// Remove <B> from Frame
fn draw_peer_id(
    f: &mut Frame, // Use Frame directly
    area: Rect,
    app: &AppState<Ed25519Sha512>,
) {
    // ...existing code...
}

// Remove <B> from Frame
fn draw_peers(
    f: &mut Frame, // Use Frame directly
    area: Rect,
    app: &AppState<Ed25519Sha512>,
) {
    // ...existing code...
}

// Remove <B> from Frame
fn draw_log(
    f: &mut Frame, // Use Frame directly
    area: Rect,
    app: &AppState<Ed25519Sha512>,
) {
    // ...existing code...
}

// Remove <B> from Frame
fn draw_status(
    f: &mut Frame, // Use Frame directly
    area: Rect,
    app: &AppState<Ed25519Sha512>,
) {
    let mut status_items = vec![
        // ... existing status items ...
    ];

    // --- Move Key Info Display INSIDE the vec! block or push afterwards ---
    // Display Key Package info if available
    if let Some(key_pkg) = &app.key_package {
        status_items.push(Line::from(Span::styled(
            format!("Key Share: Identifier {:?}", key_pkg.identifier()), // Display identifier
            Style::default().fg(Color::Green),
        )));
    } else {
        status_items.push(Line::from(Span::styled(
            "Key Share: None",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Display Group Public Key info if available
    if let Some(group_pk) = &app.group_public_key {
        // Instead of using .as_bytes() which doesn't exist, use .serialize()
        let pk_bytes = group_pk.verifying_key().serialize().unwrap_or_default();
        // Convert the serialized bytes to hex string
        let solana_pubkey = bs58::encode(pk_bytes).into_string();
        status_items.push(Line::from(Span::styled(
            format!(
                "Group Public Key: {}...", // Show truncated hex
                solana_pubkey.chars().take(16).collect::<String>()
            ),
            Style::default().fg(Color::Green),
        )));
    } else {
        status_items.push(Line::from(Span::styled(
            "Group Public Key: None",
            Style::default().fg(Color::DarkGray),
        )));
    }
    // --- End Key Info Display ---

    let status_list = List::new(status_items)
        .block(Block::default().borders(Borders::ALL).title("Status"))
        .style(Style::default().fg(Color::White));
    f.render_widget(status_list, area);
}

// Returns Ok(true) to continue, Ok(false) to quit, Err on error.
pub fn handle_key_event(
    key: KeyEvent,
    // Add generic parameter here
    app: &mut AppState<Ed25519Sha512>, // Now mutable
    input: &mut String,                // Now mutable
    input_mode: &mut bool,             // Now mutable
    // Update the sender type to use the new InternalCommand
    cmd_tx: &mpsc::UnboundedSender<InternalCommand>, // Pass as reference
) -> anyhow::Result<bool> {
    if *input_mode {
        // --- Input Mode Key Handling (mostly unchanged) ---
        match key.code {
            KeyCode::Enter => {
                let cmd_str = input.trim().to_string();
                input.clear();
                *input_mode = false; // Exit input mode immediately
                app.log.push("Exited input mode.".to_string());

                // Parse and handle command
                // Wrap shared messages when sending
                if cmd_str.starts_with("/list") {
                    let _ = cmd_tx.send(InternalCommand::SendToServer(SharedClientMsg::ListPeers));
                } else if cmd_str.starts_with("/create") {
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 5 {
                        if let (Ok(total), Ok(threshold)) = (parts[2].parse(), parts[3].parse()) {
                            let session_id = parts[1].to_string();
                            let participants = parts[4].split(',').map(|s| s.to_string()).collect();
                            let _ = cmd_tx.send(InternalCommand::SendToServer(
                                SharedClientMsg::CreateSession {
                                    session_id,
                                    total,
                                    threshold,
                                    participants,
                                },
                            ));
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
                        let _ = cmd_tx.send(InternalCommand::SendToServer(
                            SharedClientMsg::JoinSession { session_id },
                        ));
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
                            let _ = cmd_tx.send(InternalCommand::SendToServer(
                                SharedClientMsg::JoinSession {
                                    session_id: session_id_to_join,
                                },
                            ));
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
                                let _ = cmd_tx.send(InternalCommand::SendToServer(
                                    SharedClientMsg::Relay {
                                        to: target_peer_id.clone(),
                                        data,
                                    },
                                ));
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
                    // This command now sends a simple text message via WebRTCMessage::SimpleMessage
                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                    if parts.len() >= 3 {
                        let target_peer_id = parts[1].to_string();
                        let message_text = parts[2].to_string();

                        // Always log the send attempt, regardless of connection state
                        app.log.push(format!(
                            "Attempting to send direct message to {}: {}",
                            target_peer_id, message_text
                        ));

                        // Send internal command
                        let _ = cmd_tx.send(InternalCommand::SendDirect {
                            to: target_peer_id.clone(),
                            message: message_text.clone(),
                        });

                        // Log the command for visibility
                        app.log.push(format!(
                            "Command: /send {} {}",
                            target_peer_id, message_text
                        ));
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
        // --- Normal Mode Key Handling (Add scroll keys) ---
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
                    let _ = cmd_tx.send(InternalCommand::SendToServer(
                        SharedClientMsg::JoinSession { session_id },
                    ));
                } else {
                    app.log.push("No invites to accept with 'o'".to_string());
                }
            }
            KeyCode::Up => {
                // Scroll log up
                app.log_scroll = app.log_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                // Scroll log down (don't scroll past the end)
                // A simple upper bound - might need refinement depending on terminal height and wrap
                app.log_scroll = app.log_scroll.saturating_add(1);
                // Basic check: prevent scrolling too far down past the number of lines
                // This is approximate due to wrapping. A more precise calculation would
                // involve layout information which isn't easily available here.
                if app.log_scroll > app.log.len().saturating_sub(1) as u16 {
                    app.log_scroll = app.log.len().saturating_sub(1) as u16;
                }
            }
            _ => {}
        }
    }
    Ok(true) // Continue loop by default
}
