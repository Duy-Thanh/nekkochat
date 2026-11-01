//! API-key storage. Keys never touch settings.json:
//!
//! 1. The OS credential store (Windows Credential Manager, macOS Keychain, Secret Service)
//!    is used whenever it works.
//! 2. Otherwise an encrypted vault protected by a master password is used (see `vault.rs`).
//!
//! There is deliberately no plain-text fallback. If neither is usable, keys simply are not
//! persisted and the user is told why.

use crate::data_crypto::DataKey;
use crate::types::Settings;
use crate::vault::{Vault, VaultState};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use zeroize::{Zeroize, Zeroizing};

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

// ---------------------------------------------------------------------------------------------
// Bound keys
//
// A saved API key is bound to the origin (scheme://host:port) of the endpoint it was saved
// for, and only ever lives in Rust. The frontend gets "has a key" and the origin, never the key.
// Combined with the origin check below, a compromised webview cannot read a key, and cannot
// make the app send it anywhere else either.
// ---------------------------------------------------------------------------------------------

pub const EMBEDDING_KEY: &str = "embedding:api-key";
/// The key that encrypts the chat history and the document store on disk
pub const DATA_KEY_NAME: &str = "data:dek";

/// Reads the data-encryption key from secure storage.
///
/// `allow_create` generates and saves a new key when none exists. Only pass true when no
/// encrypted data exists yet: a fresh key would make existing encrypted files unreadable forever.
pub fn load_data_key(store: &dyn SecretStore, allow_create: bool) -> Result<Option<DataKey>, String> {
    match store.get(DATA_KEY_NAME)? {
        Some(encoded) => {
            let encoded = Zeroizing::new(encoded);
            let bytes = Zeroizing::new(
                B64.decode(encoded.trim())
                    .map_err(|_| "The stored data key is damaged".to_string())?,
            );
            DataKey::from_bytes(&bytes).map(Some)
        }
        None if allow_create => {
            let key = DataKey::random()?;
            let encoded = Zeroizing::new(B64.encode(key.as_bytes()));
            store.set(DATA_KEY_NAME, &encoded)?;
            Ok(Some(key))
        }
        None => Ok(None),
    }
}
const MAX_KEY_LEN: usize = 4096;

pub fn profile_key(id: &str) -> String {
    format!("profile:{}:api-key", id)
}

/// What is kept in the secure store for one key.
#[derive(Serialize, Deserialize)]
struct StoredKey {
    v: u32,
    key: String,
    origin: String,
}

fn encode(key: &str, origin: &str) -> String {
    serde_json::to_string(&StoredKey { v: 1, key: key.to_string(), origin: origin.to_string() })
        .expect("serialising two strings cannot fail")
}

enum Stored {
    Bound { key: Zeroizing<String>, origin: String },
    /// Written by an older version: a bare key with no recorded endpoint
    Legacy(Zeroizing<String>),
}

fn decode(raw: &str) -> Stored {
    match serde_json::from_str::<StoredKey>(raw) {
        Ok(s) if s.v == 1 && !s.origin.is_empty() => {
            Stored::Bound { key: Zeroizing::new(s.key), origin: s.origin }
        }
        _ => Stored::Legacy(Zeroizing::new(raw.to_string())),
    }
}

struct BoundKey {
    key: Zeroizing<String>,
    origin: String,
}

/// In-memory copy of the stored keys, filled when settings are loaded or the vault is unlocked.
#[derive(Default)]
pub struct KeyCache {
    map: Mutex<HashMap<String, BoundKey>>,
}

impl KeyCache {
    fn map(&self) -> MutexGuard<'_, HashMap<String, BoundKey>> {
        // A poisoned lock only means another thread panicked; the data is still valid
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets every key (used when the vault is locked). Values are zeroised on drop.
    pub fn clear(&self) {
        self.map().clear();
    }

    fn insert(&self, name: &str, key: Zeroizing<String>, origin: String) {
        self.map().insert(name.to_string(), BoundKey { key, origin });
    }

    fn remove(&self, name: &str) {
        self.map().remove(name);
    }

    pub fn origin(&self, name: &str) -> Option<String> {
        self.map().get(name).map(|b| b.origin.clone())
    }

