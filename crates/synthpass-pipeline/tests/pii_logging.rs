//! The Atlas §6 acceptance criterion as an executable check: **no document
//! content and no extracted field value may appear in any log line.**
//!
//! Deliberately its own integration-test binary rather than a `#[test]` inside
//! the library. `tracing` caches per-callsite interest globally the first time
//! a callsite is evaluated, so a unit test that installs a subscriber *after*
//! sibling tests have already driven the pipeline observes an empty buffer and
//! passes vacuously. A separate binary gets a fresh process, a cold cache, and
//! therefore a test that can actually fail.

use async_trait::async_trait;
use std::path::Path;
use std::sync::{Arc, Mutex};
use synthpass_core::Extraction;
use synthpass_pipeline::{InferBackend, OcrEngine, Pipeline, PipelineError, ProcessEvent};
use tokio::sync::mpsc;

/// Distinctive fake values planted in the document so a leak is unambiguous
/// rather than a judgement call.
const SENTINEL_SURNAME: &str = "ZZQXPII-SENTINEL-SURNAME";
const SENTINEL_DOC_NUMBER: &str = "ZZQX-DOCNUM-7788";
const SENTINEL_GIVEN_NAMES: &str = "ZZQXPII-GIVEN";

struct SentinelOcr;

#[async_trait]
impl OcrEngine for SentinelOcr {
    async fn to_markdown(&self, _input: &Path) -> Result<String, PipelineError> {
        // No valid check digits, so this lands on Tier 2 and exercises both
        // stages' instrumentation.
        Ok(format!(
            "Surname: {SENTINEL_SURNAME}\nGiven names: {SENTINEL_GIVEN_NAMES}\n\
             Document No: {SENTINEL_DOC_NUMBER}\nP<UTO{SENTINEL_SURNAME}<<{SENTINEL_GIVEN_NAMES}<<<"
        ))
    }
    fn describe(&self) -> String {
        "sentinel-ocr".into()
    }
}

struct SentinelBackend;

fn sentinel_extraction() -> Extraction {
    // `Extraction` is `ZeroizeOnDrop`, so struct-update syntax can't be used.
    let mut e = Extraction::default();
    e.surname = Some(SENTINEL_SURNAME.into());
    e.given_names = Some(SENTINEL_GIVEN_NAMES.into());
    e.document_number = Some(SENTINEL_DOC_NUMBER.into());
    e.extraction_method = "llm".into();
    e
}

#[async_trait]
impl InferBackend for SentinelBackend {
    async fn extract(&self, _markdown: &str) -> Result<Extraction, String> {
        Ok(sentinel_extraction())
    }
    async fn extract_stream(
        &self,
        _markdown: &str,
        _tx: &mpsc::Sender<ProcessEvent>,
    ) -> Result<Extraction, String> {
        Ok(sentinel_extraction())
    }
    fn describe(&self) -> String {
        "sentinel-backend".into()
    }
    fn model_id(&self) -> Option<String> {
        Some("sentinel-model.gguf".into())
    }
    async fn health(&self) -> Result<String, String> {
        Ok("healthy".into())
    }
}

/// Collects everything the subscriber writes so the test can assert on it.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl CapturedLogs {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer not poisoned")).into_owned()
    }
}

impl std::io::Write for CapturedLogs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("log buffer not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedLogs {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn no_document_content_reaches_the_log_stream() {
    let logs = CapturedLogs::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(logs.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("first subscriber in this process");

    let dir = std::env::temp_dir().join(format!("synthpass-pii-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let input = dir.join("doc.png");
    std::fs::write(&input, b"not really a png").expect("write input");

    let pipeline = Pipeline::new(Box::new(SentinelOcr), Box::new(SentinelBackend));
    let result = pipeline.process_document(&input).await;
    std::fs::remove_dir_all(&dir).ok();
    let result = result.expect("pipeline reaches a terminal result");
    assert!(
        result.extracted_v2.is_some(),
        "the document must actually have been extracted, or there was no PII to leak"
    );

    let captured = logs.text();
    // Guards against the vacuous pass: if the harness captured nothing, the
    // assertions below are meaningless.
    assert!(
        !captured.is_empty(),
        "captured no log output at all — the harness is broken, so this test proves nothing"
    );
    assert!(
        captured.contains("tier2"),
        "expected the Tier-2 stage to have logged something to assert against:\n{captured}"
    );

    for secret in [SENTINEL_SURNAME, SENTINEL_GIVEN_NAMES, SENTINEL_DOC_NUMBER] {
        assert!(
            !captured.contains(secret),
            "{secret:?} leaked into the log stream:\n{captured}"
        );
    }
}
