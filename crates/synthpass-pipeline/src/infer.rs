//! Tier-2 inference backend abstraction: the pipeline gets an [`Extraction`]
//! from *some* backend. [`NativeInferer`] (feature `inferer-native`, default
//! and, as of v0.7.5, the only backend) runs the Qwen GGUF in-process via
//! `synthpass-llm` / `llama-cpp-2`. The legacy gRPC backend (talking to a Python
//! sidecar) was retired in v0.7.5 once its stated one-release grace period
//! elapsed — see `CHANGELOG.md`.
//!
//! Concurrency control (the single-flight semaphore and queue-depth counter)
//! lives in [`crate::Pipeline`], not here — every backend is called through
//! the same guarded seam so the "one concurrent Tier-2 call" invariant holds.

use crate::ProcessEvent;
use async_trait::async_trait;
use synthpass_core::Extraction;
use tokio::sync::mpsc;

#[cfg(not(feature = "inferer-native"))]
compile_error!("synthpass-pipeline requires the `inferer-native` feature");

/// Produces an [`Extraction`] from OCR Markdown (Tier 2 — the LLM fallback).
#[async_trait]
pub trait InferBackend: Send + Sync {
    async fn extract(&self, markdown: &str) -> Result<Extraction, String>;
    /// Like [`Self::extract`], but forwards incremental text on `tx` as
    /// [`ProcessEvent::Delta`] while the model generates. Implementations
    /// must use non-blocking sends (`try_send`) — a stalled receiver must
    /// never extend how long the caller's concurrency permit is held.
    async fn extract_stream(
        &self,
        markdown: &str,
        tx: &mpsc::Sender<ProcessEvent>,
    ) -> Result<Extraction, String>;
    /// Short human-readable identity for logs.
    fn describe(&self) -> String;
    /// Stable model identity recorded in [`synthpass_core::v2::Provenance::Llm`] —
    /// e.g. the GGUF filename. Defaults to `None` so out-of-tree backends stay
    /// source-compatible; the pipeline records `"unknown"` in that case.
    fn model_id(&self) -> Option<String> {
        None
    }
    /// Preflight check for `synthpass doctor`: `Ok(status)` on success, `Err(reason)`
    /// otherwise. Must not panic and should be cheap (no full model load).
    async fn health(&self) -> Result<String, String>;
}

#[cfg(feature = "inferer-native")]
mod native {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tokio::sync::OnceCell;

    /// Default backend: the Qwen GGUF run in-process via `llama-cpp-2`. The
    /// model is loaded lazily (on first Tier-2 call) and kept warm for the
    /// process lifetime — `llama_cpp_2::llama_backend::LlamaBackend::init()`
    /// is itself a process-wide singleton, so this cell must only ever
    /// populate once.
    pub struct NativeInferer {
        model_path: PathBuf,
        n_ctx: u32,
        llm: OnceCell<Arc<synthpass_llm::NativeLlm>>,
    }

    impl NativeInferer {
        pub fn new(model_path: impl Into<PathBuf>, n_ctx: u32) -> Self {
            Self {
                model_path: model_path.into(),
                n_ctx,
                llm: OnceCell::new(),
            }
        }

        pub fn model_path(&self) -> &std::path::Path {
            &self.model_path
        }

        async fn get_or_load(&self) -> Result<Arc<synthpass_llm::NativeLlm>, String> {
            self.llm
                .get_or_try_init(|| async {
                    let path = self.model_path.clone();
                    let n_ctx = self.n_ctx;
                    tokio::task::spawn_blocking(move || {
                        // Verify on the actual load path, not just in `synthpass doctor` — a
                        // tampered or corrupted GGUF must fail closed before it's ever
                        // mapped into memory, not just when someone remembers to preflight.
                        if !synthpass_llm::verify::skip_verify() {
                            synthpass_llm::verify::verify_model(&path).map_err(|e| e.to_string())?;
                        }
                        synthpass_llm::NativeLlm::load(&path, n_ctx)
                    })
                    .await
                    .map_err(|e| format!("model load task panicked: {e}"))?
                    .map(Arc::new)
                })
                .await
                .cloned()
        }
    }

    #[async_trait]
    impl InferBackend for NativeInferer {
        async fn extract(&self, markdown: &str) -> Result<Extraction, String> {
            let llm = self.get_or_load().await?;
            let markdown = markdown.to_string();
            tokio::task::spawn_blocking(move || llm.extract(&markdown))
                .await
                .map_err(|e| format!("inference task panicked: {e}"))?
        }

        async fn extract_stream(
            &self,
            markdown: &str,
            tx: &mpsc::Sender<ProcessEvent>,
        ) -> Result<Extraction, String> {
            let llm = self.get_or_load().await?;
            let markdown = markdown.to_string();
            let tx = tx.clone();
            tokio::task::spawn_blocking(move || {
                llm.extract_stream(&markdown, |delta| {
                    // Non-blocking: a stalled receiver must never extend how
                    // long the caller's concurrency permit is held.
                    let _ = tx.try_send(ProcessEvent::Delta(delta.to_string()));
                })
            })
            .await
            .map_err(|e| format!("inference task panicked: {e}"))?
        }

        fn describe(&self) -> String {
            format!("native llama.cpp @ {}", self.model_path.display())
        }

        fn model_id(&self) -> Option<String> {
            // The GGUF basename is the honest, stable identity: no path
            // layout leakage, and it survives the model being relocated.
            self.model_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        }

        async fn health(&self) -> Result<String, String> {
            if !self.model_path.exists() {
                return Err(format!("model not found at {}", self.model_path.display()));
            }
            if synthpass_llm::verify::skip_verify() {
                return Ok(format!(
                    "model present at {} (sha256 verification skipped)",
                    self.model_path.display()
                ));
            }
            let path = self.model_path.clone();
            tokio::task::spawn_blocking(move || synthpass_llm::verify::verify_model(&path))
                .await
                .map_err(|e| format!("verify task panicked: {e}"))?
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "model present and sha256-verified at {}",
                self.model_path.display()
            ))
        }
    }
}
#[cfg(feature = "inferer-native")]
pub use native::NativeInferer;

/// Default GGUF path when `SYNTHPASS_MODEL_PATH` is unset — the model this
/// workspace ships against, expected at the repository root.
#[cfg(feature = "inferer-native")]
const DEFAULT_MODEL_PATH: &str = "./qwen2.5-1.5b-instruct-q4_k_m.gguf";
#[cfg(feature = "inferer-native")]
const DEFAULT_N_CTX: u32 = 2048;

/// Build the Tier-2 backend. `native` (in-process llama.cpp) is the only
/// backend as of v0.7.5. `SYNTHPASS_INFERER` is still read for backward
/// compatibility with existing env files: any value other than `native`
/// (notably the removed `grpc`) is accepted with a one-line notice rather
/// than treated as an error.
pub fn backend_from_env() -> Box<dyn InferBackend> {
    if let Ok(choice) = std::env::var("SYNTHPASS_INFERER") {
        if choice != "native" {
            eprintln!(
                "[synthpass] SYNTHPASS_INFERER={choice:?} is not a recognized backend (gRPC support \
                 was removed in v0.7.5) — using native"
            );
        }
    }
    native_choice()
}

fn native_choice() -> Box<dyn InferBackend> {
    let model_path = std::env::var("SYNTHPASS_MODEL_PATH").unwrap_or_else(|_| DEFAULT_MODEL_PATH.into());
    let n_ctx = std::env::var("SYNTHPASS_MODEL_N_CTX")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_N_CTX);
    Box::new(NativeInferer::new(model_path, n_ctx))
}
