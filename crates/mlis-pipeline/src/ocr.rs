//! OCR engine abstraction: the pipeline gets Markdown from *some* engine.
//!
//! [`RustOcrEngine`] (Cargo feature `ocr-native-rust`, **default**) runs the
//! pure-Rust `ocrs`/`rten` engine in-process — no Docker, no Python, no C
//! libraries, works unchanged on native Windows. Image-only: `ocrs` has no
//! PDF parsing. [`DoclingEngine`] (the containerized `docling-serve` service)
//! is the portable fallback and the only engine that handles PDFs.
//! [`NativeEngine`] (Cargo feature `native-ocr`, Linux/WSL only) drives the
//! in-tree `ocr-daemon` (Tesseract + Leptonica) in-process, kept as an
//! accuracy fallback with proven confidence-scored orientation correction.
//! The engine is chosen at runtime by `MLIS_OCR_ENGINE` (`rust` | `docling` |
//! `native`).

use crate::PipelineError;
use async_trait::async_trait;
use docling_rs::DoclingClient;
use std::path::Path;

/// Produces Markdown / plain text from a local image or PDF.
#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError>;
    /// Short human-readable identity for logs.
    fn describe(&self) -> String;
}

/// Default engine: the containerized `docling-serve` OCR service. Layout-aware,
/// handles PDFs, runs on every platform.
pub struct DoclingEngine {
    client: DoclingClient,
    url: String,
}

impl DoclingEngine {
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            client: DoclingClient::new(url.clone()),
            url,
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl OcrEngine for DoclingEngine {
    async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError> {
        let result = self
            .client
            .convert_file(&[input], None, None)
            .await
            .map_err(|e| PipelineError::Ocr(format!("docling-serve: {e:?}")))?;
        result
            .document
            .md_content
            .clone()
            .ok_or_else(|| PipelineError::NoMarkdown(format!("{:?}", result.errors)))
    }

    fn describe(&self) -> String {
        format!("docling-serve @ {}", self.url)
    }
}

/// Native Tesseract + Leptonica engine (Linux/WSL only, `native-ocr` feature).
/// Runs the blocking OCR on a dedicated thread so it never stalls the async
/// runtime. Image-focused (no PDF); use docling for PDFs.
#[cfg(feature = "native-ocr")]
pub struct NativeEngine {
    lang: String,
}

#[cfg(feature = "native-ocr")]
impl NativeEngine {
    pub fn new(lang: impl Into<String>) -> Self {
        Self { lang: lang.into() }
    }

    pub fn from_env() -> Self {
        Self::new(std::env::var("MLIS_OCR_LANG").unwrap_or_else(|_| "eng".into()))
    }
}

#[cfg(feature = "native-ocr")]
#[async_trait]
impl OcrEngine for NativeEngine {
    async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError> {
        let path = input.to_path_buf();
        let lang = self.lang.clone();
        tokio::task::spawn_blocking(move || ocr_daemon::image_to_text(&path, &lang))
            .await
            .map_err(|e| PipelineError::Ocr(format!("native OCR task panicked: {e}")))?
            .map_err(|e| PipelineError::Ocr(format!("native OCR: {e}")))
    }

    fn describe(&self) -> String {
        format!("native ocr-daemon (tesseract, lang={})", self.lang)
    }
}

/// Pure-Rust `ocrs`/`rten` engine (feature `ocr-native-rust`, **default**).
/// Lazy-loads both `.rten` weight files on first call and keeps the engine
/// warm for the process lifetime, mirroring `NativeInferer` in `infer.rs`.
/// Image-only — PDF input is rejected with a pointer to `MLIS_OCR_ENGINE=docling`.
#[cfg(feature = "ocr-native-rust")]
mod rust_ocr {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    pub struct RustOcrEngine {
        model_dir: PathBuf,
        auto_download: bool,
        inner: OnceCell<Arc<mlis_ocr::NativeOcr>>,
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

