//! Shared document-processing pipeline:
//! OCR → Markdown on disk → **Tier 1** deterministic ICAO 9303 MRZ validation
//! → **Tier 2** LLM fallback (see [`InferBackend`]) → structured JSON.
//!
//! Tier 1 (the [`mrz`] crate) mathematically verifies every MRZ check digit.
//! When the composite checksum validates, the extraction is provably faithful
//! to the printed document and the probabilistic LLM step is skipped entirely.
//! The LLM only runs for documents without a valid MRZ (damaged scans,
//! non-standard documents, technical manuals).
//!
//! Tier 2 is a pluggable [`InferBackend`]: [`NativeInferer`] (feature
//! `inferer-native`, default, and the only backend as of v0.7.5) runs the
//! Qwen GGUF in-process via `synthpass-llm`, staying warm for the process
//! lifetime. See `infer::backend_from_env` for how it's constructed.
//!
//! Both binaries (`synthpass` CLI and `synthpass-serve` web server) are thin
//! wrappers around [`Pipeline::process_document`].

pub use mrz;

mod infer;
pub mod jobs;
pub mod metrics;
mod ocr;
pub use infer::InferBackend;
#[cfg(feature = "inferer-native")]
pub use infer::NativeInferer;
pub use jobs::{DocumentEntry, DocumentStatus, JobHandle, JobId, JobStatus};
pub use metrics::{MetricsSnapshot, PipelineMetrics};
#[cfg(feature = "ocr-native-rust")]
pub use ocr::RustOcrEngine;
pub use ocr::{BBox, OcrEngine, OcrResult};

use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use synthpass_core::v2::{CheckDigits, ExtractionV2, ImageRef, MrzBlock, MrzFormat, Provenance};
use synthpass_core::Extraction;
use tokio::sync::{mpsc, Semaphore};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct Pipeline {
    /// The OCR engine (pure-Rust `ocrs`/`rten` — the only engine since
    /// v1.2.0). `Arc`, not `Box`: [`Pipeline`] must be cheaply cloneable so
    /// [`submit`](Pipeline::submit) can hand an owned, `'static` copy to the
    /// background task it spawns per batch job — the same reason every other
    /// field here is already `Arc`-wrapped.
    ocr: Arc<dyn OcrEngine>,
    /// Tier-2 inference backend (native llama.cpp, in-process). `Arc` for the
    /// same reason as `ocr` above.
    infer: Arc<dyn InferBackend>,
    /// Tier 3: when set, append a PII-free audit record per processed document.
    audit_log: Option<PathBuf>,
    /// Tier 3: when set, encrypt the output JSON (AES-256-GCM) to `.json.enc`.
    /// `Zeroizing` wipes the key from memory when the `Pipeline` is dropped.
    encrypt_key: Option<Zeroizing<[u8; 32]>>,
    /// Consumer GPUs (e.g. GTX 970, 3.5 GB VRAM) fit exactly one GGUF model
    /// instance by default — LLM inference is serialized so concurrent
    /// callers queue instead of racing the same `llama.cpp` context (also
    /// enforced, defense-in-depth, inside `synthpass-llm`'s `NativeLlm`
    /// generation lock). One permit = one concurrent Tier-2 call; permit
    /// count is configurable via `SYNTHPASS_LLM_CONTEXTS` (see [`from_env`])
    /// for hardware with room for more than one loaded context.
    ///
    /// **Lock ordering with [`ocr_semaphore`](Self::ocr_semaphore):** a
    /// single document's processing acquires (and fully releases) at most
    /// one of these two semaphores at a time — `ocr_and_tier1` acquires and
    /// drops `ocr_semaphore` entirely before `extract_via_inferer[_stream]`
    /// ever attempts to acquire this one. No code path holds both permits
    /// simultaneously, so the two semaphores can never form a cyclic wait
    /// (the necessary condition for deadlock) between themselves, no matter
    /// how many documents a batch job runs concurrently against them.
    ///
    /// [`from_env`]: Pipeline::from_env
    llm_semaphore: Arc<Semaphore>,
    /// Bounds how many OCR passes ([`ocr_and_tier1`](Self::ocr_and_tier1))
    /// run concurrently, independently of `llm_semaphore` above. OCR is
    /// stateless, pure-CPU work with no shared model context to serialize
    /// (unlike Tier-2's single loaded `llama.cpp` context) — the only reason
    /// to bound it at all is to avoid oversubscribing the host's cores when a
    /// batch job ([`jobs`]) fires many documents at once. Permit count is
    /// `SYNTHPASS_OCR_THREADS` (see [`env_ocr_threads`]), default `cores - 1`
    /// floored at 1. See `llm_semaphore`'s doc for why this can't deadlock
    /// against it.
    ocr_semaphore: Arc<Semaphore>,
    /// Requests currently queued or in flight against the inferer. Lets a
    /// caller (e.g. `synthpass-serve`) reject new work fast under overload instead
    /// of accepting it unboundedly and blocking. Incremented just before
    /// queuing for `llm_semaphore`, decremented when the call completes.
    llm_queue_depth: Arc<AtomicUsize>,
    /// Counters and latency histograms for `/metrics` (Atlas §5–§6). Counts
    /// and durations only — never field content; see [`metrics`] on the PII
    /// rule.
    metrics: Arc<PipelineMetrics>,
    /// Batch-job bookkeeping (submission, status, bounded completed-job
    /// retention) — see [`jobs`] and [`Pipeline::submit`]. Shared (not
    /// re-created) across every `Clone` of this `Pipeline`, so a job
    /// submitted through one clone (e.g. inside the background task
    /// `submit` spawns) is still visible to `Pipeline::job` lookups made
    /// through any other clone (e.g. `synthpass-serve`'s `AppState`).
    jobs: Arc<jobs::JobRegistry>,
}

/// Bumps `llm_queue_depth` for the lifetime of the guard — from just before
/// queuing for the semaphore permit until the Tier-2 call fully completes.
struct QueueDepthGuard<'a>(&'a AtomicUsize);

impl<'a> QueueDepthGuard<'a> {
    fn enter(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(counter)
    }
}

