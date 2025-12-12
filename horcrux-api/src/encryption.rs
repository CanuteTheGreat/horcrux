//! Data encryption module
//!
//! Provides symmetric encryption for sensitive data stored in the database
//! when HashiCorp Vault is not available.

use base64::Engine;
use horcrux_common::Result;
use rand::Rng;
use std::sync::Arc;
use tokio::sync::RwLock;

/// AES-256-GCM encryption key size in bytes
const KEY_SIZE: usize = 32;
/// Nonce size for AES-256-GCM
const NONCE_SIZE: usize = 12;
/// Authentication tag size for AES-256-GCM
const TAG_SIZE: usize = 16;

/// Encryption configuration
#[derive(Debug, Clone)]
pub struct EncryptionConfig {
    /// Master encryption key (hex-encoded)
    pub master_key: Option<String>,
    /// Path to key file
    pub key_file: Option<String>,
    /// Auto-generate key if not provided
    pub auto_generate: bool,
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self {
            master_key: None,
            key_file: None,
            auto_generate: true,
        }
    }
}

/// Encryption manager for sensitive data
pub struct EncryptionManager {
    key: Arc<RwLock<Option<[u8; KEY_SIZE]>>>,
    config: Arc<RwLock<EncryptionConfig>>,
}

impl EncryptionManager {
    /// Create a new encryption manager
    pub fn new() -> Self {
        Self {
            key: Arc::new(RwLock::new(None)),
            config: Arc::new(RwLock::new(EncryptionConfig::default())),
        }
    }

    /// Initialize with configuration
    pub async fn initialize(&self, config: EncryptionConfig) -> Result<()> {
        let key = if let Some(ref hex_key) = config.master_key {
            // Decode hex key
            Self::decode_hex_key(hex_key)?
        } else if let Some(ref key_file) = config.key_file {
            // Load from file
            Self::load_key_from_file(key_file).await?
        } else if config.auto_generate {
            // Generate new key
            Self::generate_key()
        } else {
            return Err(horcrux_common::Error::InvalidConfig(
                "No encryption key configured".to_string(),
            ));
        };

        *self.key.write().await = Some(key);
        *self.config.write().await = config;

        tracing::info!("Encryption manager initialized");
        Ok(())
    }

    /// Encrypt data
    pub async fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self.get_key().await?;
        Self::encrypt_with_key(&key, plaintext)
    }

    /// Decrypt data
    pub async fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let key = self.get_key().await?;
        Self::decrypt_with_key(&key, ciphertext)
    }

    /// Encrypt string to base64
    pub async fn encrypt_string(&self, plaintext: &str) -> Result<String> {
        let encrypted = self.encrypt(plaintext.as_bytes()).await?;
        Ok(base64::engine::general_purpose::STANDARD.encode(&encrypted))
    }

    /// Decrypt base64 string
    pub async fn decrypt_string(&self, ciphertext: &str) -> Result<String> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(ciphertext)
            .map_err(|e| horcrux_common::Error::System(format!("Base64 decode failed: {}", e)))?;
        let decrypted = self.decrypt(&decoded).await?;
        String::from_utf8(decrypted)
            .map_err(|e| horcrux_common::Error::System(format!("UTF-8 decode failed: {}", e)))
    }

    /// Check if encryption is available
    pub async fn is_available(&self) -> bool {
        self.key.read().await.is_some()
    }

    /// Generate a new encryption key
    pub fn generate_key() -> [u8; KEY_SIZE] {
        let mut key = [0u8; KEY_SIZE];
        rand::thread_rng().fill(&mut key);
        key
    }

    /// Generate a new key and return as hex string
    pub fn generate_key_hex() -> String {
        let key = Self::generate_key();
        hex::encode(key)
    }

    /// Get the current key
    async fn get_key(&self) -> Result<[u8; KEY_SIZE]> {
        self.key
            .read()
            .await
            .ok_or_else(|| horcrux_common::Error::System("Encryption not initialized".to_string()))
    }

    /// Decode a hex-encoded key
    fn decode_hex_key(hex_key: &str) -> Result<[u8; KEY_SIZE]> {
        let bytes = hex::decode(hex_key)
            .map_err(|e| horcrux_common::Error::InvalidConfig(format!("Invalid hex key: {}", e)))?;

        if bytes.len() != KEY_SIZE {
            return Err(horcrux_common::Error::InvalidConfig(format!(
                "Key must be {} bytes (got {})",
                KEY_SIZE,
                bytes.len()
            )));
        }

        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(&bytes);
        Ok(key)
    }

    /// Load key from file
    async fn load_key_from_file(path: &str) -> Result<[u8; KEY_SIZE]> {
        let contents = tokio::fs::read_to_string(path).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to read key file: {}", e))
        })?;

        Self::decode_hex_key(contents.trim())
    }

    /// Encrypt with a specific key using AES-256-GCM
    fn encrypt_with_key(key: &[u8; KEY_SIZE], plaintext: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        rand::thread_rng().fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Create cipher
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create cipher: {}", e)))?;

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| horcrux_common::Error::System(format!("Encryption failed: {}", e)))?;

        // Prepend nonce to ciphertext
        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    /// Decrypt with a specific key using AES-256-GCM
    fn decrypt_with_key(key: &[u8; KEY_SIZE], ciphertext: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };

        if ciphertext.len() < NONCE_SIZE + TAG_SIZE {
            return Err(horcrux_common::Error::System(
                "Ciphertext too short".to_string(),
            ));
        }

        // Extract nonce
        let nonce = Nonce::from_slice(&ciphertext[..NONCE_SIZE]);
        let encrypted_data = &ciphertext[NONCE_SIZE..];

        // Create cipher
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| horcrux_common::Error::System(format!("Failed to create cipher: {}", e)))?;

        // Decrypt
        cipher
            .decrypt(nonce, encrypted_data)
            .map_err(|e| horcrux_common::Error::System(format!("Decryption failed: {}", e)))
    }
}

