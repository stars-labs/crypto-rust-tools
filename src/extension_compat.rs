// Extension compatibility module for Chrome extension keystore format
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use crate::error::KeystoreError;
use crate::keystore::{WalletData, KeystoreManager};
use frost_secp256k1 as frost;
use base64::{Engine as _, engine::general_purpose};

/// Key share data format compatible with Chrome extension
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionKeyShareData {
    // Core FROST key material
    pub key_package: String,           // Serialized FROST KeyPackage (base64)
    pub public_key_package: String,    // Serialized PublicKeyPackage (base64)
    pub group_public_key: String,      // The group's public key (hex)
    
    // Session information
    pub session_id: String,
    pub device_id: String,
    pub participant_index: u16,
    
    // Threshold configuration
    pub threshold: u16,
    pub total_participants: u16,
    pub participants: Vec<String>,
    
    // Blockchain specific
    pub curve: String,  // "secp256k1" or "ed25519"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ethereum_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solana_address: Option<String>,
    
    // Metadata
    pub created_at: i64,  // Unix timestamp in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_date: Option<i64>,
}

/// Wallet metadata compatible with Chrome extension
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionWalletMetadata {
    pub id: String,
    pub name: String,
    pub blockchain: String,  // "ethereum" or "solana"
    pub address: String,
    pub session_id: String,
    
    // Visual
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    
    // Status
    pub is_active: bool,
    pub has_backup: bool,
}

/// Keystore index compatible with Chrome extension
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionKeystoreIndex {
    pub version: String,
    pub wallets: Vec<ExtensionWalletMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_wallet_id: Option<String>,
    pub device_id: String,
    
    // Security
    pub is_encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_method: Option<String>,
    pub last_modified: i64,
}

/// Encrypted key share format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionEncryptedKeyShare {
    pub wallet_id: String,
    pub algorithm: String,  // "AES-GCM"
    pub salt: String,       // base64
    pub iv: String,         // base64
    pub ciphertext: String, // base64
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_tag: Option<String>, // base64
}

