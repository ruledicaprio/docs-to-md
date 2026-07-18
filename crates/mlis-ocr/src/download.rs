//! Fetches the two `.rten` model files this crate needs from the fixed
//! `ocrs-models` S3 bucket when they aren't already present under
//! `MLIS_OCR_MODEL_DIR`. Mirrors the bootstrap `docker/entrypoint-inferer.sh`
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
    download_if_missing(&detection, DETECTION_URL)?;
    download_if_missing(&recognition, RECOGNITION_URL)?;
    Ok((detection, recognition))
}

fn download_if_missing(path: &Path, url: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    let bytes = reqwest::blocking::get(url)
        .and_then(|resp| resp.error_for_status())
        .map_err(|e| format!("failed to fetch {url}: {e}"))?
        .bytes()
        .map_err(|e| format!("failed to read response body from {url}: {e}"))?;

    // Write to a sibling temp file and rename into place, so a crash or
    // interrupted download never leaves a truncated file that looks
    // "present" (and so gets skipped) on the next run.
    let tmp_path = path.with_extension("part");
    std::fs::write(&tmp_path, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("failed to finalize {}: {e}", path.display()))?;
    Ok(())
}