impl Drop for QueueDepthGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub enum PipelineError {
    /// The OCR engine was unreachable or the conversion failed outright.
    Ocr(String),
    /// Conversion succeeded but returned no Markdown content.
    NoMarkdown(String),
    Io(std::io::Error),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ocr(e) => write!(f, "OCR failed: {e}"),
            Self::NoMarkdown(e) => write!(f, "no markdown returned from OCR: {e}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for PipelineError {}

impl From<std::io::Error> for PipelineError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Parses `SYNTHPASS_LLM_CONTEXTS`'s raw value into a Tier-2 permit count,
/// falling back to 1 for anything unset, unparsable, or non-positive.
/// `Semaphore::new(0)` would deadlock every Tier-2 call forever waiting for
/// a permit that can never exist, so this must fail safe rather than wedge
/// the pipeline. Pulled out of [`Pipeline::from_env`] as a pure function so
/// the fallback logic is unit-testable without mutating the process
/// environment (unsafe under this crate's default multi-threaded test
/// runner).
fn parse_llm_contexts(raw: Option<&str>) -> usize {
    raw.and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1)
}

/// How many concurrent Tier-2 contexts the environment is *asking* for
/// (`SYNTHPASS_LLM_CONTEXTS`, default 1). Whether it gets them is a separate
/// question — see [`Pipeline::from_env_with_llm_contexts`].
pub fn env_llm_contexts() -> usize {
    parse_llm_contexts(std::env::var("SYNTHPASS_LLM_CONTEXTS").ok().as_deref())
}

/// Parses `SYNTHPASS_OCR_THREADS`'s raw value into an OCR-stage concurrency
/// cap. Same fallback-on-garbage / floor-to-one discipline as
/// [`parse_llm_contexts`]: unset, unparsable, or non-positive all fall back
/// to `default_threads` (itself already floored at 1 by the caller) rather
/// than ever producing a 0-permit semaphore, which would deadlock every OCR
/// call forever. Split out as a pure function for the same reason
/// `parse_llm_contexts` is — unit-testable without mutating the process
/// environment.
fn parse_ocr_threads(raw: Option<&str>, default_threads: usize) -> usize {
    raw.and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or_else(|| default_threads.max(1))
}

/// Default OCR concurrency when `SYNTHPASS_OCR_THREADS` is unset: one less
/// than the host's available parallelism, floored at 1. OCR is pure CPU with
/// no shared context to serialize (unlike Tier-2's single GGUF context), so
/// using every core but one is the natural default — it lets a batch job
/// saturate the machine for OCR while still leaving headroom for the async
/// runtime, the OS, and (if concurrently active) a Tier-2 call's own
/// `spawn_blocking` work.
fn default_ocr_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(1)
        .max(1)
}

/// How many concurrent OCR passes the environment is *asking* for
/// (`SYNTHPASS_OCR_THREADS`, default: available cores − 1, floored at 1).
pub fn env_ocr_threads() -> usize {
    parse_ocr_threads(
        std::env::var("SYNTHPASS_OCR_THREADS").ok().as_deref(),
        default_ocr_threads(),
    )
}

/// Which extraction tier produced the final JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Tier 1: ICAO 9303 MRZ with all check digits valid — deterministic,
    /// LLM skipped.
    MrzDeterministic,
    /// Tier 2: LLM sidecar (no MRZ found, or its checksums failed).
    Llm,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MrzDeterministic => "mrz-deterministic",
            Self::Llm => "llm",
        }
    }
}

pub struct PipelineResult {
    /// OCR output (Markdown/plain text) from the active OCR engine.
    pub markdown: String,
    /// Where the Markdown was written (`<input>.md`).
    pub md_path: PathBuf,
    /// Where the extracted JSON was written (`<input>.json`).
    pub json_path: PathBuf,
    /// Parsed extraction JSON in the v1 shape (Tier 1 or Tier 2); `None` when
    /// Tier 2 failed. Kept populated for one-release compatibility (breaking
    /// change B3, `docs/V2-DESIGN.md` §9) — new consumers should read
    /// [`extracted_v2`].
    ///
    /// [`extracted_v2`]: PipelineResult::extracted_v2
    pub extracted: Option<Value>,
    /// The same extraction in the v2 schema (per-field confidence +
    /// provenance). `Some` exactly when [`extracted`] is.
    ///
    /// [`extracted`]: PipelineResult::extracted
    pub extracted_v2: Option<ExtractionV2>,
    /// Tier 2 failure description. OCR `markdown` is still valid when set.
    pub llm_error: Option<String>,
    /// Diagnostic notes from the Tier 2 step (e.g. a persist warning); usually
    /// empty now that the inferer streams no stdout.
    pub sidecar_stdout: String,
    /// Parsed MRZ when one was found in the OCR output — present even when
    /// its checksums failed (see `mrz.checks` for per-field results).
    pub mrz: Option<mrz::MrzData>,
    /// Which tier produced `extracted`.
    pub method: Method,
}

/// The result of [`Pipeline::ocr_and_tier1`] — a named struct rather than the
/// growing tuple it replaced, once adding OCR geometry (`ocr`) made a
/// 5-element tuple destructure unreadable at both call sites
/// ([`Pipeline::process_document`], [`Pipeline::process_document_stream`]).
/// `markdown` and `ocr.text` are the same string on purpose: `markdown` is
/// the plain value Tier 1/Tier 2 and [`PipelineResult`] have always worked
/// with, `ocr` carries the full [`OcrResult`] (geometry included) for callers
/// that need more than text — duplicating one `String` clone per document is
/// a small, one-time cost for not threading two half-overlapping shapes
/// through the rest of this function.
struct OcrStage {
    markdown: String,
    md_path: PathBuf,
    mrz_data: Option<mrz::MrzData>,
    tier1: Option<(Value, ExtractionV2)>,
    ocr: OcrResult,
}

/// Progress/terminal events emitted by [`Pipeline::process_document_stream`].
pub enum ProcessEvent {
    /// Incremental Tier-2 LLM text. Never sent on the Tier-1 fast path.
    Delta(String),
    /// Terminal: the full pipeline result, same shape as
    /// [`Pipeline::process_document`]'s return value.
    Done(Box<PipelineResult>),
    /// Terminal: an OCR-stage failure (mirrors [`PipelineError`]).
    Failed(String),
}

impl Pipeline {
    /// Construct with an explicit OCR engine and Tier-2 inference backend.
    /// Tier-3 security (audit log, encryption) is off; enable it via
    /// [`from_env`] or the `with_*` builders.
    ///
    /// [`from_env`]: Pipeline::from_env
    pub fn new(ocr: Box<dyn OcrEngine>, infer: Box<dyn InferBackend>) -> Self {
        Self::with_llm_contexts(ocr, infer, 1)
    }

    /// Like [`new`](Pipeline::new), but with an explicit LLM-context permit
    /// count instead of the hardcoded default of 1 — split out so tests can
    /// exercise a specific count directly, without going through
    /// [`from_env`](Pipeline::from_env)'s process-global environment
    /// variable (unsafe to mutate under this crate's default multi-threaded
    /// test runner).
    fn with_llm_contexts(
        ocr: Box<dyn OcrEngine>,
        infer: Box<dyn InferBackend>,
        contexts: usize,
    ) -> Self {
        // Manual construction (and every existing test) gets a serial,
        // single-threaded OCR stage by default — only `from_env` opts into
        // the host-parallel default via `env_ocr_threads`, mirroring how
        // `encrypt_key`/`audit_log` stay unset here too.
        Self::with_llm_contexts_and_ocr_threads(ocr, infer, contexts, 1)
    }

