//! Storage functionality for the keystore module.
//!
//! This module provides functions for saving and loading keystore data to disk,
//! including encrypted wallet files and the keystore index.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use super::{
    KeystoreError, Result,
    encryption::{decrypt_data, encrypt_data},
    models::{DeviceInfo, KeystoreIndex, WalletInfo},
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
    pub fn new(base_path: impl AsRef<Path>, device_name: &str) -> Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();

        // Create directory structure if it doesn't exist
        fs::create_dir_all(&base_path)?;

        // Create wallet directory structure (basic folders)
        fs::create_dir_all(base_path.join(Self::WALLETS_DIR))?;

        // Always use the provided device name as the device ID
        let device_id_path = base_path.join(Self::DEVICE_ID_FILE);
        let device_id = device_name.to_string();

        // Write/overwrite the device_id file with the provided device name
        let mut file = File::create(device_id_path)?;
        file.write_all(device_name.as_bytes())?;

        // Create the device-specific wallet directory
        fs::create_dir_all(base_path.join(Self::WALLETS_DIR).join(&device_id))?;

        // Load or create index file
        let index_path = base_path.join(Self::INDEX_FILE);
        let mut index = if index_path.exists() {
            let index_file = File::open(&index_path)?;
            serde_json::from_reader(index_file)
                .map_err(|e| KeystoreError::General(e.to_string()))?
        } else {
            KeystoreIndex::new()
        };

        // Always ensure this device is registered in the index
        let device_info = DeviceInfo::new(
            device_id.clone(),
            device_name.to_string(),
            format!(
                "device-{}",
                device_id.split('-').next().unwrap_or("unknown")
            ),
        );

        // Check if device already exists and update/add it
        if index.get_device(&device_id).is_none() {
            index.add_device(device_info);

            // Save the updated index
            let index_file = File::create(index_path)?;
            serde_json::to_writer_pretty(index_file, &index)
                .map_err(|e| KeystoreError::General(e.to_string()))?;
        }

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
        serde_json::to_writer_pretty(index_file, &self.index)
            .map_err(|e| KeystoreError::General(e.to_string()))?;
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
    pub fn list_wallets(&self) -> Vec<&super::models::WalletInfo> {
        self.index.wallets.iter().collect()
    }

    /// Gets a wallet by ID
    pub fn get_wallet(&self, wallet_id: &str) -> Option<&super::models::WalletInfo> {
        self.index.get_wallet(wallet_id)
    }

    /// Gets this device's info
    pub fn get_this_device(&self) -> Option<&DeviceInfo> {
        self.index.get_device(&self.device_id)
    }

    /// Creates a new wallet in the keystore
    pub fn create_wallet(
        &mut self,
        name: &str,
        curve_type: &str,
        blockchain: &str,
        public_address: &str,
        threshold: u16,
        total_participants: u16,
        group_public_key: &str,
        key_share_data: &[u8],
        password: &str,
        tags: Vec<String>,
        description: Option<String>,
    ) -> Result<String> {
        // Generate a unique wallet ID
        let wallet_id = Uuid::new_v4().to_string();

        // Create wallet info
        let mut wallet = WalletInfo::new(
            wallet_id.clone(),
            name.to_string(),
            curve_type.to_string(),
            blockchain.to_string(),
            public_address.to_string(),
            threshold,
            total_participants,
            group_public_key.to_string(),
            tags,
            description,
        );

        // Get this device's info
        let device = match self.get_this_device() {
            Some(device) => device.clone(),
            None => {
                return Err(KeystoreError::General(
                    "Device info not found in keystore".to_string(),
                ));
            }
        };

        // Add this device to the wallet
        wallet.add_device(device);

        // Add the wallet to the index
        self.index.add_wallet(wallet.clone());

        // Save the wallet key share data
        self.save_wallet_file(&wallet_id, key_share_data, password)?;

        // Save the index
        self.save_index()?;

        Ok(wallet_id)
    }

    /// Saves encrypted wallet data to a file
    fn save_wallet_file(&self, wallet_id: &str, data: &[u8], password: &str) -> Result<()> {
        // Create device-specific wallet directory
        let wallet_dir = self.base_path.join(Self::WALLETS_DIR).join(&self.device_id);

        // Create the directory structure if it doesn't exist
        fs::create_dir_all(&wallet_dir)?;

        // Define wallet file path
        let wallet_path = wallet_dir.join(format!("{}.dat", wallet_id));

        // Encrypt the wallet data
        let encrypted_data = encrypt_data(data, password)?;

        // Write to file
        let mut file = File::create(wallet_path)?;
        file.write_all(&encrypted_data)?;

        Ok(())
    }

    /// Loads encrypted wallet data from a file
    pub fn load_wallet_file(&self, wallet_id: &str, password: &str) -> Result<Vec<u8>> {
        // Device-specific wallet path
        let wallet_path = self
            .base_path
            .join(Self::WALLETS_DIR)
            .join(&self.device_id)
            .join(format!("{}.dat", wallet_id));

        // Read encrypted data from file
        let mut file = File::open(wallet_path)
            .map_err(|e| KeystoreError::General(format!("Failed to open wallet file: {}", e)))?;

        let mut encrypted_data = Vec::new();
        file.read_to_end(&mut encrypted_data)?;

        // Decrypt the data
        let decrypted_data = decrypt_data(&encrypted_data, password)?;

        Ok(decrypted_data)
    }

    /// Sets the current active wallet
    pub fn set_current_wallet(&mut self, wallet_id: &str) -> Result<()> {
        if self.index.get_wallet(wallet_id).is_none() {
            return Err(KeystoreError::WalletNotFound(wallet_id.to_string()));
        }

        // Update the index to reflect the change
        self.save_index()?;

        Ok(())
    }

    /// Deletes a wallet from the keystore
    pub fn delete_wallet(&mut self, wallet_id: &str) -> Result<()> {
        // Check if wallet exists
        if self.index.get_wallet(wallet_id).is_none() {
            return Err(KeystoreError::WalletNotFound(wallet_id.to_string()));
        }

        // Remove from index
        self.index.remove_wallet(wallet_id);

        // Delete wallet file
        let wallet_path = self
            .base_path
            .join(Self::WALLETS_DIR)
            .join(&self.device_id)
            .join(format!("{}.dat", wallet_id));

        if wallet_path.exists() {
            fs::remove_file(wallet_path)?;
        }

        // Save the index
        self.save_index()?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "storage_test.rs"]
mod tests;
