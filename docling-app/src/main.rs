//! docling-app — web front-end for the air-gapped document pipeline.
//!
//! GET  /             → embedded upload page
//! POST /api/extract  → multipart file upload → shared `pipeline` crate
//!                      (docling-serve OCR → Markdown → Qwen GGUF → JSON)
//!
//! Run from the repository root (`cargo run -p docling-app`) so the Python
//! sidecar finds `extract_json.py`, the `.venv` and the GGUF model.

use axum::{
    extract::{DefaultBodyLimit, Multipart, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use pipeline::{Pipeline, PipelineError};
use serde_json::{json, Value};
use std::{env, path::PathBuf, sync::Arc};

const INDEX_HTML: &str = include_str!("index.html");
const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024; // 20 MB

struct AppState {
    pipeline: Pipeline,
    work_dir: PathBuf,
    keep_work: bool,
}

type ApiError = (StatusCode, Json<Value>);

fn api_error(status: StatusCode, msg: impl std::fmt::Display) -> ApiError {
    (status, Json(json!({ "error": msg.to_string() })))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let work_dir = PathBuf::from(env::var("WORK_DIR").unwrap_or_else(|_| "work".into()));
    let keep_work = env::var("KEEP_WORK").is_ok();

    tokio::fs::create_dir_all(&work_dir).await?;

    let pipeline = Pipeline::from_env();
    println!(
        "🚀 [docling-app] Listening on http://{bind_addr} (docling-serve: {})",
        pipeline.docling_url()
    );
    if !bind_addr.starts_with("127.0.0.1") && !bind_addr.starts_with("localhost") {
        println!("⚠️  [docling-app] Non-loopback bind: this service processes PII and has no auth — put a reverse proxy with TLS + authentication in front of it.");
    }

    let state = Arc::new(AppState {
        pipeline,
        work_dir,
        keep_work,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/extract", post(extract))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn extract(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    // Take the first uploaded file field.
    let (filename, data) = loop {
        let field = multipart
            .next_field()
            .await
            .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?
            .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "no file field in upload"))?;
        if let Some(name) = field.file_name().map(str::to_owned) {
            let data = field
                .bytes()
                .await
                .map_err(|e| api_error(StatusCode::BAD_REQUEST, e))?;
            break (name, data);
        }
    };

    // Keep only a sanitized extension; docling-serve uses it for format detection.
    let ext: String = filename
        .rsplit('.')
        .next()
        .filter(|e| *e != filename)
        .map(|e| e.chars().filter(char::is_ascii_alphanumeric).collect())
        .filter(|e: &String| !e.is_empty())
        .unwrap_or_else(|| "bin".into());

    let upload_path = state.work_dir.join(format!("{}.{ext}", uuid::Uuid::new_v4()));
    tokio::fs::write(&upload_path, &data)
        .await
        .map_err(|e| api_error(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let result = state.pipeline.process_document(&upload_path).await;

    let response = match &result {
        Ok(r) => {
            let resp = json!({
                "filename": filename,
                "markdown": r.markdown,
                "extracted": r.extracted,
                "error": r.llm_error,
            });
            cleanup(&state, &[&upload_path, &r.md_path, &r.json_path]).await;
            Ok(Json(resp))
        }
        Err(e) => {
            cleanup(&state, &[&upload_path]).await;
            let status = match e {
                PipelineError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
                _ => StatusCode::BAD_GATEWAY,
            };
            Err(api_error(status, e))
        }
    };
    response
}

/// Best-effort removal of working files (PII hygiene). Set KEEP_WORK=1 to keep
/// them in the work dir for debugging.
async fn cleanup(state: &AppState, paths: &[&PathBuf]) {
    if state.keep_work {
        return;
    }
    for p in paths {
        let _ = tokio::fs::remove_file(p).await;
    }
}