    /// Like [`with_llm_contexts`](Self::with_llm_contexts), but with an
    /// explicit OCR-concurrency permit count too — split out so
    /// [`from_env_with_llm_contexts`](Self::from_env_with_llm_contexts) can
    /// supply the real `SYNTHPASS_OCR_THREADS`-derived count without every
    /// other caller (manual construction, unit tests) having to opt in.
    fn with_llm_contexts_and_ocr_threads(
        ocr: Box<dyn OcrEngine>,
        infer: Box<dyn InferBackend>,
        contexts: usize,
        ocr_threads: usize,
    ) -> Self {
        Self {
            ocr: Arc::from(ocr),
            infer: Arc::from(infer),
            audit_log: None,
            encrypt_key: None,
            llm_semaphore: Arc::new(Semaphore::new(contexts.max(1))),
            ocr_semaphore: Arc::new(Semaphore::new(ocr_threads.max(1))),
            llm_queue_depth: Arc::new(AtomicUsize::new(0)),
            metrics: Arc::new(PipelineMetrics::default()),
            jobs: Arc::new(jobs::JobRegistry::new(jobs::DEFAULT_QUEUE_CAPACITY)),
        }
    }

    /// Point-in-time counters and histograms for the `/metrics` endpoint.
    /// Queue depth is read here rather than stored, since it's a gauge the
    /// semaphore already owns.
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics.snapshot(self.llm_queue_depth() as u64)
    }

    /// Append a PII-free audit record (SHA-256 fingerprint + metadata) per
    /// document to `path`.
    pub fn with_audit_log(mut self, path: impl Into<PathBuf>) -> Self {
        self.audit_log = Some(path.into());
        self
    }

    /// Encrypt the output JSON with the given AES-256 key.
    pub fn with_encrypt_key(mut self, key: [u8; 32]) -> Self {
        self.encrypt_key = Some(Zeroizing::new(key));
        self
    }

    /// Configure from the environment: `SYNTHPASS_OCR_ENGINE` (`rust` | `native`);
    /// `SYNTHPASS_MODEL_PATH` / `SYNTHPASS_MODEL_N_CTX` for the Tier-2 model;
    /// `SYNTHPASS_LLM_CONTEXTS` for the number of concurrent Tier-2 calls
    /// allowed (default 1); and Tier-3 `SYNTHPASS_AUDIT_LOG` / `SYNTHPASS_KEY`
    /// (base64 32-byte AES-256 key).
    pub fn from_env() -> Self {
        Self::from_env_with_llm_contexts(env_llm_contexts())
    }

    /// Like [`from_env`](Pipeline::from_env), but with the Tier-2 context
    /// count supplied by the caller instead of read from
    /// `SYNTHPASS_LLM_CONTEXTS` directly.
    ///
    /// This exists for `synthpass-serve`, where the environment only gets to
    /// *ask*: an offline license can cap concurrent contexts
    /// (`max_llm_contexts`) or withhold the `multi-context` feature outright,
    /// and the caller resolves that before constructing the pipeline — so the
    /// pipeline stays unaware of licensing and the licensing stays unaware of
    /// semaphores. Pair with [`env_llm_contexts`] to learn what was asked for.
    pub fn from_env_with_llm_contexts(contexts: usize) -> Self {
        let mut pipeline = Self::with_llm_contexts_and_ocr_threads(
            ocr::engine_from_env(),
            infer::backend_from_env(),
            contexts,
            env_ocr_threads(),
        );
        pipeline.audit_log = std::env::var("SYNTHPASS_AUDIT_LOG").ok().map(PathBuf::from);
        pipeline.encrypt_key = match std::env::var("SYNTHPASS_KEY") {
            Ok(s) => match synthpass_core::crypt::key_from_base64(&s) {
                Ok(key) => Some(key),
                Err(e) => {
                    tracing::warn!(error = %e, "ignoring SYNTHPASS_KEY");
                    None
                }
            },
            Err(_) => None,
        };
        pipeline.jobs = Arc::new(jobs::JobRegistry::new(jobs::queue_capacity_from_env()));
        pipeline
    }

    /// Human-readable description of the active OCR engine.
    pub fn ocr_engine(&self) -> String {
        self.ocr.describe()
    }

    /// Human-readable description of the active Tier-2 inference backend.
    pub fn infer_describe(&self) -> String {
        self.infer.describe()
    }

    /// Preflight check for `synthpass doctor`: `Ok(status)` on success, `Err(reason)`
    /// otherwise.
    pub async fn infer_health(&self) -> Result<String, String> {
        self.infer.health().await
    }

    /// Tier-2 requests currently queued or in flight against the inferer.
    /// Compare against a configured cap to reject new work fast instead of
    /// accepting it unboundedly.
    pub fn llm_queue_depth(&self) -> usize {
        self.llm_queue_depth.load(Ordering::Relaxed)
    }

    /// Available Tier-2 permits — i.e. how many concurrent
    /// [`extract_via_inferer`](Pipeline::extract_via_inferer) calls could
    /// proceed right now without queuing. Test-only: a real caller has no
    /// use for this (queue depth is the actionable signal), but it's the
    /// most direct way to prove [`with_llm_contexts`](Pipeline::with_llm_contexts)
    /// actually configured the semaphore it claims to.
    #[cfg(test)]
    fn llm_available_permits(&self) -> usize {
        self.llm_semaphore.available_permits()
    }

    /// Same as [`llm_available_permits`](Self::llm_available_permits), but
    /// for the OCR-stage semaphore (B2) — proves
    /// [`with_llm_contexts_and_ocr_threads`](Self::with_llm_contexts_and_ocr_threads)
    /// actually configured it.
    #[cfg(test)]
    fn ocr_available_permits(&self) -> usize {
        self.ocr_semaphore.available_permits()
    }

    /// Tier 2: call the active inference backend and get back the canonical
    /// [`Extraction`] schema. Serialized behind `llm_semaphore` so this
    /// process never fires overlapping Tier-2 calls, regardless of backend.
    ///
    /// Normalizes the `Ok` value in place ([`synthpass_core::normalize::extraction`])
    /// before returning — a Tier-2 read comes from the document's free-form
    /// visual zone, not the constrained MRZ dialect, so a parity miss like
    /// `"CROATIA"` vs `HRV` is the model reading correctly and just
    /// reproducing the visual zone's own formatting; this is the single
    /// point every Tier-2 caller passes through
    /// ([`Pipeline::process_document`]/[`Pipeline::process_document_stream`]
    /// both build their v2 record and persisted JSON from this return value),
    /// so normalizing here — and only here, never on Tier 1's already
    /// checksum-proven `extraction_from_mrz` output — covers every consumer
    /// including the batch job queue in one place.
    ///
    /// [`Pipeline::process_document`]: Pipeline::process_document
    /// [`Pipeline::process_document_stream`]: Pipeline::process_document_stream
    pub async fn extract_via_inferer(&self, markdown: &str) -> Result<Extraction, String> {
        let _depth = QueueDepthGuard::enter(&self.llm_queue_depth);
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .expect("llm_semaphore is never closed");

        let started = Instant::now();
        let mut result = self.infer.extract(markdown).await;
        let elapsed = started.elapsed();
        self.metrics.tier2_seconds.observe(elapsed);
        match &result {
            Ok(_) => tracing::debug!(
                stage = "tier2",
                elapsed_ms = elapsed.as_millis() as u64,
                "Tier-2 extraction complete"
            ),
            Err(e) => {
                self.metrics.record_tier2_failure();
                tracing::warn!(
                    stage = "tier2",
                    elapsed_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "Tier-2 extraction failed"
                );
            }
        }
        if let Ok(extraction) = &mut result {
            synthpass_core::normalize::extraction(extraction);
        }
        result
    }

    /// Stage 1-2: OCR the input, write `<input>.md`, and check for a
    /// checksum-valid MRZ (Tier 1). Shared by [`process_document`] and
    /// [`process_document_stream`] — the two differ only in how they handle a
    /// Tier-1 miss (unary vs. streaming Tier-2 fallback).
    ///
    /// [`process_document`]: Pipeline::process_document
    /// [`process_document_stream`]: Pipeline::process_document_stream
    async fn ocr_and_tier1(&self, input: &Path) -> Result<OcrStage, PipelineError> {
        // Spans carry stage identity and *shape* only — byte counts, booleans,
        // durations. Never `markdown`, never a field value: `/metrics` and the
        // log stream both sit outside the trust boundary that the document
        // itself does not leave. Portrait/rotation are shape too (a box and a
        // number of degrees, not document content), so they're safe to log
        // alongside the rest.
        let ocr_started = Instant::now();
        // Bound OCR concurrency independently of Tier-2's `llm_semaphore`
        // (B2, `SYNTHPASS_OCR_THREADS` / `env_ocr_threads`). The permit is
        // acquired and (via `_ocr_permit`'s scope) fully released before
        // this function returns — well before `extract_via_inferer[_stream]`
        // is ever called for this same document — see `ocr_semaphore`'s
        // field doc for why that ordering is what keeps the two semaphores
        // from being able to deadlock each other.
        let _ocr_permit = self
            .ocr_semaphore
            .acquire()
            .await
            .expect("ocr_semaphore is never closed");
        // `recognize_detailed`, not `to_markdown`: the richer call gets us
        // portrait/mrz_band/rotation geometry for free from engines that
        // support it (`RustOcrEngine`), and degrades to plain text via
        // `OcrEngine`'s default body for any engine that doesn't — see
        // `ocr::OcrEngine::recognize_detailed`'s doc.
        let ocr_result = match self.ocr.recognize_detailed(input).await {
            Ok(result) => result,
            Err(e) => {
                self.metrics.record_ocr_failure();
                tracing::warn!(stage = "ocr", error = %e, "OCR stage failed");
                return Err(e);
            }
        };
        let ocr_elapsed = ocr_started.elapsed();
        self.metrics.ocr_seconds.observe(ocr_elapsed);
        tracing::debug!(
            stage = "ocr",
            elapsed_ms = ocr_elapsed.as_millis() as u64,
            markdown_bytes = ocr_result.text.len(),
            portrait_detected = ocr_result.portrait.is_some(),
            rotation_degrees = ocr_result.rotation,
            "OCR complete"
        );

        let markdown = ocr_result.text.clone();
        let md_path = input.with_extension("md");
        tokio::fs::write(&md_path, &markdown).await?;

        // Tier 1: deterministic ICAO 9303 MRZ validation. A valid composite
        // checksum mathematically proves the read — no LLM needed.
        let mrz_data = mrz::find_and_parse(&markdown).ok();
        let tier1 = mrz_data.as_ref().filter(|m| m.valid()).map(|m| {
            (
                serde_json::to_value(extraction_from_mrz(m)).expect("Extraction serializes"),
                extraction_v2_from_mrz(m),
            )
        });
        tracing::debug!(
            stage = "tier1",
            mrz_found = mrz_data.is_some(),
            checksums_valid = tier1.is_some(),
            "Tier-1 MRZ validation complete"
        );
        Ok(OcrStage {
            markdown,
            md_path,
            mrz_data,
            tier1,
            ocr: ocr_result,
        })
    }

    /// Run the full pipeline on a local image/PDF. Writes `<input>.md` and
    /// `<input>.json` next to the input file.
    ///
    /// An OCR failure is an `Err`; an LLM-inferer failure is *not* — it
    /// degrades to `llm_error: Some(..)` with the OCR Markdown still returned.
    pub async fn process_document(&self, input: &Path) -> Result<PipelineResult, PipelineError> {
        let stage = self.ocr_and_tier1(input).await?;
        let mut json_path = stage.md_path.with_extension("json");

        // Tier 2: semantic JSON extraction via the in-process LLM inferer,
        // used only when Tier 1 found no checksum-valid MRZ.
        let (extracted, mut extracted_v2, method, llm_error) = if let Some((value, v2)) =
            stage.tier1
        {
            (Some(value), Some(v2), Method::MrzDeterministic, None)
        } else {
            match self.extract_via_inferer(&stage.markdown).await {
                Ok(extraction) => {
                    let v2 = lift_tier2_extraction(&extraction, self.infer.model_id());
                    let value = serde_json::to_value(&extraction).expect("Extraction serializes");
                    (Some(value), Some(v2), Method::Llm, None)
                }
                Err(e) => (None, None, Method::Llm, Some(e)),
            }
        };

        // Portrait geometry is independent of which tier produced the field
        // extraction — it comes from the same OCR pass regardless — so it's
        // filled in here, once, for whichever v2 record the branch above
        // produced.
        if let (Some(v2), Some(bbox)) = (extracted_v2.as_mut(), stage.ocr.portrait.as_ref()) {
            v2.portrait = Some(portrait_image_ref(bbox));
        }

        // Persist (encrypting if a key is configured) + append a PII-free audit
        // record. A persistence failure never invalidates the in-memory result.
        let mut sidecar_stdout = String::new();
        if let Some(value) = &extracted {
            match self
                .write_outputs(
                    input,
                    &json_path,
                    value,
                    extracted_v2.as_ref(),
                    method,
                    stage.mrz_data.as_ref(),
                )
                .await
            {
                Ok(written) => json_path = written,
                Err(e) => sidecar_stdout = format!("warning: could not persist output: {e}"),
            }
        }

        self.metrics.record_document(method);
        Ok(PipelineResult {
            markdown: stage.markdown,
            md_path: stage.md_path,
            json_path,
            extracted,
            extracted_v2,
            llm_error,
            sidecar_stdout,
            mrz: stage.mrz_data,
            method,
        })
    }

    /// Streaming variant of [`process_document`] for callers (`synthpass-serve`)
    /// that want to render Tier-2 LLM progress instead of freezing during the
    /// wait. Emits `Delta` events as the model generates, then exactly one
    /// terminal `Done` or `Failed` event on `tx`.
    ///
    /// [`process_document`]: Pipeline::process_document
    pub async fn process_document_stream(&self, input: &Path, tx: mpsc::Sender<ProcessEvent>) {
        let stage = match self.ocr_and_tier1(input).await {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(ProcessEvent::Failed(e.to_string())).await;
                return;
            }
        };
        let mut json_path = stage.md_path.with_extension("json");

        let (extracted, mut extracted_v2, method, llm_error) = if let Some((value, v2)) =
            stage.tier1
        {
            (Some(value), Some(v2), Method::MrzDeterministic, None)
        } else {
            match self.extract_via_inferer_stream(&stage.markdown, &tx).await {
                Ok(extraction) => {
                    let v2 = lift_tier2_extraction(&extraction, self.infer.model_id());
                    let value = serde_json::to_value(&extraction).expect("Extraction serializes");
                    (Some(value), Some(v2), Method::Llm, None)
                }
                Err(e) => (None, None, Method::Llm, Some(e)),
            }
        };

        // See the matching comment in `process_document`.
        if let (Some(v2), Some(bbox)) = (extracted_v2.as_mut(), stage.ocr.portrait.as_ref()) {
            v2.portrait = Some(portrait_image_ref(bbox));
        }

        let mut sidecar_stdout = String::new();
        if let Some(value) = &extracted {
            match self
                .write_outputs(
                    input,
                    &json_path,
                    value,
                    extracted_v2.as_ref(),
                    method,
                    stage.mrz_data.as_ref(),
                )
                .await
            {
                Ok(written) => json_path = written,
                Err(e) => sidecar_stdout = format!("warning: could not persist output: {e}"),
            }
        }

        let _ = tx
            .send(ProcessEvent::Done(Box::new(PipelineResult {
                markdown: stage.markdown,
                md_path: stage.md_path,
                json_path,
                extracted,
                extracted_v2,
                llm_error,
                sidecar_stdout,
                mrz: stage.mrz_data,
                method,
            })))
            .await;
    }

    /// Tier 2 streaming: like [`extract_via_inferer`] but forwards incremental
    /// text on `tx` as [`ProcessEvent::Delta`] while the model generates.
    /// Normalizes the `Ok` value the same way and for the same reason —
    /// see [`extract_via_inferer`]'s doc.
    ///
    /// [`extract_via_inferer`]: Pipeline::extract_via_inferer
    async fn extract_via_inferer_stream(
        &self,
        markdown: &str,
        tx: &mpsc::Sender<ProcessEvent>,
    ) -> Result<Extraction, String> {
        let _depth = QueueDepthGuard::enter(&self.llm_queue_depth);
        let _permit = self
            .llm_semaphore
            .acquire()
            .await
            .expect("llm_semaphore is never closed");
        let mut result = self.infer.extract_stream(markdown, tx).await;
        if let Ok(extraction) = &mut result {
            synthpass_core::normalize::extraction(extraction);
        }
        result
    }

    /// Write the extraction JSON (encrypted to `<input>.json.enc` when a key is
    /// set, else plaintext `<input>.json`) and append an audit record when a log
    /// is configured. Returns the path actually written.
    ///
    /// The persisted shape is **v2 by default**; `SYNTHPASS_JSON_V1=1` writes the
    /// legacy v1 shape instead — a documented deprecation shim for one major
    /// release (`docs/V2-DESIGN.md` §9, B2/B3).
    async fn write_outputs(
        &self,
        input: &Path,
        json_path: &Path,
        value: &Value,
        value_v2: Option<&ExtractionV2>,
        method: Method,
        mrz: Option<&mrz::MrzData>,
    ) -> std::io::Result<PathBuf> {
        let legacy_v1 = std::env::var("SYNTHPASS_JSON_V1").as_deref() == Ok("1");
        let pretty = Zeroizing::new(match (legacy_v1, value_v2) {
            (false, Some(v2)) => serde_json::to_string_pretty(v2).expect("ExtractionV2 serializes"),
            // Legacy shim requested, or (defensively) no v2 record available.
            _ => serde_json::to_string_pretty(value).expect("Value serializes"),
        });

        let written = if let Some(key) = &self.encrypt_key {
            let blob = synthpass_core::crypt::encrypt(key, pretty.as_bytes())
                .map_err(std::io::Error::other)?;
            let enc_path = json_path.with_extension("json.enc");
            tokio::fs::write(&enc_path, blob).await?;
            enc_path
        } else {
            tokio::fs::write(json_path, pretty.as_bytes()).await?;
            json_path.to_path_buf()
        };

        if let Some(log) = &self.audit_log {
            // Fingerprint the input bytes; never log the PII itself.
            let input_bytes = tokio::fs::read(input).await.unwrap_or_default();
            let record = synthpass_core::audit::AuditRecord::new(
                synthpass_core::audit::sha256_hex(&input_bytes),
                method.as_str(),
                mrz.map(|m| m.valid()),
                mrz.map(|m| m.document_type.clone()),
            );
            let _ = synthpass_core::audit::append(log, &record);
        }

        Ok(written)
    }
}

