use crate::protocal::signal::SessionInfo;
use crate::utils::state::{AppState, DkgStateDisplay}; // Now correctly imports DkgStateDisplay trait
use crate::{InternalCommand, SharedClientMsg, MeshStatus}; // Add MeshStatus import
use crossterm::event::{KeyCode, KeyEvent};
use frost_ed25519::Ed25519Sha512; // Keep for AppState generic
use ratatui::{
    Frame, // Add Frame import
    Terminal,
    backend::Backend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},                                         // Remove Spans
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap}, // Keep Wrap
};
use std::collections::HashSet;
use std::io;
use tokio::sync::mpsc; // For command channel // Import SessionInfo

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

        let title_block = Block::default()
            .title(format!(" Peer ID: {} ", app.peer_id)) // Add spacing
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded); // Use rounded borders
        f.render_widget(title_block, main_chunks[0]);

        let session_participants: HashSet<String> = app
            .session
            .as_ref()
            .map(|s| s.participants.iter().cloned().collect())
            .unwrap_or_default();

        let peer_list_items = app
            .peers
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

        let log_text: Vec<Line> = app.log.iter().map(|l| Line::from(l.clone())).collect();
        let log_widget = Paragraph::new(log_text)
            .block(Block::default().title(" Log (Scroll: Up/Down) ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }) // Enable wrapping
            .scroll((app.log_scroll, 0)); // Apply vertical scroll offset
        f.render_widget(log_widget, main_chunks[2]);

        // --- Status Widget ---
        draw_status_section(f, app, main_chunks[3]);

        // --- Input Widget ---
        let input_title = if input_mode { " Input (Esc to cancel) " } else { " Help " };
        let input_display_text = if input_mode {
            format!("> {}", input)
        } else {
            // Add "Save Log: s" to the help text
            "Scroll Log: ↑/↓ | Input: i | Accept Invite: o | Save Log: s | Mesh Ready: r | Quit: q".to_string()
        };
        let input_box = Paragraph::new(input_display_text)
            .style(if input_mode { Style::default().fg(Color::Yellow) } else { Style::default() })
            .block(Block::default().title(input_title).borders(Borders::ALL));
        f.render_widget(input_box, main_chunks[4]);

        // --- Cursor for Input Mode ---
        if input_mode {
            // Calculate cursor position based on input text length
            // Add 1 for block border, 2 for the "> " prefix
            let cursor_x = main_chunks[4].x + input.chars().count() as u16 + 3;
            let cursor_y = main_chunks[4].y + 1; // Inside the input box border
            let position = Rect::new(cursor_x, cursor_y, 1, 1);
            f.set_cursor_position(position);
        }
    })?;
    Ok(())
}