impl Default for EncryptionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation() {
        let key1 = EncryptionManager::generate_key();
        let key2 = EncryptionManager::generate_key();

        // Keys should be different
        assert_ne!(key1, key2);

        // Keys should be correct size
        assert_eq!(key1.len(), KEY_SIZE);
    }

    #[test]
    fn test_key_hex_generation() {
        let hex_key = EncryptionManager::generate_key_hex();

        // Hex key should be 64 characters (32 bytes * 2)
        assert_eq!(hex_key.len(), KEY_SIZE * 2);

        // Should be valid hex
        assert!(hex::decode(&hex_key).is_ok());
    }

    #[tokio::test]
    async fn test_encryption_roundtrip() {
        let manager = EncryptionManager::new();
        manager
            .initialize(EncryptionConfig {
                master_key: Some(EncryptionManager::generate_key_hex()),
                key_file: None,
                auto_generate: false,
            })
            .await
            .unwrap();

        let plaintext = b"Hello, World! This is a test message.";
        let ciphertext = manager.encrypt(plaintext).await.unwrap();
        let decrypted = manager.decrypt(&ciphertext).await.unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[tokio::test]
    async fn test_string_encryption_roundtrip() {
        let manager = EncryptionManager::new();
        manager
            .initialize(EncryptionConfig {
                master_key: Some(EncryptionManager::generate_key_hex()),
                key_file: None,
                auto_generate: false,
            })
            .await
            .unwrap();

        let plaintext = "Sensitive kubeconfig data here";
        let encrypted = manager.encrypt_string(plaintext).await.unwrap();
        let decrypted = manager.decrypt_string(&encrypted).await.unwrap();

        assert_eq!(plaintext, decrypted);

        // Encrypted string should be base64
        assert!(base64::engine::general_purpose::STANDARD
            .decode(&encrypted)
            .is_ok());
    }

    #[tokio::test]
    async fn test_different_ciphertexts() {
        let manager = EncryptionManager::new();
        manager
            .initialize(EncryptionConfig {
                master_key: Some(EncryptionManager::generate_key_hex()),
                key_file: None,
                auto_generate: false,
            })
            .await
            .unwrap();

        let plaintext = "Same message";

        // Same plaintext should produce different ciphertexts (due to random nonce)
        let ct1 = manager.encrypt_string(plaintext).await.unwrap();
        let ct2 = manager.encrypt_string(plaintext).await.unwrap();

        assert_ne!(ct1, ct2);

        // Both should decrypt to same plaintext
        assert_eq!(manager.decrypt_string(&ct1).await.unwrap(), plaintext);
        assert_eq!(manager.decrypt_string(&ct2).await.unwrap(), plaintext);
    }

    #[tokio::test]
    async fn test_auto_generate_key() {
        let manager = EncryptionManager::new();
        manager
            .initialize(EncryptionConfig::default())
            .await
            .unwrap();

        assert!(manager.is_available().await);

        let plaintext = "Test data";
        let encrypted = manager.encrypt_string(plaintext).await.unwrap();
        let decrypted = manager.decrypt_string(&encrypted).await.unwrap();

        assert_eq!(plaintext, decrypted);
    }

    #[tokio::test]
    async fn test_invalid_ciphertext() {
        let manager = EncryptionManager::new();
        manager
            .initialize(EncryptionConfig::default())
            .await
            .unwrap();

        // Too short
        let result = manager.decrypt(&[0u8; 10]).await;
        assert!(result.is_err());

        // Invalid base64 for string decryption
        let result = manager.decrypt_string("not-valid-base64!!!").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_wrong_key_fails() {
        let manager1 = EncryptionManager::new();
        manager1
            .initialize(EncryptionConfig {
                master_key: Some(EncryptionManager::generate_key_hex()),
                key_file: None,
                auto_generate: false,
            })
            .await
            .unwrap();

        let manager2 = EncryptionManager::new();
        manager2
            .initialize(EncryptionConfig {
                master_key: Some(EncryptionManager::generate_key_hex()),
                key_file: None,
                auto_generate: false,
            })
            .await
            .unwrap();

        let plaintext = "Secret data";
        let encrypted = manager1.encrypt_string(plaintext).await.unwrap();

        // Decrypting with different key should fail
        let result = manager2.decrypt_string(&encrypted).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_hex_key() {
        let result = EncryptionManager::decode_hex_key("not-hex");
        assert!(result.is_err());

        // Wrong length
        let result = EncryptionManager::decode_hex_key("deadbeef");
        assert!(result.is_err());
    }
}
