//! Fetches the two `.rten` model files this crate needs from the fixed
//! `ocrs-models` S3 bucket when they aren't already present under
//! `SYNTHPASS_OCR_MODEL_DIR`. Mirrors the bootstrap `docker/entrypoint-inferer.sh`
//! used to perform for the GGUF — there's no Docker init step to hide this in
//! for the *default* OCR path anymore, so it runs in-process instead.

use std::path::{Path, PathBuf};

const DETECTION_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten";
const RECOGNITION_URL: &str =
    "https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten";

pub const DETECTION_FILENAME: &str = "text-detection.rten";
pub const RECOGNITION_FILENAME: &str = "text-recognition.rten";

/// Ensure both model files exist under `model_dir`, downloading whichever are
/// missing, and return their paths. Blocking (`reqwest::blocking`) — callers
/// on an async runtime must run this via `spawn_blocking`.
pub fn ensure_models(model_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    let detection = model_dir.join(DETECTION_FILENAME);
    let recognition = model_dir.join(RECOGNITION_FILENAME);
    download_if_missing(
        &detection,
        DETECTION_URL,
        crate::verify::expected_detection_sha256,
    )?;
    download_if_missing(
        &recognition,
        RECOGNITION_URL,
        crate::verify::expected_recognition_sha256,
    )?;
    Ok((detection, recognition))
}

fn download_if_missing(
    path: &Path,
    url: &str,
    expected_sha256: impl FnOnce() -> String,
) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let bytes = reqwest::blocking::get(url)
        .and_then(|resp| resp.error_for_status())
        .map_err(|e| format!("failed to fetch {url}: {e}"))?
        .bytes()
        .map_err(|e| format!("failed to read response body from {url}: {e}"))?;

    commit_verified(path, &bytes, url, expected_sha256)
}

/// Write `bytes` to a sibling temp file, check them against the known-good hash,
/// and only then rename into `path`.
///
/// Writing to a temp file first means a crash or interrupted download never
/// leaves a truncated file that looks "present" (and so gets skipped) on the
/// next run. Checking *before* the rename covers the matching case: the load
/// path in synthpass-pipeline verifies again and is the real security boundary,
/// but a bad download that had already taken the final filename would be
/// skipped by `download_if_missing`'s `path.exists()` check on every later run
/// — the cache would stay poisoned until someone deleted the file by hand.
/// Discarding the temp file keeps a transient bad fetch retryable.
fn commit_verified(
    path: &Path,
    bytes: &[u8],
    source: &str,
    expected_sha256: impl FnOnce() -> String,
) -> Result<(), String> {
    let tmp_path = path.with_extension("part");
    std::fs::write(&tmp_path, bytes)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;

    if !crate::verify::skip_verify() {
        let expected = expected_sha256();
        let actual = synthpass_core::audit::sha256_hex(bytes);
        if actual != expected {
            std::fs::remove_file(&tmp_path).ok();
            return Err(format!(
                "downloaded {source} but its SHA-256 does not match the known-good hash \
                 (expected {expected}, got {actual}) — discarded the partial file, \
                 so re-running will retry the download"
            ));
        }
    }

    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("failed to finalize {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "synthpass-ocr-download-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A download whose bytes don't match must leave *nothing* behind — neither
    /// the final file (which would poison the cache, since `download_if_missing`
    /// skips any path that exists) nor the `.part` scratch file.
    #[test]
    fn bad_bytes_are_discarded_not_committed() {
        let dir = tmp_dir("bad");
        let path = dir.join(DETECTION_FILENAME);

        let err = commit_verified(&path, b"not a real rten file", "test://detection", || {
            crate::verify::KNOWN_GOOD_SHA256_DETECTION.to_string()
        })
        .expect_err("hash mismatch should be rejected");

        assert!(err.contains("does not match the known-good hash"), "{err}");
        assert!(!path.exists(), "bad download must not take the final name");
        assert!(
            !path.with_extension("part").exists(),
            "temp file must be cleaned up"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Matching bytes are renamed into place and readable.
    #[test]
    fn good_bytes_are_committed() {
        let dir = tmp_dir("good");
        let path = dir.join(RECOGNITION_FILENAME);
        let payload = b"pretend model bytes";
        let expected = synthpass_core::audit::sha256_hex(payload);

        commit_verified(&path, payload, "test://recognition", || expected)
            .expect("matching hash should commit");

        assert_eq!(std::fs::read(&path).unwrap(), payload);
        assert!(!path.with_extension("part").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
