//! Handler functions for keystore-related commands.

use std::sync::Arc;
use tokio::sync::Mutex;
use serde_json::json;

use crate::{
    keystore::Keystore,
    utils::state::AppState};

/// Handles the init_keystore command
pub async fn handle_init_keystore<C: frost_core::Ciphersuite + Send + Sync + 'static>(
    path: String,
    device_name: String,
    state: Arc<Mutex<AppState<C>>>,
) {
    let mut app_state = state.lock().await;
    
    match Keystore::new(&path, &device_name) {
        Ok(keystore) => {
            app_state.log.push(format!("Keystore initialized at {} for device '{}'", path, device_name));
            app_state.keystore = Some(Arc::new(keystore));
        }
        Err(e) => {
            app_state.log.push(format!("Failed to initialize keystore: {}", e));
        }
    }
}

/// Handles the list_wallets command
pub async fn handle_list_wallets<C: frost_core::Ciphersuite + Send + Sync + 'static>(
    state: Arc<Mutex<AppState<C>>>
) {
    let mut app_state = state.lock().await;
    
    if let Some(keystore) = &app_state.keystore {
        // First, collect the wallet info
        let wallets = keystore.list_wallets();
        
        if wallets.is_empty() {
            app_state.log.push("No wallets found in keystore.".to_string());
        } else {
            // Clone wallet information to avoid borrow issues
            let wallet_infos = wallets
                .iter()
                .map(|w| (
                    w.wallet_id.clone(),
                    w.name.clone(),
                    w.threshold,
                    w.total_participants,
                    w.blockchain.clone()
                ))
                .collect::<Vec<_>>();
            
            // Now that we're done with the keystore borrow, update the UI
            app_state.log.push("Available wallets:".to_string());
            
            for (id, name, threshold, total, blockchain) in wallet_infos {
                app_state.log.push(format!(
                    "ID: {} | Name: {} | Threshold: {}/{} | Blockchain: {}",
                    id, name, threshold, total, blockchain
                ));
            }
        }
    } else {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
    }
}

/// Handles the create_wallet command, creating a new wallet from DKG results
pub async fn handle_create_wallet<C: frost_core::Ciphersuite + Send + Sync + 'static>(
    name: String,
    description: Option<String>,
    password: String,
    tags: Vec<String>,
    state: Arc<Mutex<AppState<C>>>,
) {
    let mut app_state = state.lock().await;
    
    // Check if keystore is initialized
    if app_state.keystore.is_none() {
        app_state.log.push("Keystore is not initialized. Use /init_keystore first.".to_string());
        return;
    }
    
    // Check if DKG is completed
    if !matches!(app_state.dkg_state, crate::utils::state::DkgState::Complete) {
        app_state.log.push("DKG process is not complete. Cannot create wallet yet.".to_string());
        return;
    }
    
    // Get required data from DKG results
    if app_state.key_package.is_none() || app_state.group_public_key.is_none() || app_state.session.is_none() {
        app_state.log.push("Missing DKG results. Cannot create wallet.".to_string());
        return;
    }
    
    // Determine curve type and blockchain based on TypeId
    use std::any::TypeId;
    
    let curve_type_id = TypeId::of::<C>();
    let (curve_type, blockchain) = if curve_type_id == TypeId::of::<frost_secp256k1::Secp256K1Sha256>() {
        ("secp256k1", "ethereum")
    } else if curve_type_id == TypeId::of::<frost_ed25519::Ed25519Sha512>() {
        ("ed25519", "solana")
    } else {
        ("unknown", "unknown")
    };
    
    // Get public address
    let public_address = if blockchain == "ethereum" {
        app_state.etherum_public_key.clone().unwrap_or_else(|| "N/A".to_string())
    } else {
        app_state.solana_public_key.clone().unwrap_or_else(|| "N/A".to_string())
    };
    
    // Clone necessary data before dropping the lock
    let session_id = app_state.session.as_ref().unwrap().session_id.clone();
    let threshold = app_state.session.as_ref().unwrap().threshold;
    let total_participants = app_state.session.as_ref().unwrap().total;
    let device_id = app_state.device_id.clone();
    
    // Serialize the key package data
    let key_package_json = serde_json::to_string(app_state.key_package.as_ref().unwrap()).unwrap_or_default();
    let group_public_key_json = serde_json::to_string(app_state.group_public_key.as_ref().unwrap()).unwrap_or_default();
    
    // Serialize the KeyPackage and other necessary data
    let key_share_data = json!({
        "key_package": key_package_json,
        "group_public_key": group_public_key_json,
        "session_id": session_id,
        "device_id": device_id
    }).to_string();
    
    // Create wallet in keystore
    // We need to get a mutable reference to the inner keystore
    let keystore_clone = app_state.keystore.as_ref().unwrap().clone();
    
    // We need to drop the app_state lock before we try to get a mutable reference to keystore
    drop(app_state);
    
    // Since keystore is behind Arc, we need to get a mutable reference to it
    // This is unsafe but needed because Rust doesn't support Arc::get_mut with shared references
    // In a real-world application, we might want to use a better synchronization mechanism
    let keystore_ptr = Arc::into_raw(keystore_clone) as *mut Keystore;
    let result = unsafe {
        let keystore_mut = &mut *keystore_ptr;
        
        keystore_mut.create_wallet(
            &name,
            curve_type,
            blockchain,
            &public_address,
            threshold,
            total_participants,
            &group_public_key_json, // Already serialized
            key_share_data.as_bytes(),
            &password,
            tags,
            description,
        )
    };
    
    // Re-wrap the pointer in an Arc so it will be properly deallocated
    let _keystore = unsafe { Arc::from_raw(keystore_ptr) };
    
    // Now regain the lock and update the app state
    let mut app_state = state.lock().await;
    
    match result {
        Ok(wallet_id) => {
            app_state.log.push(format!("Wallet created successfully with ID: {}", wallet_id));
            app_state.current_wallet_id = Some(wallet_id);
        },
        Err(e) => {
            app_state.log.push(format!("Failed to create wallet: {}", e));
        }
    }
}