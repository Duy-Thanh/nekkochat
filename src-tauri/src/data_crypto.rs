//! Encryption at rest for the chat history and the document store.
//!
//! A random 256-bit data key lives in secure storage (the OS keychain, or the encrypted vault
//! when there is none) next to the API keys. Files are sealed with XChaCha20-Poly1305:
//!
//! ```text
//! "NKCENC1\0" | 24-byte random nonce | ciphertext + 16-byte tag
//! ```
//!
//! The file kind ("sessions", "rag") is authenticated together with the data, so one file cannot
//! be swapped for the other, and any modification makes decryption fail instead of yielding
//! altered data.

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, Generate, Key, KeyInit, Payload},
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"NKCENC1\0";
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;

/// The key that encrypts the data files. Wiped from memory when dropped.
#[derive(Clone)]
pub struct DataKey(Zeroizing<Vec<u8>>);

impl DataKey {
    pub fn random() -> Result<Self, String> {
        let mut bytes = Zeroizing::new(vec![0u8; KEY_LEN]);
        getrandom::fill(&mut bytes).map_err(|e| format!("No secure random source available: {}", e))?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != KEY_LEN {
            return Err("The stored data key has the wrong length".to_string());
        }
        Ok(Self(Zeroizing::new(bytes.to_vec())))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn cipher(key: &DataKey) -> Result<XChaCha20Poly1305, String> {
    let k = Key::<XChaCha20Poly1305>::try_from(key.as_bytes()).map_err(|_| "Invalid data key".to_string())?;
    Ok(XChaCha20Poly1305::new(&k))
}

fn aad(kind: &str) -> Vec<u8> {
    let mut a = MAGIC.to_vec();
    a.extend_from_slice(kind.as_bytes());
    a
}

pub fn encrypt(key: &DataKey, kind: &str, plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let nonce = XNonce::generate();
    let ciphertext = cipher(key)?
        .encrypt(&nonce, Payload { msg: plaintext, aad: &aad(kind) })
        .map_err(|_| "Encryption failed".to_string())?;

    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt(key: &DataKey, kind: &str, bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>, String> {
    const FAIL: &str = "Could not decrypt the data (wrong key, or the file is damaged)";
    let body = bytes.strip_prefix(MAGIC).ok_or("Not an encrypted file")?;
    if body.len() < NONCE_LEN + 16 {
        return Err(FAIL.to_string());
    }
    let (nonce, ciphertext) = body.split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce).map_err(|_| FAIL.to_string())?;
    let plaintext = cipher(key)?
        .decrypt(&nonce, Payload { msg: ciphertext, aad: &aad(kind) })
        .map_err(|_| FAIL.to_string())?;
    Ok(Zeroizing::new(plaintext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let key = DataKey::random().unwrap();
        let sealed = encrypt(&key, "sessions", b"[{\"title\":\"secret chat\"}]").unwrap();
        assert!(is_encrypted(&sealed));
        assert!(!String::from_utf8_lossy(&sealed).contains("secret chat"));
        assert_eq!(&*decrypt(&key, "sessions", &sealed).unwrap(), b"[{\"title\":\"secret chat\"}]");
    }

    #[test]
    fn every_write_uses_a_fresh_nonce() {
        let key = DataKey::random().unwrap();
        let a = encrypt(&key, "sessions", b"same").unwrap();
        let b = encrypt(&key, "sessions", b"same").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_key_tampering_and_truncation_are_detected() {
        let key = DataKey::random().unwrap();
        let sealed = encrypt(&key, "sessions", b"data").unwrap();

        assert!(decrypt(&DataKey::random().unwrap(), "sessions", &sealed).is_err());

        let mut flipped = sealed.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert!(decrypt(&key, "sessions", &flipped).is_err());

        let mut nonce_flipped = sealed.clone();
        nonce_flipped[MAGIC.len()] ^= 1;
        assert!(decrypt(&key, "sessions", &nonce_flipped).is_err());

        assert!(decrypt(&key, "sessions", &sealed[..sealed.len() - 5]).is_err());
        assert!(decrypt(&key, "sessions", &sealed[..MAGIC.len() + 3]).is_err());
        assert!(decrypt(&key, "sessions", b"plain text").is_err());
    }

    #[test]
    fn one_kind_of_file_cannot_pass_for_another() {
        let key = DataKey::random().unwrap();
        let sealed = encrypt(&key, "sessions", b"data").unwrap();
        assert!(decrypt(&key, "rag", &sealed).is_err());
    }

    #[test]
    fn plaintext_is_not_mistaken_for_encrypted() {
        assert!(!is_encrypted(b"[{\"id\":\"1\"}]"));
        assert!(!is_encrypted(b""));
    }

    #[test]
    fn key_length_is_enforced() {
        assert!(DataKey::from_bytes(&[0u8; 31]).is_err());
        assert!(DataKey::from_bytes(&[0u8; 33]).is_err());
        assert!(DataKey::from_bytes(&[0u8; 32]).is_ok());
    }
}
