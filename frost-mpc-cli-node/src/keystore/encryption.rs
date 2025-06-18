//! Encryption utilities for the keystore module.
//!
//! This module provides functions for encrypting and decrypting keystore data
//! using AES-256-GCM with Argon2id key derivation.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2, Params,
};
use rand::{rng, RngCore};

use crate::keystore::KeystoreError;

// Constants for encryption parameters
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32; // 256 bits

/// Encrypts data with a password using AES-256-GCM with Argon2id key derivation.
///
/// The output format is: `salt (16 bytes) + nonce (12 bytes) + ciphertext`
pub fn encrypt_data(data: &[u8], password: &str) -> crate::keystore::Result<Vec<u8>> {
    // Generate a random salt
    let mut salt = [0u8; SALT_LEN];
    rng().fill_bytes(&mut salt);
    let salt_string = SaltString::encode_b64(&salt)
        .map_err(|e| KeystoreError::General(format!("Salt encoding error: {}", e)))?;

    // Derive key using Argon2id
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        Params::new(4096, 3, 1, Some(KEY_LEN)).unwrap(),
    );

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| KeystoreError::EncryptionError(format!("Password hashing error: {}", e)))?;
    
    let binding = password_hash.hash.unwrap();
    let hash_bytes = binding.as_bytes();
    let key = Key::<Aes256Gcm>::from_slice(hash_bytes);

    // Generate a random nonce
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt the data
    let cipher = Aes256Gcm::new(key);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|e| KeystoreError::EncryptionError(format!("Encryption error: {}", e)))?;

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
pub fn decrypt_data(encrypted_data: &[u8], password: &str) -> crate::keystore::Result<Vec<u8>> {
    // Check if the data is long enough to contain the salt and nonce
    if encrypted_data.len() < SALT_LEN + NONCE_LEN {
        return Err(KeystoreError::DecryptionError("Invalid encrypted data format".to_string()));
    }

    // Extract salt and nonce
    let salt = &encrypted_data[0..SALT_LEN];
    let nonce_bytes = &encrypted_data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &encrypted_data[SALT_LEN + NONCE_LEN..];

    let salt_string = SaltString::encode_b64(salt)
        .map_err(|e| KeystoreError::DecryptionError(format!("Salt decoding error: {}", e)))?;

    // Derive key using Argon2id with the same parameters as encryption
    let argon2 = Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        Params::new(4096, 3, 1, Some(KEY_LEN)).unwrap(),
    );

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| KeystoreError::DecryptionError(format!("Password hashing error: {}", e)))?;
    
    let binding = password_hash.hash.unwrap();
    let hash_bytes = binding.as_bytes();
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
#[path = "encryption_test.rs"]
mod tests;