/// Keystore backup format
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionKeystoreBackup {
    pub version: String,
    pub device_id: String,
    pub exported_at: i64,
    pub wallets: Vec<ExtensionBackupWallet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionBackupWallet {
    pub metadata: ExtensionWalletMetadata,
    pub encrypted_share: ExtensionEncryptedKeyShare,
}

/// Extension compatibility manager
pub struct ExtensionCompatManager;

impl ExtensionCompatManager {
    /// Convert CLI wallet data to extension format
    pub fn convert_to_extension_format(
        wallet_data: &WalletData,
        wallet_info: &crate::keystore::WalletInfo,
        device_info: &crate::device::DeviceInfo,
    ) -> Result<ExtensionKeyShareData, KeystoreError> {
        // Extract participant information
        let participants: Vec<String> = wallet_info.devices
            .iter()
            .map(|d| d.device_id.clone())
            .collect();
        
        // Find participant index
        let participant_index = wallet_info.devices
            .iter()
            .position(|d| d.device_id == device_info.device_id)
            .ok_or_else(|| KeystoreError::InvalidData("Device not found in participants".into()))?;
        
        // Serialize key package and public key package
        let key_package_bytes = bincode::serialize(&wallet_data.key_package)
            .map_err(|e| KeystoreError::SerializationError(e.to_string()))?;
        let key_package_base64 = general_purpose::STANDARD.encode(&key_package_bytes);
        
        let public_key_bytes = bincode::serialize(&wallet_data.group_public_key)
            .map_err(|e| KeystoreError::SerializationError(e.to_string()))?;
        let public_key_base64 = general_purpose::STANDARD.encode(&public_key_bytes);
        
        // Convert group public key to hex
        let group_public_key_hex = hex::encode(&wallet_data.group_public_key);
        
        // Determine blockchain and addresses
        let (blockchain, ethereum_address, solana_address) = match wallet_info.blockchain.as_str() {
            "ethereum" => ("secp256k1", Some(wallet_info.address.clone()), None),
            "solana" => ("ed25519", None, Some(wallet_info.address.clone())),
            _ => return Err(KeystoreError::InvalidData("Unknown blockchain".into())),
        };
        
        Ok(ExtensionKeyShareData {
            key_package: key_package_base64,
            public_key_package: public_key_base64,
            group_public_key: group_public_key_hex,
            session_id: wallet_data.session_id.clone(),
            device_id: wallet_data.device_id.clone(),
            participant_index: (participant_index + 1) as u16, // 1-based in extension
            threshold: wallet_info.threshold as u16,
            total_participants: wallet_info.total as u16,
            participants,
            curve: blockchain.to_string(),
            ethereum_address,
            solana_address,
            created_at: wallet_info.created_at.timestamp_millis(),
            last_used: wallet_info.last_used.map(|t| t.timestamp_millis()),
            backup_date: None,
        })
    }
    
    /// Convert extension format to CLI wallet data
    pub fn convert_from_extension_format(
        extension_data: &ExtensionKeyShareData,
    ) -> Result<(WalletData, crate::keystore::WalletInfo), KeystoreError> {
        // Decode base64 key package
        let key_package_bytes = general_purpose::STANDARD
            .decode(&extension_data.key_package)
            .map_err(|e| KeystoreError::InvalidData(format!("Base64 decode error: {}", e)))?;
        
        let key_package: frost::keys::KeyPackage = bincode::deserialize(&key_package_bytes)
            .map_err(|e| KeystoreError::DeserializationError(e.to_string()))?;
        
        // Decode base64 public key package
        let public_key_bytes = general_purpose::STANDARD
            .decode(&extension_data.public_key_package)
            .map_err(|e| KeystoreError::InvalidData(format!("Base64 decode error: {}", e)))?;
        
        let group_public_key: frost::keys::PublicKeyPackage = bincode::deserialize(&public_key_bytes)
            .map_err(|e| KeystoreError::DeserializationError(e.to_string()))?;
        
        // Create wallet data
        let wallet_data = WalletData {
            key_package,
            group_public_key,
            session_id: extension_data.session_id.clone(),
            device_id: extension_data.device_id.clone(),
        };
        
        // Determine blockchain and address
        let (blockchain, address) = match extension_data.curve.as_str() {
            "secp256k1" => ("ethereum", extension_data.ethereum_address.as_ref()
                .ok_or_else(|| KeystoreError::InvalidData("Missing Ethereum address".into()))?),
            "ed25519" => ("solana", extension_data.solana_address.as_ref()
                .ok_or_else(|| KeystoreError::InvalidData("Missing Solana address".into()))?),
            _ => return Err(KeystoreError::InvalidData("Unknown curve".into())),
        };
        
        // Create device info for participants
        let devices: Vec<crate::keystore::DeviceInfo> = extension_data.participants
            .iter()
            .enumerate()
            .map(|(idx, device_id)| crate::keystore::DeviceInfo {
                device_id: device_id.clone(),
                name: format!("Device {}", idx + 1),
                identifier: frost::Identifier::try_from((idx + 1) as u16).unwrap(),
            })
            .collect();
        
        // Create wallet info
        let wallet_info = crate::keystore::WalletInfo {
            wallet_id: extension_data.session_id.clone(),
            name: format!("Wallet {}", extension_data.session_id),
            blockchain: blockchain.to_string(),
            address: address.clone(),
            threshold: extension_data.threshold as usize,
            total: extension_data.total_participants as usize,
            devices,
            created_at: DateTime::<Utc>::from_timestamp_millis(extension_data.created_at)
                .unwrap_or_else(Utc::now),
            last_used: extension_data.last_used
                .and_then(|ts| DateTime::<Utc>::from_timestamp_millis(ts)),
        };
        
        Ok((wallet_data, wallet_info))
    }
    
    /// Export wallet in extension backup format
    pub fn export_extension_backup(
        keystore: &KeystoreManager,
        wallet_id: &str,
        password: &str,
    ) -> Result<ExtensionKeystoreBackup, KeystoreError> {
        // Load wallet
        let wallet_data = keystore.load_wallet(wallet_id, password)?;
        let wallet_info = keystore.get_wallet_info(wallet_id)?;
        let device_info = keystore.get_device_info()?;
        
        // Convert to extension format
        let key_share_data = Self::convert_to_extension_format(
            &wallet_data,
            &wallet_info,
            &device_info,
        )?;
        
        // Create metadata
        let metadata = ExtensionWalletMetadata {
            id: wallet_id.to_string(),
            name: wallet_info.name.clone(),
            blockchain: wallet_info.blockchain.clone(),
            address: wallet_info.address.clone(),
            session_id: wallet_data.session_id.clone(),
            color: None,
            icon: None,
            is_active: true,
            has_backup: true,
        };
        
        // Encrypt key share data with PBKDF2 (for Chrome extension compatibility)
        let encrypted_share = Self::encrypt_for_extension(&key_share_data, password)?;
        
        Ok(ExtensionKeystoreBackup {
            version: "1.0.0".to_string(),
            device_id: device_info.device_id.clone(),
            exported_at: Utc::now().timestamp_millis(),
            wallets: vec![ExtensionBackupWallet {
                metadata,
                encrypted_share,
            }],
        })
    }
    
    /// Import wallet from extension backup format
    pub fn import_extension_backup(
        keystore: &mut KeystoreManager,
        backup: &ExtensionKeystoreBackup,
        password: &str,
        new_password: &str,
    ) -> Result<(), KeystoreError> {
        for wallet in &backup.wallets {
            // Decrypt key share data
            let key_share_data = Self::decrypt_from_extension(
                &wallet.encrypted_share,
                password,
            )?;
            
            // Convert to CLI format
            let (wallet_data, mut wallet_info) = Self::convert_from_extension_format(&key_share_data)?;
            
            // Update wallet name from metadata
            wallet_info.name = wallet.metadata.name.clone();
            
            // Save to keystore with new password
            keystore.create_wallet(&wallet_info, &wallet_data, new_password)?;
        }
        
        Ok(())
    }
    
    /// Encrypt data using PBKDF2 (Chrome extension compatible)
    fn encrypt_for_extension(
        data: &ExtensionKeyShareData,
        password: &str,
    ) -> Result<ExtensionEncryptedKeyShare, KeystoreError> {
        use aes_gcm::{Aes256Gcm, Key, Nonce};
        use aes_gcm::aead::{Aead, NewAead};
        use pbkdf2::pbkdf2_hmac;
        use sha2::Sha256;
        use rand::Rng;
        
        // Generate salt and IV
        let mut rng = rand::thread_rng();
        let salt: [u8; 16] = rng.gen();
        let iv: [u8; 12] = rng.gen();
        
        // Derive key using PBKDF2
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            &salt,
            100_000,
            &mut key,
        );
        
        // Encrypt
        let cipher = Aes256Gcm::new(Key::from_slice(&key));
        let nonce = Nonce::from_slice(&iv);
        
        let plaintext = serde_json::to_vec(data)
            .map_err(|e| KeystoreError::SerializationError(e.to_string()))?;
        
        let ciphertext = cipher.encrypt(nonce, plaintext.as_ref())
            .map_err(|e| KeystoreError::EncryptionError(e.to_string()))?;
        
        Ok(ExtensionEncryptedKeyShare {
            wallet_id: data.session_id.clone(),
            algorithm: "AES-GCM".to_string(),
            salt: general_purpose::STANDARD.encode(&salt),
            iv: general_purpose::STANDARD.encode(&iv),
            ciphertext: general_purpose::STANDARD.encode(&ciphertext),
            auth_tag: None, // Included in ciphertext for AES-GCM
        })
    }
    
    /// Decrypt data using PBKDF2 (Chrome extension compatible)
    fn decrypt_from_extension(
        encrypted: &ExtensionEncryptedKeyShare,
        password: &str,
    ) -> Result<ExtensionKeyShareData, KeystoreError> {
        use aes_gcm::{Aes256Gcm, Key, Nonce};
        use aes_gcm::aead::{Aead, NewAead};
        use pbkdf2::pbkdf2_hmac;
        use sha2::Sha256;
        
        // Decode base64
        let salt = general_purpose::STANDARD.decode(&encrypted.salt)
            .map_err(|e| KeystoreError::InvalidData(format!("Salt decode error: {}", e)))?;
        let iv = general_purpose::STANDARD.decode(&encrypted.iv)
            .map_err(|e| KeystoreError::InvalidData(format!("IV decode error: {}", e)))?;
        let ciphertext = general_purpose::STANDARD.decode(&encrypted.ciphertext)
            .map_err(|e| KeystoreError::InvalidData(format!("Ciphertext decode error: {}", e)))?;
        
        // Derive key using PBKDF2
        let mut key = [0u8; 32];
        pbkdf2_hmac::<Sha256>(
            password.as_bytes(),
            &salt,
            100_000,
            &mut key,
        );
        
        // Decrypt
        let cipher = Aes256Gcm::new(Key::from_slice(&key));
        let nonce = Nonce::from_slice(&iv);
        
        let plaintext = cipher.decrypt(nonce, ciphertext.as_ref())
            .map_err(|e| KeystoreError::DecryptionError(e.to_string()))?;
        
        let data: ExtensionKeyShareData = serde_json::from_slice(&plaintext)
            .map_err(|e| KeystoreError::DeserializationError(e.to_string()))?;
        
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_extension_format_conversion() {
        // Test conversion between CLI and extension formats
        // Implementation would go here
    }
}