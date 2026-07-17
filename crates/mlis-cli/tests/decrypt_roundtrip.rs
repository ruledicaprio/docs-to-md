//! Black-box tests for `mlis decrypt`: fabricates an AES-256-GCM payload with
//! `mlis_core::crypt::encrypt` (no OCR/pipeline needed, since `decrypt` only
//! touches the ciphertext) and spawns the built `mlis` binary against it.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::path::PathBuf;
use std::process::Command;

/// Removes the fixture file even if an assertion panics mid-test.
struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn random_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    for (i, b) in key.iter_mut().enumerate() {
        // Not cryptographically meaningful here — just needs to differ per test run.
        *b = (std::process::id() as u8)
            .wrapping_add(i as u8)
            .wrapping_mul(31);
    }
    key
}

fn write_fixture(name: &str, ciphertext: &[u8]) -> TempFileGuard {
    let path = std::env::temp_dir().join(format!(
        "mlis-cli-test-{name}-{}.json.enc",
        std::process::id()
    ));
    std::fs::write(&path, ciphertext).expect("write encrypted fixture");
    TempFileGuard(path)
}

#[test]
fn decrypt_roundtrips_encrypted_payload() {
    let key = random_key();
    let plaintext = br#"{"surname":"DOE","document_number":"X1234567"}"#;
    let ciphertext = mlis_core::crypt::encrypt(&key, plaintext).expect("encrypt fixture");
    let fixture = write_fixture("roundtrip", &ciphertext);

    let output = Command::new(env!("CARGO_BIN_EXE_mlis"))
        .args(["decrypt", fixture.0.to_str().unwrap()])
        .env("MLIS_KEY", STANDARD.encode(key))
        .output()
        .expect("run `mlis decrypt`");

    assert!(
        output.status.success(),
        "decrypt exited non-zero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, plaintext);
}

#[test]
fn decrypt_fails_with_wrong_key() {
    let key = random_key();
    let mut wrong_key = key;
    wrong_key[0] ^= 0xFF;

    let plaintext = b"top secret extraction JSON";
    let ciphertext = mlis_core::crypt::encrypt(&key, plaintext).expect("encrypt fixture");
    let fixture = write_fixture("wrongkey", &ciphertext);

    let output = Command::new(env!("CARGO_BIN_EXE_mlis"))
        .args(["decrypt", fixture.0.to_str().unwrap()])
        .env("MLIS_KEY", STANDARD.encode(wrong_key))
        .output()
        .expect("run `mlis decrypt`");

    // `decrypt_command` prints the error to stderr and returns Ok(()), so the
    // exit code stays 0 — what matters is that no plaintext ever reaches stdout.
    assert!(output.stdout.is_empty(), "plaintext leaked on wrong key");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("decrypt failed"),
        "expected a decrypt-failed message on stderr, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
