//! Handler functions for keystore-related commands.

use frost_core::Ciphersuite;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use crate::utils::state::{AppState, InternalCommand};
use crate::keystore::{Keystore, KeystoreError};

/// Handles the init_keystore command
pub async fn handle_init_keystore<C: Ciphersuite + Send + Sync + 'static>(
    path: String,
    device_name: String,
    state: Arc<Mutex<AppState<C>>>,
) {
    let mut app_state = state.lock().await;
    
    let result = match Keystore::new(&path, &device_name) {
        Ok(keystore) => {
            app_state.log.push(format!("Keystore initialized at {} for device '{}'", path, device_name));
            app_state.keystore = Some(Arc::new(keystore));
            Ok(())
        }
        Err(e) => {
            Err(format!("Failed to initialize keystore: {}", e))
        }
    };
    
    if let Err(e) = result {
        app_state.log.push(e);
    }
}

/// Handles the list_wallets command
pub async fn handle_list_wallets<C: Ciphersuite + Send + Sync + 'static>(
    state: Arc<Mutex<AppState<C>>>
) {
    let mut app_state = state.lock().await;
    
    if let Some(keystore) = &app_state.keystore {
        let wallets = keystore.list_wallets();
        
        if wallets.is_empty() {
            app_state.log.push("No wallets found in keystore.".to_string());
        } else {
            app_state.log.push("Available wallets:".to_string());
            for wallet in wallets {
                app_state.log.push(format!(
                    "ID: {} | Name: {} | Threshold: {}/{} | Blockchain: {}",
                    wallet.wallet_id,
                    wallet.name,
                    wallet.threshold,
                    wallet.total_participants,
                    wallet.blockchain
                ));
            }
        }
    } else {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
    }
}

/// Handles the create_wallet command
pub async fn handle_create_wallet<C: Ciphersuite + Send + Sync + 'static>(
    name: String,
    password: String,
    description: Option<String>,
    tags: Option<Vec<String>>,
    state: Arc<Mutex<AppState<C>>>,
) where
    C: Ciphersuite + serde::Serialize,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let mut app_state = state.lock().await;
    
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    if app_state.key_package.is_none() || app_state.identifier_map.is_none() || app_state.group_public_key.is_none() {
        app_state.log.push("No active key package. Complete DKG first.".to_string());
        return;
    }
    
    let keystore = app_state.keystore.as_ref().unwrap().clone();
    let key_package = app_state.key_package.as_ref().unwrap().clone();
    let identifier_map = app_state.identifier_map.as_ref().unwrap().clone();
    let group_public_key = app_state.group_public_key.as_ref().unwrap().clone();
    
    // Determine curve type and blockchain from the session or DKG info
    let curve_type = if let Some(session) = &app_state.session {
        session.curve.clone()
    } else {
        "unknown".to_string() // Should be determined from the curve
    };
    
    let blockchain = match curve_type.as_str() {
        "secp256k1" => "ethereum",
        "ed25519" => "solana",
        _ => "unknown",
    };
    
    // Determine public address
    let public_address = if curve_type == "secp256k1" {
        app_state.etherum_public_key.clone().unwrap_or_else(|| "unknown".to_string())
    } else {
        app_state.solana_public_key.clone().unwrap_or_else(|| "unknown".to_string())
    };
    
    // Get threshold and total participants from the session
    let (threshold, total_participants) = if let Some(session) = &app_state.session {
        (session.threshold, session.total)
    } else {
        (0, 0) // Should be determined from the DKG info
    };
    
    // Convert the group public key to a string representation
    let public_key_bytes = format!("{:?}", group_public_key);
    
    // Create tags if provided, or use defaults
    let tags = tags.unwrap_or_else(|| vec![blockchain.to_string()]);
    
    // Drop the lock before making the async call to avoid deadlock
    drop(app_state);
    
    // Create the wallet in the keystore
    let result = keystore.create_wallet(
        &name,
        None, // Generate a new wallet ID
        key_package,
        identifier_map,
        &public_key_bytes,
        threshold,
        total_participants,
        &curve_type,
        blockchain,
        &public_address,
        &password,
        tags,
        description,
    );
    
    // Re-acquire the lock to update the state
    let mut app_state = state.lock().await;
    
    match result {
        Ok(wallet_id) => {
            app_state.current_wallet_id = Some(wallet_id.clone());
            app_state.log.push(format!("Wallet '{}' created with ID: {}", name, wallet_id));
        }
        Err(e) => {
            app_state.log.push(format!("Failed to create wallet: {}", e));
        }
    }
}