    /// The saved key, but only if it is bound to the origin of `url`. A key is never
    /// handed out for a different endpoint.
    pub fn key_for(&self, name: &str, url: &str) -> Result<Option<Zeroizing<String>>, String> {
        let map = self.map();
        let Some(bound) = map.get(name) else {
            return Ok(None);
        };
        let wanted = crate::net::origin_of(url)?;
        if bound.origin != wanted {
            return Err(format!(
                "The saved API key belongs to {} and will not be sent to {}. Open Settings and enter the key again for this URL.",
                bound.origin, wanted
            ));
        }
        Ok(Some(bound.key.clone()))
    }
}

fn check_key(key: &str) -> Result<&str, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("The API key is empty".to_string());
    }
    if key.len() > MAX_KEY_LEN {
        return Err("The API key is too long".to_string());
    }
    if key.chars().any(char::is_control) {
        return Err("The API key contains invalid characters".to_string());
    }
    Ok(key)
}

/// Saves a key bound to the origin of `url`. Returns that origin.
pub fn set_key(
    store: &dyn SecretStore,
    cache: &KeyCache,
    name: &str,
    key: &str,
    url: &str,
) -> Result<String, String> {
    let key = check_key(key)?;
    let origin = crate::net::origin_of(url)?;
    store.set(name, &encode(key, &origin))?;
    cache.insert(name, Zeroizing::new(key.to_string()), origin.clone());
    Ok(origin)
}

/// Removes a key from the cache and, when a store is available, from the store too.
pub fn delete_key(store: Option<&dyn SecretStore>, cache: &KeyCache, name: &str) -> Result<(), String> {
    cache.remove(name);
    match store {
        Some(s) => s.delete(name),
        None => Ok(()),
    }
}

/// Reads one key from the store into the cache, upgrading old formats on the way.
fn load_one(
    store: &dyn SecretStore,
    cache: &KeyCache,
    name: &str,
    legacy_file_key: &mut String,
    url: Option<&str>,
    warnings: &mut Vec<String>,
) {
    let origin_for_legacy = url.and_then(|u| crate::net::origin_of(u).ok());

    let adopt_legacy = |key: Zeroizing<String>, warnings: &mut Vec<String>| match &origin_for_legacy {
        Some(origin) => match store.set(name, &encode(&key, origin)) {
            Ok(()) => cache.insert(name, key, origin.clone()),
            Err(e) => warnings.push(format!("Could not upgrade a saved API key: {}", e)),
        },
        None => warnings.push(
            "A saved API key has no known endpoint and was ignored. Enter it again in Settings.".to_string(),
        ),
    };

    match store.get(name) {
        Ok(Some(raw)) => match decode(&raw) {
            Stored::Bound { key, origin } => cache.insert(name, key, origin),
            Stored::Legacy(key) => adopt_legacy(key, warnings),
        },
        Ok(None) => {
            // A key that older versions left in settings.json: move it into secure storage
            if !legacy_file_key.is_empty() {
                adopt_legacy(Zeroizing::new(std::mem::take(legacy_file_key)), warnings);
            }
        }
        Err(e) => warnings.push(format!("Could not read a saved API key: {}", e)),
    }
    legacy_file_key.zeroize();
}

/// Rebuilds the cache from the store for every key `settings` refers to, then fills in the
/// derived `has_api_key` / `key_origin` fields. Returns warnings.
pub fn load_into_cache(store: &dyn SecretStore, cache: &KeyCache, settings: &mut Settings) -> Vec<String> {
    let mut warnings = Vec::new();
    cache.clear();

    for p in &mut settings.profiles {
        let name = profile_key(&p.id);
        let url = p.base_url.clone();
        load_one(store, cache, &name, &mut p.api_key, Some(&url), &mut warnings);
    }
    let embedding_url = settings.embedding.base_url.clone();
    let embedding_url = (!embedding_url.trim().is_empty()).then_some(embedding_url);
    load_one(
        store,
        cache,
        EMBEDDING_KEY,
        &mut settings.embedding.api_key,
        embedding_url.as_deref(),
        &mut warnings,
    );

    fill_key_info(settings, cache);
    warnings.dedup();
    warnings
}

/// Sets `has_api_key` / `key_origin` from the cache.
pub fn fill_key_info(settings: &mut Settings, cache: &KeyCache) {
    for p in &mut settings.profiles {
        let origin = cache.origin(&profile_key(&p.id));
        p.has_api_key = origin.is_some();
        p.key_origin = origin.unwrap_or_default();
    }
    let origin = cache.origin(EMBEDDING_KEY);
    settings.embedding.has_api_key = origin.is_some();
    settings.embedding.key_origin = origin.unwrap_or_default();
}

