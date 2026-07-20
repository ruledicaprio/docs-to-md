//! Signing + keygen for the vendor issuer. Gated behind the `vendor`
//! feature — never compiled into the customer-facing `synthpass`/`synthpass-serve`
//! binaries, so they carry no private-key handling or keygen RNG dependency
//! at all. Only [`bin/synthpass-license-issuer.rs`](../src/bin/synthpass-license-issuer.rs)
//! and this crate's own tests use this module.

use crate::{LicenseError, LicensePayload, SignedLicense, FORMAT_VERSION};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::rand_core::UnwrapErr;
use ed25519_dalek::{Signer, SigningKey};
use getrandom::SysRng;

/// Generate a new Ed25519 keypair: `(private_key_b64, public_key_b64)`. The
/// private key must be stored offline and never committed to the repo; the
/// public key is safe to embed in `pubkey.b64` (rotation is a one-file
/// swap) — a public key is not a secret.
pub fn generate_keypair() -> (String, String) {
    let mut rng = UnwrapErr(SysRng);
    let signing_key = SigningKey::generate(&mut rng);
    let priv_b64 = STANDARD.encode(signing_key.to_bytes());
    let pub_b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());
    (priv_b64, pub_b64)
}

/// Parse a base64-encoded 32-byte Ed25519 signing (private) key, e.g. from
/// `SYNTHPASS_LICENSE_PRIVKEY`.
pub fn signing_key_from_base64(b64: &str) -> Result<SigningKey, LicenseError> {
    let bytes = STANDARD
        .decode(b64.trim())
        .map_err(|_| LicenseError::Malformed("private key not valid base64".into()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| LicenseError::Malformed("private key must be 32 bytes".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// Sign `payload` over its exact serialized bytes, producing the on-disk
/// envelope — see the crate-level docs for why this matters (canonical
/// bytes, no re-serialize-then-verify).
pub fn issue(signing_key: &SigningKey, payload: &LicensePayload) -> SignedLicense {
    let payload_bytes = serde_json::to_vec(payload).expect("LicensePayload always serializes");
    let signature = signing_key.sign(&payload_bytes);
    SignedLicense {
        format: FORMAT_VERSION,
        payload: STANDARD.encode(&payload_bytes),
        signature: STANDARD.encode(signature.to_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verify_with_key;

    #[test]
    fn generated_keypair_round_trips_through_issue_and_verify() {
        // Verifies against the key directly, not via SYNTHPASS_LICENSE_PUBKEY —
        // mutating that process-global env var isn't safe across parallel
        // test threads (see `lib.rs`'s test module for the same reasoning).
        let (priv_b64, pub_b64) = generate_keypair();
        let signing_key = signing_key_from_base64(&priv_b64).unwrap();
        let verifying_key = crate::keys::decode_verifying_key(&pub_b64).unwrap();

        let payload = LicensePayload {
            license_id: "gen-test".into(),
            customer: "Vendor Test".into(),
            hw_fingerprint: String::new(),
            issued_unix: 1_700_000_000,
            expires_unix: 4_000_000_000,
            tier: "enterprise".into(),
            features: vec![],
            mlis_min_version: None,
        };
        let signed = issue(&signing_key, &payload);
        let verified = verify_with_key(&signed, &verifying_key);

        assert_eq!(verified.unwrap(), payload);
    }

    #[test]
    fn rejects_malformed_private_key() {
        assert!(signing_key_from_base64("not base64!!").is_err());
        assert!(signing_key_from_base64(&STANDARD.encode(b"too short")).is_err());
    }
}
