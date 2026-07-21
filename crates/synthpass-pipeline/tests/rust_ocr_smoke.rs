//! Smoke test: the pure-Rust `ocrs`/`rten` OCR engine (feature
//! `ocr-native-rust`) drives the pipeline end-to-end against a real sample
//! image, with no Docker/Python running at all — proving the "zero Docker by
//! default" claim. Requires the two real `.rten` model files (downloaded +
//! checksum-verified by CI's `rust` job); ignored by default since a plain
//! `cargo test` shouldn't need network access or ~12 MB of model weights.
//!
//! This does NOT assert `Method::MrzDeterministic` is reached. Manual runs
//! against this workspace's specimen samples (600x421 and 2000x2666 JPEGs)
//! show `ocrs`'s out-of-the-box accuracy is not yet clean enough to
//! reconstruct a checksum-valid MRZ line reliably — filler runs get
//! mis-recognized or truncated (see `docs/ARCHITECTURE.md`'s honest
//! limitations note). Tier 1's `mrz` crate already tolerates common OCR
//! lookalike errors (`K`/`L` for `<`, short lines), but not every failure
//! mode. So this test proves the weaker, still-meaningful claim: the OCR
//! stage itself succeeds and the pipeline reaches a terminal result (either
//! tier) rather than an OCR-stage error.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use synthpass_core::Extraction;
use synthpass_pipeline::{InferBackend, Pipeline, ProcessEvent, RustOcrEngine};
use tokio::sync::mpsc;

/// Locates `name` (a bare filename, no path) anywhere under `samples/`,
/// searching recursively — so this survives `samples/` being reorganized
/// into continent/class subfolders without every call site needing the
/// exact subpath hardcoded.
fn find_sample(name: &str) -> PathBuf {
    fn search(dir: &Path, name: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = search(&path, name) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|f| f.to_str()) == Some(name) {
                return Some(path);
            }
        }
        None
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    search(&repo_root.join("samples"), name)
        .unwrap_or_else(|| panic!("sample file not found anywhere under samples/: {name}"))
}

/// A trivial Tier-2 backend so this test can complete even when Tier 1
/// misses (see module docs) — this test isn't about Tier-2 accuracy.
struct StubInferer;

#[async_trait]
impl InferBackend for StubInferer {
    async fn extract(&self, _markdown: &str) -> Result<Extraction, String> {
        Ok(Extraction::default())
    }

    async fn extract_stream(
        &self,
        _markdown: &str,
        _tx: &mpsc::Sender<ProcessEvent>,
    ) -> Result<Extraction, String> {
        Ok(Extraction::default())
    }

    fn describe(&self) -> String {
        "stub (test)".into()
    }

    async fn health(&self) -> Result<String, String> {
        Ok("n/a".into())
    }
}

#[tokio::test]
#[ignore = "needs the real .rten model files (~12 MB) at the repo root — see \
            CI's `rust` job, or download text-detection.rten/text-recognition.rten \
            from https://ocrs-models.s3-accelerate.amazonaws.com/"]
async fn rust_ocr_engine_reaches_a_terminal_pipeline_result() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ocr = RustOcrEngine::new(repo_root.clone(), false);
    let pipeline = Pipeline::new(Box::new(ocr), Box::new(StubInferer));

    // Copy the sample into a scratch dir so this test's `.md`/`.json` output
    // never lands in the tracked `samples/` directory.
    let src = find_sample("Croatian_passport_data_page.jpg");
    let dst = std::env::temp_dir().join(format!(
        "synthpass-pipeline-rust-ocr-smoke-{}.jpg",
        std::process::id()
    ));
    std::fs::copy(&src, &dst).expect("sample image exists and is copyable");

    let result = pipeline.process_document(&dst).await;

    std::fs::remove_file(&dst).ok();
    std::fs::remove_file(dst.with_extension("md")).ok();
    std::fs::remove_file(dst.with_extension("json")).ok();

    let result = result.expect("pipeline should reach a terminal result via the native-rust OCR engine, not an OCR-stage error");
    assert!(
        !result.markdown.is_empty(),
        "expected non-empty OCR markdown output"
    );
    assert!(
        result.extracted.is_some(),
        "expected an extraction from one of the two tiers"
    );
}

/// Added in v0.7.5 alongside dropping PDF/docling support: proves JPEG, PNG,
/// and WebP all flow through the OCR engine end-to-end, not just that
/// `image::load` accepts them in isolation — these three plus TIFF/BMP/GIF
/// (untested here, same `image` crate decode path) are the phone-common
/// formats this milestone confirmed support for. See `docs/ARCHITECTURE.md`'s
/// "Supported input formats" note. HEIC/HEIF is deliberately NOT covered here
/// — it's meant to be rejected, not decoded (see `ocr.rs`'s `looks_like_heic`).
#[tokio::test]
#[ignore = "needs the real .rten model files (~12 MB) at the repo root — see \
            CI's `rust` job, or download text-detection.rten/text-recognition.rten \
            from https://ocrs-models.s3-accelerate.amazonaws.com/"]
async fn rust_ocr_engine_handles_common_phone_image_formats() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let src = find_sample("Croatian_passport_data_page.jpg");
    let img = image::open(&src).expect("sample image decodes");

    for (format, ext) in [
        (image::ImageFormat::Jpeg, "jpg"),
        (image::ImageFormat::Png, "png"),
        (image::ImageFormat::WebP, "webp"),
    ] {
        let ocr = RustOcrEngine::new(repo_root.clone(), false);
        let pipeline = Pipeline::new(Box::new(ocr), Box::new(StubInferer));

        let dst = std::env::temp_dir().join(format!(
            "synthpass-pipeline-format-smoke-{}-{ext}.{ext}",
            std::process::id()
        ));
        img.save_with_format(&dst, format)
            .unwrap_or_else(|e| panic!("re-encoding sample to {ext} should succeed: {e}"));

        let result = pipeline.process_document(&dst).await;

        std::fs::remove_file(&dst).ok();
        std::fs::remove_file(dst.with_extension("md")).ok();
        std::fs::remove_file(dst.with_extension("json")).ok();

        let result = result.unwrap_or_else(|e| {
            panic!("{ext} input should reach a terminal result, not an OCR-stage error: {e}")
        });
        assert!(
            !result.markdown.is_empty(),
            "expected non-empty OCR markdown output for {ext}"
        );
    }
}
