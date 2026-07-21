//! synthpass-serve — web front-end for the multi-level-id-strip pipeline.
//!
//! GET  /             → embedded upload page
//! GET  /health       → liveness + OCR/inference backend + license status (no auth)
//! POST /api/extract  → multipart file upload → shared `synthpass-pipeline` crate
//!                      (OCR engine → Markdown → Tier 1 MRZ → Tier 2 LLM → JSON)
//!
//! Run from the repository root (`cargo run -p synthpass-serve`) so the inferer
//! sidecar and OCR engine resolve.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Query, Request, State},
    http::{
        header::{AUTHORIZATION, RETRY_AFTER},
        HeaderMap, HeaderValue, StatusCode,
    },
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, Json, Response,
    },
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use synthpass_pipeline::{Pipeline, ProcessEvent};
use tokio_stream::{wrappers::ReceiverStream, Stream, StreamExt};

const INDEX_HTML: &str = include_str!("index.html");
const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024; // 20 MB

struct AppState {
    pipeline: Pipeline,
    work_dir: PathBuf,
    keep_work: bool,
    /// Tier 3: when set, every request must present `Authorization: Bearer <token>`.
    token: Option<String>,
    /// Reject new uploads with 503 once `Pipeline::llm_queue_depth()` reaches
    /// this many queued/in-flight Tier-2 requests, instead of accepting them
    /// unboundedly and leaving them to block behind the single-GPU semaphore.
    max_queue_depth: usize,
    /// The license's `expires_unix`, cached at boot. `None` when
    /// `SYNTHPASS_LICENSE_SKIP=1`. Full signature verification only happens once
    /// at boot (see [`license_refusal`]); this cheap expiry-only comparison
    /// runs per-request so a long-running server stops serving once expired,
    /// without re-verifying the signature on every request.
    license_expires_unix: Option<u64>,
}

type ApiError = (StatusCode, HeaderMap, Json<Value>);

/// Seconds a queue-full client should wait before retrying — a fixed,
/// conservative value rather than anything derived from actual queue
/// dynamics (this server has no ETA to offer, just "not now").
const QUEUE_FULL_RETRY_AFTER_SECS: u64 = 5;

/// The vendor media type legacy clients can send to keep the v1 response shape.
const LEGACY_V1_ACCEPT: &str = "application/vnd.mlis.v1+json";

/// Legacy-client version negotiation (breaking change B2, `docs/V2-DESIGN.md`
/// §9): the `/api/extract` SSE `result` event carries `extracted_v2` by
/// default; a client asking for `?v=1` or sending
/// `Accept: application/vnd.mlis.v1+json` gets the v1-only shape for one
/// major release. Pure function of the request parts so it's unit-testable.
fn wants_legacy_v1(params: &HashMap<String, String>, headers: &HeaderMap) -> bool {
    if params.get("v").map_or(false, |v| v == "1") {
        return true;
    }
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map_or(false, |accept| accept.contains(LEGACY_V1_ACCEPT))
}

fn api_error(status: StatusCode, msg: impl std::fmt::Display) -> ApiError {
    (
        status,
        HeaderMap::new(),
        Json(json!({ "error": msg.to_string() })),
    )
}

/// Like [`api_error`], but with a `Retry-After` header — scoped strictly to
/// the queue-full case (the only 503 this server issues that the client can
/// meaningfully do something about by waiting; an expired license won't fix
/// itself in `N` seconds, so that path stays on plain `api_error`).
fn queue_full_error(msg: impl std::fmt::Display) -> ApiError {
    let (status, mut headers, body) = api_error(StatusCode::SERVICE_UNAVAILABLE, msg);
    headers.insert(
        RETRY_AFTER,
        HeaderValue::from_str(&QUEUE_FULL_RETRY_AFTER_SECS.to_string())
            .expect("a formatted u64 is always a valid header value"),
    );
    (status, headers, body)
}

/// `true` iff `license_expires_unix` names a real expiry that's already
/// passed. `None` (license checking skipped via `SYNTHPASS_LICENSE_SKIP=1`)
/// is never considered expired. Factored out of `extract`'s inline check so
/// `/health` can report the same status without duplicating the comparison.
fn license_is_expired(license_expires_unix: Option<u64>) -> bool {
    license_expires_unix
        .is_some_and(|expires_unix| synthpass_license::current_unix() > expires_unix)
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
            "refusing to bind non-loopback ({bind_addr}) without SYNTHPASS_TOKEN — this service \
             processes identity documents. Set SYNTHPASS_TOKEN to require Bearer auth (and set \
             SYNTHPASS_TLS_CERT/SYNTHPASS_TLS_KEY for TLS)."
        ))
    } else {
        None
    }
}

