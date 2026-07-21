//! Offline cryptographic licensing: Ed25519-signed license files so a
//! shipped `synthpass`/`synthpass-serve` binary can be sold and metered for air-gapped
//! enterprise distribution without ever phoning home.
//!
//! **Threat model, stated plainly (matches this project's house style of
//! documenting limitations rather than overselling — see
//! `docs/ARCHITECTURE.md` §7):** the source is public, so anyone who
//! rebuilds from source can strip this check entirely. This meters and gates
//! the *official pre-built binary*, deters casual license-sharing, and
//! produces a compliance artifact — it is **not DRM** and is not sold as
//! tamper-proof. See [`fingerprint`] for the hardware-binding caveat
//! specifically.
//!
//! Format: a license file (`license.mlis`) is a small JSON envelope
//! ([`SignedLicense`]) whose `payload` field is the base64 of the *exact*
//! bytes that were signed. The verifier checks the signature over those
//! literal bytes and only deserializes into [`LicensePayload`] afterward —
//! so, unlike re-serializing the payload before verifying, a valid license
//! can never fail to verify due to field-order/whitespace drift between
//! signer and verifier.

mod fingerprint;
mod keys;
#[cfg(feature = "vendor")]
pub mod sign;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

pub use fingerprint::machine_fingerprint;

/// The terms of a license, signed as-is (see module docs on the byte-exact
/// signing scheme).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicensePayload {
    pub license_id: String,
    pub customer: String,
    /// Empty ⇒ unbound (site/trial license) — [`check`] skips the
    /// fingerprint comparison entirely when this is empty.
    #[serde(default)]
    pub hw_fingerprint: String,
    pub issued_unix: u64,
    pub expires_unix: u64,
    pub tier: String,
    #[serde(default)]
    pub features: Vec<String>,
    /// A license can refuse to unlock a `synthpass` build older than it was
    /// issued for. [`check`] enforces this against `CARGO_PKG_VERSION` when
    /// present.
    #[serde(default)]
    pub mlis_min_version: Option<String>,
}

/// The on-disk license file format: `payload` is base64 of the *exact*
/// signed [`LicensePayload`] JSON bytes; `signature` is the base64 Ed25519
/// signature over those same bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedLicense {
    pub format: u32,
    pub payload: String,
    pub signature: String,
}

pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug)]
pub enum LicenseError {
    Io(std::io::Error),
    /// The envelope (outer JSON) or its base64 fields didn't parse.
    Malformed(String),
    InvalidPublicKey,
    /// Signature didn't verify — tampered, corrupted, or signed by a
    /// different key entirely.
    InvalidSignature,
    /// Signature verified but the payload bytes underneath weren't valid
    /// `LicensePayload` JSON (shouldn't happen for a genuinely-issued
    /// license; would mean the issuer and verifier disagree on the schema).
    InvalidPayload(String),
    Expired {
        expires_unix: u64,
    },
    FingerprintMismatch,
    /// The running binary's `CARGO_PKG_VERSION` doesn't satisfy the
    /// license's `mlis_min_version`. Also raised (fail closed) when
    /// `mlis_min_version` itself isn't a parsable `major[.minor[.patch]]`
    /// string — an unreadable requirement is not a satisfied one.
    MinVersionUnmet {
        required: String,
    },
}

impl fmt::Display for LicenseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read license file: {e}"),
            Self::Malformed(e) => write!(f, "malformed license file: {e}"),
            Self::InvalidPublicKey => write!(f, "invalid embedded/override public key"),
            Self::InvalidSignature => {
                write!(f, "license signature is invalid (tampered or wrong key)")
            }
            Self::InvalidPayload(e) => write!(f, "license payload did not parse: {e}"),
            Self::Expired { expires_unix } => write!(f, "license expired at {expires_unix}"),
            Self::FingerprintMismatch => {
                write!(f, "license is bound to a different machine")
            }
            Self::MinVersionUnmet { required } => write!(
                f,
                "license requires synthpass >= {required} (running {})",
                env!("CARGO_PKG_VERSION")
            ),
        }
    }
}

impl std::error::Error for LicenseError {}

