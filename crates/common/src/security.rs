// crates/common/src/security.rs
use crate::{Error, Result, Venue};
use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

const SERVICE_NAME: &str = "com.yourco.hft";
const APP_KEY_ACCOUNT: &str = "app_master_key";

/// API credentials with automatic zeroing on drop
#[derive(Clone, Serialize, Deserialize, ZeroizeOnDrop)]
pub struct ApiCredentials {
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: Option<String>,
    pub is_paper: bool,
}

impl ApiCredentials {
    pub fn new(api_key: String, api_secret: String, is_paper: bool) -> Self {
        Self {
            api_key,
            api_secret,
            passphrase: None,
            is_paper,
        }
    }
    
    pub fn with_passphrase(mut self, passphrase: String) -> Self {
        self.passphrase = Some(passphrase);
        self
    }
}

/// Secure credential store
pub struct CredentialStore {
    cipher: Option<Aes256Gcm>,
}

impl CredentialStore {
    /// Initialize credential store with app-level encryption
    pub fn new() -> Result<Self> {
        let cipher = Self::get_or_create_app_key()?;
        Ok(Self { cipher: Some(cipher) })
    }
    
    /// Initialize without app-level encryption (OS keychain only)
    pub fn new_simple() -> Self {
        Self { cipher: None }
    }
    
    fn get_or_create_app_key() -> Result<Aes256Gcm> {
        let entry = Entry::new(SERVICE_NAME, APP_KEY_ACCOUNT)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        match entry.get_password() {
            Ok(key_b64) => {
                let key_bytes = BASE64.decode(key_b64)
                    .map_err(|e| Error::Internal(format!("Invalid app key: {}", e)))?;
                Aes256Gcm::new_from_slice(&key_bytes)
                    .map_err(|e| Error::Internal(format!("Invalid key length: {}", e)))
            }
            Err(_) => {
                // Generate new app key
                let key = Aes256Gcm::generate_key(&mut OsRng);
                let key_b64 = BASE64.encode(&key);
                entry.set_password(&key_b64)
                    .map_err(|e| Error::Internal(format!("Failed to save app key: {:?}", e)))?;
                Ok(Aes256Gcm::new(&key))
            }
        }
    }
    
    fn account_key(venue: &Venue, label: &str, live: bool) -> String {
        format!("{:?}:{}:{}", venue, label, if live { "live" } else { "paper" })
    }
    
    /// Save credentials securely
    pub fn save(&self, venue: Venue, label: &str, creds: &ApiCredentials) -> Result<()> {
        let account = Self::account_key(&venue, label, !creds.is_paper);
        let entry = Entry::new(SERVICE_NAME, &account)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        let json = serde_json::to_string(&creds)
            .map_err(|e| Error::Serialization(e))?;
        
        let data = if let Some(cipher) = &self.cipher {
            // Encrypt with app key
            let nonce = Nonce::from_slice(b"unique nonce"); // In production, use random nonce + store
            let ciphertext = cipher.encrypt(nonce, json.as_bytes())
                .map_err(|e| Error::Internal(format!("Encryption failed: {}", e)))?;
            BASE64.encode(&ciphertext)
        } else {
            json
        };
        
        entry.set_password(&data)
            .map_err(|e| Error::Internal(format!("Failed to save: {:?}", e)))?;
        
        Ok(())
    }
    
    /// Load credentials
    pub fn load(&self, venue: Venue, label: &str, live: bool) -> Result<ApiCredentials> {
        let account = Self::account_key(&venue, label, live);
        let entry = Entry::new(SERVICE_NAME, &account)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        let data = entry.get_password()
            .map_err(|e| Error::NotFound(format!("Credentials not found: {:?}", e)))?;
        
        let json = if let Some(cipher) = &self.cipher {
            let ciphertext = BASE64.decode(&data)
                .map_err(|e| Error::Internal(format!("Invalid encrypted data: {}", e)))?;
            let nonce = Nonce::from_slice(b"unique nonce");
            let plaintext = cipher.decrypt(nonce, ciphertext.as_ref())
                .map_err(|e| Error::Internal(format!("Decryption failed: {}", e)))?;
            String::from_utf8(plaintext)
                .map_err(|e| Error::Internal(format!("Invalid UTF-8: {}", e)))?
        } else {
            data
        };
        
        serde_json::from_str(&json)
            .map_err(|e| Error::Serialization(e))
    }
    
    /// Delete credentials
    pub fn delete(&self, venue: Venue, label: &str, live: bool) -> Result<()> {
        let account = Self::account_key(&venue, label, live);
        let entry = Entry::new(SERVICE_NAME, &account)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        entry.delete_password()
            .map_err(|e| Error::Internal(format!("Failed to delete: {:?}", e)))?;
        
        Ok(())
    }
    
    /// List all stored accounts
    pub fn list_accounts(&self) -> Vec<String> {
        // Note: keyring crate doesn't provide list functionality
        // In production, maintain a separate index
        vec![]
    }
}

/// Data source API keys
#[derive(Clone, Serialize, Deserialize, ZeroizeOnDrop)]
pub struct DataSourceKeys {
    pub gecko_terminal: Option<String>,
    pub birdeye: Option<String>,
    pub the_graph: Option<String>,
    pub crypto_panic: Option<String>,
    pub flipside: Option<String>,
}

impl DataSourceKeys {
    const KEY_ACCOUNT: &'static str = "data_source_keys";
    
    pub fn save(&self, store: &CredentialStore) -> Result<()> {
        let entry = Entry::new(SERVICE_NAME, Self::KEY_ACCOUNT)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        let json = serde_json::to_string(&self)?;
        entry.set_password(&json)
            .map_err(|e| Error::Internal(format!("Failed to save: {:?}", e)))?;
        
        Ok(())
    }
    
    pub fn load(store: &CredentialStore) -> Result<Self> {
        let entry = Entry::new(SERVICE_NAME, Self::KEY_ACCOUNT)
            .map_err(|e| Error::Internal(format!("Keychain init: {:?}", e)))?;
        
        let json = entry.get_password()
            .map_err(|e| Error::NotFound(format!("Keys not found: {:?}", e)))?;
        
        serde_json::from_str(&json)
            .map_err(|e| Error::Serialization(e))
    }
}

/// HMAC signature for API requests
pub fn sign_request(secret: &str, message: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    
    type HmacSha256 = Hmac<Sha256>;
    
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(message.as_bytes());
    
    hex::encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_credential_roundtrip() {
        let store = CredentialStore::new_simple();
        let creds = ApiCredentials::new(
            "test_key".to_string(),
            "test_secret".to_string(),
            true,
        );
        
        store.save(Venue::Hyperliquid, "test", &creds).unwrap();
        let loaded = store.load(Venue::Hyperliquid, "test", false).unwrap();
        
        assert_eq!(loaded.api_key, "test_key");
        assert_eq!(loaded.api_secret, "test_secret");
        assert_eq!(loaded.is_paper, true);
        
        store.delete(Venue::Hyperliquid, "test", false).unwrap();
    }
    
    #[test]
    fn test_hmac_signing() {
        let signature = sign_request("secret", "message");
        assert!(!signature.is_empty());
        assert_eq!(signature.len(), 64); // SHA256 hex
    }
}