/// Handles the load_wallet command
pub async fn handle_load_wallet<C: Ciphersuite + Send + Sync + 'static>(
    wallet_id: String,
    password: String,
    state: Arc<Mutex<AppState<C>>>,
) where
    C: Ciphersuite + serde::de::DeserializeOwned,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let mut app_state = state.lock().await;
    
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    let keystore = app_state.keystore.as_ref().unwrap().clone();
    
    // Drop the lock before making the async call to avoid deadlock
    drop(app_state);
    
    // Load the wallet from the keystore
    let result = keystore.load_wallet::<C>(&wallet_id, &password);
    
    // Re-acquire the lock to update the state
    let mut app_state = state.lock().await;
    
    match result {
        Ok((key_package, identifier_map, group_public_key)) => {
            app_state.key_package = Some(key_package);
            app_state.identifier_map = Some(identifier_map);
            app_state.group_public_key = group_public_key;
            app_state.current_wallet_id = Some(wallet_id.clone());
            app_state.dkg_state = crate::utils::state::DkgState::Complete; // Mark DKG as complete since we loaded a key
            
            app_state.log.push(format!("Wallet '{}' loaded successfully", wallet_id));
            
            // Extract blockchain-specific info if available
            if let Some(wallet_info) = keystore.get_wallet(&wallet_id) {
                match wallet_info.blockchain.as_str() {
                    "ethereum" => {
                        app_state.etherum_public_key = Some(wallet_info.public_address.clone());
                    }
                    "solana" => {
                        app_state.solana_public_key = Some(wallet_info.public_address.clone());
                    }
                    _ => {}
                }
                
                // Log additional wallet details
                app_state.log.push(format!(
                    "Wallet details: Name: {}, Threshold: {}/{}, Blockchain: {}",
                    wallet_info.name,
                    wallet_info.threshold,
                    wallet_info.total_participants,
                    wallet_info.blockchain
                ));
            }
        }
        Err(e) => {
            app_state.log.push(format!("Failed to load wallet: {}", e));
        }
    }
}

/// Handles the export_share command
pub async fn handle_export_share<C: Ciphersuite + Send + Sync + 'static>(
    wallet_id: String,
    file_path: String,
    password: String,
    state: Arc<Mutex<AppState<C>>>,
) where
    C: Ciphersuite + serde::Serialize + serde::de::DeserializeOwned,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let mut app_state = state.lock().await;
    
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    let keystore = app_state.keystore.as_ref().unwrap().clone();
    
    // Drop the lock before making the async call to avoid deadlock
    drop(app_state);
    
    // Export the share from the keystore
    let result = keystore.export_share::<C>(&wallet_id, &password, &password); // Use same password for export
    
    // Re-acquire the lock to update the state
    let mut app_state = state.lock().await;
    
    match result {
        Ok(encrypted_share) => {
            // Write the encrypted share to the specified file
            match std::fs::write(&file_path, encrypted_share) {
                Ok(_) => {
                    app_state.log.push(format!("Share for wallet '{}' exported to {}", wallet_id, file_path));
                }
                Err(e) => {
                    app_state.log.push(format!("Failed to write share to file: {}", e));
                }
            }
        }
        Err(e) => {
            app_state.log.push(format!("Failed to export share: {}", e));
        }
    }
}

/// Handles the import_share command
pub async fn handle_import_share<C: Ciphersuite + Send + Sync + 'static>(
    wallet_id: String,
    file_path: String,
    password: String,
    state: Arc<Mutex<AppState<C>>>,
) where
    C: Ciphersuite + serde::Serialize + serde::de::DeserializeOwned,
    <<C as Ciphersuite>::Group as frost_core::Group>::Element: Send + Sync,
    <<<C as Ciphersuite>::Group as frost_core::Group>::Field as frost_core::Field>::Scalar: Send + Sync,
{
    let mut app_state = state.lock().await;
    
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    let keystore = app_state.keystore.as_ref().unwrap().clone();
    
    // Read the encrypted share from the file
    let encrypted_share = match std::fs::read(&file_path) {
        Ok(data) => data,
        Err(e) => {
            app_state.log.push(format!("Failed to read share file: {}", e));
            return;
        }
    };
    
    // Drop the lock before making the async call to avoid deadlock
    drop(app_state);
    
    // Import the share to the keystore
    let result = keystore.import_share::<C>(&encrypted_share, &password, &password); // Use same password for import
    
    // Re-acquire the lock to update the state
    let mut app_state = state.lock().await;
    
    match result {
        Ok(imported_wallet_id) => {
            if imported_wallet_id == wallet_id {
                app_state.log.push(format!("Share for wallet '{}' imported successfully", wallet_id));
            } else {
                app_state.log.push(format!(
                    "Warning: Imported share is for wallet '{}' (expected '{}')",
                    imported_wallet_id, wallet_id
                ));
            }
        }
        Err(e) => {
            app_state.log.push(format!("Failed to import share: {}", e));
        }
    }
}

/// Handles the delete_wallet command
pub async fn handle_delete_wallet<C: Ciphersuite + Send + Sync + 'static>(
    wallet_id: String,
    state: Arc<Mutex<AppState<C>>>,
) {
    let mut app_state = state.lock().await;
    
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    let keystore = app_state.keystore.as_ref().unwrap().clone();
    
    // Drop the lock before making the async call to avoid deadlock
    drop(app_state);
    
    // Delete the wallet from the keystore
    let result = keystore.delete_wallet(&wallet_id);
    
    // Re-acquire the lock to update the state
    let mut app_state = state.lock().await;
    
    match result {
        Ok(true) => {
            // If the deleted wallet was the current one, clear the current state
            if app_state.current_wallet_id.as_deref() == Some(&wallet_id) {
                app_state.current_wallet_id = None;
                app_state.key_package = None;
                app_state.identifier_map = None;
                app_state.group_public_key = None;
                app_state.dkg_state = crate::utils::state::DkgState::Idle;
            }
            
            app_state.log.push(format!("Wallet '{}' deleted successfully", wallet_id));
        }
        Ok(false) => {
            app_state.log.push(format!("Wallet '{}' not found", wallet_id));
        }
        Err(e) => {
            app_state.log.push(format!("Failed to delete wallet: {}", e));
        }
    }
}