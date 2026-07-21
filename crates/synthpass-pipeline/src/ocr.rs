//! OCR engine abstraction: the pipeline gets Markdown from *some* engine.
//! Image-only as of v0.7.5 — the Docker-based `docling-serve` engine (the
//! project's only PDF-capable path) was retired along with PDF input support;
//! see `CHANGELOG.md`.
//!
//! [`RustOcrEngine`] (Cargo feature `ocr-native-rust`, **default**) runs the
//! pure-Rust `ocrs`/`rten` engine in-process — no Docker, no Python, no C
//! libraries, works unchanged on native Windows. It has been the only OCR
//! engine since v1.2.0: the Tesseract-based `ocr-daemon` fallback (Linux/WSL
//! only, C library chain) was retired once the pure-Rust engine's corpus-
//! measured Tier-1 hit rate reached 100% in v1.1.0 — see CHANGELOG.
//!
//! Supported input: JPEG, PNG, WebP, TIFF, BMP, GIF (whatever the `image`
//! crate's default features decode). Not supported: PDF (see above) and
//! HEIC/HEIF (Apple's default photo format) — no permissively-licensed
//! pure-Rust decoder exists yet (the two that do are AGPL-3.0), so HEIC/HEIF
//! input is rejected with a clear message rather than silently failing or
//! pulling in AGPL code. See `docs/ARCHITECTURE.md`'s "Supported input
//! formats" note.

use crate::PipelineError;
use async_trait::async_trait;
use std::path::Path;

#[cfg(not(feature = "ocr-native-rust"))]
compile_error!("synthpass-pipeline requires the `ocr-native-rust` feature");

/// Axis-aligned bounding box in source-image pixel coordinates (`x`/`y` =
/// top-left corner, `w`/`h` = extent), plain `f32`s. **Pipeline-owned, not a
/// re-export of `synthpass_ocr::geometry::BBox`** — `synthpass-ocr` is only a
/// dependency of this crate behind the optional `ocr-native-rust` feature
/// (see this crate's `Cargo.toml`), so any of its types leaking into
/// [`OcrEngine`]'s public signature would make every other feature
/// combination (an out-of-tree `OcrEngine` impl with `ocr-native-rust` off)
/// fail to compile. [`RustOcrEngine`]'s `recognize_detailed` override is the
/// one place a `synthpass_ocr::geometry::BBox` is converted into this type —
/// see its body.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Structured OCR output: recognized text plus the layout geometry
/// `synthpass-ocr`'s `NativeOcr::recognize_detailed` can produce when the
/// active engine supports it. **Pipeline-owned** — see [`BBox`]'s doc for why
/// this can never contain a `synthpass-ocr` type.
#[derive(Debug, Clone, Default)]
pub struct OcrResult {
    pub text: String,
    /// The region scored as the MRZ zone by content-and-geometry heuristic,
    /// if any scored high enough to be confident. Not yet consumed by the
    /// pipeline (Tier 1 still finds the MRZ by pattern search over `text`,
    /// unchanged) — carried here so a future caller doesn't need another
    /// trait-widening round to get at it.
    pub mrz_band: Option<BBox>,
    /// The region scored as the ID photo. **Crop coordinates only** — VISION.md
    /// §2's permanent non-goal applies here exactly as it does at the
    /// [`Pipeline::process_document`](crate::Pipeline::process_document)
    /// call site that consumes this field: no face recognition, no
    /// biometric matching, ever — a bounding box is not a face.
    pub portrait: Option<BBox>,
    /// Rotation (degrees, clockwise) applied before recognition; `0` if the
    /// engine didn't rotate (either because the page needed none, or because
    /// the active engine doesn't support orientation detection at all).
    pub rotation: u16,
}

impl OcrResult {
    /// Wrap plain text with no geometry — the shape every engine that only
    /// implements [`OcrEngine::to_markdown`] gets for free via
    /// [`OcrEngine::recognize_detailed`]'s default body.
    pub fn from_text(text: String) -> Self {
        Self {
            text,
            mrz_band: None,
            portrait: None,
            rotation: 0,
        }
    }
}

/// Produces Markdown / plain text from a local image.
#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError>;

    /// [`to_markdown`](Self::to_markdown)'s richer sibling: same text, plus
    /// layout geometry when the engine can produce it. **Additive, not
    /// breaking**: the default body just wraps `to_markdown`'s output with
    /// [`OcrResult::from_text`], so every existing [`OcrEngine`] impl —
    /// in-tree or out-of-tree — keeps compiling and behaving exactly as
    /// before without touching a line. Only [`RustOcrEngine`] overrides this;
    /// see its impl for where `synthpass_ocr::geometry::BBox` gets converted
    /// to this crate's own [`BBox`].
    async fn recognize_detailed(&self, input: &Path) -> Result<OcrResult, PipelineError> {
        Ok(OcrResult::from_text(self.to_markdown(input).await?))
    }

    /// Short human-readable identity for logs.
    fn describe(&self) -> String;
}

