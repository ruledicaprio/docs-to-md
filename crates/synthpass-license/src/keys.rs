//! The embedded Ed25519 verifying (public) key that shipped binaries check
//! license signatures against. A public key isn't a secret, so baking it in
//! as a checked-in file is safe; rotation is a one-file swap.
//!
//! `SYNTHPASS_LICENSE_PUBKEY` overrides it at runtime (base64, same 32-byte
//! encoding) — mirrors the `SYNTHPASS_MODEL_SHA256`/`SYNTHPASS_OCR_*_SHA256`
//! known-good-plus-override convention used elsewhere in this workspace, so
//! tests can inject a throwaway keypair without touching the shipped default.

use crate::LicenseError;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::VerifyingKey;

/// The key baked into this build. **Placeholder** — generate a real keypair
/// with `synthpass-license-issuer keygen` and replace this file before issuing
/// any real licenses; see `docs/ARCHITECTURE.md`'s licensing section.
const EMBEDDED_PUBLIC_KEY_B64: &str = include_str!("../pubkey.b64");

/// The verifying key to check license signatures against: `SYNTHPASS_LICENSE_PUBKEY`
/// if set, else the embedded default.
pub fn verifying_key() -> Result<VerifyingKey, LicenseError> {
    let b64 = std::env::var("SYNTHPASS_LICENSE_PUBKEY")
        .unwrap_or_else(|_| EMBEDDED_PUBLIC_KEY_B64.trim().to_string());
    decode_verifying_key(&b64)
}

pub(crate) fn decode_verifying_key(b64: &str) -> Result<VerifyingKey, LicenseError> {
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|_| LicenseError::InvalidPublicKey)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| LicenseError::InvalidPublicKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| LicenseError::InvalidPublicKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_key_decodes() {
        // Whatever ships in pubkey.b64 must at least be a structurally valid
        // key, even before it's replaced with the vendor's real one.
        verifying_key().expect("embedded pubkey.b64 should decode");
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            decode_verifying_key("not base64!!"),
            Err(LicenseError::InvalidPublicKey)
        ));
        assert!(matches!(
            decode_verifying_key(&STANDARD.encode(b"too short")),
            Err(LicenseError::InvalidPublicKey)
        ));
    }
}
