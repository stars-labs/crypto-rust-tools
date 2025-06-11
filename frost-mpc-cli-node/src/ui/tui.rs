use crate::protocal::signal::SessionInfo;
use crate::utils::state::{AppState, DkgStateDisplay}; // Now correctly imports DkgStateDisplay trait
use crate::{InternalCommand, ClientMsg, MeshStatus}; // Add MeshStatus import
use crossterm::event::{KeyCode, KeyEvent};
use std::any::TypeId; // Added for ciphersuite check
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
use frost_core::Ciphersuite;

pub fn draw_main_ui<B: Backend, C: Ciphersuite>(
    terminal: &mut Terminal<B>,
    app: &AppState<C>,
    input: &str,
    input_mode: bool,
) -> io::Result<()> {
    terminal.draw(|f| {
        // Main layout: Title, Devices, Log, Status, Input
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([
                Constraint::Length(3), // Title area
                Constraint::Length(5), // Devices area
                Constraint::Min(5),    // Log area (flexible height)
                Constraint::Length(8), // Status area (increased height for wrapping)
                Constraint::Length(3), // Input area
            ])
            .split(f.area());

        let title_block = Block::default()
            .title(format!(" Device ID: {} ", app.device_id)) // Add spacing
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded); // Use rounded borders
        f.render_widget(title_block, main_chunks[0]);

        let session_participants: HashSet<String> = app
            .session
            .as_ref()
            .map(|s| s.participants.iter().cloned().collect())
            .unwrap_or_default();

        let device_list_items = app
            .devices
            .iter()
            .filter(|p| !p.trim().eq_ignore_ascii_case(app.device_id.trim()))
            .map(|p| {
                let status_str = if session_participants.contains(p) {
                    // First check if there's an explicit status
                    if let Some(s) = app.device_statuses.get(p) {
                        // For clarity, add connection role in the status display
                        let role_prefix = if app.device_id < *p { "→" } else { "←" }; // Simplified comparison
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
                let style = match app.device_statuses.get(p) {
                    Some(webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Connected) => Style::default().fg(Color::Green),
                    Some(webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Connecting) => Style::default().fg(Color::Yellow),
                    Some(webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Failed) |
                    Some(webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Disconnected) |
                    Some(webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Closed) => Style::default().fg(Color::Red),
                    _ => Style::default(),
                };

                ListItem::new(format!("{} ({})", p, status_str)).style(style)
            })
            .collect::<Vec<_>>();

        let devices_widget =
            List::new(device_list_items) // Use the formatted list
                .block(Block::default().title(" Devices (Signaling) ").borders(Borders::ALL));
        f.render_widget(devices_widget, main_chunks[1]);

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
            // Help text for commands
            "Scroll: ↑/↓ | Input: i | Accept Invite: o | Quit: q | Keys auto-saved after DKG | Sign: /sign <hex>".to_string()
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

    // show curve 
    let c_type_id = TypeId::of::<T>();

    let curve_name = if c_type_id == TypeId::of::<frost_secp256k1::Secp256K1Sha256>() {
        "secp256k1"
    } else if c_type_id == TypeId::of::<frost_ed25519::Ed25519Sha512>() {
        "ed25519"
    } else {
        "unknown"
    };
    status_items.push(Line::from(vec![
        Span::styled("Curve: ", Style::default().fg(Color::Yellow)),
        Span::raw(curve_name),
    ]));

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
            MeshStatus::PartiallyReady { ready_devices, total_devices } => format!("Partially Ready ({}/{})", ready_devices.len(), total_devices),
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
        Span::styled(app.dkg_state.display_status(), dkg_style),
    ]));

    // Add signing status display
    let signing_style = if app.signing_state.is_active() {
        Style::default().fg(Color::Green)
    } else if matches!(app.signing_state, crate::utils::state::SigningState::Complete { .. }) {
        Style::default().fg(Color::Blue)
    } else if matches!(app.signing_state, crate::utils::state::SigningState::Failed { .. }) {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Gray)
    };

    status_items.push(Line::from(vec![
        Span::styled("Signing Status: ", Style::default().fg(Color::Yellow)),
        Span::styled(app.signing_state.display_status(), signing_style),
    ]));

    if curve_name == "secp256k1" && app.etherum_public_key.is_some() {
        status_items.push(Line::from(vec![
            Span::styled("Ethereum Address: ", Style::default().fg(Color::Yellow)),
            Span::raw(app.etherum_public_key.clone().unwrap()),
        ]));
    } else if curve_name == "ed25519" && app.solana_public_key.is_some() {
        status_items.push(Line::from(vec![
            Span::styled("Solana Address: ", Style::default().fg(Color::Yellow)),
            Span::raw(app.solana_public_key.clone().unwrap()),
        ]));
    } else {
        status_items.push(Line::from(vec![
            Span::styled("Address: ", Style::default().fg(Color::Yellow)),
            Span::raw("N/A"),
        ]));        
    }

    // Display additional connection info if in a session
    if let Some(session) = &app.session {
        let connected_devices = app.device_statuses
            .iter()
            .filter(|&(_, &status)| 
                matches!(status, webrtc::device_connection::device_connection_state::RTCDeviceConnectionState::Connected)
            )
            .count();
            
        let total_devices = session.participants.len() - 1; // Exclude self
        
        let connection_status = format!("{}/{} devices connected", connected_devices, total_devices);
        let connection_style = if connected_devices == total_devices {
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
pub fn handle_key_event<C>(
    key: KeyEvent,
    // Add generic parameter here
    app: &mut AppState<C>, // Now mutable and generic
    input: &mut String,                // Now mutable
    input_mode: &mut bool,             // Now mutable
    // Update the sender type to use the new InternalCommand
    cmd_tx: &mpsc::UnboundedSender<InternalCommand<C>>, // Pass as reference
) -> anyhow::Result<bool> where C: Ciphersuite {
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
                if cmd_str == "/list" {
                    let _ = cmd_tx.send(InternalCommand::SendToServer(ClientMsg::ListDevices));
                } else if cmd_str.starts_with("/list_wallets") {
                    // Handle the /list_wallets command
                    let _ = cmd_tx.send(InternalCommand::ListWallets);
                    app.log.push("Listing available wallets...".to_string());
                } else if cmd_str.starts_with("/propose") {
                    // Handle the propose command as per documentation
                    // Format: /propose <session_id> <total> <threshold> <device1,device2,...>
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
                            "Invalid /propose format. Use: /propose <session_id> <total> <threshold> <device1,device2,...>"
                                .to_string(),
                        );
                    }
                } else if cmd_str.starts_with("/accept") {
                    // Enhanced /accept command that handles both session proposals and signing requests
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let id = parts[1].to_string();
                        
                        // First, check if it's a session proposal
                        if app.invites.iter().any(|invite| invite.session_id == id) {
                            // Send command to accept the session proposal
                            let _ = cmd_tx.send(InternalCommand::AcceptSessionProposal(id.clone()));
                            app.log.push(format!("Accepting session proposal '{}'", id));
                        }
                        // If not a session proposal, check if it's a signing request
                        else if let Some(signing_id) = app.signing_state.get_signing_id() {
                            if signing_id == id && matches!(app.signing_state, crate::utils::state::SigningState::AwaitingAcceptance { .. }) {
                                // Send command to accept the signing request
                                let _ = cmd_tx.send(InternalCommand::AcceptSigning {
                                    signing_id: id.clone(),
                                });
                                app.log.push(format!("Accepting signing request '{}'", id));
                            } else {
                                app.log.push(format!("No pending signing request with ID '{}'. Current signing state: {}", id, app.signing_state.display_status()));
                            }
                        }
                        // Neither session proposal nor signing request found
                        else {
                            app.log.push(format!("No session proposal or signing request found with ID '{}'. Use /acceptSign <id> for signing requests or check available invites.", id));
                        }
                    } else {
                        app.log.push("Invalid /accept format. Use: /accept <id> (works for both session proposals and signing requests)".to_string());
                    }
                } else if cmd_str.starts_with("/sign") {
                    // Handle the /sign command for transaction signing
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let transaction_data = parts[1].to_string();
                        
                        // Validate transaction data is hex
                        if transaction_data.chars().all(|c| c.is_ascii_hexdigit()) {
                            let _ = cmd_tx.send(InternalCommand::InitiateSigning {
                                transaction_data: transaction_data.clone(),
                            });
                            app.log.push(format!("Initiating signing for transaction: {}", transaction_data));
                        } else {
                            app.log.push("Invalid transaction data. Must be hexadecimal string.".to_string());
                        }
                    } else {
                        app.log.push("Invalid /sign format. Use: /sign <transaction_hex>".to_string());
                    }
                } else if cmd_str.starts_with("/init_keystore") {
                    // Handle the /init_keystore command - use convention over configuration
                    
                    // Standard path: ~/.frost_keystore
                    let home_dir = dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let path = home_dir.join(".frost_keystore").to_string_lossy().into_owned();
                    
                    // Device name based on device ID
                    let device_name = format!("device-{}", app.device_id);
                    
                    let _ = cmd_tx.send(InternalCommand::InitKeystore {
                        path: path.clone(),
                        device_name: device_name.clone(),
                    });
                    app.log.push(format!("Initializing keystore at {}", path));
                } else if cmd_str.starts_with("/create_wallet") {
                    // Handle the /create_wallet command - use convention over configuration
                    
                    // First check if DKG is complete
                    if !matches!(app.dkg_state, crate::utils::state::DkgState::Complete) {
                        app.log.push("⚠️ DKG process is not complete. Cannot create wallet yet.".to_string());
                        app.log.push("Complete the DKG process first by joining a session and completing key generation.".to_string());
                    } 
                    // Check if keystore is initialized
                    else if app.keystore.is_none() {
                        app.log.push("⚠️ Keystore failed to initialize automatically. Restart the application or check file permissions.".to_string());
                    }
                    else {
                    
                    // Generate a wallet name based on the DKG session or date
                    let name = if let Some(session) = &app.session {
                        format!("wallet-{}", session.session_id)
                    } else {
                        // Use current date/time if no session
                        let now = chrono::Local::now();
                        format!("wallet-{}", now.format("%Y-%m-%d-%H%M"))
                    };
                    
                    // Use device ID as a simple default password
                    // In a real app, we might want to generate a secure random password or prompt the user
                    let password = app.device_id.clone();
                    
                    // Create simple description
                    let description = if let Some(session) = &app.session {
                        Some(format!("Threshold {}/{} wallet created on {}", 
                            session.threshold, 
                            session.total,
                            chrono::Local::now().format("%Y-%m-%d %H:%M")
                        ))
                    } else {
                        None
                    };
                    
                    // Default tags based on the cryptographic curve
                    let curve_name = if app.solana_public_key.is_some() {
                        "ed25519"
                    } else if app.etherum_public_key.is_some() {
                        "secp256k1" 
                    } else {
                        "unknown"
                    };
                    
                    let tags = vec![curve_name.to_string()];
                    
                    let _ = cmd_tx.send(InternalCommand::CreateWallet {
                        name: name.clone(),
                        description: description.clone(),
                        password: password.clone(),
                        tags: tags.clone(),
                    });
                    
                    app.log.push(format!("Creating wallet '{}' with DKG key share from the completed session", name));
                    app.log.push("⚙️ Storing FROST threshold signature key share in your keystore...".to_string());
                    app.log.push("🔑 Password set to your device ID. Remember to back up your keystore!".to_string());
                    } // close the else block
                } else if cmd_str.starts_with("/acceptSign") {
                    // Handle the /acceptSign command
                    let parts: Vec<_> = cmd_str.split_whitespace().collect();
                    if parts.len() == 2 {
                        let signing_id = parts[1].to_string();
                        
                        let _ = cmd_tx.send(InternalCommand::AcceptSigning {
                            signing_id: signing_id.clone(),
                        });
                        app.log.push(format!("Accepting signing request: {}", signing_id));
                    } else {
                        app.log.push("Invalid /acceptSign format. Use: /acceptSign <signing_id>".to_string());
                    }
                } else if cmd_str.starts_with("/relay") {
                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                    if parts.len() == 3 {
                        let target_device_id = parts[1].to_string();
                        let json_str = parts[2];
                        match serde_json::from_str::<serde_json::Value>(json_str) {
                            Ok(data) => {
                                let _ = cmd_tx.send(InternalCommand::SendToServer(
                                    ClientMsg::Relay {
                                        to: target_device_id.clone(),
                                        data,
                                    },
                                ));
                                app.log
                                    .push(format!("Relaying message to {}", target_device_id));
                            }
                            Err(e) => {
                                app.log.push(format!("Invalid JSON for /relay: {}", e));
                            }
                        }
                    } else {
                        app.log.push(
                            "Invalid /relay format. Use: /relay <device_id> <json_data>".to_string(),
                        );
                    }
                } else if cmd_str.starts_with("/send") {
                    // This command now sends a simple text message via WebRTCMessage::SimpleMessage
                    let parts: Vec<_> = cmd_str.splitn(3, ' ').collect();
                    if parts.len() >= 3 {
                        let target_device_id = parts[1].to_string();
                        let message_text = parts[2].to_string();

                        // Always log the send attempt, regardless of connection state
                        app.log.push(format!(
                            "Attempting to send direct message to {}: {}",
                            target_device_id, message_text
                        ));

                        // Send internal command
                        let _ = cmd_tx.send(InternalCommand::SendDirect {
                            to: target_device_id.clone(),
                            message: message_text.clone(),
                        });

                        // Log the command for visibility
                        app.log.push(format!(
                            "Command: /send {} {}",
                            target_device_id, message_text
                        ));
                    } else {
                        app.log.push(
                            "Invalid /send format. Use: /send <device_id> <message>".to_string(),
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
                // Save log to <device_id>.log
                let filename = format!("{}.log", app.device_id.trim());
                match std::fs::write(&filename, app.log.join("\n")) {
                    Ok(_) => app.log.push(format!("Log saved to {}", filename)),
                    Err(e) => app.log.push(format!("Failed to save log: {}", e)),
                }
            }
            KeyCode::Char('d') => {
                // Quick test - Send predefined session proposal
                let session_id = "wallet_2of3".to_string();
                let participants = vec!["mpc-1".to_string(), "mpc-2".to_string(), "mpc-3".to_string()];
                
                let _ = cmd_tx.send(InternalCommand::ProposeSession {
                    session_id: session_id.clone(),
                    total: 3,
                    threshold: 2,
                    participants: participants.clone(),
                });
                
                app.log.push(format!(
                    "Quick test: Proposed session '{}' with {} participants (threshold: {})",
                    session_id, 3, 2
                ));
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
