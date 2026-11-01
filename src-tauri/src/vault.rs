//! Encrypted software vault for API keys, used when the OS keychain is unavailable
//! (for example Linux without a Secret Service). It exists so that keys are NEVER written
//! to disk as plain text.
//!
//! Format (`secrets.vault`, JSON): Argon2id derives a 256-bit key from the user's master
//! password; the secrets are encrypted with XChaCha20-Poly1305. The KDF parameters and salt
//! are bound to the ciphertext as associated data, so editing them in the file makes
//! decryption fail instead of silently weakening the key derivation.
//!
//! The master password is never stored. If it is lost the vault cannot be recovered; it can
//! only be reset, which deletes the saved keys.

use crate::secrets::SecretStore;
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Key, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use zeroize::{Zeroize, Zeroizing};

const FORMAT_VERSION: u32 = 1;
pub const MIN_PASSPHRASE_CHARS: usize = 8;

// OWASP recommends at least m=19 MiB, t=2, p=1 for Argon2id; this is deliberately stronger.
const DEFAULT_M_COST_KIB: u32 = 64 * 1024;
const DEFAULT_T_COST: u32 = 3;
const DEFAULT_P_COST: u32 = 1;

// Upper bounds applied to values read from the file, so a tampered file cannot make unlock
// allocate gigabytes or run for minutes.
const MAX_M_COST_KIB: u32 = 1024 * 1024;
const MAX_T_COST: u32 = 20;
const MAX_P_COST: u32 = 8;

const WRONG_PASSWORD: &str = "Wrong master password, or the vault file is damaged";

#[derive(Debug, Clone, Copy)]
pub struct Kdf {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl Default for Kdf {
    fn default() -> Self {
        Self {
            m_cost_kib: DEFAULT_M_COST_KIB,
            t_cost: DEFAULT_T_COST,
            p_cost: DEFAULT_P_COST,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultFile {
    version: u32,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

impl VaultFile {
    fn kdf(&self) -> Result<Kdf, String> {
        if self.version != FORMAT_VERSION {
            return Err(format!("Unsupported vault version {}", self.version));
        }
        if self.m_cost > MAX_M_COST_KIB || self.t_cost > MAX_T_COST || self.p_cost > MAX_P_COST {
            return Err("Vault key-derivation parameters are out of range".to_string());
        }
        Ok(Kdf { m_cost_kib: self.m_cost, t_cost: self.t_cost, p_cost: self.p_cost })
    }

    /// Everything that must not be altered without invalidating the ciphertext.
    fn aad(&self) -> Vec<u8> {
        format!(
            "nekkochat-vault|v{}|m={}|t={}|p={}|salt={}",
            self.version, self.m_cost, self.t_cost, self.p_cost, self.salt
        )
        .into_bytes()
    }
}

struct Unlocked {
    key: Zeroizing<Vec<u8>>,
    /// Header of the file on disk (salt, params, latest nonce/ciphertext)
    file: VaultFile,
    secrets: HashMap<String, Zeroizing<String>>,
}

enum State {
    /// No vault file yet
    Missing,
    Locked(VaultFile),
    Unlocked(Unlocked),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    Missing,
    Locked,
    Unlocked,
}

pub struct Vault {
    path: PathBuf,
    kdf: Kdf,
    state: Mutex<State>,
}

fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).map_err(|e| format!("No secure random source available: {}", e))?;
    Ok(buf)
}

fn derive_key(passphrase: &str, salt: &[u8], kdf: Kdf) -> Result<Zeroizing<Vec<u8>>, String> {
    let params = Params::new(kdf.m_cost_kib, kdf.t_cost, kdf.p_cost, Some(32))
        .map_err(|e| format!("Invalid key-derivation parameters: {}", e))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new(vec![0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|e| format!("Key derivation failed: {}", e))?;
    Ok(out)
}

fn cipher(key: &[u8]) -> Result<XChaCha20Poly1305, String> {
    let key = Key::<XChaCha20Poly1305>::try_from(key).map_err(|_| "Invalid key length".to_string())?;
    Ok(XChaCha20Poly1305::new(&key))
}

/// Encrypts `secrets` into a fresh file body. A new random nonce is used for every write.
fn seal(
    key: &[u8],
    base: &VaultFile,
    secrets: &HashMap<String, Zeroizing<String>>,
) -> Result<VaultFile, String> {
    let plain: HashMap<&str, &str> = secrets.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let plaintext = Zeroizing::new(serde_json::to_vec(&plain).map_err(|e| e.to_string())?);

    let mut file = base.clone();
    let nonce = XNonce::generate();
    let ciphertext = cipher(key)?
        .encrypt(&nonce, Payload { msg: &plaintext, aad: &file.aad() })
        .map_err(|_| "Encryption failed".to_string())?;
    file.nonce = B64.encode(nonce.as_slice());
    file.ciphertext = B64.encode(ciphertext);
    Ok(file)
}

fn open_file(key: &[u8], file: &VaultFile) -> Result<HashMap<String, Zeroizing<String>>, String> {
    let nonce_bytes = B64.decode(&file.nonce).map_err(|_| WRONG_PASSWORD.to_string())?;
    let nonce = XNonce::try_from(nonce_bytes.as_slice()).map_err(|_| WRONG_PASSWORD.to_string())?;
    let ciphertext = B64.decode(&file.ciphertext).map_err(|_| WRONG_PASSWORD.to_string())?;

    let plaintext = Zeroizing::new(
        cipher(key)?
            .decrypt(&nonce, Payload { msg: &ciphertext, aad: &file.aad() })
            .map_err(|_| WRONG_PASSWORD.to_string())?,
    );
    let map: HashMap<String, String> =
        serde_json::from_slice(&plaintext).map_err(|_| WRONG_PASSWORD.to_string())?;
    Ok(map.into_iter().map(|(k, v)| (k, Zeroizing::new(v))).collect())
}

fn check_new_passphrase(passphrase: &str) -> Result<(), String> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(format!("The master password must be at least {} characters", MIN_PASSPHRASE_CHARS));
    }
    Ok(())
}

impl Vault {
    pub fn open(path: PathBuf) -> Self {
        Self::open_with(path, Kdf::default())
    }