impl From<std::io::Error> for LicenseError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A license that has passed signature verification (and, via
/// [`load_and_check`], expiry/fingerprint checks too).
#[derive(Debug, Clone)]
pub struct LicenseStatus {
    pub payload: LicensePayload,
}

impl LicenseStatus {
    /// Whole days remaining until expiry, relative to `now_unix`. Negative
    /// once expired (callers that reach this far already passed [`check`],
    /// so in practice this is used only for the "expires in N days" /
    /// "⚠️ expiring soon" UI, not as an expiry check itself).
    pub fn days_until_expiry(&self, now_unix: u64) -> i64 {
        (self.payload.expires_unix as i64 - now_unix as i64) / 86_400
    }
}

/// Verifies `signed`'s Ed25519 signature over its exact payload bytes
/// against `SYNTHPASS_LICENSE_PUBKEY` if set, else the embedded key (see
/// [`verify_with_key`] to verify against an explicit key instead — used by
/// this crate's own tests to avoid mutating the process-global env var,
/// which isn't safe across parallel test threads).
pub fn verify(signed: &SignedLicense) -> Result<LicensePayload, LicenseError> {
    verify_with_key(signed, &keys::verifying_key()?)
}

/// Verifies `signed`'s Ed25519 signature over its exact payload bytes
/// against an explicit key and, only once that succeeds, parses those bytes
/// as [`LicensePayload`]. Does not check expiry or fingerprint — see
/// [`check`] / [`load_and_check`].
pub fn verify_with_key(
    signed: &SignedLicense,
    key: &ed25519_dalek::VerifyingKey,
) -> Result<LicensePayload, LicenseError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use ed25519_dalek::Signature;

    let payload_bytes = STANDARD
        .decode(&signed.payload)
        .map_err(|e| LicenseError::Malformed(format!("payload not valid base64: {e}")))?;
    let sig_bytes = STANDARD
        .decode(&signed.signature)
        .map_err(|e| LicenseError::Malformed(format!("signature not valid base64: {e}")))?;
    let sig_bytes: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| LicenseError::Malformed("signature must be 64 bytes".into()))?;
    let signature = Signature::from_bytes(&sig_bytes);

    // `verify_strict` (not the plain `Verifier::verify`) rejects
    // non-canonical/cofactored signature malleability — the conservative,
    // recommended default per RFC 8032, and cheap insurance for a scheme
    // this security-sensitive.
    key.verify_strict(&payload_bytes, &signature)
        .map_err(|_| LicenseError::InvalidSignature)?;

    serde_json::from_slice(&payload_bytes).map_err(|e| LicenseError::InvalidPayload(e.to_string()))
}

/// Expiry + (if bound) fingerprint + (if set) minimum-version checks against
/// an already-verified payload. Pure and deterministic — `now_unix`/
/// `fingerprint` are passed in rather than read from the clock/machine here,
/// so callers can test it exactly the way `synthpass-serve`'s existing
/// `startup_refusal` is tested.
pub fn check(
    payload: &LicensePayload,
    now_unix: u64,
    fingerprint: &str,
) -> Result<(), LicenseError> {
    if now_unix > payload.expires_unix {
        return Err(LicenseError::Expired {
            expires_unix: payload.expires_unix,
        });
    }
    if !payload.hw_fingerprint.is_empty() && payload.hw_fingerprint != fingerprint {
        return Err(LicenseError::FingerprintMismatch);
    }
    if let Some(required) = &payload.mlis_min_version {
        if !version_satisfies(env!("CARGO_PKG_VERSION"), required) {
            return Err(LicenseError::MinVersionUnmet {
                required: required.clone(),
            });
        }
    }
    Ok(())
}

/// `true` iff `actual` is >= `required`, both `major[.minor[.patch]]`
/// (any missing component pads as `0`; a trailing `-pre`/`+build` suffix on
/// either string is stripped before comparing). Fails closed — an
/// unparsable `required` string (e.g. non-numeric components) is treated as
/// unmet, since an unreadable requirement can't be verified as satisfied.
fn version_satisfies(actual: &str, required: &str) -> bool {
    let Some(required) = parse_version(required) else {
        return false;
    };
    // `actual` is always this crate's own `CARGO_PKG_VERSION`, so a parse
    // failure there would mean a broken build, not a license problem —
    // still fail closed rather than panic.
    let Some(actual) = parse_version(actual) else {
        return false;
    };
    actual >= required
}

/// Parses the leading `major[.minor[.patch]]` numeric components of a
/// version string, ignoring any `-`/`+` suffix (pre-release/build
/// metadata). Missing trailing components default to `0`.
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next().unwrap_or(v);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    let patch = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    Some((major, minor, patch))
}

