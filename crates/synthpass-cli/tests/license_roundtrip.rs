//! Black-box tests for `synthpass fingerprint` / `synthpass verify-license` / the
//! extraction path's license gate. Patterned on `decrypt_roundtrip.rs`:
//! fabricates signed license files directly with a plain `ed25519-dalek`
//! keypair (NOT via `synthpass-license`'s `vendor` feature — `synthpass-cli` must
//! never build with that active, in any mode, so the shipped binary's
//! dependency tree stays provably signing-free) and spawns the built `synthpass`
//! binary against them, injecting the matching verifying key via
//! `SYNTHPASS_LICENSE_PUBKEY`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use std::path::PathBuf;
use std::process::Command;
use synthpass_license::{LicensePayload, SignedLicense, FORMAT_VERSION};

/// Removes the fixture file even if an assertion panics mid-test.
struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A deterministic test keypair — no RNG dependency needed, `from_bytes` is
/// plain key-material construction (any 32 bytes is a valid Ed25519 seed).
fn keypair() -> (SigningKey, String) {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let pubkey_b64 = STANDARD.encode(signing_key.verifying_key().to_bytes());
    (signing_key, pubkey_b64)
}

fn sign(signing_key: &SigningKey, payload: &LicensePayload) -> SignedLicense {
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
        license_id: "test-license".into(),
        customer: "Test Customer".into(),
        hw_fingerprint: hw_fingerprint.into(),
        issued_unix: 1_700_000_000,
        expires_unix,
        tier: "enterprise".into(),
        features: vec![],
        mlis_min_version: None,
    }
}

fn write_license_fixture(name: &str, signed: &SignedLicense) -> TempFileGuard {
    let path = std::env::temp_dir().join(format!(
        "synthpass-cli-test-{name}-{}.mlis",
        std::process::id()
    ));
    let json = serde_json::to_string_pretty(signed).expect("SignedLicense serializes");
    std::fs::write(&path, json).expect("write license fixture");
    TempFileGuard(path)
}

#[test]
fn fingerprint_is_stable_and_non_empty() {
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_synthpass"))
            .arg("fingerprint")
            .output()
            .expect("run `synthpass fingerprint`")
    };
    let (a, b) = (run(), run());
    assert!(a.status.success());
    assert!(!a.stdout.is_empty());
    assert_eq!(a.stdout, b.stdout, "fingerprint must be stable across runs");
}

#[test]
fn verify_license_accepts_a_valid_unbound_license() {
    let (signing_key, pubkey_b64) = keypair();
    let signed = sign(&signing_key, &sample_payload("", 4_000_000_000));
    let fixture = write_license_fixture("valid", &signed);

    let output = Command::new(env!("CARGO_BIN_EXE_synthpass"))
        .args(["verify-license", fixture.0.to_str().unwrap()])
        .env("SYNTHPASS_LICENSE_PUBKEY", &pubkey_b64)
        .output()
        .expect("run `synthpass verify-license`");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("enterprise"),
        "expected tier in output: {stdout}"
    );
}

#[test]
fn verify_license_rejects_an_expired_license() {
    let (signing_key, pubkey_b64) = keypair();
    let signed = sign(&signing_key, &sample_payload("", 100)); // expired long ago
    let fixture = write_license_fixture("expired", &signed);

    let output = Command::new(env!("CARGO_BIN_EXE_synthpass"))
        .args(["verify-license", fixture.0.to_str().unwrap()])
        .env("SYNTHPASS_LICENSE_PUBKEY", &pubkey_b64)
        .output()
        .expect("run `synthpass verify-license`");

    assert!(
        !output.status.success(),
        "expired license must exit non-zero"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expired"),
        "expected an expiry message on stderr, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn verify_license_fails_closed_on_a_tampered_file() {
    let (signing_key, pubkey_b64) = keypair();
    let mut signed = sign(&signing_key, &sample_payload("", 4_000_000_000));

    // Flip one byte of the (base64-decoded) payload.
    let mut raw = STANDARD.decode(&signed.payload).unwrap();
    let last = raw.len() - 1;
    raw[last] ^= 0xff;
    signed.payload = STANDARD.encode(raw);

    let fixture = write_license_fixture("tampered", &signed);

    let output = Command::new(env!("CARGO_BIN_EXE_synthpass"))
        .args(["verify-license", fixture.0.to_str().unwrap()])
        .env("SYNTHPASS_LICENSE_PUBKEY", &pubkey_b64)
        .output()
        .expect("run `synthpass verify-license`");

    assert!(
        !output.status.success(),
        "tampered license must fail closed"
    );
    assert!(
        output.stdout.is_empty(),
        "no license data should leak on a tampered file"
    );
}

#[test]
fn extraction_path_refuses_expired_license_but_skip_bypasses_the_gate() {
    let (signing_key, pubkey_b64) = keypair();
    let signed = sign(&signing_key, &sample_payload("", 100)); // expired
    let license_fixture = write_license_fixture("extract-gate", &signed);

    // A dummy input file that just needs to exist — the license gate runs
    // before any OCR/pipeline work touches its content.
    let input_path = std::env::temp_dir().join(format!(
        "synthpass-cli-test-dummy-input-{}.jpg",
        std::process::id()
    ));
    std::fs::write(&input_path, b"not a real image").expect("write dummy input");
    let _input_guard = TempFileGuard(input_path.clone());

    // Without skip: refused, and the message names the license, not some
    // downstream OCR/pipeline failure.
    let output = Command::new(env!("CARGO_BIN_EXE_synthpass"))
        .arg(input_path.to_str().unwrap())
        .env("SYNTHPASS_LICENSE_PUBKEY", &pubkey_b64)
        .env(
            "SYNTHPASS_LICENSE_PATH",
            license_fixture.0.to_str().unwrap(),
        )
        .output()
        .expect("run `synthpass <file>`");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("license"),
        "expected a license-related refusal, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // With SYNTHPASS_LICENSE_SKIP=1: the gate is bypassed. It will still fail
    // downstream (no real OCR models staged in this test environment), but
    // that failure must not be the license message — and must stay local
    // (no network), so point the OCR engine at an empty dir with
    // auto-download off rather than let it try to fetch real models.
    let empty_model_dir = std::env::temp_dir().join(format!(
        "synthpass-cli-test-empty-model-dir-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&empty_model_dir).expect("create empty model dir");

    let output = Command::new(env!("CARGO_BIN_EXE_synthpass"))
        .arg(input_path.to_str().unwrap())
        .env("SYNTHPASS_LICENSE_PUBKEY", &pubkey_b64)
        .env(
            "SYNTHPASS_LICENSE_PATH",
            license_fixture.0.to_str().unwrap(),
        )
        .env("SYNTHPASS_LICENSE_SKIP", "1")
        .env("SYNTHPASS_OCR_AUTO_DOWNLOAD", "0")
        .env("SYNTHPASS_OCR_MODEL_DIR", &empty_model_dir)
        .output()
        .expect("run `synthpass <file>` with SYNTHPASS_LICENSE_SKIP=1");

    std::fs::remove_dir_all(&empty_model_dir).ok();

    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("license check failed"),
        "SYNTHPASS_LICENSE_SKIP=1 should bypass the license gate entirely, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
