//! Encryption utilities for the keystore module.
//!
//! This module provides functions for encrypting and decrypting keystore data
//! using AES-256-GCM with Argon2id key derivation.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2, Params,
};
use rand_core::RngCore;

use crate::keystore::KeystoreError;

// Constants for encryption parameters
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32; // 256 bits

/// Encrypts data with a password using AES-256-GCM with Argon2id key derivation.
///
/// The output format is: `salt (16 bytes) + nonce (12 bytes) + ciphertext`
///
/// # Arguments
///
/// * `data` - The data to encrypt
/// * `password` - The password to use for encryption
///
/// # Returns
///
/// The encrypted data as a vector of bytes
pub fn encrypt_data(data: &[u8], password: &str) -> Result<Vec<u8>, KeystoreError> {
    // Generate a random salt
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let salt_string = SaltString::encode_b64(&salt).map_err(|e| KeystoreError::EncryptionError(e.to_string()))?;

    // Derive key using Argon2id
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        Params::new(4096, 3, 1, Some(KEY_LEN)).unwrap(),
    );

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| KeystoreError::EncryptionError(e.to_string()))?;
    
    let hash_bytes = password_hash.hash.unwrap().as_bytes();
    let key = Key::<Aes256Gcm>::from_slice(hash_bytes);

    // Generate a random nonce
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt the data
    let cipher = Aes256Gcm::new(key);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| KeystoreError::EncryptionError(e.to_string()))?;

    // Combine salt, nonce, and ciphertext
    let mut result = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypts data that was encrypted with `encrypt_data`.
///
/// The input format is expected to be: `salt (16 bytes) + nonce (12 bytes) + ciphertext`
///
/// # Arguments
///
/// * `encrypted_data` - The encrypted data
/// * `password` - The password to use for decryption
///
/// # Returns
///
/// The decrypted data as a vector of bytes
pub fn decrypt_data(encrypted_data: &[u8], password: &str) -> Result<Vec<u8>, KeystoreError> {
    // Check if the data is long enough to contain the salt and nonce
    if encrypted_data.len() < SALT_LEN + NONCE_LEN {
        return Err(KeystoreError::DecryptionError("Invalid encrypted data format".to_string()));
    }

    // Extract salt and nonce
    let salt = &encrypted_data[0..SALT_LEN];
    let nonce_bytes = &encrypted_data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &encrypted_data[SALT_LEN + NONCE_LEN..];

    let salt_string = SaltString::encode_b64(salt).map_err(|e| KeystoreError::DecryptionError(e.to_string()))?;

    // Derive key using Argon2id with the same parameters as encryption
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        Params::new(4096, 3, 1, Some(KEY_LEN)).unwrap(),
    );

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| KeystoreError::DecryptionError(e.to_string()))?;
    
    let hash_bytes = password_hash.hash.unwrap().as_bytes();
    let key = Key::<Aes256Gcm>::from_slice(hash_bytes);

    // Decrypt the data
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| KeystoreError::InvalidPassword)?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_decryption() {
        let data = b"This is a test message";
        let password = "test_password";

        let encrypted = encrypt_data(data, password).unwrap();
        let decrypted = decrypt_data(&encrypted, password).unwrap();

        assert_eq!(data.to_vec(), decrypted);
    }

    #[test]
    fn test_decryption_wrong_password() {
        let data = b"This is a test message";
        let password = "test_password";
        let wrong_password = "wrong_password";

        let encrypted = encrypt_data(data, password).unwrap();
        let result = decrypt_data(&encrypted, wrong_password);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), KeystoreError::InvalidPassword));
    }

    #[test]
    fn test_decryption_corrupted_data() {
        let data = b"This is a test message";
        let password = "test_password";

        let mut encrypted = encrypt_data(data, password).unwrap();
        
        // Corrupt the ciphertext
        if encrypted.len() > SALT_LEN + NONCE_LEN + 1 {
            encrypted[SALT_LEN + NONCE_LEN + 1] ^= 0x01;
        }
        
        let result = decrypt_data(&encrypted, password);
        assert!(result.is_err());
    }
}