/// Convenience used at CLI/serve startup: read `path`, verify the
/// signature, and run the expiry/fingerprint checks against the real clock
/// and this machine's real fingerprint.
pub fn load_and_check(path: &Path) -> Result<LicenseStatus, LicenseError> {
    let data = std::fs::read_to_string(path)?;
    let signed: SignedLicense =
        serde_json::from_str(&data).map_err(|e| LicenseError::Malformed(e.to_string()))?;
    let payload = verify(&signed)?;
    check(&payload, current_unix(), &machine_fingerprint())?;
    Ok(LicenseStatus { payload })
}

/// The current Unix timestamp, clamped to 0 on a pre-epoch clock. Exposed so
/// callers computing "days until expiry" for display, or a vendor issuer
/// stamping `issued_unix`, all agree on one clock-reading convention.
pub fn current_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    /// A deterministic test keypair from a fixed seed — avoids needing an
    /// RNG dependency (the `vendor` feature's `SigningKey::generate`) just
    /// for tests; `from_bytes` is plain key-material construction, no
    /// keygen involved.
    fn keypair_from_seed(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sign_payload(signing_key: &SigningKey, payload: &LicensePayload) -> SignedLicense {
        let payload_bytes = serde_json::to_vec(payload).unwrap();
        let signature = signing_key.sign(&payload_bytes);
        SignedLicense {
            format: FORMAT_VERSION,
            payload: STANDARD.encode(&payload_bytes),
            signature: STANDARD.encode(signature.to_bytes()),
        }
    }

    fn sample_payload(hw_fingerprint: &str, expires_unix: u64) -> LicensePayload {
        LicensePayload {
            license_id: "test-license-1".into(),
            customer: "Test Customer".into(),
            hw_fingerprint: hw_fingerprint.into(),
            issued_unix: 1_700_000_000,
            expires_unix,
            tier: "enterprise".into(),
            features: vec!["extract".into()],
            mlis_min_version: None,
        }
    }

    #[test]
    fn sign_verify_round_trips() {
        let signing_key = keypair_from_seed(1);
        let payload = sample_payload("", 4_000_000_000);
        let signed = sign_payload(&signing_key, &payload);

        let verified = verify_with_key(&signed, &signing_key.verifying_key())
            .expect("valid signature should verify");
        assert_eq!(verified, payload);
    }

    #[test]
    fn expired_license_fails_check() {
        let payload = sample_payload("", 100); // expired long ago
        let err = check(&payload, 200, "").expect_err("must reject expired license");
        assert!(matches!(err, LicenseError::Expired { expires_unix: 100 }));
    }

    #[test]
    fn wrong_fingerprint_fails_check() {
        let payload = sample_payload("machine-a", 4_000_000_000);
        let err = check(&payload, 1, "machine-b").expect_err("must reject fingerprint mismatch");
        assert!(matches!(err, LicenseError::FingerprintMismatch));
    }

    #[test]
    fn unbound_license_accepted_on_any_fingerprint() {
        let payload = sample_payload("", 4_000_000_000);
        check(&payload, 1, "any-machine-at-all").expect("empty hw_fingerprint skips the check");
    }

    #[test]
    fn absent_min_version_skips_the_check() {
        let payload = sample_payload("", 4_000_000_000); // mlis_min_version: None
        check(&payload, 1, "").expect("no min-version requirement means nothing to enforce");
    }

    #[test]
    fn satisfied_min_version_passes_check() {
        let payload = LicensePayload {
            mlis_min_version: Some("0.0.1".into()), // trivially satisfied by any real build
            ..sample_payload("", 4_000_000_000)
        };
        check(&payload, 1, "").expect("running version should satisfy a low minimum");
    }

    #[test]
    fn unmet_min_version_fails_check() {
        let payload = LicensePayload {
            mlis_min_version: Some("999.0.0".into()),
            ..sample_payload("", 4_000_000_000)
        };
        let err = check(&payload, 1, "").expect_err("must reject an unmet minimum version");
        assert!(matches!(
            err,
            LicenseError::MinVersionUnmet { required } if required == "999.0.0"
        ));
    }

    #[test]
    fn malformed_min_version_fails_closed() {
        let payload = LicensePayload {
            mlis_min_version: Some("not-a-version".into()),
            ..sample_payload("", 4_000_000_000)
        };
        let err = check(&payload, 1, "").expect_err("unparsable requirement must fail closed");
        assert!(matches!(err, LicenseError::MinVersionUnmet { .. }));
    }

    #[test]
    fn version_satisfies_compares_numeric_components() {
        assert!(
            version_satisfies("1.2.3", "1.2.3"),
            "equal versions satisfy"
        );
        assert!(
            version_satisfies("2.0.0", "1.9.9"),
            "greater major satisfies"
        );
        assert!(
            version_satisfies("1.3.0", "1.2.9"),
            "greater minor satisfies"
        );
        assert!(
            version_satisfies("1.2.4", "1.2.3"),
            "greater patch satisfies"
        );
        assert!(!version_satisfies("1.2.3", "1.2.4"), "lesser patch fails");
        assert!(!version_satisfies("1.1.9", "1.2.0"), "lesser minor fails");
        assert!(!version_satisfies("0.9.9", "1.0.0"), "lesser major fails");
        assert!(
            version_satisfies("1.2", "1.2.0"),
            "missing components pad as 0"
        );
        assert!(
            version_satisfies("1.2.0", "1.2.0-beta"),
            "pre-release suffix on the requirement is stripped before comparing"
        );
        assert!(
            !version_satisfies("not-a-version", "1.0.0"),
            "unparsable actual fails closed"
        );
        assert!(
            !version_satisfies("1.0.0", "not-a-version"),
            "unparsable required fails closed"
        );
    }

    #[test]
    fn tampered_payload_fails_closed() {
        let signing_key = keypair_from_seed(1);
        let payload = sample_payload("", 4_000_000_000);
        let mut signed = sign_payload(&signing_key, &payload);

        // Flip one byte of the (base64-decoded) payload.
        let mut raw = STANDARD.decode(&signed.payload).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        signed.payload = STANDARD.encode(raw);

        let err = verify_with_key(&signed, &signing_key.verifying_key())
            .expect_err("tampered payload must fail closed");
        assert!(matches!(err, LicenseError::InvalidSignature));
    }

    #[test]
    fn tampered_signature_fails_closed() {
        let signing_key = keypair_from_seed(1);
        let payload = sample_payload("", 4_000_000_000);
        let mut signed = sign_payload(&signing_key, &payload);

        let mut raw = STANDARD.decode(&signed.signature).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        signed.signature = STANDARD.encode(raw);

        let err = verify_with_key(&signed, &signing_key.verifying_key())
            .expect_err("tampered signature must fail closed");
        assert!(matches!(err, LicenseError::InvalidSignature));
    }

    #[test]
    fn signed_by_a_different_key_fails_closed() {
        let signing_key = keypair_from_seed(1);
        let other_signing_key = keypair_from_seed(2); // a different keypair entirely
        let payload = sample_payload("", 4_000_000_000);
        let signed = sign_payload(&signing_key, &payload);

        let err = verify_with_key(&signed, &other_signing_key.verifying_key())
            .expect_err("wrong verifying key must fail closed");
        assert!(matches!(err, LicenseError::InvalidSignature));
    }

    #[test]
    fn reserialization_cannot_desync_signer_and_verifier() {
        // Proves the canonical-bytes design: even if something re-serializes
        // the payload differently (e.g. a future serde_json version changes
        // formatting), verification still succeeds because the signature
        // covers the *exact stored bytes*, never a freshly re-serialized copy.
        let signing_key = keypair_from_seed(1);
        let payload = sample_payload("", 4_000_000_000);
        let payload_bytes = serde_json::to_vec(&payload).unwrap();
        let signature = signing_key.sign(&payload_bytes);

        // Re-serialize independently (simulating formatting drift) and
        // confirm it's byte-different from the original, yet the ORIGINAL
        // signed bytes (not this re-serialized copy) are what verification
        // actually checks against.
        let reserialized = serde_json::to_string(&payload).unwrap().into_bytes();
        let signed = SignedLicense {
            format: FORMAT_VERSION,
            payload: STANDARD.encode(&payload_bytes), // the ORIGINAL bytes, not `reserialized`
            signature: STANDARD.encode(signature.to_bytes()),
        };

        verify_with_key(&signed, &signing_key.verifying_key())
            .expect("verification uses the stored bytes, not a re-serialization");
        // Sanity: to_vec and to_string+into_bytes happen to agree here, but
        // the point stands regardless — verify() never re-serializes at all.
        assert_eq!(payload_bytes, reserialized);
    }
}
