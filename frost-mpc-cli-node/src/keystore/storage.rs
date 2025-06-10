//! Storage functionality for the keystore module.
//!
//! This module provides functions for saving and loading keystore data to disk,
//! including encrypted wallet files and the keystore index.

use frost_core::{Ciphersuite, Identifier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::{
    encryption::{decrypt_data, encrypt_data},
    models::{DeviceInfo, KeystoreFile, KeystoreIndex, WalletInfo},
    KeystoreError, Result, KEYSTORE_VERSION,
};

/// Main keystore interface
pub struct Keystore {
    /// Base path for keystore files
    base_path: PathBuf,
    
    /// Unique identifier for this device
    device_id: String,
    
    /// User-friendly name for this device
    device_name: String,
    
    /// Keystore index containing metadata for wallets and devices
    index: KeystoreIndex,
}

impl Keystore {
    /// File name for the keystore index
    const INDEX_FILE: &'static str = "index.json";
    
    /// File name for the device ID
    const DEVICE_ID_FILE: &'static str = "device_id";
    
    /// Directory for wallet files
    const WALLETS_DIR: &'static str = "wallets";

    /// Creates a new keystore at the specified path with the given device name.
    /// If the keystore already exists, it will be loaded.
    ///
    /// # Arguments
    ///
    /// * `base_path` - Base path for keystore files
    /// * `device_name` - User-friendly name for this device
    ///
    /// # Returns
    ///
    /// A new `Keystore` instance
    pub fn new(base_path: impl AsRef<Path>, device_name: &str) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        
        // Create directory structure if it doesn't exist
        fs::create_dir_all(&base_path)?;
        fs::create_dir_all(base_path.join(Self::WALLETS_DIR))?;
        
        // Generate or load device ID
        let device_id_path = base_path.join(Self::DEVICE_ID_FILE);
        let device_id = if device_id_path.exists() {
            let mut file = File::open(device_id_path)?;
            let mut device_id = String::new();
            file.read_to_string(&mut device_id)?;
            device_id
        } else {
            // Generate a new UUID for the device
            let device_id = Uuid::new_v4().to_string();
            let mut file = File::create(device_id_path)?;
            file.write_all(device_id.as_bytes())?;
            device_id
        };
        
        // Load or create index file
        let index_path = base_path.join(Self::INDEX_FILE);
        let index = if index_path.exists() {
            let index_file = File::open(index_path)?;
            serde_json::from_reader(index_file).map_err(KeystoreError::SerializationError)?
        } else {
            let mut index = KeystoreIndex::new();
            
            // Add this device to the index
            let device_info = DeviceInfo::new(
                device_id.clone(),
                device_name.to_string(),
                format!("device-{}", device_id.split('-').next().unwrap_or("unknown")),
                String::new(), // Empty identifier - will be set when used in a wallet
            );
            index.add_device(device_info);
            
            // Save the index
            let index_file = File::create(index_path)?;
            serde_json::to_writer_pretty(index_file, &index).map_err(KeystoreError::SerializationError)?;
            
            index
        };
        
        Ok(Self {
            base_path,
            device_id,
            device_name: device_name.to_string(),
            index,
        })
    }

    /// Saves the keystore index to disk
    fn save_index(&self) -> Result<()> {
        let index_path = self.base_path.join(Self::INDEX_FILE);
        let index_file = File::create(index_path)?;
        serde_json::to_writer_pretty(index_file, &self.index).map_err(KeystoreError::SerializationError)?;
        Ok(())
    }

    /// Gets the device ID for this keystore
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Gets the device name for this keystore
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Lists all wallets in the keystore index
    pub fn list_wallets(&self) -> Vec<&WalletInfo> {
        self.index.wallets.iter().collect()
    }

    /// Gets a wallet by ID
    pub fn get_wallet(&self, wallet_id: &str) -> Option<&WalletInfo> {
        self.index.get_wallet(wallet_id)
    }

    /// Gets this device's info
    pub fn get_this_device(&self) -> Option<&DeviceInfo> {
        self.index.get_device(&self.device_id)
    }

    /// Creates a new wallet from the results of a DKG process
    ///
    /// # Arguments
    ///
    /// * `name` - User-friendly name for the wallet
    /// * `wallet_id` - Optional wallet ID (generated if not provided)
    /// * `key_package` - Key package from DKG
    /// * `identifier_map` - Map of peer IDs to identifiers
    /// * `public_key_bytes` - Serialized group public key
    /// * `threshold` - Minimum number of participants required to sign
    /// * `total_participants` - Total number of participants
    /// * `curve_type` - Type of cryptographic curve ("secp256k1" or "ed25519")
    /// * `blockchain` - Blockchain the wallet is intended for ("ethereum" or "solana")
    /// * `public_address` - Public blockchain address derived from the group public key
    /// * `password` - Password to encrypt the wallet
    /// * `tags` - User-defined tags for organizing wallets
    /// * `description` - Optional description for the wallet
    ///
    /// # Returns
    ///
    /// The ID of the created wallet
    pub fn create_wallet<C: Ciphersuite + Serialize>(
        &mut self,
        name: &str,
        wallet_id: Option<String>,
        key_package: frost_core::keys::KeyPackage<C>,
        identifier_map: BTreeMap<String, Identifier<C>>,
        public_key_bytes: &str,
        threshold: u16,
        total_participants: u16,
        curve_type: &str,
        blockchain: &str,
        public_address: &str,
        password: &str,
        tags: Vec<String>,
        description: Option<String>,
    ) -> Result<String> {
        // Generate wallet ID if not provided
        let wallet_id = wallet_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        
        // Get this device's identifier from the identifier_map
        let self_identifier = identifier_map
            .get(&self.device_id)
            .ok_or_else(|| KeystoreError::General("Device identifier not found in map".to_string()))?
            .clone();
        
        // Create wallet info for the index
        let mut wallet_info = WalletInfo::new(
            wallet_id.clone(),
            name.to_string(),
            curve_type.to_string(),
            blockchain.to_string(),
            public_address.to_string(),
            threshold,
            total_participants,
            public_key_bytes.to_string(),
            tags,
            description,
        );
        
        // Add this device to the wallet
        let mut device_info = self.get_this_device()
            .ok_or_else(|| KeystoreError::DeviceNotFound(self.device_id.clone()))?
            .clone();
        
        // Update the device's identifier
        device_info.identifier = format!("{:?}", self_identifier);
        device_info.update_last_seen();
        
        wallet_info.add_device(device_info.clone());
        self.index.add_wallet(wallet_info);
        self.index.add_device(device_info);
        self.save_index()?;
        
        // Create keystore file
        let keystore_file = KeystoreFile::new(
            wallet_id.clone(),
            self.device_id.clone(),
            key_package,
            identifier_map,
        );
        
        // Serialize and encrypt
        let serialized = serde_json::to_vec(&keystore_file).map_err(KeystoreError::SerializationError)?;
        let encrypted = encrypt_data(&serialized, password)?;
        
        // Save to file
        let wallet_path = self.base_path
            .join(Self::WALLETS_DIR)
            .join(format!("{}.key", wallet_id));
        
        let mut file = File::create(wallet_path)?;
        file.write_all(&encrypted)?;
        
        Ok(wallet_id)
    }

    /// Loads a wallet's key material
    ///
    /// # Arguments
    ///
    /// * `wallet_id` - ID of the wallet to load
    /// * `password` - Password to decrypt the wallet
    ///
    /// # Returns
    ///
    /// The key package, identifier map, and group public key for the wallet
    pub fn load_wallet<C: Ciphersuite + for<'de> Deserialize<'de>>(
        &self,
        wallet_id: &str,
        password: &str,
    ) -> Result<(
        frost_core::keys::KeyPackage<C>,
        BTreeMap<String, Identifier<C>>,
        Option<frost_core::keys::PublicKeyPackage<C>>,
    )> {
        // Check if wallet exists in the index
        let _wallet_info = self.index.get_wallet(wallet_id)
            .ok_or_else(|| KeystoreError::WalletNotFound(wallet_id.to_string()))?;
        
        // Load the wallet file
        let wallet_path = self.base_path
            .join(Self::WALLETS_DIR)
            .join(format!("{}.key", wallet_id));
        
        let mut file = File::open(wallet_path)?;
        let mut encrypted = Vec::new();
        file.read_to_end(&mut encrypted)?;
        
        // Decrypt and deserialize
        let decrypted = decrypt_data(&encrypted, password)?;
        let keystore_file: KeystoreFile<C> = serde_json::from_slice(&decrypted)
            .map_err(KeystoreError::SerializationError)?;
        
        // Extract key material
        let key_package = keystore_file.key_package;
        let identifier_map = keystore_file.identifier_map;
        
        // Try to compute the group public key
        let group_public_key = key_package.public_key_package();
        
        Ok((key_package, identifier_map, group_public_key))
    }

    /// Exports a share for backup or sharing with another device
    ///
    /// # Arguments
    ///
    /// * `wallet_id` - ID of the wallet to export
    /// * `password` - Password to decrypt the wallet
    /// * `export_password` - Password to encrypt the exported share
    ///
    /// # Returns
    ///
    /// The encrypted share data
    pub fn export_share<C: Ciphersuite + Serialize + for<'de> Deserialize<'de>>(
        &self,
        wallet_id: &str,
        password: &str,
        export_password: &str,
    ) -> Result<Vec<u8>> {
        // Load the wallet
        let (key_package, identifier_map, _) = self.load_wallet::<C>(wallet_id, password)?;
        
        // Create keystore file for export
        let keystore_file = KeystoreFile::new(
            wallet_id.to_string(),
            self.device_id.clone(),
            key_package,
            identifier_map,
        );
        
        // Serialize and encrypt with the export password
        let serialized = serde_json::to_vec(&keystore_file).map_err(KeystoreError::SerializationError)?;
        let encrypted = encrypt_data(&serialized, export_password)?;
        
        Ok(encrypted)
    }

    /// Imports a share from another device
    ///
    /// # Arguments
    ///
    /// * `encrypted_share` - Encrypted share data
    /// * `import_password` - Password to decrypt the imported share
    /// * `storage_password` - Password to encrypt the share for local storage
    ///
    /// # Returns
    ///
    /// The wallet ID of the imported share
    pub fn import_share<C: Ciphersuite + Serialize + for<'de> Deserialize<'de>>(
        &mut self,
        encrypted_share: &[u8],
        import_password: &str,
        storage_password: &str,
    ) -> Result<String> {
        // Decrypt the imported share
        let decrypted = decrypt_data(encrypted_share, import_password)?;
        let keystore_file: KeystoreFile<C> = serde_json::from_slice(&decrypted)
            .map_err(KeystoreError::SerializationError)?;
        
        let wallet_id = keystore_file.wallet_id.clone();
        let source_device_id = keystore_file.device_id.clone();
        
        // Check if wallet exists in the index
        if let Some(wallet_info) = self.index.get_wallet(&wallet_id) {
            // Wallet exists, but we need to make sure this share isn't already imported
            for device in &wallet_info.devices {
                if device.device_id == source_device_id {
                    // This share is already imported, update it
                    break;
                }
            }
        } else {
            // Wallet doesn't exist in index, get public key package to create it
            let public_key_package = keystore_file.key_package.public_key_package()
                .ok_or_else(|| KeystoreError::InvalidKeyPackage)?;
            
            // For simplicity, we'll create a basic wallet info
            // In a real implementation, you'd probably want to get more details from the imported share
            let mut wallet_info = WalletInfo::new(
                wallet_id.clone(),
                format!("Imported Wallet {}", wallet_id),
                "unknown".to_string(),  // You'd determine this from the imported share
                "unknown".to_string(),  // You'd determine this from the imported share
                "unknown".to_string(),  // You'd determine this from the imported share
                0,  // You'd determine this from the imported share
                0,  // You'd determine this from the imported share
                format!("{:?}", public_key_package),
                vec!["imported".to_string()],
                Some("Imported wallet".to_string()),
            );
            
            // We'd also add the source device to the wallet
            let device_info = DeviceInfo::new(
                source_device_id.clone(),
                format!("Device {}", source_device_id),
                "unknown".to_string(),
                "unknown".to_string(),
            );
            wallet_info.add_device(device_info);
            
            self.index.add_wallet(wallet_info);
            self.save_index()?;
        }
        
        // Save the imported share with the new storage password
        // We'll save it as wallet_id_source_device_id.share to distinguish it from our own key
        let serialized = serde_json::to_vec(&keystore_file).map_err(KeystoreError::SerializationError)?;
        let encrypted = encrypt_data(&serialized, storage_password)?;
        
        let share_path = self.base_path
            .join(Self::WALLETS_DIR)
            .join(format!("{}_{}.share", wallet_id, source_device_id));
        
        let mut file = File::create(share_path)?;
        file.write_all(&encrypted)?;
        
        Ok(wallet_id)
    }

    /// Deletes a wallet from the keystore
    ///
    /// # Arguments
    ///
    /// * `wallet_id` - ID of the wallet to delete
    ///
    /// # Returns
    ///
    /// `true` if the wallet was deleted, `false` if it didn't exist
    pub fn delete_wallet(&mut self, wallet_id: &str) -> Result<bool> {
        // Remove wallet from index
        let removed = self.index.remove_wallet(wallet_id);
        
        if removed {
            self.save_index()?;
            
            // Delete all wallet files (both .key and .share files)
            let wallets_dir = self.base_path.join(Self::WALLETS_DIR);
            let key_path = wallets_dir.join(format!("{}.key", wallet_id));
            
            if key_path.exists() {
                fs::remove_file(key_path)?;
            }
            
            // Find and delete all share files for this wallet
            let share_prefix = format!("{}_", wallet_id);
            for entry in fs::read_dir(wallets_dir)? {
                let entry = entry?;
                let path = entry.path();
                
                if let Some(file_name) = path.file_name() {
                    if let Some(file_name_str) = file_name.to_str() {
                        if file_name_str.starts_with(&share_prefix) && file_name_str.ends_with(".share") {
                            fs::remove_file(path)?;
                        }
                    }
                }
            }
            
            Ok(true)
        } else {
            Ok(false)
        }
    }
}