/// The current UTC date, for date-plausibility checks. Derived from the system
/// clock with pure arithmetic (via [`mrz::Date::from_epoch_days`]) so the `mrz`
/// crate itself stays clock-free.
fn today() -> mrz::Date {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    mrz::Date::from_epoch_days((secs / 86_400) as i64)
}

/// Map validated MRZ data onto the canonical [`Extraction`] schema — the same
/// shape Tier 2 and the WASM demo produce. Enriches with the resolved country
/// names and a date-plausibility summary (checksum-valid does not imply
/// in-date — see [`mrz::MrzData::validity`]).
fn extraction_from_mrz(m: &mrz::MrzData) -> Extraction {
    let v = m.validity(today());
    Extraction {
        document_type: Some(m.document_type.clone()),
        issuing_country: Some(m.issuing_country.clone()),
        issuing_country_name: m.issuing_country_name().map(str::to_string),
        document_number: Some(m.document_number.clone()),
        surname: Some(m.surname.clone()),
        given_names: Some(m.given_names.clone()),
        nationality: Some(m.nationality.clone()),
        nationality_name: m.nationality_name().map(str::to_string),
        date_of_birth: Some(m.date_of_birth.clone()),
        sex: Some(m.sex.clone()),
        date_of_expiry: Some(m.date_of_expiry.clone()),
        personal_number: m.personal_number.clone(),
        mrz_line: Some(m.mrz_lines.clone()),
        mrz_checksums_valid: Some(true),
        validity: Some(synthpass_core::Validity {
            dates_well_formed: v.dates_well_formed,
            in_date: v.in_date,
            dob_before_expiry: v.dob_before_expiry,
            days_until_expiry: v.days_until_expiry,
        }),
        extraction_method: Method::MrzDeterministic.as_str().to_string(),
    }
}