/// Default path for the license file when `SYNTHPASS_LICENSE_PATH` is unset.
const DEFAULT_LICENSE_PATH: &str = "license.mlis";

/// License startup gate: refuse to boot without a valid license, unless
/// `SYNTHPASS_LICENSE_SKIP=1`. Pure — takes the already-computed check result
/// rather than doing file IO itself, so it's unit-testable the same way
/// [`startup_refusal`] is. Returns the status to cache in `AppState` on
/// success (`None` when skipped), or the refusal message to exit with.
fn license_refusal(
    skip: bool,
    check: Result<synthpass_license::LicenseStatus, synthpass_license::LicenseError>,
) -> Result<Option<synthpass_license::LicenseStatus>, String> {
    if skip {
        return Ok(None);
    }
    match check {
        Ok(status) => Ok(Some(status)),
        Err(e) => Err(format!(
            "refusing to start without a valid license: {e} (set SYNTHPASS_LICENSE_SKIP=1 for local development)"
        )),
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
    let token = env::var("SYNTHPASS_TOKEN").ok().filter(|s| !s.is_empty());
    let max_queue_depth = env::var("SYNTHPASS_MAX_QUEUE_DEPTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);

    // Refuse to expose PII processing wide-open: a non-loopback bind requires
    // an auth token (and, ideally, TLS in front).
    if let Some(reason) = startup_refusal(&bind_addr, &token) {
        return Err(reason.into());
    }

    let license_skip = env::var("SYNTHPASS_LICENSE_SKIP").as_deref() == Ok("1");
    let license_path =
        env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    let license_status = match license_refusal(
        license_skip,
        synthpass_license::load_and_check(Path::new(&license_path)),
    ) {
        Ok(status) => status,
        Err(reason) => return Err(reason.into()),
    };
    let license_expires_unix = license_status.as_ref().map(|s| s.payload.expires_unix);

    tokio::fs::create_dir_all(&work_dir).await?;

    let pipeline = Pipeline::from_env();
    let scheme = if env::var("SYNTHPASS_TLS_CERT").is_ok() {
        "https"
    } else {
        "http"
    };
    let license_desc = match &license_status {
        Some(status) => format!(
            "{} (expires in {} days)",
            status.payload.tier,
            status.days_until_expiry(synthpass_license::current_unix())
        ),
        None => "skipped".to_string(),
    };
    println!(
        "🚀 [synthpass-serve] Listening on {scheme}://{bind_addr} (ocr: {}, auth: {}, license: {license_desc})",
        pipeline.ocr_engine(),
        if token.is_some() { "bearer" } else { "none" }
    );

    let state = Arc::new(AppState {
        pipeline,
        work_dir,
        keep_work,
        token,
        max_queue_depth,
        license_expires_unix,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/extract", post(extract))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        // Merged in after the auth layer so /health stays reachable without
        // credentials — infra health probes typically don't carry one, and a
        // health check that itself requires auth defeats part of its purpose.
        .merge(Router::new().route("/health", get(health)))
        .with_state(state);

    // Optional TLS (rustls) when both cert and key are provided.
    match (
        env::var("SYNTHPASS_TLS_CERT"),
        env::var("SYNTHPASS_TLS_KEY"),
    ) {
        (Ok(cert), Ok(key)) => {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let addr: std::net::SocketAddr = bind_addr.parse().map_err(|e| {
                format!("SYNTHPASS_TLS requires BIND_ADDR as IP:port ({bind_addr}): {e}")
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

/// Liveness/readiness probe: OCR engine + inference-backend identity, a real
/// (but bounded — [`synthpass_pipeline::Pipeline::infer_health`] already
/// enforces its own budget) inference health check, and license-expiry
/// status. No PII, no auth required (see the router wiring in `main`).
///
/// Always responds `200`: a health check reporting an unhealthy component is
/// itself a successful probe (unlike `/api/extract`, which must actively
/// refuse to do work in those states) — callers should inspect the body, not
/// just the status code.
async fn health(State(state): State<Arc<AppState>>) -> Json<Value> {
    let infer = state.pipeline.infer_health().await;
    let infer_ok = infer.is_ok();
    let license_expired = license_is_expired(state.license_expires_unix);
    Json(json!({
        "status": if infer_ok && !license_expired { "ok" } else { "degraded" },
        "ocr_engine": state.pipeline.ocr_engine(),
        "infer": {
            "backend": state.pipeline.infer_describe(),
            "ok": infer_ok,
            "detail": infer.unwrap_or_else(|e| e),
        },
        "license": {
            "expired": license_expired,
        },
    }))
}

async fn extract(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // Version negotiation happens up front; the SSE closure below captures
    // the verdict and decides whether `extracted_v2` rides along (B2).
    let legacy_v1 = wants_legacy_v1(&params, &headers);

    // Reject fast under overload rather than accepting the upload and
    // leaving it to block behind the single-GPU inference semaphore. Tier-1
    // (MRZ) requests never touch the queue, so this only ever costs a
    // Tier-2-bound request a queued slot it wouldn't have gotten anyway.
    if state.pipeline.llm_queue_depth() >= state.max_queue_depth {
        return Err(queue_full_error(
            "inference queue is full — try again shortly",
        ));
    }

    // Cheap expiry-only check (no signature re-verification — that only
    // happens once at boot) so a long-running server past its license's
    // expiry stops serving instead of running indefinitely on a stale check.
    if license_is_expired(state.license_expires_unix) {
        return Err(api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "license has expired",
        ));
    }

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

    // Keep only a sanitized extension; the OCR engine uses it for format detection
    // (image vs. the now-unsupported PDF/HEIC — see `synthpass-pipeline::ocr`).
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

    // Drive the pipeline in the background, forwarding events to the SSE
    // stream as they arrive so the browser sees Tier-2 progress instead of a
    // single blocking response.
    let (tx, rx) = tokio::sync::mpsc::channel::<ProcessEvent>(16);
    let bg_state = state.clone();
    let bg_upload_path = upload_path.clone();
    tokio::spawn(async move {
        bg_state
            .pipeline
            .process_document_stream(&bg_upload_path, tx)
            .await;
    });

    let stream = ReceiverStream::new(rx).then(move |event| {
        let state = state.clone();
        let filename = filename.clone();
        let upload_path = upload_path.clone();
        async move {
            let sse_event = match event {
                ProcessEvent::Delta(text) => Event::default().event("delta").data(text),
                ProcessEvent::Done(result) => {
                    let mut resp = json!({
                        "filename": filename,
                        "markdown": result.markdown,
                        "extracted": result.extracted,
                        "method": result.method.as_str(),
                        "mrz": result.mrz.as_ref().map(|m| serde_json::to_value(m).expect("MrzData serializes")),
                        "error": result.llm_error,
                    });
                    // v2 schema by default; suppressed for legacy clients that
                    // negotiated v1 via `?v=1` or the vendor Accept type (B2).
                    if !legacy_v1 {
                        resp["extracted_v2"] = result
                            .extracted_v2
                            .as_ref()
                            .map(|v2| serde_json::to_value(v2).expect("ExtractionV2 serializes"))
                            .unwrap_or(Value::Null);
                    }
                    cleanup(&state, &[&upload_path, &result.md_path, &result.json_path]).await;
                    Event::default().event("result").data(resp.to_string())
                }
                ProcessEvent::Failed(msg) => {
                    cleanup(&state, &[&upload_path]).await;
                    Event::default().event("error").data(msg)
                }
            };
            Ok::<_, Infallible>(sse_event)
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
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
    use synthpass_pipeline::{NativeInferer, Pipeline, RustOcrEngine};
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

    fn headers_with_accept(accept: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::ACCEPT, accept.parse().unwrap());
        h
    }

    #[test]
    fn legacy_v1_default_is_v2() {
        assert!(!wants_legacy_v1(&HashMap::new(), &HeaderMap::new()));
    }

    #[test]
    fn legacy_v1_via_query_param() {
        let params = HashMap::from([("v".to_string(), "1".to_string())]);
        assert!(wants_legacy_v1(&params, &HeaderMap::new()));
    }

    #[test]
    fn legacy_v1_query_param_must_be_exactly_1() {
        let params = HashMap::from([("v".to_string(), "2".to_string())]);
        assert!(!wants_legacy_v1(&params, &HeaderMap::new()));
    }

    #[test]
    fn legacy_v1_via_accept_header() {
        assert!(wants_legacy_v1(
            &HashMap::new(),
            &headers_with_accept("application/vnd.mlis.v1+json")
        ));
        // Tolerates a compound Accept list, as browsers send.
        assert!(wants_legacy_v1(
            &HashMap::new(),
            &headers_with_accept("text/html, application/vnd.mlis.v1+json;q=0.9")
        ));
    }

    #[test]
    fn legacy_v1_plain_json_accept_stays_v2() {
        assert!(!wants_legacy_v1(
            &HashMap::new(),
            &headers_with_accept("application/json")
        ));
    }

    #[test]
    fn license_is_expired_cases() {
        assert!(
            !license_is_expired(None),
            "skipped license checking is never expired"
        );
        assert!(
            !license_is_expired(Some(4_000_000_000)),
            "far-future expiry is not expired"
        );
        assert!(
            license_is_expired(Some(0)),
            "unix-epoch expiry is in the past"
        );
    }

    fn sample_license_payload(expires_unix: u64) -> synthpass_license::LicensePayload {
        synthpass_license::LicensePayload {
            license_id: "test-license".into(),
            customer: "Test Customer".into(),
            hw_fingerprint: String::new(),
            issued_unix: 0,
            expires_unix,
            tier: "enterprise".into(),
            features: vec![],
            mlis_min_version: None,
        }
    }

    #[test]
    fn license_refuses_boot_when_check_fails_and_not_skipped() {
        let err = synthpass_license::LicenseError::Expired { expires_unix: 0 };
        assert!(license_refusal(false, Err(err)).is_err());
    }

    #[test]
    fn license_allows_boot_when_skipped_even_if_check_fails() {
        let err = synthpass_license::LicenseError::Expired { expires_unix: 0 };
        let result = license_refusal(true, Err(err));
        assert!(
            matches!(result, Ok(None)),
            "skip must bypass a failed check"
        );
    }

    #[test]
    fn license_returns_status_when_check_succeeds() {
        let status = synthpass_license::LicenseStatus {
            payload: sample_license_payload(4_000_000_000),
        };
        let result = license_refusal(false, Ok(status));
        assert!(matches!(result, Ok(Some(_))));
    }

    #[tokio::test]
    async fn extract_rejects_with_503_when_queue_is_full() {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        // max_queue_depth: 0 means "full" even with zero in-flight requests —
        // exercises the rejection branch without needing a real inferer.
        let state = Arc::new(AppState {
            pipeline,
            work_dir: PathBuf::from("work"),
            keep_work: false,
            token: None,
            max_queue_depth: 0,
            license_expires_unix: None,
        });
        let app = Router::new()
            .route("/api/extract", post(extract))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/extract")
            .header("content-type", "multipart/form-data; boundary=X-BOUNDARY-X")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get(RETRY_AFTER),
            Some(&HeaderValue::from_static("5")),
            "queue-full is the one 503 a client can meaningfully retry after"
        );
    }

    #[tokio::test]
    async fn extract_rejects_with_503_when_license_expired() {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        let state = Arc::new(AppState {
            pipeline,
            work_dir: PathBuf::from("work"),
            keep_work: false,
            token: None,
            max_queue_depth: 4,
            license_expires_unix: Some(0), // expired at the Unix epoch
        });
        let app = Router::new()
            .route("/api/extract", post(extract))
            .with_state(state);

        let req = Request::builder()
            .method("POST")
            .uri("/api/extract")
            .header("content-type", "multipart/form-data; boundary=X-BOUNDARY-X")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            resp.headers().get(RETRY_AFTER),
            None,
            "an expired license won't fix itself by waiting, so this path stays plain"
        );
    }

    fn health_app(license_expires_unix: Option<u64>) -> Router {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        let state = Arc::new(AppState {
            pipeline,
            work_dir: PathBuf::from("work"),
            keep_work: false,
            token: None,
            max_queue_depth: 4,
            license_expires_unix,
        });
        Router::new()
            .route("/health", get(health))
            .with_state(state)
    }

    async fn health_body(app: Router) -> Value {
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "health always responds 200");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_reports_expected_fields_when_license_valid() {
        let body = health_body(health_app(Some(4_000_000_000))).await;
        assert!(body["ocr_engine"].is_string());
        assert!(body["infer"]["backend"].is_string());
        assert!(body["infer"]["ok"].is_boolean());
        assert_eq!(body["license"]["expired"], false);
    }

    #[tokio::test]
    async fn health_reflects_expired_license() {
        let body = health_body(health_app(Some(0))).await;
        assert_eq!(body["license"]["expired"], true);
        assert_eq!(
            body["status"], "degraded",
            "an expired license alone should mark the service degraded"
        );
    }

    /// A minimal `AppState` for exercising `require_auth` in isolation, without
    /// touching a real OCR engine or inferer (neither is called by the
    /// middleware itself).
    fn state_with_token(token: Option<&str>) -> Arc<AppState> {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        Arc::new(AppState {
            pipeline,
            work_dir: PathBuf::from("work"),
            keep_work: false,
            token: token.map(str::to_string),
            max_queue_depth: 4,
            license_expires_unix: None,
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
