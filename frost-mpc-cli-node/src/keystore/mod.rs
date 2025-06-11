//! Keystore module for secure storage of FROST key shares.
//!
//! This module provides functionality to securely store and manage FROST key shares
//! across multiple devices and wallets. It supports encryption, backup, and recovery
//! mechanisms in line with the threshold security model.

mod encryption;
mod models;
mod storage;

pub use storage::Keystore;

/// Error types that can occur during keystore operations
#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    
    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Decryption error: {0}")]
    DecryptionError(String),

    #[error("Wallet not found: {0}")]
    WalletNotFound(String),

    #[error("Device not found: {0}")]
    DeviceNotFound(String),

    #[error("Invalid password")]
    InvalidPassword,

    #[error("General keystore error: {0}")]
    General(String),
}

/// Result type for keystore operations
pub type Result<T> = std::result::Result<T, KeystoreError>;

/// Current keystore file format version
pub const KEYSTORE_VERSION: u8 = 1;