    pub fn open_with(path: PathBuf, kdf: Kdf) -> Self {
        let state = match fs::read_to_string(&path) {
            Err(_) => State::Missing,
            Ok(content) => match serde_json::from_str::<VaultFile>(&content) {
                Ok(file) => State::Locked(file),
                Err(e) => {
                    // Keep the unreadable file for the user instead of overwriting it
                    eprintln!("[vault] {:?} is unreadable ({}); moved to .bak", path, e);
                    let _ = fs::rename(&path, path.with_extension("vault.bak"));
                    State::Missing
                }
            },
        };
        Self { path, kdf, state: Mutex::new(state) }
    }

    pub fn state(&self) -> VaultState {
        match &*self.state.lock().unwrap() {
            State::Missing => VaultState::Missing,
            State::Locked(_) => VaultState::Locked,
            State::Unlocked(_) => VaultState::Unlocked,
        }
    }

    fn write(&self, file: &VaultFile) -> Result<(), String> {
        let content = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
        crate::write_atomic(&self.path, &content)
    }

    /// Creates a new, empty, unlocked vault protected by `passphrase`.
    pub fn create(&self, passphrase: &str) -> Result<(), String> {
        check_new_passphrase(passphrase)?;
        let mut state = self.state.lock().unwrap();
        if !matches!(*state, State::Missing) {
            return Err("A vault already exists".to_string());
        }

        let salt = random_bytes::<16>()?;
        let base = VaultFile {
            version: FORMAT_VERSION,
            m_cost: self.kdf.m_cost_kib,
            t_cost: self.kdf.t_cost,
            p_cost: self.kdf.p_cost,
            salt: B64.encode(salt),
            nonce: String::new(),
            ciphertext: String::new(),
        };
        let key = derive_key(passphrase, &salt, self.kdf)?;
        let secrets = HashMap::new();
        let file = seal(&key, &base, &secrets)?;
        self.write(&file)?;
        *state = State::Unlocked(Unlocked { key, file, secrets });
        Ok(())
    }

    pub fn unlock(&self, passphrase: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let file = match &*state {
            State::Unlocked(_) => return Ok(()),
            State::Missing => return Err("There is no vault to unlock".to_string()),
            State::Locked(file) => file.clone(),
        };

        let salt = B64.decode(&file.salt).map_err(|_| WRONG_PASSWORD.to_string())?;
        let key = derive_key(passphrase, &salt, file.kdf()?)?;
        let secrets = open_file(&key, &file)?;
        *state = State::Unlocked(Unlocked { key, file, secrets });
        Ok(())
    }

