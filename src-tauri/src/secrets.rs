//! API-key storage. Keys never touch settings.json:
//!
//! 1. The OS credential store (Windows Credential Manager, macOS Keychain, Secret Service)
//!    is used whenever it works.
//! 2. Otherwise an encrypted vault protected by a master password is used (see `vault.rs`).
//!
//! There is deliberately no plain-text fallback. If neither is usable, keys simply are not
//! persisted and the user is told why.

use crate::types::Settings;
use crate::vault::{Vault, VaultState};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

const SERVICE: &str = "com.nekkochan.nekkochat";

pub trait SecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The real OS credential store.
pub struct KeyringStore;

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())
    }

    /// Checks that the store really works (write, read back, delete), not just that it exists.
    pub fn probe() -> Result<(), String> {
        let key = "probe";
        let store = KeyringStore;
        store.set(key, "ok")?;
        let read = store.get(key)?;
        store.delete(key)?;
        if read.as_deref() == Some("ok") {
            Ok(())
        } else {
            Err("the credential store did not return the value that was written".to_string())
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        Self::entry(key)?.set_password(value).map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// What the UI needs to know about key storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretsStatus {
    /// Keys are in the OS keychain
    Keychain,
    /// No keychain; the encrypted vault is open and keys are saved in it
    VaultUnlocked,
    /// No keychain; a vault exists but needs the master password
    VaultLocked,
    /// No keychain and no vault yet: keys cannot be saved until a master password is set
    VaultMissing,
}

pub enum Backend {
    Keychain,
    Vault(Arc<Vault>),
}

impl Backend {
    /// Uses the OS keychain if it works, otherwise the encrypted vault at `vault_path`.
    /// Setting `NEKKOCHAT_FORCE_VAULT` skips the keychain and always uses the vault.
    pub fn detect(vault_path: PathBuf) -> Self {
        if std::env::var_os("NEKKOCHAT_FORCE_VAULT").is_some_and(|v| !v.is_empty()) {
            return Backend::Vault(Arc::new(Vault::open(vault_path)));
        }
        match KeyringStore::probe() {
            Ok(()) => Backend::Keychain,
            Err(e) => {
                eprintln!("[secrets] OS keychain unavailable ({}); using the encrypted vault", e);
                Backend::Vault(Arc::new(Vault::open(vault_path)))
            }
        }
    }

    pub fn status(&self) -> SecretsStatus {
        match self {
            Backend::Keychain => SecretsStatus::Keychain,
            Backend::Vault(v) => match v.state() {
                VaultState::Unlocked => SecretsStatus::VaultUnlocked,
                VaultState::Locked => SecretsStatus::VaultLocked,
                VaultState::Missing => SecretsStatus::VaultMissing,
            },
        }
    }

    /// The store to read/write right now, or `None` while keys cannot be accessed
    /// (vault locked or not created yet).
    pub fn store(&self) -> Option<&dyn SecretStore> {
        match self {
            Backend::Keychain => Some(&KeyringStore),
            Backend::Vault(v) if v.state() == VaultState::Unlocked => Some(v.as_ref()),
            Backend::Vault(_) => None,
        }
    }

    pub fn vault(&self) -> Result<&Arc<Vault>, String> {
        match self {
            Backend::Vault(v) => Ok(v),
            Backend::Keychain => {
                Err("The OS keychain is in use, so no master password is needed".to_string())
            }
        }
    }
}

fn profile_key(id: &str) -> String {
    format!("profile:{}:api-key", id)
}

const EMBEDDING_KEY: &str = "embedding:api-key";

/// Fills in API keys from the store. A key found in the store wins over one in the file.
/// Keys that could not be read are remembered in `unreadable`, so a later save does not
/// mistake "failed to read" for "the user cleared it" and delete the real key.
pub fn apply_on_load(
    store: &dyn SecretStore,
    settings: &mut Settings,
    unreadable: &mut HashSet<String>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut fill = |key: String, slot: &mut String| match store.get(&key) {
        Ok(found) => {
            unreadable.remove(&key);
            if let Some(v) = found {
                *slot = v;
            }
        }
        Err(e) => {
            warnings.push(format!("Could not read a saved API key: {}", e));
            unreadable.insert(key);
        }
    };

    for p in &mut settings.profiles {
        fill(profile_key(&p.id), &mut p.api_key);
    }
    fill(EMBEDDING_KEY.to_string(), &mut settings.embedding.api_key);

    warnings.dedup();
    warnings
}

/// Moves API keys into the store and ALWAYS blanks them in `settings`, so they are never
/// written to disk, even when storing fails. `previous_profile_ids` are the profiles in the
/// existing file, used to clean up deleted ones.
pub fn move_on_save(
    store: &dyn SecretStore,
    settings: &mut Settings,
    previous_profile_ids: &[String],
    unreadable: &mut HashSet<String>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut stash = |key: String, slot: &mut String| {
        if slot.is_empty() {
            // The user cleared the key. But if we never managed to read it, an empty value
            // means "unknown", not "cleared": leave the stored one alone.
            if !unreadable.contains(&key) {
                let _ = store.delete(&key);
            }
        } else {
            match store.set(&key, slot) {
                Ok(()) => {
                    unreadable.remove(&key);
                }
                Err(e) => warnings.push(format!(
                    "An API key could not be saved to secure storage ({}). It was not written to disk, so you will need to enter it again after restarting.",
                    e
                )),
            }
        }
        slot.clear();
    };

    for p in &mut settings.profiles {
        stash(profile_key(&p.id), &mut p.api_key);
    }
    stash(EMBEDDING_KEY.to_string(), &mut settings.embedding.api_key);

    for id in previous_profile_ids {
        if !settings.profiles.iter().any(|p| &p.id == id) {
            let _ = store.delete(&profile_key(id));
        }
    }

    warnings.dedup();
    warnings
}

/// For when no store is available: blanks every key so none reaches the disk.
/// Returns how many non-empty keys were dropped.
pub fn withhold_keys(settings: &mut Settings) -> usize {
    let mut dropped = 0;
    let mut clear = |slot: &mut String| {
        if !slot.is_empty() {
            dropped += 1;
            slot.clear();
        }
    };
    for p in &mut settings.profiles {
        clear(&mut p.api_key);
    }
    clear(&mut settings.embedding.api_key);
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EmbeddingConfig, Profile};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeStore {
        data: RefCell<HashMap<String, String>>,
        broken: bool,
        /// Reads fail but writes work, like a keychain that is temporarily locked
        read_fails: bool,
    }

    impl SecretStore for FakeStore {
        fn get(&self, key: &str) -> Result<Option<String>, String> {
            if self.broken || self.read_fails {
                return Err("no store".into());
            }
            Ok(self.data.borrow().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> Result<(), String> {
            if self.broken {
                return Err("no store".into());
            }
            self.data.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            if self.broken {
                return Err("no store".into());
            }
            self.data.borrow_mut().remove(key);
            Ok(())
        }
    }

    fn settings(keys: &[(&str, &str)], embedding_key: &str) -> Settings {
        Settings {
            active_profile_id: keys.first().map(|k| k.0.to_string()).unwrap_or_default(),
            profiles: keys
                .iter()
                .map(|(id, key)| Profile {
                    id: id.to_string(),
                    name: id.to_string(),
                    provider: "openai".into(),
                    base_url: "https://x/v1".into(),
                    api_key: key.to_string(),
                    model: "m".into(),
                })
                .collect(),
            embedding: EmbeddingConfig {
                base_url: String::new(),
                api_key: embedding_key.into(),
                model: String::new(),
            },
            provider: None,
            base_url: None,
            api_key: None,
            model: None,
        }
    }

    #[test]
    fn save_moves_keys_out_of_the_file_and_load_restores_them() {
        let store = FakeStore::default();
        let mut unreadable = HashSet::new();
        let mut s = settings(&[("a", "key-a"), ("b", "key-b")], "emb-key");
        assert!(move_on_save(&store, &mut s, &[], &mut unreadable).is_empty());

        // What is written to disk contains no secrets
        let on_disk = serde_json::to_string(&s).unwrap();
        assert!(!on_disk.contains("key-a") && !on_disk.contains("emb-key"));

        let mut loaded: Settings = serde_json::from_str(&on_disk).unwrap();
        assert!(apply_on_load(&store, &mut loaded, &mut unreadable).is_empty());
        assert_eq!(loaded.profiles[0].api_key, "key-a");
        assert_eq!(loaded.profiles[1].api_key, "key-b");
        assert_eq!(loaded.embedding.api_key, "emb-key");
    }

    #[test]
    fn keys_are_never_written_to_disk_even_when_the_store_fails() {
        let store = FakeStore { broken: true, ..Default::default() };
        let mut s = settings(&[("a", "key-a")], "emb-secret-value");
        let warnings = move_on_save(&store, &mut s, &[], &mut HashSet::new());
        assert_eq!(warnings.len(), 1);
        let on_disk = serde_json::to_string(&s).unwrap();
        assert!(!on_disk.contains("key-a") && !on_disk.contains("emb-secret-value"));
    }

    #[test]
    fn a_key_in_an_old_plain_text_file_is_migrated_then_scrubbed() {
        let store = FakeStore::default();
        let mut unreadable = HashSet::new();
        let mut loaded = settings(&[("a", "old-plain")], "");
        assert!(apply_on_load(&store, &mut loaded, &mut unreadable).is_empty());
        assert_eq!(loaded.profiles[0].api_key, "old-plain");

        move_on_save(&store, &mut loaded, &["a".into()], &mut unreadable);
        assert!(loaded.profiles[0].api_key.is_empty());
        assert_eq!(store.get("profile:a:api-key").unwrap().as_deref(), Some("old-plain"));
    }

    #[test]
    fn clearing_a_key_removes_it_from_the_store() {
        let store = FakeStore::default();
        store.set("profile:a:api-key", "k").unwrap();
        let mut s = settings(&[("a", "")], "");
        move_on_save(&store, &mut s, &["a".into()], &mut HashSet::new());
        assert_eq!(store.get("profile:a:api-key").unwrap(), None);
    }

    #[test]
    fn a_failed_read_does_not_cause_the_stored_key_to_be_deleted() {
        let store = FakeStore { read_fails: true, ..Default::default() };
        store.set("profile:a:api-key", "precious").unwrap();
        let mut unreadable = HashSet::new();

        // Load fails, so the UI holds an empty key and will save it back as empty
        let mut loaded = settings(&[("a", "")], "");
        assert_eq!(apply_on_load(&store, &mut loaded, &mut unreadable).len(), 1);
        move_on_save(&store, &mut loaded, &["a".into()], &mut unreadable);
        assert_eq!(store.data.borrow().get("profile:a:api-key").map(String::as_str), Some("precious"));
    }

    #[test]
    fn deleted_profiles_are_cleaned_from_the_store() {
        let store = FakeStore::default();
        store.set("profile:gone:api-key", "k").unwrap();
        let mut s = settings(&[("a", "x")], "");
        move_on_save(&store, &mut s, &["a".into(), "gone".into()], &mut HashSet::new());
        assert_eq!(store.get("profile:gone:api-key").unwrap(), None);
    }

    #[test]
    fn withhold_keys_blanks_everything_and_counts() {
        let mut s = settings(&[("a", "k1"), ("b", "")], "emb");
        assert_eq!(withhold_keys(&mut s), 2);
        assert!(s.profiles.iter().all(|p| p.api_key.is_empty()));
        assert!(s.embedding.api_key.is_empty());
        assert_eq!(withhold_keys(&mut s), 0);
    }

    /// Talks to the real OS credential store. Run manually: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn real_os_keychain_round_trip() {
        let store = KeyringStore;
        let key = "test:round-trip";
        store.set(key, "secret-value").unwrap();
        assert_eq!(store.get(key).unwrap().as_deref(), Some("secret-value"));
        store.delete(key).unwrap();
        assert_eq!(store.get(key).unwrap(), None);
        // Deleting something that doesn't exist is not an error
        store.delete(key).unwrap();
        KeyringStore::probe().unwrap();
    }
}