        async fn get_or_load(&self) -> Result<Arc<mlis_ocr::NativeOcr>, String> {
            self.inner
                .get_or_try_init(|| async {
                    let model_dir = self.model_dir.clone();
                    let auto_download = self.auto_download;
                    tokio::task::spawn_blocking(move || {
                        let (detection, recognition) = if auto_download {
                            mlis_ocr::download::ensure_models(&model_dir)?
                        } else {
                            (
                                model_dir.join(mlis_ocr::download::DETECTION_FILENAME),
                                model_dir.join(mlis_ocr::download::RECOGNITION_FILENAME),
                            )
                        };
                        mlis_ocr::NativeOcr::load(&detection, &recognition)
                    })
                    .await
                    .map_err(|e| format!("ocr model load task panicked: {e}"))?
                    .map(Arc::new)
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

    #[async_trait]
    impl OcrEngine for RustOcrEngine {
        async fn to_markdown(&self, input: &Path) -> Result<String, PipelineError> {
            if !looks_like_image(input) {
                return Err(PipelineError::Ocr(
                    "PDF input requires MLIS_OCR_ENGINE=docling — the native-rust engine is \
                     image-only"
                        .into(),
                ));
            }
            let ocr = self.get_or_load().await.map_err(PipelineError::Ocr)?;
            let path = input.to_path_buf();
            tokio::task::spawn_blocking(move || ocr.recognize(&path))
                .await
                .map_err(|e| PipelineError::Ocr(format!("ocr task panicked: {e}")))?
                .map_err(PipelineError::Ocr)
        }

        fn describe(&self) -> String {
            format!("pure-rust ocr (ocrs/rten) @ {}", self.model_dir.display())
        }
    }
}
#[cfg(feature = "ocr-native-rust")]
pub use rust_ocr::RustOcrEngine;

/// Default model directory when `MLIS_OCR_MODEL_DIR` is unset — the repo root.
#[cfg(feature = "ocr-native-rust")]
const DEFAULT_OCR_MODEL_DIR: &str = ".";

/// Build the OCR engine selected by `MLIS_OCR_ENGINE` (`rust` default;
/// `docling` for the containerized service, which is required for PDFs;
/// `native` for the Linux-only Tesseract engine). Falls back to docling (with
/// a warning) if `rust` or `native` is requested from a build lacking the
/// corresponding feature.
pub fn engine_from_env() -> Box<dyn OcrEngine> {
    let docling_url =
        std::env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:5001".into());
    match std::env::var("MLIS_OCR_ENGINE")
        .unwrap_or_else(|_| "rust".into())
        .as_str()
    {
        "native" => native_or_fallback(docling_url),
        "docling" => Box::new(DoclingEngine::new(docling_url)),
        _ => rust_or_fallback(docling_url),
    }
}

#[cfg(feature = "native-ocr")]
fn native_or_fallback(_docling_url: String) -> Box<dyn OcrEngine> {
    Box::new(NativeEngine::from_env())
}

#[cfg(not(feature = "native-ocr"))]
fn native_or_fallback(docling_url: String) -> Box<dyn OcrEngine> {
    eprintln!(
        "[mlis] MLIS_OCR_ENGINE=native but this build lacks the `native-ocr` feature \
         (Linux/WSL only) — falling back to docling-serve"
    );
    Box::new(DoclingEngine::new(docling_url))
}

#[cfg(feature = "ocr-native-rust")]
fn rust_or_fallback(_docling_url: String) -> Box<dyn OcrEngine> {
    let model_dir =
        std::env::var("MLIS_OCR_MODEL_DIR").unwrap_or_else(|_| DEFAULT_OCR_MODEL_DIR.into());
    let auto_download = std::env::var("MLIS_OCR_AUTO_DOWNLOAD").as_deref() != Ok("0");
    Box::new(RustOcrEngine::new(model_dir, auto_download))
}

#[cfg(not(feature = "ocr-native-rust"))]
fn rust_or_fallback(docling_url: String) -> Box<dyn OcrEngine> {
    eprintln!(
        "[mlis] MLIS_OCR_ENGINE=rust (default) but this build lacks the `ocr-native-rust` \
         feature — falling back to docling-serve"
    );
    Box::new(DoclingEngine::new(docling_url))
}