    /// Forgets the key and secrets from memory. They stay encrypted on disk.
    pub fn lock(&self) {
        let mut state = self.state.lock().unwrap();
        if let State::Unlocked(u) = &*state {
            let file = u.file.clone();
            *state = State::Locked(file); // dropping `Unlocked` zeroizes key and secrets
        }
    }

    pub fn change_passphrase(&self, old: &str, new: &str) -> Result<(), String> {
        check_new_passphrase(new)?;
        let mut state = self.state.lock().unwrap();
        let State::Unlocked(u) = &mut *state else {
            return Err("The vault is locked".to_string());
        };

        // Prove knowledge of the current password before changing it
        let salt = B64.decode(&u.file.salt).map_err(|_| WRONG_PASSWORD.to_string())?;
        let old_key = derive_key(old, &salt, u.file.kdf()?)?;
        open_file(&old_key, &u.file)?;

        let new_salt = random_bytes::<16>()?;
        let mut base = u.file.clone();
        base.salt = B64.encode(new_salt);
        let new_key = derive_key(new, &new_salt, u.file.kdf()?)?;
        let file = seal(&new_key, &base, &u.secrets)?;
        self.write(&file)?;
        u.key = new_key;
        u.file = file;
        Ok(())
    }

    /// Deletes the vault and every secret in it. Used when the master password is lost.
    pub fn reset(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Failed to delete the vault: {}", e)),
        }
        *state = State::Missing;
        Ok(())
    }
}

impl Drop for Unlocked {
    fn drop(&mut self) {
        self.key.zeroize();
        // `Zeroizing<String>` values wipe themselves when the map is dropped
    }
}

impl SecretStore for Vault {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match &*self.state.lock().unwrap() {
            State::Unlocked(u) => Ok(u.secrets.get(key).map(|v| v.to_string())),
            _ => Err("The vault is locked".to_string()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let State::Unlocked(u) = &mut *state else {
            return Err("The vault is locked".to_string());
        };
        let previous = u.secrets.insert(key.to_string(), Zeroizing::new(value.to_string()));
        match seal(&u.key, &u.file, &u.secrets).and_then(|f| self.write(&f).map(|_| f)) {
            Ok(file) => {
                u.file = file;
                Ok(())
            }
            Err(e) => {
                // Keep memory consistent with what is on disk
                match previous {
                    Some(p) => u.secrets.insert(key.to_string(), p),
                    None => u.secrets.remove(key),
                };
                Err(e)
            }
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let State::Unlocked(u) = &mut *state else {
            return Err("The vault is locked".to_string());
        };
        let Some(previous) = u.secrets.remove(key) else {
            return Ok(());
        };
        match seal(&u.key, &u.file, &u.secrets).and_then(|f| self.write(&f).map(|_| f)) {
            Ok(file) => {
                u.file = file;
                Ok(())
            }
            Err(e) => {
                u.secrets.insert(key.to_string(), previous);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Cheap parameters so tests are fast; production defaults are covered by one slower test
    const FAST: Kdf = Kdf { m_cost_kib: 64, t_cost: 1, p_cost: 1 };

    fn temp_path() -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nekkochat-vault-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir.join("secrets.vault")
    }

    fn new_vault() -> (Vault, PathBuf) {
        let path = temp_path();
        (Vault::open_with(path.clone(), FAST), path)
    }

    #[test]
    fn secrets_survive_lock_and_a_restart() {
        let (v, path) = new_vault();
        assert_eq!(v.state(), VaultState::Missing);
        v.create("correct horse battery").unwrap();
        v.set("profile:a:api-key", "sk-secret-123").unwrap();

        v.lock();
        assert_eq!(v.state(), VaultState::Locked);
        assert!(v.get("profile:a:api-key").is_err());

        // A fresh process only has the file
        let reopened = Vault::open_with(path, FAST);
        assert_eq!(reopened.state(), VaultState::Locked);
        reopened.unlock("correct horse battery").unwrap();
        assert_eq!(reopened.get("profile:a:api-key").unwrap().as_deref(), Some("sk-secret-123"));
    }

    #[test]
    fn the_file_on_disk_contains_no_plaintext() {
        let (v, path) = new_vault();
        v.create("correct horse battery").unwrap();
        v.set("profile:a:api-key", "sk-very-distinctive-secret").unwrap();
        let content = fs::read_to_string(path).unwrap();
        assert!(!content.contains("sk-very-distinctive-secret"));
        assert!(!content.contains("profile:a"));
    }

    #[test]
    fn wrong_password_is_rejected_and_vault_stays_locked() {
        let (v, _) = new_vault();
        v.create("correct horse battery").unwrap();
        v.lock();
        assert_eq!(v.unlock("wrong password!!").unwrap_err(), WRONG_PASSWORD);
        assert_eq!(v.state(), VaultState::Locked);
    }

    #[test]
    fn short_master_password_is_rejected() {
        let (v, _) = new_vault();
        assert!(v.create("short").is_err());
        assert_eq!(v.state(), VaultState::Missing);
    }

    #[test]
    fn tampering_with_ciphertext_or_kdf_params_is_detected() {
        let (v, path) = new_vault();
        v.create("correct horse battery").unwrap();
        v.set("k", "value").unwrap();
        v.lock();
        let original = fs::read_to_string(&path).unwrap();

        // Flip a bit of the ciphertext
        let mut file: VaultFile = serde_json::from_str(&original).unwrap();
        let mut ct = B64.decode(&file.ciphertext).unwrap();
        ct[0] ^= 1;
        file.ciphertext = B64.encode(ct);
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert!(Vault::open_with(path.clone(), FAST).unlock("correct horse battery").is_err());

        // Weaken the KDF parameters: must not unlock, even with the right password
        let mut file: VaultFile = serde_json::from_str(&original).unwrap();
        file.t_cost = 2;
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        assert!(Vault::open_with(path.clone(), FAST).unlock("correct horse battery").is_err());

        // Absurd parameters are refused before any memory is allocated
        let mut file: VaultFile = serde_json::from_str(&original).unwrap();
        file.m_cost = u32::MAX;
        fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();
        let err = Vault::open_with(path, FAST).unlock("correct horse battery").unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn every_write_uses_a_fresh_nonce() {
        let (v, path) = new_vault();
        v.create("correct horse battery").unwrap();
        v.set("k", "v1").unwrap();
        let first: VaultFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        v.set("k", "v1").unwrap();
        let second: VaultFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_ne!(first.nonce, second.nonce);
    }

    #[test]
    fn change_passphrase_requires_the_old_one_and_rekeys() {
        let (v, path) = new_vault();
        v.create("old password 123").unwrap();
        v.set("k", "value").unwrap();

        assert!(v.change_passphrase("not the old one", "new password 456").is_err());
        v.change_passphrase("old password 123", "new password 456").unwrap();

        let reopened = Vault::open_with(path, FAST);
        assert!(reopened.unlock("old password 123").is_err());
        reopened.unlock("new password 456").unwrap();
        assert_eq!(reopened.get("k").unwrap().as_deref(), Some("value"));
    }

    #[test]
    fn delete_and_reset_remove_data() {
        let (v, path) = new_vault();
        v.create("correct horse battery").unwrap();
        v.set("k", "value").unwrap();
        v.delete("k").unwrap();
        assert_eq!(v.get("k").unwrap(), None);
        v.delete("missing").unwrap();

        v.set("k", "value").unwrap();
        v.reset().unwrap();
        assert_eq!(v.state(), VaultState::Missing);
        assert!(!path.exists());
        // A new vault can be created afterwards and starts empty
        v.create("another password").unwrap();
        assert_eq!(v.get("k").unwrap(), None);
    }

    #[test]
    fn corrupted_file_is_backed_up_not_overwritten() {
        let path = temp_path();
        fs::write(&path, "not json at all").unwrap();
        let v = Vault::open_with(path.clone(), FAST);
        assert_eq!(v.state(), VaultState::Missing);
        assert!(path.with_extension("vault.bak").exists());
    }

    #[test]
    fn production_parameters_work() {
        let path = temp_path();
        let v = Vault::open(path.clone());
        v.create("correct horse battery").unwrap();
        v.set("k", "value").unwrap();
        v.lock();
        v.unlock("correct horse battery").unwrap();
        assert_eq!(v.get("k").unwrap().as_deref(), Some("value"));
        let file: VaultFile = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!((file.m_cost, file.t_cost, file.p_cost), (65536, 3, 1));
    }

    /// Prints how long the vault takes with the production parameters.
    /// Run: `cargo test --release kdf_timing -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn kdf_timing() {
        let v = Vault::open(temp_path());
        let t = std::time::Instant::now();
        v.create("correct horse battery").unwrap();
        println!("[timing] vault create: {} ms", t.elapsed().as_millis());
        v.lock();
        let t = std::time::Instant::now();
        v.unlock("correct horse battery").unwrap();
        println!("[timing] vault unlock: {} ms", t.elapsed().as_millis());
    }
}
