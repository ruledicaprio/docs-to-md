//! AES-256-GCM encryption of the extraction payload (Tier 3).
//!
//! The output JSON contains PII (names, document numbers, dates). When a key is
//! configured, the pipeline writes `<input>.json.enc` = `nonce ‖ ciphertext`
//! instead of plaintext. A fresh random 96-bit nonce is prepended per message,
//! and GCM authenticates the ciphertext (tamper-evident).

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::fmt;
use zeroize::Zeroizing;

const NONCE_LEN: usize = 12;

#[derive(Debug)]
pub enum CryptError {
    /// Key was not 32 bytes (after base64-decoding).
    BadKey,
    Encrypt,
    /// Decryption/authentication failed (wrong key or tampered data).
    Decrypt,
    /// Ciphertext shorter than the nonce prefix.
    Malformed,
}

impl fmt::Display for CryptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::BadKey => "invalid AES-256 key (need 32 bytes, base64)",
            Self::Encrypt => "encryption failed",
            Self::Decrypt => "decryption failed (wrong key or tampered data)",
            Self::Malformed => "ciphertext too short (missing nonce)",
        };
        f.write_str(s)
    }
}

impl std::error::Error for CryptError {}

/// Decode a base64 string into a 32-byte AES-256 key, wrapped so it is wiped
/// from memory when dropped. The intermediate decoded `Vec<u8>` is also
/// zeroized before being discarded, so the plaintext key doesn't linger in
/// two buffers.
pub fn key_from_base64(s: &str) -> Result<Zeroizing<[u8; 32]>, CryptError> {
    let mut bytes = STANDARD.decode(s.trim()).map_err(|_| CryptError::BadKey)?;
    let result: Result<[u8; 32], CryptError> =
        bytes.as_slice().try_into().map_err(|_| CryptError::BadKey);
    zeroize::Zeroize::zeroize(&mut bytes);
    result.map(Zeroizing::new)
}

/// Encrypt `plaintext` with AES-256-GCM. Output = `nonce (12B) ‖ ciphertext`.
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, CryptError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptError::BadKey)?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|_| CryptError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a `nonce ‖ ciphertext` blob produced by [`encrypt`].
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, CryptError> {
    if data.len() < NONCE_LEN {
        return Err(CryptError::Malformed);
    }
    let (nonce, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptError::BadKey)?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|_| CryptError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips() {
        let key = [7u8; 32];
        let msg = br#"{"surname":"DOE","document_number":"X1"}"#;
        let enc = encrypt(&key, msg).unwrap();
        assert_ne!(
            &enc[12..],
            &msg[..],
            "ciphertext must differ from plaintext"
        );
        assert_eq!(decrypt(&key, &enc).unwrap(), msg);
    }

    #[test]
    fn wrong_key_fails() {
        let enc = encrypt(&[1u8; 32], b"secret").unwrap();
        assert!(matches!(
            decrypt(&[2u8; 32], &enc),
            Err(CryptError::Decrypt)
        ));
    }

    #[test]
    fn tamper_is_detected() {
        let key = [3u8; 32];
        let mut enc = encrypt(&key, b"secret").unwrap();
        let last = enc.len() - 1;
        enc[last] ^= 0xff; // flip a ciphertext bit
        assert!(decrypt(&key, &enc).is_err());
    }

    #[test]
    fn nonce_makes_ciphertexts_unique() {
        let key = [9u8; 32];
        let a = encrypt(&key, b"same").unwrap();
        let b = encrypt(&key, b"same").unwrap();
        assert_ne!(a, b, "random nonce should make outputs differ");
    }

    #[test]
    fn key_from_base64_validates_length() {
        assert!(key_from_base64("c2hvcnQ=").is_err()); // "short" → 5 bytes
        let good = STANDARD.encode([0u8; 32]);
        assert_eq!(*key_from_base64(&good).unwrap(), [0u8; 32]);
    }
}
