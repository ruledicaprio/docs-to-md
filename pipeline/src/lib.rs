//! Shared document-processing pipeline:
//! docling-serve OCR → Markdown on disk → local GGUF LLM sidecar → structured JSON.
//!
//! Both binaries (`docling-client` CLI and `docling-app` web server) are thin
//! wrappers around [`Pipeline::process_document`]. The v0.3.0 deterministic
//! MRZ parser (ICAO 9303) will live here as `mrz.rs`.

use docling_rs::DoclingClient;
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

pub struct Pipeline {
    docling: DoclingClient,
    docling_url: String,
    python_exe: String,
    /// Consumer GPUs (e.g. GTX 970, 3.5 GB VRAM) fit exactly one GGUF model
    /// instance — LLM inference is serialized so concurrent callers queue
    /// instead of OOM-ing.
    llm_lock: Mutex<()>,
}

#[derive(Debug)]
pub enum PipelineError {
    /// docling-serve was unreachable or the conversion failed outright.
    Docling(String),
    /// Conversion succeeded but returned no Markdown content.
    NoMarkdown(String),
    Io(std::io::Error),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Docling(e) => write!(f, "docling-serve conversion failed: {e}"),
            Self::NoMarkdown(e) => write!(f, "no markdown returned from docling-serve: {e}"),
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

pub struct PipelineResult {
    /// OCR output from docling-serve.
    pub markdown: String,
    /// Where the Markdown was written (`<input>.md`).
    pub md_path: PathBuf,
    /// Where the sidecar wrote the extracted JSON (`<input>.json`).
    pub json_path: PathBuf,
    /// Parsed JSON from the LLM sidecar; `None` when the LLM step failed.
    pub extracted: Option<Value>,
    /// Sidecar failure description. OCR `markdown` is still valid when set.
    pub llm_error: Option<String>,
    /// Captured sidecar stdout (model load / extraction preview messages).
    pub sidecar_stdout: String,
}

impl Pipeline {
    pub fn new(docling_url: impl Into<String>, python_exe: impl Into<String>) -> Self {
        let docling_url = docling_url.into();
        Self {
            docling: DoclingClient::new(docling_url.clone()),
            docling_url,
            python_exe: python_exe.into(),
            llm_lock: Mutex::new(()),
        }
    }

    /// Configure from `DOCLING_URL` / `PYTHON_EXE` env vars, with defaults
    /// matching a repo-root working directory on Windows.
    pub fn from_env() -> Self {
        Self::new(
            std::env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:5001".into()),
            std::env::var("PYTHON_EXE").unwrap_or_else(|_| r".venv\Scripts\python.exe".into()),
        )
    }

    pub fn docling_url(&self) -> &str {
        &self.docling_url
    }

    /// Run the full pipeline on a local image/PDF. Writes `<input>.md` and
    /// `<input>.json` next to the input file.
    ///
    /// An OCR failure is an `Err`; an LLM-sidecar failure is *not* — it
    /// degrades to `llm_error: Some(..)` with the OCR Markdown still returned.
    ///
    /// Must be called with the process working directory at the repo root so
    /// the sidecar finds `extract_json.py` and the GGUF model.
    pub async fn process_document(&self, input: &Path) -> Result<PipelineResult, PipelineError> {
        // Stage 1: OCR via docling-serve.
        let result = self
            .docling
            .convert_file(&[input], None, None)
            .await
            .map_err(|e| PipelineError::Docling(format!("{e:?}")))?;

        let markdown = result
            .document
            .md_content
            .clone()
            .ok_or_else(|| PipelineError::NoMarkdown(format!("{:?}", result.errors)))?;

        let md_path = input.with_extension("md");
        tokio::fs::write(&md_path, &markdown).await?;
        let json_path = md_path.with_extension("json");

        // Stage 2: semantic JSON extraction via the Python/GGUF sidecar.
        let output = {
            let _guard = self.llm_lock.lock().await;
            tokio::process::Command::new(&self.python_exe)
                .arg("extract_json.py")
                .arg(&md_path)
                .output()
                .await
        };

        let mut sidecar_stdout = String::new();
        let (extracted, llm_error) = match output {
            Ok(out) => {
                sidecar_stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                if out.status.success() {
                    match tokio::fs::read_to_string(&json_path).await {
                        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                            Ok(v) => (Some(v), None),
                            Err(e) => (None, Some(format!("sidecar produced invalid JSON: {e}"))),
                        },
                        Err(_) => (
                            None,
                            Some("sidecar exited 0 but produced no JSON file".to_string()),
                        ),
                    }
                } else {
                    (
                        None,
                        Some(format!(
                            "sidecar exited with {}. stderr: {}",
                            out.status,
                            String::from_utf8_lossy(&out.stderr)
                        )),
                    )
                }
            }
            Err(e) => (
                None,
                Some(format!(
                    "could not spawn {} — is the .venv present? ({e})",
                    self.python_exe
                )),
            ),
        };

        Ok(PipelineResult {
            markdown,
            md_path,
            json_path,
            extracted,
            llm_error,
            sidecar_stdout,
        })
    }
}