fn draw_status_section<T: frost_core::Ciphersuite>(
    f: &mut Frame<'_>,
    app: &AppState<T>,
    area: Rect,
) {
    let mut status_items = Vec::new();

    // Session display - show active session only
    if let Some(session) = &app.session {
        status_items.push(Line::from(vec![
            Span::styled("Session: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!(
                "{} ({} of {}, threshold {})",
                session.session_id,
                session.participants.len(),
                session.total,
                session.threshold
            )),
        ]));
        
        // Add mesh status information as documented in cli_usage.md
        let mesh_status_str = match &app.mesh_status {
            MeshStatus::Incomplete => "Incomplete".to_string(),
            MeshStatus::PartiallyReady { ready_peers, total_peers } => format!("Partially Ready ({}/{})", ready_peers.len(), total_peers),
            MeshStatus::Ready => "Ready".to_string(),
        };
        
        let mesh_style = match &app.mesh_status {
            MeshStatus::Incomplete => Style::default().fg(Color::Red),
            MeshStatus::PartiallyReady { .. } => Style::default().fg(Color::Yellow),
            MeshStatus::Ready => Style::default().fg(Color::Green),
        };
        
        status_items.push(Line::from(vec![
            Span::styled("Mesh Status: ", Style::default().fg(Color::Yellow)),
            Span::styled(mesh_status_str, mesh_style),
        ]));
    } else {
        status_items.push(Line::from(vec![
            Span::styled("Session: ", Style::default().fg(Color::Yellow)),
            Span::raw("None"),
        ]));
        
        status_items.push(Line::from(vec![
            Span::styled("Mesh Status: ", Style::default().fg(Color::Yellow)),
            Span::raw("N/A"),
        ]));
    }

    // Invites display - only show invites that aren't the active session
    let pending_invites: Vec<&SessionInfo> = app
        .invites
        .iter()
        .filter(|invite| {
            app.session
                .as_ref()
                .map(|s| s.session_id != invite.session_id)
                .unwrap_or(true)
        })
        .collect();

    if pending_invites.is_empty() {
        status_items.push(Line::from(vec![
            Span::styled("Invites: ", Style::default().fg(Color::Yellow)),
            Span::raw("None"),
        ]));
    } else {
        status_items.push(Line::from(vec![
            Span::styled("Invites: ", Style::default().fg(Color::Yellow)),
            Span::raw(
                pending_invites
                    .iter()
                    .map(|i| i.session_id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]));
    }

    // DKG Status using the new display trait - enhanced with more detailed state info
    let dkg_status = match app.dkg_state {
        crate::DkgState::Idle => "Idle".to_string(),
        crate::DkgState::CommitmentsInProgress => "Commitments In Progress".to_string(),
        crate::DkgState::CommitmentsComplete => "Commitments Complete".to_string(),
        crate::DkgState::SharesInProgress => "Share Distribution In Progress".to_string(),
        crate::DkgState::VerificationInProgress => "Verification In Progress".to_string(),
        crate::DkgState::Complete => {
            if let Some(pubkey) = &app.group_public_key {
                format!("Complete - Group Public Key: {:?}", pubkey) // Use debug formatting
            } else {
                "Complete".to_string()
            }
        },
        crate::DkgState::Failed(_) => "Failed".to_string(), // Fix: Use tuple variant pattern
        _ => app.dkg_state.display_status(),
    };
    
    let dkg_style = if app.dkg_state.is_active() {
        Style::default().fg(Color::Green)
    } else if app.dkg_state.is_completed() {
        Style::default().fg(Color::Blue)
    } else if matches!(app.dkg_state, crate::DkgState::Failed(_)) { // Fix: Use tuple variant pattern
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    };

    status_items.push(Line::from(vec![
        Span::styled("DKG Status: ", Style::default().fg(Color::Yellow)),
        Span::styled(dkg_status, dkg_style),
    ]));

    // Display additional connection info if in a session
    if let Some(session) = &app.session {
        let connected_peers = app.peer_statuses
            .iter()
            .filter(|&(_, &status)| 
                matches!(status, webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::Connected)
            )
            .count();
            
        let total_peers = session.participants.len() - 1; // Exclude self
        
        let connection_status = format!("{}/{} peers connected", connected_peers, total_peers);
        let connection_style = if connected_peers == total_peers {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Yellow)
        };
        
        status_items.push(Line::from(vec![
            Span::styled("WebRTC Connections: ", Style::default().fg(Color::Yellow)),
            Span::styled(connection_status, connection_style),
        ]));
    }

    let status_block = Block::default().title(" Status ").borders(Borders::ALL);
    let status_text = Paragraph::new(status_items)
        .block(status_block)
        .wrap(Wrap { trim: true });
    f.render_widget(status_text, area);
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
                } else if cmd_str.starts_with("/propose") {
                    // Handle the propose command as per documentation
                    // Format: /propose <session_id> <total> <threshold> <peer1,peer2,...>
                    let parts: Vec<_> = cmd_str.splitn(5, ' ').collect();
                    if parts.len() == 5 {
                        let session_id = parts[1].to_string();
                        
                        // Parse total participants
                        if let Ok(total) = parts[2].parse::<u16>() {
                            // Parse threshold
                            if let Ok(threshold) = parts[3].parse::<u16>() {
                                // Parse participants list
                                let participants: Vec<String> = parts[4]
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .collect();
                                
                                // Validate inputs
                                if total < 2 {
                                    app.log.push("Total participants must be at least 2".to_string());
                                } else if threshold < 1 || threshold > total {
                                    app.log.push(format!(
                                        "Threshold must be between 1 and {} (total participants)",
                                        total
                                    ));
                                } else if participants.len() != total as usize {
                                    app.log.push(format!(
                                        "Number of participants ({}) doesn't match the specified total ({})",
                                        participants.len(), total
                                    ));
                                } else {
                                    // Send the command to propose a session
                                    let _ = cmd_tx.send(InternalCommand::ProposeSession {
                                        session_id: session_id.clone(),
                                        total,
                                        threshold,
                                        participants: participants.clone(),
                                    });
                                    
                                    app.log.push(format!(
                                        "Proposed session '{}' with {} participants (threshold: {})",
                                        session_id, total, threshold
                                    ));
                                }
                            } else {
                                app.log.push("Invalid threshold value. Must be a positive number.".to_string());
                            }
                        } else {
                            app.log.push("Invalid total value. Must be a positive number.".to_string());
                        }
                    } else {
                        app.log.push(
                            "Invalid /propose format. Use: /propose <session_id> <total> <threshold> <peer1,peer2,...>"
                                .to_string(),
                        );
                    }
                } else if cmd_str.starts_with("/accept") {
                    // Add support for the /accept command
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let session_id = parts[1].to_string();
                        
                        // Check if the session proposal exists
                        if app.invites.iter().any(|invite| invite.session_id == session_id) {
                            // Send command to accept the session proposal
                            let _ = cmd_tx.send(InternalCommand::AcceptSessionProposal(session_id.clone()));
                            app.log.push(format!("Accepting session proposal '{}'", session_id));
                        } else {
                            app.log.push(format!("No session proposal found with ID '{}'", session_id));
                        }
                    } else {
                        app.log.push("Invalid /accept format. Use: /accept <session_id>".to_string());
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
                } else if cmd_str.starts_with("/mesh_ready") {
                    // Add support for manually signaling mesh readiness
                    let _ = cmd_tx.send(InternalCommand::SendOwnMeshReadySignal);                
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
            KeyCode::Char('o') => {
                // Accept the first pending invitation
                if let Some(invite) = app.invites.first() {
                    let session_id = invite.session_id.clone();
                    let _ = cmd_tx.send(InternalCommand::AcceptSessionProposal(session_id.clone()));
                    app.log.push(format!("Accepting session proposal '{}'", session_id));
                } else {
                    app.log.push("No pending session invitations".to_string());
                }
            }
            KeyCode::Char('q') => {
                app.log.push("Quitting...".to_string());
                return Ok(false); // Signal to quit
            }
            KeyCode::Char('s') => {
                // Save log to <peer_id>.log
                let filename = format!("{}.log", app.peer_id.trim());
                match std::fs::write(&filename, app.log.join("\n")) {
                    Ok(_) => app.log.push(format!("Log saved to {}", filename)),
                    Err(e) => app.log.push(format!("Failed to save log: {}", e)),
                }
            }
            KeyCode::Char('r') => {
                let _ = cmd_tx.send(InternalCommand::SendOwnMeshReadySignal);                
            }
            KeyCode::Up => {
                app.log_scroll = app.log_scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                app.log_scroll = app.log_scroll.saturating_add(1);
            }
            _ => {}
        }
    }
    Ok(true) // Continue loop by default
}