/// Map validated MRZ data onto the v2 schema directly: every field is
/// checksum-proven (confidence 1.0, [`Provenance::MrzChecksum`]) and the raw
/// zone plus exact per-check-digit results ride along in [`MrzBlock`] — the
/// detail `From<Extraction>` has to guess at, filled in precisely here.
fn extraction_v2_from_mrz(m: &mrz::MrzData) -> ExtractionV2 {
    let format = match m.format {
        mrz::Format::Td1 => MrzFormat::Td1,
        mrz::Format::Td2 => MrzFormat::Td2,
        mrz::Format::Td3 => MrzFormat::Td3,
    };
    let mut v2 = ExtractionV2::from(extraction_from_mrz(m));
    v2.document.mrz_format = Some(format);
    v2.mrz = Some(MrzBlock {
        lines: m.mrz_lines.clone(),
        format,
        checks: CheckDigits {
            document_number: m.checks.document_number,
            date_of_birth: m.checks.date_of_birth,
            date_of_expiry: m.checks.date_of_expiry,
            personal_number: m.checks.personal_number,
            composite: m.checks.composite,
        },
    });
    v2
}

/// Convert an OCR-detected portrait box into the wire [`ImageRef`] (`u32`
/// pixel coordinates). `as u32` is a saturating cast (no UB on out-of-range
/// floats since Rust 1.45), so a pathological negative or huge component
/// degrades to `0`/`u32::MAX` rather than wrapping — the clamp available at
/// this layer. `synthpass-ocr`'s portrait heuristic already guarantees the
/// box sits inside the real image's pixel grid (its search space is built
/// directly from the decoded image's own `width()`/`height()`), so there is
/// no independent image-bounds figure to clamp against here without
/// re-decoding the image a second time just to re-derive a bound the
/// producing crate already enforced.
///
/// **VISION.md §2 permanent non-goal**: this is a crop-coordinate conversion
/// only, never face recognition or biometric matching. The box says *where*
/// a photo-shaped, text-free region was found by layout heuristic; it says
/// nothing about whether a face is actually present there.
fn portrait_image_ref(bbox: &BBox) -> ImageRef {
    ImageRef {
        x: bbox.x.max(0.0) as u32,
        y: bbox.y.max(0.0) as u32,
        width: bbox.w.max(0.0) as u32,
        height: bbox.h.max(0.0) as u32,
    }
}

