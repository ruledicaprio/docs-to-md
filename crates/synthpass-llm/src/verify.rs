//! Model file integrity check — the same GGUF sha256 verification the
//! Docker inferer's `entrypoint-inferer.sh` performs after downloading, now
//! run by the Rust binary itself before loading the model into memory.

use std::path::Path;
use synthpass_core::audit::Sha256MismatchError;

/// The Qwen2.5-1.5B-Instruct Q4_K_M GGUF this workspace ships against.
/// Override with `SYNTHPASS_MODEL_SHA256` (e.g. when testing a different model).
pub const KNOWN_GOOD_SHA256: &str =
    "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e";

/// [`Sha256MismatchError`] under this crate's established name.
pub type VerifyError = Sha256MismatchError;

/// Verify `path`'s sha256 against `SYNTHPASS_MODEL_SHA256` if set, else
/// [`KNOWN_GOOD_SHA256`]. Set `SYNTHPASS_MODEL_SKIP_VERIFY=1` to skip (a warning is
/// still worth logging at the call site).
pub fn verify_model(path: &Path) -> Result<(), VerifyError> {
    let expected =
        std::env::var("SYNTHPASS_MODEL_SHA256").unwrap_or_else(|_| KNOWN_GOOD_SHA256.into());
    synthpass_core::audit::verify_file_sha256(path, &expected)
}

/// Whether verification should be skipped for this run (`SYNTHPASS_MODEL_SKIP_VERIFY=1`).
pub fn skip_verify() -> bool {
    std::env::var("SYNTHPASS_MODEL_SKIP_VERIFY").as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use synthpass_core::audit::sha256_hex;

    #[test]
    fn detects_mismatch() {
        let path =
            std::env::temp_dir().join(format!("synthpass-llm-verify-test-{}", std::process::id()));
        std::fs::write(&path, b"not the real model").unwrap();
        let err = verify_model(&path).expect_err("should mismatch");
        std::fs::remove_file(&path).ok();
        assert!(matches!(err, VerifyError::Mismatch { .. }));
    }

    #[test]
    fn respects_env_override() {
        let path =
            std::env::temp_dir().join(format!("synthpass-llm-verify-test2-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let expected = sha256_hex(b"hello");
        // SAFETY: test-only env var mutation, no other thread reads it concurrently in this test binary section.
        unsafe { std::env::set_var("SYNTHPASS_MODEL_SHA256", &expected) };
        let result = verify_model(&path);
        unsafe { std::env::remove_var("SYNTHPASS_MODEL_SHA256") };
        std::fs::remove_file(&path).ok();
        assert!(result.is_ok());
    }
}