/// Strips the derived fields so they are not written to settings.json.
pub fn clear_key_info(settings: &mut Settings) {
    for p in &mut settings.profiles {
        p.has_api_key = false;
        p.key_origin.clear();
        p.api_key.zeroize();
    }
    settings.embedding.has_api_key = false;
    settings.embedding.key_origin.clear();
    settings.embedding.api_key.zeroize();
    // The oldest single-provider format kept its key at the top level
    if let Some(key) = settings.api_key.as_mut() {
        key.zeroize();
    }
    settings.api_key = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EmbeddingConfig, Profile};
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeStore {
        data: RefCell<HashMap<String, String>>,
        broken: bool,
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

    fn settings(profiles: &[(&str, &str, &str)]) -> Settings {
        // (id, base_url, legacy api key in file)
        Settings {
            active_profile_id: profiles.first().map(|p| p.0.to_string()).unwrap_or_default(),
            profiles: profiles
                .iter()
                .map(|(id, url, key)| Profile {
                    id: id.to_string(),
                    name: id.to_string(),
                    provider: "openai".into(),
                    base_url: url.to_string(),
                    model: "m".into(),
                    api_key: key.to_string(),
                    has_api_key: false,
                    key_origin: String::new(),
                })
                .collect(),
            embedding: EmbeddingConfig::default(),
            provider: None,
            base_url: None,
            api_key: None,
            model: None,
        }
    }

    #[test]
    fn stored_format_round_trips_and_legacy_is_detected() {
        let raw = encode("sk-1", "https://api.openai.com");
        match decode(&raw) {
            Stored::Bound { key, origin } => {
                assert_eq!(key.as_str(), "sk-1");
                assert_eq!(origin, "https://api.openai.com");
            }
            _ => panic!("expected a bound key"),
        }
        assert!(matches!(decode("sk-plain-old-key"), Stored::Legacy(_)));
        assert!(matches!(decode(r#"{"v":1,"key":"k","origin":""}"#), Stored::Legacy(_)));
    }

    #[test]
    fn a_key_is_only_handed_out_for_its_own_origin() {
        let store = FakeStore::default();
        let cache = KeyCache::default();
        set_key(&store, &cache, "profile:a:api-key", "sk-secret", "https://api.openai.com/v1").unwrap();

        // Same origin, any path: allowed
        let k = cache.key_for("profile:a:api-key", "https://api.openai.com/v1/chat/completions").unwrap();
        assert_eq!(k.unwrap().as_str(), "sk-secret");

        // Other host, other port, other scheme: refused with an explanation
        for evil in [
            "https://attacker.example.com/v1",
            "https://api.openai.com:8443/v1",
            "https://api.openai.com.evil.com/v1",
            "https://evil.com/api.openai.com",
        ] {
            let err = cache.key_for("profile:a:api-key", evil).unwrap_err();
            assert!(err.contains("will not be sent"), "{evil}: {err}");
        }
        // No key saved: nothing to leak
        assert!(cache.key_for("profile:other:api-key", "https://evil.com").unwrap().is_none());
    }

    #[test]
    fn set_key_validates_input() {
        let store = FakeStore::default();
        let cache = KeyCache::default();
        assert!(set_key(&store, &cache, "n", "   ", "https://a.com").is_err());
        assert!(set_key(&store, &cache, "n", "has\nnewline", "https://a.com").is_err());
        assert!(set_key(&store, &cache, "n", &"x".repeat(5000), "https://a.com").is_err());
        assert!(set_key(&store, &cache, "n", "ok-key", "http://evil.com").is_err());
        assert!(set_key(&store, &cache, "n", "ok-key", "https://user:pw@a.com").is_err());
        assert!(store.data.borrow().is_empty(), "nothing is stored when validation fails");
        // Surrounding whitespace from copy/paste is trimmed
        set_key(&store, &cache, "n", "  ok-key \n", "https://a.com").unwrap();
        assert_eq!(cache.key_for("n", "https://a.com").unwrap().unwrap().as_str(), "ok-key");
    }

    #[test]
    fn loading_fills_the_cache_and_flags_but_never_exposes_the_key() {
        let store = FakeStore::default();
        store.set("profile:a:api-key", &encode("sk-a", "https://a.com")).unwrap();
        let cache = KeyCache::default();
        let mut s = settings(&[("a", "https://a.com/v1", ""), ("b", "https://b.com/v1", "")]);

        assert!(load_into_cache(&store, &cache, &mut s).is_empty());
        assert!(s.profiles[0].has_api_key);
        assert_eq!(s.profiles[0].key_origin, "https://a.com");
        assert!(!s.profiles[1].has_api_key);

        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("sk-a"), "the key must not appear in anything sent to the frontend");
    }

    #[test]
    fn a_bare_legacy_key_is_bound_to_the_profile_endpoint_and_rewritten() {
        let store = FakeStore::default();
        store.set("profile:a:api-key", "sk-old-raw").unwrap();
        let cache = KeyCache::default();
        let mut s = settings(&[("a", "https://api.example.com/v1", "")]);

        assert!(load_into_cache(&store, &cache, &mut s).is_empty());
        assert!(s.profiles[0].has_api_key);
        let rewritten = store.get("profile:a:api-key").unwrap().unwrap();
        assert!(matches!(decode(&rewritten), Stored::Bound { .. }));
        assert_eq!(
            cache.key_for("profile:a:api-key", "https://api.example.com").unwrap().unwrap().as_str(),
            "sk-old-raw"
        );
    }

    #[test]
    fn a_plain_text_key_left_in_the_old_file_is_migrated() {
        let store = FakeStore::default();
        let cache = KeyCache::default();
        let mut s = settings(&[("a", "https://api.example.com/v1", "sk-from-file")]);

        assert!(load_into_cache(&store, &cache, &mut s).is_empty());
        assert!(s.profiles[0].has_api_key);
        assert!(s.profiles[0].api_key.is_empty(), "the file copy is wiped from memory");
        assert!(store.get("profile:a:api-key").unwrap().unwrap().contains("sk-from-file"));
    }

    #[test]
    fn an_unreadable_store_yields_a_warning_and_no_key() {
        let store = FakeStore { read_fails: true, ..Default::default() };
        let cache = KeyCache::default();
        let mut s = settings(&[("a", "https://a.com/v1", "")]);
        let warnings = load_into_cache(&store, &cache, &mut s);
        assert_eq!(warnings.len(), 1);
        assert!(!s.profiles[0].has_api_key);
    }

    #[test]
    fn the_data_key_is_created_once_then_reused() {
        let store = FakeStore::default();
        assert!(load_data_key(&store, false).unwrap().is_none(), "no key and creation not allowed");

        let first = load_data_key(&store, true).unwrap().unwrap();
        let again = load_data_key(&store, false).unwrap().unwrap();
        assert_eq!(first.as_bytes(), again.as_bytes());
        // Asking to create must still return the existing key, not replace it
        let third = load_data_key(&store, true).unwrap().unwrap();
        assert_eq!(first.as_bytes(), third.as_bytes());
    }

    #[test]
    fn a_damaged_or_unreadable_data_key_is_an_error_never_silently_replaced() {
        let store = FakeStore::default();
        store.set(DATA_KEY_NAME, "!!! not base64 !!!").unwrap();
        assert!(load_data_key(&store, true).is_err());
        assert_eq!(store.get(DATA_KEY_NAME).unwrap().as_deref(), Some("!!! not base64 !!!"));

        let unreadable = FakeStore { read_fails: true, ..Default::default() };
        assert!(load_data_key(&unreadable, true).is_err(), "must not create a key it cannot verify is absent");
    }

    #[test]
    fn delete_key_clears_cache_and_store() {
        let store = FakeStore::default();
        let cache = KeyCache::default();
        set_key(&store, &cache, "n", "k", "https://a.com").unwrap();
        delete_key(Some(&store), &cache, "n").unwrap();
        assert!(cache.origin("n").is_none());
        assert!(store.get("n").unwrap().is_none());
    }

    #[test]
    fn clear_key_info_strips_everything_derived() {
        let mut s = settings(&[("a", "https://a.com", "legacy")]);
        s.api_key = Some("top-level-legacy".into());
        s.profiles[0].has_api_key = true;
        s.profiles[0].key_origin = "https://a.com".into();
        clear_key_info(&mut s);
        assert!(s.api_key.is_none());
        assert!(!s.profiles[0].has_api_key && s.profiles[0].key_origin.is_empty() && s.profiles[0].api_key.is_empty());
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
        store.delete(key).unwrap();
        KeyringStore::probe().unwrap();
    }
}
