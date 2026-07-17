//! mlis-serve — web front-end for the multi-level-id-strip pipeline.
//!
//! GET  /             → embedded upload page
//! POST /api/extract  → multipart file upload → shared `mlis-pipeline` crate
//!                      (OCR engine → Markdown → Tier 1 MRZ → Tier 2 LLM → JSON)
//!
//! Run from the repository root (`cargo run -p mlis-serve`) so the inferer
//! sidecar and OCR engine resolve.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::{Html, Json, Response},
    routing::{get, post},
    Router,
};
use mlis_pipeline::{Pipeline, PipelineError};
use serde_json::{json, Value};
use std::{env, path::PathBuf, sync::Arc};

const INDEX_HTML: &str = include_str!("index.html");
const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024; // 20 MB

struct AppState {
    pipeline: Pipeline,
    work_dir: PathBuf,
    keep_work: bool,
    /// Tier 3: when set, every request must present `Authorization: Bearer <token>`.
    token: Option<String>,
}

type ApiError = (StatusCode, Json<Value>);

fn api_error(status: StatusCode, msg: impl std::fmt::Display) -> ApiError {
    (status, Json(json!({ "error": msg.to_string() })))
}

/// Whether `addr` (a `host:port`, `[ipv6]:port`, or bare host) names a loopback
/// interface. Compares the parsed host exactly rather than a string prefix, so
/// `"127.0.0.1.evil.example:8080"` isn't mistaken for `127.0.0.1`.
fn is_loopback(addr: &str) -> bool {
    let host = if let Some(rest) = addr.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        addr.rsplit_once(':').map_or(addr, |(host, _)| host)
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Tier-3 startup gate: refuse to expose PII processing on a non-loopback bind
/// without a bearer token configured. Returns the refusal message the process
/// should exit with, or `None` when it's safe to serve.
fn startup_refusal(bind_addr: &str, token: &Option<String>) -> Option<String> {
    if !is_loopback(bind_addr) && token.is_none() {
        Some(format!(
            "refusing to bind non-loopback ({bind_addr}) without MLIS_TOKEN — this service \
             processes identity documents. Set MLIS_TOKEN to require Bearer auth (and set \
             MLIS_TLS_CERT/MLIS_TLS_KEY for TLS)."
        ))
    } else {
        None
    }
}

/// Tier-3 auth: when a token is configured, require `Authorization: Bearer <token>`.
async fn require_auth(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(token) = &state.token {
        let presented = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "));
        if presented != Some(token.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(req).await)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let work_dir = PathBuf::from(env::var("WORK_DIR").unwrap_or_else(|_| "work".into()));
    let keep_work = env::var("KEEP_WORK").is_ok();
    let token = env::var("MLIS_TOKEN").ok().filter(|s| !s.is_empty());

    // Refuse to expose PII processing wide-open: a non-loopback bind requires
    // an auth token (and, ideally, TLS in front).
    if let Some(reason) = startup_refusal(&bind_addr, &token) {
        return Err(reason.into());
    }

    tokio::fs::create_dir_all(&work_dir).await?;

    let pipeline = Pipeline::from_env();
    let scheme = if env::var("MLIS_TLS_CERT").is_ok() {
        "https"
    } else {
        "http"
    };
    println!(
        "🚀 [mlis-serve] Listening on {scheme}://{bind_addr} (ocr: {}, auth: {})",
        pipeline.ocr_engine(),
        if token.is_some() { "bearer" } else { "none" }
    );

    let state = Arc::new(AppState {
        pipeline,
        work_dir,
        keep_work,
        token,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/extract", post(extract))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .with_state(state);

    // Optional TLS (rustls) when both cert and key are provided.
    match (env::var("MLIS_TLS_CERT"), env::var("MLIS_TLS_KEY")) {
        (Ok(cert), Ok(key)) => {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let addr: std::net::SocketAddr = bind_addr.parse().map_err(|e| {
                format!("MLIS_TLS requires BIND_ADDR as IP:port ({bind_addr}): {e}")
            })?;
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            axum_server::bind_rustls(addr, config)
                .serve(app.into_make_service())
                .await?;
        }
        _ => {
            let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
            axum::serve(listener, app).await?;
        }
    }
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

    let upload_path = state
        .work_dir
        .join(format!("{}.{ext}", uuid::Uuid::new_v4()));
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
                "method": r.method.as_str(),
                "mrz": r.mrz.as_ref().map(|m| serde_json::to_value(m).expect("MrzData serializes")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use mlis_pipeline::{DoclingEngine, Pipeline};
    use tower::ServiceExt;

    #[test]
    fn is_loopback_recognizes_local_addresses() {
        assert!(is_loopback("127.0.0.1:8080"));
        assert!(is_loopback("localhost:8080"));
        assert!(is_loopback("[::1]:8080"));
        assert!(!is_loopback("0.0.0.0:8080"));
        assert!(!is_loopback("192.168.1.5:8080"));
        // Guards against a substring match on an unrelated host that merely
        // starts with a loopback-looking prefix.
        assert!(!is_loopback("127.0.0.1.evil.example:8080"));
    }

    #[test]
    fn startup_refuses_non_loopback_without_token() {
        assert!(startup_refusal("0.0.0.0:8080", &None).is_some());
    }

    #[test]
    fn startup_allows_non_loopback_with_token() {
        assert!(startup_refusal("0.0.0.0:8080", &Some("secret".into())).is_none());
    }

    #[test]
    fn startup_allows_loopback_without_token() {
        assert!(startup_refusal("127.0.0.1:8080", &None).is_none());
    }

    /// A minimal `AppState` for exercising `require_auth` in isolation, without
    /// touching a real OCR engine or inferer (neither is called by the
    /// middleware itself).
    fn state_with_token(token: Option<&str>) -> Arc<AppState> {
        let pipeline = Pipeline::new(
            Box::new(DoclingEngine::new("http://localhost:5001")),
            "http://127.0.0.1:50051",
        );
        Arc::new(AppState {
            pipeline,
            work_dir: PathBuf::from("work"),
            keep_work: false,
            token: token.map(str::to_string),
        })
    }

    fn probe_app(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/probe", get(|| async { StatusCode::OK }))
            .layer(middleware::from_fn_with_state(state.clone(), require_auth))
            .with_state(state)
    }

    async fn probe(app: Router, auth_header: Option<&str>) -> StatusCode {
        let mut req = Request::builder().uri("/probe");
        if let Some(h) = auth_header {
            req = req.header(AUTHORIZATION, h);
        }
        let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        resp.status()
    }

    #[tokio::test]
    async fn auth_rejects_missing_bearer_when_token_configured() {
        let app = probe_app(state_with_token(Some("secret")));
        assert_eq!(probe(app, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_rejects_wrong_bearer_when_token_configured() {
        let app = probe_app(state_with_token(Some("secret")));
        assert_eq!(
            probe(app, Some("Bearer wrong")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn auth_rejects_non_bearer_scheme() {
        let app = probe_app(state_with_token(Some("secret")));
        assert_eq!(
            probe(app, Some("Basic secret")).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn auth_accepts_correct_bearer_when_token_configured() {
        let app = probe_app(state_with_token(Some("secret")));
        assert_eq!(probe(app, Some("Bearer secret")).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_allows_any_request_when_no_token_configured() {
        let app = probe_app(state_with_token(None));
        assert_eq!(probe(app, None).await, StatusCode::OK);
    }
}
