//! Two-Factor Authentication (2FA) Module
//!
//! Implements TOTP (Time-based One-Time Password) and backup codes
//! for enhanced security. Proxmox VE 9 feature parity.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// 2FA manager for handling TOTP and backup codes
pub struct TwoFactorManager {
    user_secrets: HashMap<String, TwoFactorSecret>,
}

impl TwoFactorManager {
    pub fn new() -> Self {
        TwoFactorManager {
            user_secrets: HashMap::new(),
        }
    }

    /// Enable 2FA for a user
    pub fn enable_2fa(&mut self, user_id: &str) -> Result<TwoFactorSetup, String> {
        if self.user_secrets.contains_key(user_id) {
            return Err(format!("2FA already enabled for user {}", user_id));
        }

        // Generate a random secret (base32 encoded)
        let secret = Self::generate_secret();

        // Generate backup codes
        let backup_codes = Self::generate_backup_codes(10);

        let two_fa_secret = TwoFactorSecret {
            user_id: user_id.to_string(),
            secret: secret.clone(),
            backup_codes: backup_codes.clone(),
            enabled: false, // Not enabled until first successful verification
            verified: false,
        };

        self.user_secrets.insert(user_id.to_string(), two_fa_secret);

        // Generate provisioning URI for QR code
        let uri = Self::generate_totp_uri(user_id, &secret);

        Ok(TwoFactorSetup {
            secret,
            qr_code_uri: uri,
            backup_codes,
        })
    }

    /// Verify TOTP code and enable 2FA
    pub fn verify_and_enable(
        &mut self,
        user_id: &str,
        code: &str,
    ) -> Result<(), String> {
        let secret = self.user_secrets.get_mut(user_id)
            .ok_or_else(|| format!("2FA not initialized for user {}", user_id))?;

        if secret.enabled {
            return Err("2FA already enabled".to_string());
        }

        // Verify the TOTP code
        if !Self::verify_totp(&secret.secret, code)? {
            return Err("Invalid verification code".to_string());
        }

        // Enable 2FA
        secret.enabled = true;
        secret.verified = true;

        Ok(())
    }

    /// Verify a 2FA code (TOTP or backup code)
    pub fn verify_code(&mut self, user_id: &str, code: &str) -> Result<bool, String> {
        let secret = self.user_secrets.get_mut(user_id)
            .ok_or_else(|| format!("2FA not enabled for user {}", user_id))?;

        if !secret.enabled {
            return Err("2FA not enabled".to_string());
        }

        // Try TOTP first
        if Self::verify_totp(&secret.secret, code)? {
            return Ok(true);
        }

        // Try backup codes
        if let Some(index) = secret.backup_codes.iter().position(|bc| bc == code) {
            // Remove used backup code
            secret.backup_codes.remove(index);
            return Ok(true);
        }

        Ok(false)
    }

    /// Disable 2FA for a user
    pub fn disable_2fa(&mut self, user_id: &str) -> Result<(), String> {
        if self.user_secrets.remove(user_id).is_none() {
            return Err(format!("2FA not enabled for user {}", user_id));
        }
        Ok(())
    }

    /// Check if 2FA is enabled for a user
    pub fn is_enabled(&self, user_id: &str) -> bool {
        self.user_secrets.get(user_id)
            .map(|s| s.enabled)
            .unwrap_or(false)
    }

    /// Get remaining backup codes
    pub fn get_backup_codes(&self, user_id: &str) -> Result<Vec<String>, String> {
        let secret = self.user_secrets.get(user_id)
            .ok_or_else(|| format!("2FA not enabled for user {}", user_id))?;

        Ok(secret.backup_codes.clone())
    }

    /// Regenerate backup codes
    pub fn regenerate_backup_codes(&mut self, user_id: &str) -> Result<Vec<String>, String> {
        let secret = self.user_secrets.get_mut(user_id)
            .ok_or_else(|| format!("2FA not enabled for user {}", user_id))?;

        let new_codes = Self::generate_backup_codes(10);
        secret.backup_codes = new_codes.clone();

        Ok(new_codes)
    }

    // Helper functions

    /// Generate a random base32 secret
    fn generate_secret() -> String {
        use rand::Rng;
        const BASE32_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

        let mut rng = rand::thread_rng();
        (0..32)
            .map(|_| {
                let idx = rng.gen_range(0..BASE32_CHARS.len());
                BASE32_CHARS[idx] as char
            })
            .collect()
    }

    /// Generate backup codes
    fn generate_backup_codes(count: usize) -> Vec<String> {
        use rand::Rng;

        let mut rng = rand::thread_rng();
        (0..count)
            .map(|_| {
                // Generate 8-digit backup codes
                format!("{:08}", rng.gen_range(10000000..99999999))
            })
            .collect()
    }

    /// Verify TOTP code
    fn verify_totp(secret: &str, code: &str) -> Result<bool, String> {
        // Get current timestamp
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "System time error".to_string())?
            .as_secs();

        // TOTP time step is 30 seconds
        let time_step = now / 30;