/// Lift a Tier-2 v1 [`Extraction`] into [`ExtractionV2`], stamping the real
/// model identity into the provenance — the bare `From` lift can only record
/// `"unknown"`, since it has no access to the backend that produced the value.
fn lift_tier2_extraction(extraction: &Extraction, model_id: Option<String>) -> ExtractionV2 {
    let mut v2 = ExtractionV2::from(extraction);
    v2.provenance = Provenance::Llm {
        model: model_id.unwrap_or_else(|| "unknown".to_string()),
    };
    v2
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use synthpass_core::v2::LLM_HEURISTIC_CONFIDENCE;

    /// Serializes tests that read or mutate `SYNTHPASS_JSON_V1` — the env var is
    /// process-global, and `cargo test` runs cases on parallel threads.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Minimal `OcrEngine` used only to satisfy `Pipeline::new`'s type in
    /// tests that exercise Tier 2 directly — its `to_markdown` is never called.
    struct NoopOcr;

    #[async_trait::async_trait]
    impl OcrEngine for NoopOcr {
        async fn to_markdown(&self, _input: &Path) -> Result<String, PipelineError> {
            unreachable!("these tests call extract_via_inferer directly, not to_markdown")
        }
        fn describe(&self) -> String {
            "noop".into()
        }
    }

    /// A mock Tier-2 backend, replacing the pre-v0.7.5 gRPC mock server —
    /// `Pipeline`'s queue/stream behavior is backend-agnostic by design (the
    /// whole point of [`InferBackend`]), so a plain in-process trait impl
    /// exercises it just as well as a real network round-trip did.
    struct MockBackend;

    #[async_trait::async_trait]
    impl InferBackend for MockBackend {
        async fn extract(&self, markdown: &str) -> Result<Extraction, String> {
            Ok(mock_extraction(markdown))
        }

        async fn extract_stream(
            &self,
            markdown: &str,
            tx: &mpsc::Sender<ProcessEvent>,
        ) -> Result<Extraction, String> {
            let _ = tx.try_send(ProcessEvent::Delta("mock-delta".into()));
            Ok(mock_extraction(markdown))
        }

        fn describe(&self) -> String {
            "mock".into()
        }

        fn model_id(&self) -> Option<String> {
            Some("mock-model.gguf".into())
        }

        async fn health(&self) -> Result<String, String> {
            Ok("mock healthy".into())
        }
    }

    fn mock_extraction(markdown: &str) -> Extraction {
        // Struct-update syntax (`..Extraction::default()`) can't be used
        // here: `Extraction` implements `Drop` (via `ZeroizeOnDrop`), and
        // Rust disallows partial moves out of a base value of a `Drop` type.
        let mut e = Extraction::default();
        e.document_type = Some("P".into());
        e.surname = Some("DOE".into());
        e.given_names = Some("JOHN".into());
        e.document_number = Some("X1234567".into());
        e.mrz_line = Some(markdown.chars().take(4).collect());
        e.extraction_method = Method::Llm.as_str().to_string();
        e
    }

    fn mock_pipeline() -> Pipeline {
        Pipeline::new(Box::new(NoopOcr), Box::new(MockBackend))
    }

    /// A distinctive fake holder name planted in the OCR text so a leak into
    /// the log stream is unambiguous rather than a judgement call.
    const PII_SENTINEL: &str = "ZZQXPII-SENTINEL-SURNAME";

    /// An OCR engine that returns document text containing [`PII_SENTINEL`].
    struct PiiOcr;

    #[async_trait::async_trait]
    impl OcrEngine for PiiOcr {
        async fn to_markdown(&self, _input: &Path) -> Result<String, PipelineError> {
            Ok(format!(
                "Surname: {PII_SENTINEL}\nP<UTO{PII_SENTINEL}<<JOHN<<<<<<<<<<<<<<<<<<<"
            ))
        }
        fn describe(&self) -> String {
            "pii-ocr".into()
        }
    }

    #[tokio::test]
    async fn metrics_count_documents_by_tier_and_time_the_stages() {
        let dir = std::env::temp_dir().join(format!("synthpass-metrics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let input = dir.join("doc.png");
        std::fs::write(&input, b"not really a png").expect("write input");

        let pipeline = Pipeline::new(Box::new(PiiOcr), Box::new(MockBackend));
        let before = pipeline.metrics_snapshot();
        assert_eq!(before.documents_tier1 + before.documents_tier2, 0);

        pipeline
            .process_document(&input)
            .await
            .expect("pipeline reaches a terminal result");
        std::fs::remove_dir_all(&dir).ok();

        let after = pipeline.metrics_snapshot();
        // PiiOcr's text has no valid check digits, so this is a Tier-2 document.
        assert_eq!(after.documents_tier2, 1, "Tier-2 document counted");
        assert_eq!(after.documents_tier1, 0);
        assert_eq!(after.ocr_seconds.count, 1, "OCR stage timed");
        assert_eq!(after.tier2_seconds.count, 1, "Tier-2 stage timed");
        assert_eq!(
            after.queue_depth, 0,
            "the queue guard released once the call completed"
        );
    }

    #[tokio::test]
    async fn tier2_returns_backend_extraction() {
        let pipeline = mock_pipeline();

        let e = pipeline
            .extract_via_inferer("P<UTO passport markdown")
            .await
            .expect("inferer extract");

        assert_eq!(e.surname.as_deref(), Some("DOE"));
        assert_eq!(e.given_names.as_deref(), Some("JOHN"));
        assert_eq!(e.document_number.as_deref(), Some("X1234567"));
        assert_eq!(e.mrz_line.as_deref(), Some("P<UT")); // proves markdown reached the backend
        assert_eq!(e.extraction_method, "llm");
    }

    #[tokio::test]
    async fn extract_via_inferer_stream_forwards_deltas_and_result() {
        let pipeline = mock_pipeline();

        let (tx, mut rx) = mpsc::channel(16);
        let extraction = pipeline
            .extract_via_inferer_stream("P<UTO passport markdown", &tx)
            .await
            .expect("inferer extract_stream");
        drop(tx);

        let mut deltas = Vec::new();
        while let Some(event) = rx.recv().await {
            match event {
                ProcessEvent::Delta(d) => deltas.push(d),
                _ => panic!("extract_via_inferer_stream should only send Delta events"),
            }
        }

        assert_eq!(deltas, vec!["mock-delta".to_string()]);
        assert_eq!(extraction.surname.as_deref(), Some("DOE"));
        assert_eq!(extraction.document_number.as_deref(), Some("X1234567"));
        assert_eq!(extraction.extraction_method, "llm");
    }

    #[tokio::test]
    async fn llm_queue_depth_tracks_in_flight_calls_and_returns_to_zero() {
        let pipeline = mock_pipeline();

        assert_eq!(pipeline.llm_queue_depth(), 0, "idle before any call");

        pipeline
            .extract_via_inferer("P<UTO passport markdown")
            .await
            .expect("inferer extract");
        assert_eq!(
            pipeline.llm_queue_depth(),
            0,
            "guard releases after completion"
        );

        let (tx, _rx) = mpsc::channel(16);
        pipeline
            .extract_via_inferer_stream("P<UTO passport markdown", &tx)
            .await
            .expect("inferer extract_stream");
        assert_eq!(
            pipeline.llm_queue_depth(),
            0,
            "guard releases after streaming completion too"
        );
    }

    #[test]
    fn parse_llm_contexts_falls_back_to_one_on_bad_input() {
        assert_eq!(parse_llm_contexts(None), 1, "unset falls back to 1");
        assert_eq!(
            parse_llm_contexts(Some("0")),
            1,
            "zero would deadlock every Tier-2 call"
        );
        assert_eq!(
            parse_llm_contexts(Some("abc")),
            1,
            "unparsable falls back to 1"
        );
        assert_eq!(
            parse_llm_contexts(Some("-1")),
            1,
            "negative falls back to 1"
        );
    }

    #[test]
    fn parse_llm_contexts_accepts_a_positive_count() {
        assert_eq!(parse_llm_contexts(Some("3")), 3);
        assert_eq!(parse_llm_contexts(Some("1")), 1);
    }

    #[test]
    fn with_llm_contexts_configures_that_many_permits() {
        let pipeline = Pipeline::with_llm_contexts(Box::new(NoopOcr), Box::new(MockBackend), 3);
        assert_eq!(pipeline.llm_available_permits(), 3);
    }

    #[test]
    fn with_llm_contexts_floors_zero_to_one_permit() {
        // Defense-in-depth: even if a future caller bypasses
        // parse_llm_contexts's own guard, the semaphore constructor itself
        // must never end up with zero permits (that deadlocks every Tier-2
        // call forever).
        let pipeline = Pipeline::with_llm_contexts(Box::new(NoopOcr), Box::new(MockBackend), 0);
        assert_eq!(pipeline.llm_available_permits(), 1);
    }

    // ── B2: independent OCR-stage semaphore ──

    #[test]
    fn parse_ocr_threads_falls_back_to_the_given_default_on_bad_input() {
        assert_eq!(
            parse_ocr_threads(None, 4),
            4,
            "unset falls back to the default"
        );
        assert_eq!(
            parse_ocr_threads(Some("0"), 4),
            4,
            "zero would deadlock every OCR call"
        );
        assert_eq!(
            parse_ocr_threads(Some("abc"), 4),
            4,
            "unparsable falls back"
        );
        assert_eq!(parse_ocr_threads(Some("-1"), 4), 4, "negative falls back");
    }

    #[test]
    fn parse_ocr_threads_accepts_a_positive_count() {
        assert_eq!(parse_ocr_threads(Some("6"), 4), 6);
        assert_eq!(parse_ocr_threads(Some("1"), 4), 1);
    }

    #[test]
    fn default_ocr_threads_is_never_zero() {
        // Whatever this host's core count is, the default must never produce
        // a 0-permit semaphore — even on a reported single-core host.
        assert!(default_ocr_threads() >= 1);
    }

    #[test]
    fn with_llm_contexts_and_ocr_threads_configures_both_semaphores_independently() {
        let pipeline = Pipeline::with_llm_contexts_and_ocr_threads(
            Box::new(NoopOcr),
            Box::new(MockBackend),
            2,
            5,
        );
        assert_eq!(pipeline.llm_available_permits(), 2);
        assert_eq!(pipeline.ocr_available_permits(), 5);
    }

    #[test]
    fn with_llm_contexts_and_ocr_threads_floors_zero_ocr_threads_to_one() {
        let pipeline = Pipeline::with_llm_contexts_and_ocr_threads(
            Box::new(NoopOcr),
            Box::new(MockBackend),
            1,
            0,
        );
        assert_eq!(pipeline.ocr_available_permits(), 1);
    }

    #[tokio::test]
    async fn ocr_and_llm_semaphores_do_not_hold_each_other_up() {
        // A single-permit pipeline on both semaphores: if OCR and Tier-2
        // ever needed to hold both permits at once for one document, this
        // would deadlock. It doesn't, because `ocr_and_tier1` releases its
        // permit (function return) before `extract_via_inferer` ever
        // requests the other one — see the lock-ordering doc on
        // `Pipeline::llm_semaphore`/`ocr_semaphore`.
        let pipeline = Pipeline::with_llm_contexts_and_ocr_threads(
            Box::new(NoopOcr),
            Box::new(MockBackend),
            1,
            1,
        );
        let extraction = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pipeline.extract_via_inferer("P<UTO passport markdown"),
        )
        .await
        .expect("must not deadlock")
        .expect("inferer extract");
        assert_eq!(extraction.extraction_method, "llm");
    }

    // ── M1: ExtractionV2 through the full pipeline (OCR → tier → JSON) ──

    /// An `OcrEngine` that always returns the same fixed Markdown, so
    /// `process_document` can run end-to-end without model files or images.
    struct StaticOcr(&'static str);

    #[async_trait::async_trait]
    impl OcrEngine for StaticOcr {
        async fn to_markdown(&self, _input: &Path) -> Result<String, PipelineError> {
            Ok(self.0.to_string())
        }
        fn describe(&self) -> String {
            "static".into()
        }
    }

    /// The Croatian TD3 specimen MRZ from the `mrz` crate's corpus tests —
    /// every check digit valid.
    const HRV_TD3_MARKDOWN: &str = "## PUTOVNICA\n\nP<HRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<<<<<<<<<<\n0070070071HRV8212258F1407019<<<<<<<<<<<<<<06\n";

    /// Write a throwaway input file in the temp dir and return its path plus
    /// the dir (for cleanup). `process_document` writes `<input>.md`/`.json`
    /// next to the input, hence the per-test directory.
    async fn temp_input(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("synthpass-m1-{tag}-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp dir");
        let input = dir.join("input.jpg");
        tokio::fs::write(&input, b"not a real image - OCR is mocked")
            .await
            .expect("write temp input");
        (input, dir)
    }

    #[tokio::test]
    // `ENV_LOCK` deliberately guards the whole test body (including its
    // awaits) — it exists purely to serialize two tests that touch the
    // process-global `SYNTHPASS_JSON_V1` env var, not to protect anything
    // async runtimes contend on, so holding it across `.await` here is safe
    // (no other task ever holds it while awaiting something this one needs).
    #[allow(clippy::await_holding_lock)]
    async fn tier1_produces_proven_v2_extraction() {
        // Locks against the SYNTHPASS_JSON_V1 shim test below: this case asserts
        // the *default* on-disk shape is v2.
        let _guard = ENV_LOCK.lock().unwrap();
        let (input, dir) = temp_input("tier1").await;
        let pipeline = Pipeline::new(Box::new(StaticOcr(HRV_TD3_MARKDOWN)), Box::new(MockBackend));

        let result = pipeline.process_document(&input).await.expect("process");

        assert_eq!(result.method, Method::MrzDeterministic);

        // v1 compat field: still populated, still the legacy shape.
        let v1 = result.extracted.as_ref().expect("v1 json populated");
        assert_eq!(v1["document_number"], json_str("007007007"));
        assert_eq!(v1["mrz_checksums_valid"], serde_json::json!(true));
        assert!(
            v1.get("schema_version").is_none(),
            "v1 shape must not grow schema_version"
        );

        // v2: provenance + per-check-digit detail, and confidence honest
        // about which fields the ICAO check digits actually cover. Only
        // document_number/date_of_birth/date_of_expiry/personal_number carry
        // a real check digit (verified against mrz::parser's composite
        // ranges); document_type/issuing_country/surname/given_names/
        // nationality/sex are structural parses, not proofs — see
        // synthpass_core::v2::FieldConfidence::mrz_checksum_scope.
        let v2 = result.extracted_v2.as_ref().expect("v2 extraction");
        assert_eq!(v2.schema_version, 2);
        assert_eq!(v2.provenance, Provenance::MrzChecksum);
        assert!(
            !v2.confidence.all_proven(),
            "TD3 line 1 has no check digit; the record must not claim it does"
        );
        assert_eq!(v2.confidence.document_number, 1.0);
        assert_eq!(v2.confidence.date_of_birth, 1.0);
        assert_eq!(v2.confidence.date_of_expiry, 1.0);
        assert_eq!(v2.fields.document_number.as_deref(), Some("007007007"));
        assert_eq!(v2.document.mrz_format, Some(MrzFormat::Td3));
        let mrz = v2.mrz.as_ref().expect("MRZ block present on Tier 1");
        assert!(mrz.checks.all_valid());
        assert_eq!(mrz.format, MrzFormat::Td3);
        assert!(mrz.lines.contains("P<HRVSPECIMEN"));

        // On-disk JSON defaults to the v2 shape.
        let on_disk: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&result.json_path)
                .await
                .expect("read persisted json"),
        )
        .expect("persisted json parses");
        assert_eq!(on_disk["schema_version"], serde_json::json!(2));
        assert_eq!(on_disk["provenance"]["kind"], json_str("mrz_checksum"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn tier2_produces_llm_provenance_v2_extraction() {
        let (input, dir) = temp_input("tier2").await;
        let pipeline = Pipeline::new(
            Box::new(StaticOcr("just prose — no MRZ anywhere")),
            Box::new(MockBackend),
        );

        let result = pipeline.process_document(&input).await.expect("process");

        assert_eq!(result.method, Method::Llm);
        let v2 = result.extracted_v2.as_ref().expect("v2 extraction");
        assert_eq!(
            v2.provenance,
            Provenance::Llm {
                model: "mock-model.gguf".into()
            },
            "the backend's real model id is stamped, not 'unknown'"
        );
        // `mock_extraction` sets document_type/surname/given_names/
        // document_number to plausible-looking values, so their per-field
        // heuristic score is upgraded above the flat baseline (see
        // synthpass-core::v2::heuristic_field_confidence); fields it leaves
        // absent (e.g. personal_number) stay at the flat baseline.
        assert!(!v2.confidence.all_proven());
        assert!(v2.confidence.document_number > LLM_HEURISTIC_CONFIDENCE);
        assert_eq!(v2.confidence.personal_number, LLM_HEURISTIC_CONFIDENCE);
        assert!(
            result.extracted.is_some(),
            "v1 compat field still populated"
        );
        // The mock echoes 4 chars of the markdown into v1's `mrz_line`; the
        // lift carries it over but with every check digit false — a Tier-2
        // MRZ-shaped echo is never checksum-proven.
        let mrz = v2.mrz.as_ref().expect("lifted MRZ placeholder");
        assert!(!mrz.checks.all_valid());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    // See the matching `#[allow]` on `tier1_produces_proven_v2_extraction`.
    #[allow(clippy::await_holding_lock)]
    async fn synthpass_json_v1_env_writes_legacy_shape_on_disk() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("SYNTHPASS_JSON_V1", "1");
        let (input, dir) = temp_input("shim").await;
        let pipeline = Pipeline::new(Box::new(StaticOcr(HRV_TD3_MARKDOWN)), Box::new(MockBackend));

        let result = pipeline.process_document(&input).await.expect("process");
        std::env::remove_var("SYNTHPASS_JSON_V1");

        // In-memory results are unaffected — only the persisted shape changes.
        assert!(result.extracted_v2.is_some());
        let on_disk: Value = serde_json::from_str(
            &tokio::fs::read_to_string(&result.json_path)
                .await
                .expect("read persisted json"),
        )
        .expect("persisted json parses");
        assert!(
            on_disk.get("schema_version").is_none(),
            "SYNTHPASS_JSON_V1=1 must write the legacy v1 shape"
        );
        assert_eq!(on_disk["document_number"], json_str("007007007"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// Tiny helper so expected-string assertions read clearly.
    fn json_str(s: &str) -> Value {
        Value::String(s.to_string())
    }
}
