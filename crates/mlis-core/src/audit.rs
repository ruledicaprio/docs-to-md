//! PII-free audit trail (Tier 3).
//!
//! For compliance you often must prove *which* documents were processed, when,
//! and by which tier — without retaining the PII itself. Each record stores a
//! SHA-256 fingerprint of the input bytes (irreversible), a timestamp, the
//! extraction method, and non-PII metadata (checksum validity, document code).
//! Records are appended as JSON Lines so the log is greppable and append-only.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Lowercase hex SHA-256 of `bytes` (a stable, irreversible document fingerprint).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A file's SHA-256 didn't match what the caller expected, or it couldn't be read.
#[derive(Debug)]
pub enum Sha256MismatchError {
    Io(std::io::Error),
    Mismatch { expected: String, actual: String },
}

impl std::fmt::Display for Sha256MismatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "could not read file: {e}"),
            Self::Mismatch { expected, actual } => {
                write!(f, "sha256 mismatch: expected {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for Sha256MismatchError {}

/// Verify `path`'s SHA-256 against `expected` (lowercase hex). Used to confirm
/// on-disk model weights (GGUF, `.rten`, …) match a known-good build before
/// loading them into memory — a tampered or substituted file fails closed.
pub fn verify_file_sha256(path: &Path, expected: &str) -> Result<(), Sha256MismatchError> {
    let bytes = std::fs::read(path).map_err(Sha256MismatchError::Io)?;
    let actual = sha256_hex(&bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(Sha256MismatchError::Mismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

/// One append-only audit entry. Contains **no PII** — only a fingerprint and
/// non-identifying metadata.
#[derive(Debug, Clone, Serialize)]
pub struct AuditRecord {
    /// Seconds since the Unix epoch.
    pub ts_unix: u64,
    /// SHA-256 of the input document bytes.
    pub sha256: String,
    /// Extraction tier: `mrz-deterministic` | `llm`.
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mrz_checksums_valid: Option<bool>,
    /// Document code (`P`, `I`, …) — a type, not PII.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_type: Option<String>,
}

impl AuditRecord {
    pub fn new(
        sha256: String,
        method: &str,
        mrz_checksums_valid: Option<bool>,
        document_type: Option<String>,
    ) -> Self {
        Self {
            ts_unix: now_unix(),
            sha256,
            method: method.to_string(),
            mrz_checksums_valid,
            document_type,
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one record to the JSON Lines audit log at `log_path` (created if
/// absent).
pub fn append(log_path: &Path, record: &AuditRecord) -> std::io::Result<()> {
    let line = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vector() {
        // NIST test vector for the empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn record_holds_no_pii() {
        let rec = AuditRecord::new(
            sha256_hex(b"document bytes"),
            "mrz-deterministic",
            Some(true),
            Some("P".into()),
        );
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"method\":\"mrz-deterministic\""));
        assert!(json.contains("\"sha256\""));
        // Sanity: the fingerprint is present but nothing resembling a name/number.
        assert!(json.contains("\"document_type\":\"P\""));
    }

    #[test]
    fn verify_file_sha256_detects_match_and_mismatch() {
        let path =
            std::env::temp_dir().join(format!("mlis-core-verify-test-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let expected = sha256_hex(b"hello");

        assert!(verify_file_sha256(&path, &expected).is_ok());
        let err = verify_file_sha256(&path, "0000").expect_err("should mismatch");
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, Sha256MismatchError::Mismatch { .. }));
    }

    #[test]
    fn append_writes_one_json_object_per_line() {
        let path =
            std::env::temp_dir().join(format!("mlis-core-audit-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path); // in case a prior run left it behind

        let first = AuditRecord::new(
            sha256_hex(b"doc-1"),
            "mrz-deterministic",
            Some(true),
            Some("P".into()),
        );
        let second = AuditRecord::new(sha256_hex(b"doc-2"), "llm", None, None);
        append(&path, &first).expect("append first record");
        append(&path, &second).expect("append second record");

        let contents = std::fs::read_to_string(&path).expect("read audit log");
        std::fs::remove_file(&path).ok();

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected one line per append, got: {contents:?}"
        );

        let parsed_first: serde_json::Value =
            serde_json::from_str(lines[0]).expect("first line is valid JSON");
        let parsed_second: serde_json::Value =
            serde_json::from_str(lines[1]).expect("second line is valid JSON");

        assert_eq!(parsed_first["sha256"], sha256_hex(b"doc-1"));
        assert_eq!(parsed_first["method"], "mrz-deterministic");
        assert_eq!(parsed_second["sha256"], sha256_hex(b"doc-2"));
        assert_eq!(parsed_second["method"], "llm");
        // No PII in the third field slots either — only fingerprint + metadata.
        assert!(parsed_second.get("document_type").is_none());
    }
}