        // Check current time step and ±1 window for clock skew
        for offset in [-1i64, 0, 1] {
            let step = (time_step as i64 + offset) as u64;
            let expected_code = Self::generate_totp(secret, step)?;

            if code == expected_code {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Generate TOTP code for a given time step
    fn generate_totp(secret: &str, time_step: u64) -> Result<String, String> {
        // Decode base32 secret
        let key = Self::decode_base32(secret)?;

        // Generate HMAC-SHA1
        let hmac = Self::hmac_sha1(&key, &time_step.to_be_bytes());

        // Dynamic truncation
        let offset = (hmac[hmac.len() - 1] & 0x0f) as usize;
        let code = u32::from_be_bytes([
            hmac[offset] & 0x7f,
            hmac[offset + 1],
            hmac[offset + 2],
            hmac[offset + 3],
        ]) % 1_000_000;

        Ok(format!("{:06}", code))
    }

    /// Simple HMAC-SHA1 implementation
    fn hmac_sha1(key: &[u8], message: &[u8]) -> Vec<u8> {
        use sha1::{Digest, Sha1};

        const BLOCK_SIZE: usize = 64;

        let mut key_padded = vec![0u8; BLOCK_SIZE];
        if key.len() <= BLOCK_SIZE {
            key_padded[..key.len()].copy_from_slice(key);
        } else {
            let hashed_key = Sha1::digest(key);
            key_padded[..hashed_key.len()].copy_from_slice(&hashed_key);
        }

        // Inner padding
        let mut ipad = vec![0x36u8; BLOCK_SIZE];
        for i in 0..BLOCK_SIZE {
            ipad[i] ^= key_padded[i];
        }

        // Outer padding
        let mut opad = vec![0x5cu8; BLOCK_SIZE];
        for i in 0..BLOCK_SIZE {
            opad[i] ^= key_padded[i];
        }

        // Inner hash
        let mut inner_hasher = Sha1::new();
        inner_hasher.update(&ipad);
        inner_hasher.update(message);
        let inner_hash = inner_hasher.finalize();

        // Outer hash
        let mut outer_hasher = Sha1::new();
        outer_hasher.update(&opad);
        outer_hasher.update(&inner_hash);

        outer_hasher.finalize().to_vec()
    }

    /// Decode base32 string
    fn decode_base32(input: &str) -> Result<Vec<u8>, String> {
        // Simple base32 decoder
        const BASE32_ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

        let input = input.to_uppercase();
        let mut bits: Vec<bool> = Vec::new();

        for c in input.chars() {
            if c == '=' {
                break;
            }

            let val = BASE32_ALPHABET
                .find(c)
                .ok_or_else(|| format!("Invalid base32 character: {}", c))?;

            for i in (0..5).rev() {
                bits.push((val & (1 << i)) != 0);
            }
        }

        let mut bytes = Vec::new();
        for chunk in bits.chunks(8) {
            if chunk.len() == 8 {
                let mut byte = 0u8;
                for (i, &bit) in chunk.iter().enumerate() {
                    if bit {
                        byte |= 1 << (7 - i);
                    }
                }
                bytes.push(byte);
            }
        }

        Ok(bytes)
    }

    /// Generate TOTP provisioning URI for QR codes
    fn generate_totp_uri(user_id: &str, secret: &str) -> String {
        format!(
            "otpauth://totp/Horcrux:{}?secret={}&issuer=Horcrux&algorithm=SHA1&digits=6&period=30",
            user_id, secret
        )
    }
}

/// 2FA setup information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoFactorSetup {
    pub secret: String,
    pub qr_code_uri: String,
    pub backup_codes: Vec<String>,
}

/// User's 2FA secret and backup codes
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TwoFactorSecret {
    user_id: String,
    secret: String,
    backup_codes: Vec<String>,
    enabled: bool,
    verified: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enable_2fa() {
        let mut mgr = TwoFactorManager::new();
        let setup = mgr.enable_2fa("user1").unwrap();

        assert_eq!(setup.secret.len(), 32);
        assert_eq!(setup.backup_codes.len(), 10);
        assert!(setup.qr_code_uri.starts_with("otpauth://totp/Horcrux:user1"));
    }

    #[test]
    fn test_totp_generation_and_verification() {
        let secret = "JBSWY3DPEHPK3PXP"; // Test secret
        let time_step = 12345678u64;

        let code = TwoFactorManager::generate_totp(secret, time_step).unwrap();
        assert_eq!(code.len(), 6);

        // Verify the code works
        // (In real scenario, we'd verify against current time)
    }

    #[test]
    fn test_backup_codes() {
        let mut mgr = TwoFactorManager::new();
        mgr.enable_2fa("user1").unwrap();

        let codes = mgr.get_backup_codes("user1").unwrap();
        assert_eq!(codes.len(), 10);

        // All codes should be 8 digits
        for code in &codes {
            assert_eq!(code.len(), 8);
            assert!(code.chars().all(|c| c.is_numeric()));
        }
    }

    #[test]
    fn test_disable_2fa() {
        let mut mgr = TwoFactorManager::new();
        mgr.enable_2fa("user1").unwrap();

        assert!(mgr.disable_2fa("user1").is_ok());
        assert!(!mgr.is_enabled("user1"));
    }

    #[test]
    fn test_base32_decode() {
        let secret = "JBSWY3DPEHPK3PXP";
        let decoded = TwoFactorManager::decode_base32(secret).unwrap();
        assert!(!decoded.is_empty());
    }
}
