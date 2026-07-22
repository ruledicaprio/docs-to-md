//! Model file integrity check for the two `.rten` weight files this crate
//! loads, mirroring `synthpass-llm/src/verify.rs`'s GGUF check but run twice (once
//! per detection/recognition model).

use std::path::Path;
use synthpass_core::audit::Sha256MismatchError;

include!("known_good_hashes.rs");

/// [`Sha256MismatchError`] under this crate's established name.
pub type VerifyError = Sha256MismatchError;

/// The hash the detection model must have: `SYNTHPASS_OCR_DETECTION_SHA256` if
/// set, else [`KNOWN_GOOD_SHA256_DETECTION`]. Public so the download path can
/// check bytes *before* committing them to disk (see `download.rs`) using the
/// same override rules this module's file-based checks apply.
pub fn expected_detection_sha256() -> String {
    std::env::var("SYNTHPASS_OCR_DETECTION_SHA256")
        .unwrap_or_else(|_| KNOWN_GOOD_SHA256_DETECTION.into())
}

/// The hash the recognition model must have — see [`expected_detection_sha256`].
pub fn expected_recognition_sha256() -> String {
    std::env::var("SYNTHPASS_OCR_RECOGNITION_SHA256")
        .unwrap_or_else(|_| KNOWN_GOOD_SHA256_RECOGNITION.into())
}

/// Verify `path` (the detection model) against `SYNTHPASS_OCR_DETECTION_SHA256` if
/// set, else [`KNOWN_GOOD_SHA256_DETECTION`].
pub fn verify_detection_model(path: &Path) -> Result<(), VerifyError> {
    synthpass_core::audit::verify_file_sha256(path, &expected_detection_sha256())
}

/// Verify `path` (the recognition model) against `SYNTHPASS_OCR_RECOGNITION_SHA256`
/// if set, else [`KNOWN_GOOD_SHA256_RECOGNITION`].
pub fn verify_recognition_model(path: &Path) -> Result<(), VerifyError> {
    synthpass_core::audit::verify_file_sha256(path, &expected_recognition_sha256())
}

/// Whether verification should be skipped for this run (`SYNTHPASS_OCR_MODEL_SKIP_VERIFY=1`).
pub fn skip_verify() -> bool {
    std::env::var("SYNTHPASS_OCR_MODEL_SKIP_VERIFY").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use synthpass_core::audit::sha256_hex;

    #[test]
    fn detects_mismatch() {
        let path =
            std::env::temp_dir().join(format!("synthpass-ocr-verify-test-{}", std::process::id()));
        std::fs::write(&path, b"not a real rten file").unwrap();
        let err = verify_detection_model(&path).expect_err("should mismatch");
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, VerifyError::Mismatch { .. }));
    }

    #[test]
    fn respects_env_override() {
        let path =
            std::env::temp_dir().join(format!("synthpass-ocr-verify-test2-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let expected = sha256_hex(b"hello");
        // SAFETY: test-only env var mutation, no other thread reads it concurrently in this test binary section.
        unsafe { std::env::set_var("SYNTHPASS_OCR_RECOGNITION_SHA256", &expected) };
        let result = verify_recognition_model(&path);
        unsafe { std::env::remove_var("SYNTHPASS_OCR_RECOGNITION_SHA256") };
        std::fs::remove_file(&path).ok();
        assert!(result.is_ok());
    }
}
