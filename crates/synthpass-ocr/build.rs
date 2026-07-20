//! Only does real work when the `embedded-models` feature is on. Locates the
//! two already-downloaded `.rten` model files (via `SYNTHPASS_OCR_MODEL_DIR`,
//! default `.`), verifies their SHA-256 against the same known-good hashes
//! `src/verify.rs` checks at runtime for the non-embedded path, and copies
//! them into `OUT_DIR` for `src/embedded.rs`'s `include_bytes!`. Fails the
//! build (fail closed, not silently embedding a wrong/missing file) if a
//! model is absent or its hash doesn't match.
//!
//! CI/local flows already have a "download + checksum-verify the .rten
//! models" step (see `.github/workflows/ci.yml`'s `rust` job) — this reuses
//! that same file layout rather than re-implementing HTTP fetch here.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

include!("src/known_good_hashes.rs");

fn main() {
    if std::env::var("CARGO_FEATURE_EMBEDDED_MODELS").is_err() {
        return;
    }

    // Cargo runs build scripts with the CWD set to *this crate's own*
    // directory (crates/synthpass-ocr), not the workspace root — unlike the
    // runtime `SYNTHPASS_OCR_MODEL_DIR` default in download.rs, which resolves
    // relative to whatever directory the compiled binary is later run from.
    // Default here to the workspace root (two levels up), where the
    // "Download + checksum-verify the OCR .rten models" CI step and local
    // dev workflows both already put the files.
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root (two levels up from crates/synthpass-ocr) exists");
    let model_dir: PathBuf = std::env::var("SYNTHPASS_OCR_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or(workspace_root);
    let out_dir: PathBuf = std::env::var("OUT_DIR")
        .expect("OUT_DIR set by cargo")
        .into();

    embed_one(
        &model_dir.join("text-detection.rten"),
        &out_dir.join("text-detection.rten"),
        "SYNTHPASS_OCR_DETECTION_SHA256",
        KNOWN_GOOD_SHA256_DETECTION,
    );
    embed_one(
        &model_dir.join("text-recognition.rten"),
        &out_dir.join("text-recognition.rten"),
        "SYNTHPASS_OCR_RECOGNITION_SHA256",
        KNOWN_GOOD_SHA256_RECOGNITION,
    );

    println!("cargo:rerun-if-env-changed=SYNTHPASS_OCR_MODEL_DIR");
    println!("cargo:rerun-if-env-changed=SYNTHPASS_OCR_DETECTION_SHA256");
    println!("cargo:rerun-if-env-changed=SYNTHPASS_OCR_RECOGNITION_SHA256");
}

fn embed_one(src: &Path, dst: &Path, override_env: &str, known_good: &str) {
    println!("cargo:rerun-if-changed={}", src.display());

    let bytes = std::fs::read(src).unwrap_or_else(|e| {
        panic!(
            "embedded-models: could not read {} ({e}) — run the model download step \
             (see .github/workflows/ci.yml's \"Download + checksum-verify the OCR .rten models\" \
             step) or set SYNTHPASS_OCR_MODEL_DIR to point at an existing copy",
            src.display()
        )
    });

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hex(&hasher.finalize());
    let expected = std::env::var(override_env).unwrap_or_else(|_| known_good.to_string());
    assert_eq!(
        actual,
        expected,
        "embedded-models: {} SHA-256 mismatch (expected {expected}, got {actual}) — refusing to \
         embed a model that doesn't match the known-good hash",
        src.display()
    );

    std::fs::copy(src, dst).unwrap_or_else(|e| {
        panic!(
            "embedded-models: failed to copy {} to {}: {e}",
            src.display(),
            dst.display()
        )
    });
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
