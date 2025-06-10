//! Data models for the keystore module.
//!
//! This module defines the data structures used by the keystore, including
//! wallet information, device metadata, and key packages.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::keystore::KEYSTORE_VERSION;

/// Gets the current Unix timestamp in seconds
fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Information about a wallet stored in the keystore
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WalletInfo {
    /// Unique identifier for this wallet (UUID)
    pub wallet_id: String,
    
    /// User-friendly name for the wallet
    pub name: String,
    
    /// Type of cryptographic curve used ("secp256k1" or "ed25519")
    pub curve_type: String,
    
    /// Blockchain the wallet is intended for ("ethereum" or "solana")
    pub blockchain: String,
    
    /// Public blockchain address derived from the group public key
    pub public_address: String,
    
    /// Minimum number of participants required to sign (threshold)
    pub threshold: u16,
    
    /// Total number of participants in the wallet
    pub total_participants: u16,
    
    /// Unix timestamp when the wallet was created
    pub created_at: u64,
    
    /// Serialized group public key for this wallet
    pub group_public_key: String,
    
    /// Devices that have shares for this wallet
    pub devices: Vec<DeviceInfo>,
    
    /// User-defined tags for organizing wallets
    pub tags: Vec<String>,
    
    /// Optional description for the wallet
    pub description: Option<String>,
}

impl WalletInfo {
    /// Creates a new wallet info
    pub fn new(
        wallet_id: String,
        name: String,
        curve_type: String,
        blockchain: String,
        public_address: String,
        threshold: u16,
        total_participants: u16,
        group_public_key: String,
        tags: Vec<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            wallet_id,
            name,
            curve_type,
            blockchain,
            public_address,
            threshold,
            total_participants,
            created_at: current_timestamp(),
            group_public_key,
            devices: Vec::new(),
            tags,
            description,
        }
    }

    /// Adds a device to this wallet
    pub fn add_device(&mut self, device: DeviceInfo) {
        // Replace if the device ID already exists, otherwise add
        if let Some(idx) = self.devices.iter().position(|d| d.device_id == device.device_id) {
            self.devices[idx] = device;
        } else {
            self.devices.push(device);
        }
    }
}

/// Information about a device that can participate in signing
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    /// Unique identifier for this device
    pub device_id: String,
    
    /// User-friendly name for the device
    pub name: String,
    
    /// Peer ID used in the FROST protocol
    pub peer_id: String,
    
    /// Serialized FROST identifier
    pub identifier: String,
    
    /// Last time this device was seen/used
    pub last_seen: u64,
}

impl DeviceInfo {
    /// Creates a new device info
    pub fn new(
        device_id: String,
        name: String,
        peer_id: String,
        identifier: String,
    ) -> Self {
        Self {
            device_id,
            name,
            peer_id,
            identifier,
            last_seen: current_timestamp(),
        }
    }

    /// Updates the last seen timestamp to the current time
    pub fn update_last_seen(&mut self) {
        self.last_seen = current_timestamp();
    }
}

/// Master index of all wallets and devices
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct KeystoreIndex {
    /// Keystore format version
    pub version: u8,
    
    /// List of all wallets
    pub wallets: Vec<WalletInfo>,
    
    /// List of all devices
    pub devices: Vec<DeviceInfo>,
}

impl KeystoreIndex {
    /// Creates a new, empty keystore index
    pub fn new() -> Self {
        Self {
            version: KEYSTORE_VERSION,
            wallets: Vec::new(),
            devices: Vec::new(),
        }
    }

    /// Adds or updates a wallet in the index
    pub fn add_wallet(&mut self, wallet: WalletInfo) {
        // Replace if wallet ID already exists, otherwise add
        if let Some(idx) = self.wallets.iter().position(|w| w.wallet_id == wallet.wallet_id) {
            self.wallets[idx] = wallet;
        } else {
            self.wallets.push(wallet);
        }
    }

    /// Adds or updates a device in the index
    pub fn add_device(&mut self, device: DeviceInfo) {
        // Replace if device ID already exists, otherwise add
        if let Some(idx) = self.devices.iter().position(|d| d.device_id == device.device_id) {
            self.devices[idx] = device;
        } else {
            self.devices.push(device);
        }
    }

    /// Gets a wallet by ID
    pub fn get_wallet(&self, wallet_id: &str) -> Option<&WalletInfo> {
        self.wallets.iter().find(|w| w.wallet_id == wallet_id)
    }

    /// Gets a mutable wallet by ID
    pub fn get_wallet_mut(&mut self, wallet_id: &str) -> Option<&mut WalletInfo> {
        self.wallets.iter_mut().find(|w| w.wallet_id == wallet_id)
    }

    /// Gets a device by ID
    pub fn get_device(&self, device_id: &str) -> Option<&DeviceInfo> {
        self.devices.iter().find(|d| d.device_id == device_id)
    }

    /// Gets a mutable device by ID
    pub fn get_device_mut(&mut self, device_id: &str) -> Option<&mut DeviceInfo> {
        self.devices.iter_mut().find(|d| d.device_id == device_id)
    }

    /// Removes a wallet from the index
    pub fn remove_wallet(&mut self, wallet_id: &str) -> bool {
        let len_before = self.wallets.len();
        self.wallets.retain(|w| w.wallet_id != wallet_id);
        len_before != self.wallets.len()
    }
}