/// Pure-Rust `ocrs`/`rten` engine (feature `ocr-native-rust`, **default**).
/// Lazy-loads both `.rten` weight files on first call and keeps the engine
/// warm for the process lifetime, mirroring `NativeInferer` in `infer.rs`.
/// PDF and HEIC/HEIF input are rejected with a clear, actionable message
/// (see [`OcrEngine::to_markdown`] below) rather than a generic failure.
mod rust_ocr {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    pub struct RustOcrEngine {
        model_dir: PathBuf,
        // Only read on the non-`ocr-embedded` load path (see `get_or_load`
        // below) — genuinely unused when models are compiled in instead.
        #[cfg_attr(feature = "ocr-embedded", allow(dead_code))]
        auto_download: bool,
        inner: OnceCell<Arc<synthpass_ocr::NativeOcr>>,
    }

    impl RustOcrEngine {
        pub fn new(model_dir: impl Into<PathBuf>, auto_download: bool) -> Self {
            Self {
                model_dir: model_dir.into(),
                auto_download,
                inner: OnceCell::new(),
            }
        }

        pub fn model_dir(&self) -> &std::path::Path {
            &self.model_dir
        }

        async fn get_or_load(&self) -> Result<Arc<synthpass_ocr::NativeOcr>, String> {
            self.inner
                .get_or_try_init(|| async {
                    #[cfg(feature = "ocr-embedded")]
                    {
                        // No filesystem/network access at all — the models
                        // are already in the binary (see synthpass-ocr/build.rs).
                        return tokio::task::spawn_blocking(
                            synthpass_ocr::NativeOcr::load_embedded,
                        )
                        .await
                        .map_err(|e| format!("ocr model load task panicked: {e}"))?
                        .map(Arc::new);
                    }
                    #[cfg(not(feature = "ocr-embedded"))]
                    {
                        let model_dir = self.model_dir.clone();
                        let auto_download = self.auto_download;
                        tokio::task::spawn_blocking(move || {
                            let (detection, recognition) = if auto_download {
                                synthpass_ocr::download::ensure_models(&model_dir)?
                            } else {
                                (
                                    model_dir.join(synthpass_ocr::download::DETECTION_FILENAME),
                                    model_dir.join(synthpass_ocr::download::RECOGNITION_FILENAME),
                                )
                            };
                            // Verify on the actual load path, not just in `synthpass doctor` —
                            // a tampered or corrupted-but-complete download (whether
                            // fetched just now or cached from a previous run) must fail
                            // closed before it's ever loaded into the OCR engine.
                            if !synthpass_ocr::verify::skip_verify() {
                                synthpass_ocr::verify::verify_detection_model(&detection)
                                    .map_err(|e| e.to_string())?;
                                synthpass_ocr::verify::verify_recognition_model(&recognition)
                                    .map_err(|e| e.to_string())?;
                            }
                            synthpass_ocr::NativeOcr::load(&detection, &recognition)
                        })
                        .await
                        .map_err(|e| format!("ocr model load task panicked: {e}"))?
                        .map(Arc::new)
                    }
                })
                .await
                .cloned()
        }
    }

    /// True if `path`'s extension looks like a raster image `ocrs` can read.
    fn looks_like_image(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .is_some_and(|e| {
                matches!(
                    e.as_str(),
                    "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" | "gif"
                )
            })
    }

    /// True if `path`'s extension is HEIC/HEIF — checked ahead of the general
    /// image allowlist so the rejection message names the real reason (no
    /// permissively-licensed pure-Rust decoder) instead of a generic
    /// "unsupported format" error.
    fn looks_like_heic(path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .is_some_and(|e| matches!(e.as_str(), "heic" | "heif"))
    }

    /// The HEIC/PDF rejection checks shared by [`OcrEngine::to_markdown`] and
    /// [`OcrEngine::recognize_detailed`] below — pulled out so the two engine
    /// entry points can never drift on which inputs they reject or why.
    fn reject_unsupported_input(input: &Path) -> Result<(), PipelineError> {
        if looks_like_heic(input) {
            return Err(PipelineError::Ocr(
                "HEIC/HEIF input is not yet supported — no permissively-licensed \
                 pure-Rust decoder exists (the available ones are AGPL-3.0). Convert to \
                 JPEG or PNG first."
                    .into(),
            ));
        }
        if !looks_like_image(input) {
            return Err(PipelineError::Ocr(
                "PDF input is not supported — synthpass is image-only as of v0.7.5. Convert \
                 to an image first."
                    .into(),
            ));
        }
        Ok(())
    }

    /// Convert `synthpass-ocr`'s owned geometry type into this crate's own
    /// [`BBox`] — the one place a `synthpass_ocr::geometry::BBox` is named
    /// anywhere in `synthpass-pipeline`, confined to this `ocr-native-rust`-gated
    /// module so it never reaches [`OcrEngine`]'s public signature (see
    /// [`BBox`]'s doc for why that boundary matters).
    fn convert_bbox(b: synthpass_ocr::geometry::BBox) -> BBox {
        BBox {
            x: b.x,
            y: b.y,
            w: b.w,
            h: b.h,
        }
    }

    #[async_trait]
    impl OcrEngine for RustOcrEngine {
        async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError> {
            reject_unsupported_input(input)?;
            let ocr = self.get_or_load().await.map_err(PipelineError::Ocr)?;
            let path = input.to_path_buf();
            tokio::task::spawn_blocking(move || ocr.recognize(&path))
                .await
                .map_err(|e| PipelineError::Ocr(format!("ocr task panicked: {e}")))?
                .map_err(PipelineError::Ocr)
        }

        async fn recognize_detailed(&self, input: &Path) -> Result<OcrResult, PipelineError> {
            reject_unsupported_input(input)?;
            let ocr = self.get_or_load().await.map_err(PipelineError::Ocr)?;
            let path = input.to_path_buf();
            let page = tokio::task::spawn_blocking(move || ocr.recognize_detailed(&path))
                .await
                .map_err(|e| PipelineError::Ocr(format!("ocr task panicked: {e}")))?
                .map_err(PipelineError::Ocr)?;
            Ok(OcrResult {
                text: page.text,
                mrz_band: page.mrz_band.map(convert_bbox),
                portrait: page.portrait.map(convert_bbox),
                rotation: page.rotation,
            })
        }

        fn describe(&self) -> String {
            format!("pure-rust ocr (ocrs/rten) @ {}", self.model_dir.display())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // These don't need real model files: both rejections happen on the
        // extension check, before `get_or_load()` ever runs — the path need
        // not even exist.

        #[tokio::test]
        async fn rejects_pdf_with_an_actionable_message() {
            let engine = RustOcrEngine::new(".", false);
            let err = engine
                .to_markdown(Path::new("document.pdf"))
                .await
                .expect_err("PDF input must be rejected, not silently processed");
            let PipelineError::Ocr(msg) = err else {
                panic!("expected PipelineError::Ocr");
            };
            assert!(msg.contains("PDF"), "message should name PDF: {msg}");
            assert!(
                msg.contains("image-only"),
                "message should explain why: {msg}"
            );
        }

        #[tokio::test]
        async fn rejects_heic_with_an_actionable_message() {
            let engine = RustOcrEngine::new(".", false);
            for ext in ["heic", "heif", "HEIC"] {
                let err = engine
                    .to_markdown(Path::new(&format!("photo.{ext}")))
                    .await
                    .expect_err("HEIC/HEIF input must be rejected, not silently processed");
                let PipelineError::Ocr(msg) = err else {
                    panic!("expected PipelineError::Ocr");
                };
                assert!(
                    msg.contains("HEIC") || msg.contains("HEIF"),
                    "message should name HEIC/HEIF: {msg}"
                );
                assert!(
                    msg.contains("JPEG") || msg.contains("PNG"),
                    "message should suggest a conversion target: {msg}"
                );
            }
        }
    }
}
pub use rust_ocr::RustOcrEngine;

/// Default model directory when `SYNTHPASS_OCR_MODEL_DIR` is unset — the repo root.
const DEFAULT_OCR_MODEL_DIR: &str = ".";

/// Build the OCR engine. `SYNTHPASS_OCR_ENGINE` survives for compatibility, but
/// `rust` is the only engine since v1.2.0 (the Tesseract-based `native`
/// engine was retired) — any other value warns and uses the pure-Rust engine.
pub fn engine_from_env() -> Box<dyn OcrEngine> {
    let requested = std::env::var("SYNTHPASS_OCR_ENGINE").unwrap_or_else(|_| "rust".into());
    if requested != "rust" {
        tracing::warn!(
            requested = %requested,
            "SYNTHPASS_OCR_ENGINE is no longer available (the Tesseract `native` engine was \
             retired in v1.2.0) — using the pure-Rust engine"
        );
    }
    let model_dir =
        std::env::var("SYNTHPASS_OCR_MODEL_DIR").unwrap_or_else(|_| DEFAULT_OCR_MODEL_DIR.into());
    let auto_download = std::env::var("SYNTHPASS_OCR_AUTO_DOWNLOAD").as_deref() != Ok("0");
    Box::new(RustOcrEngine::new(model_dir, auto_download))
}
