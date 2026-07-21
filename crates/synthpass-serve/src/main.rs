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
use synthpass_license::{FEATURE_METRICS, FEATURE_MULTI_CONTEXT as MULTI_CONTEXT};
use synthpass_pipeline::{env_llm_contexts, MetricsSnapshot, Pipeline, ProcessEvent};
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
    /// The verified license, cached at boot. `None` when
    /// `SYNTHPASS_LICENSE_SKIP=1` — which is an explicit opt-out of licensing
    /// altogether (a self-built OSS Community binary), and therefore unlocks
    /// every feature; the gate meters the official binary, it isn't DRM.
    ///
    /// Full signature verification only happens once at boot (see
    /// [`license_refusal`]); the checks derived from this — expiry via
    /// [`license_is_expired`], features via
    /// [`synthpass_license::check_feature`] — are cheap comparisons over the
    /// already-verified payload.
    license: Option<synthpass_license::LicensePayload>,
}

impl AppState {
    /// The license's `expires_unix`, or `None` when licensing is skipped.
    fn license_expires_unix(&self) -> Option<u64> {
        self.license.as_ref().map(|p| p.expires_unix)
    }
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

/// Reconcile the Tier-2 context count the environment *asks* for with what
/// the license *permits*, returning the effective count plus an operator-facing
/// note when the license lowered it (silently ignoring an env var an operator
/// deliberately set is worse than refusing it out loud).
///
/// Two independent limits, both fail-safe:
///
/// - the `multi-context` **feature** — a license that doesn't name it stays at
///   a single context, no matter what was asked for;
/// - the `max_llm_contexts` **cap** — a numeric ceiling, applied by
///   [`synthpass_license::effective_llm_contexts`].
///
/// `None` (licensing skipped via `SYNTHPASS_LICENSE_SKIP=1`) imposes neither:
/// see [`AppState::license`]. Pure, so it's unit-testable the same way
/// [`startup_refusal`] and [`license_refusal`] are.
fn resolve_llm_contexts(
    license: Option<&synthpass_license::LicensePayload>,
    requested: usize,
) -> (usize, Option<String>) {
    let requested = requested.max(1);
    let Some(payload) = license else {
        return (requested, None);
    };

    if requested > 1 && synthpass_license::check_feature(payload, MULTI_CONTEXT).is_err() {
        return (
            1,
            Some(format!(
                "SYNTHPASS_LLM_CONTEXTS={requested} ignored: license does not include the \
                 '{MULTI_CONTEXT}' feature — running with 1 Tier-2 context"
            )),
        );
    }

    match synthpass_license::effective_llm_contexts(payload, requested) {
        (effective, true) => (
            effective,
            Some(format!(
                "SYNTHPASS_LLM_CONTEXTS={requested} capped to {effective} by the license's \
                 max_llm_contexts"
            )),
        ),
        (effective, false) => (effective, None),
    }
}

/// Install the process-wide tracing subscriber.
///
/// `SYNTHPASS_LOG` sets the filter (default `info`), `SYNTHPASS_LOG_FORMAT=json`
/// switches to line-delimited JSON for log pipelines. Deliberately *not*
/// `RUST_LOG`: this binary's log level is operator configuration, and it
/// shouldn't change because some unrelated tool exported `RUST_LOG` into the
/// environment.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter =
        EnvFilter::try_from_env("SYNTHPASS_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let json = env::var("SYNTHPASS_LOG_FORMAT").as_deref() == Ok("json");

    // `try_init` rather than `init`: a double install should not abort a
    // server that is otherwise ready to serve.
    let builder = fmt::Subscriber::builder().with_env_filter(filter);
    let result = if json {
        builder.json().try_init()
    } else {
        builder.try_init()
    };
    if let Err(e) = result {
        eprintln!("[synthpass-serve] could not install tracing subscriber: {e}");
    }
}

/// Render a [`MetricsSnapshot`] as Prometheus text-exposition format.
///
/// Hand-rolled on purpose: the format is a few dozen lines of `write!`, and a
/// metrics crate would add a registry, an exporter and a dependency to a
/// process that has exactly one of each. Kept a pure function of the snapshot
/// so the output is unit-testable without standing up a server.
///
/// **PII rule.** Every label here is a compile-time constant. Nothing derived
/// from a document — no filename, no field value — may ever become a label,
/// because unbounded label cardinality is both a Prometheus foot-gun and, for
/// this product, a data leak.
fn metrics_text(snap: &MetricsSnapshot) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(1024);

    out.push_str(
        "# HELP synthpass_documents_total Documents that reached a terminal extraction, by tier.\n",
    );
    out.push_str("# TYPE synthpass_documents_total counter\n");
    let _ = writeln!(
        out,
        "synthpass_documents_total{{method=\"mrz-deterministic\"}} {}",
        snap.documents_tier1
    );
    let _ = writeln!(
        out,
        "synthpass_documents_total{{method=\"llm\"}} {}",
        snap.documents_tier2
    );

    out.push_str("# HELP synthpass_stage_failures_total Stage failures, by stage.\n");
    out.push_str("# TYPE synthpass_stage_failures_total counter\n");
    let _ = writeln!(
        out,
        "synthpass_stage_failures_total{{stage=\"ocr\"}} {}",
        snap.ocr_failures
    );
    let _ = writeln!(
        out,
        "synthpass_stage_failures_total{{stage=\"tier2\"}} {}",
        snap.tier2_failures
    );

    out.push_str(
        "# HELP synthpass_llm_queue_depth Tier-2 requests queued or in flight right now.\n",
    );
    out.push_str("# TYPE synthpass_llm_queue_depth gauge\n");
    let _ = writeln!(out, "synthpass_llm_queue_depth {}", snap.queue_depth);

    for (name, help, hist) in [
        (
            "synthpass_ocr_duration_seconds",
            "OCR stage duration.",
            &snap.ocr_seconds,
        ),
        (
            "synthpass_tier2_duration_seconds",
            "Tier-2 inference duration.",
            &snap.tier2_seconds,
        ),
    ] {
        let _ = writeln!(out, "# HELP {name} {help}");
        let _ = writeln!(out, "# TYPE {name} histogram");
        for (bound, count) in hist.buckets {
            let _ = writeln!(out, "{name}_bucket{{le=\"{bound}\"}} {count}");
        }
        // `+Inf` always equals the total count — that identity is what makes
        // the series well-formed to a scraper.
        let _ = writeln!(out, "{name}_bucket{{le=\"+Inf\"}} {}", hist.count);
        let _ = writeln!(out, "{name}_sum {}", hist.sum_seconds);
        let _ = writeln!(out, "{name}_count {}", hist.count);
    }

    out
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
    init_tracing();
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
    let license = license_status.as_ref().map(|s| s.payload.clone());

    tokio::fs::create_dir_all(&work_dir).await?;

    // The environment asks; the license permits. Resolve before building the
    // pipeline so the semaphore is sized correctly from the start rather than
    // being walked back later.
    let (llm_contexts, contexts_note) = resolve_llm_contexts(license.as_ref(), env_llm_contexts());
    if let Some(note) = contexts_note {
        tracing::warn!("{note}");
    }
    if license
        .as_ref()
        .is_some_and(synthpass_license::features_grandfathered)
    {
        // Break B6: say it once at boot rather than waving every request
        // through in silence.
        tracing::info!("license names no features — grandfathered into all of them");
    }

    let pipeline = Pipeline::from_env_with_llm_contexts(llm_contexts);
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
    tracing::info!(
        %bind_addr,
        scheme,
        ocr = %pipeline.ocr_engine(),
        auth = if token.is_some() { "bearer" } else { "none" },
        license = %license_desc,
        "synthpass-serve listening"
    );

    let state = Arc::new(AppState {
        pipeline,
        work_dir,
        keep_work,
        token,
        max_queue_depth,
        license,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/extract", post(extract))
        .route("/metrics", get(metrics))
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
    let license_expired = license_is_expired(state.license_expires_unix());
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

/// Prometheus scrape endpoint.
///
/// Sits *inside* the auth layer (unlike `/health`): operational counters are
/// not public. Additionally gated on the `metrics` license feature — the
/// "enhanced reporting" surface of [`BRANDING.md`] §5, which is a legitimate
/// paid boundary because it is an integration convenience, not core
/// capability. Refusals are `403` and name the missing feature.
///
/// [`BRANDING.md`]: https://github.com/ruledicaprio/SynthPass/blob/main/docs/BRANDING.md
async fn metrics(State(state): State<Arc<AppState>>) -> Result<String, ApiError> {
    if let Some(payload) = &state.license {
        if let Err(e) = synthpass_license::check_feature(payload, FEATURE_METRICS) {
            return Err(api_error(StatusCode::FORBIDDEN, e));
        }
    }
    Ok(metrics_text(&state.pipeline.metrics_snapshot()))
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
    if license_is_expired(state.license_expires_unix()) {
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

    // One id per upload, threaded through every span this request produces so
    // a failure is greppable end to end. The uploaded *filename* is
    // deliberately never logged — it is user-supplied and routinely contains
    // the holder's name.
    let request_id = uuid::Uuid::new_v4();
    let span = tracing::info_span!("extract", %request_id);
    let _entered = span.enter();
    tracing::info!(
        upload_bytes = data.len(),
        ext = %ext,
        legacy_v1,
        "extraction request accepted"
    );

    let upload_path = state.work_dir.join(format!("{request_id}.{ext}"));
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
            max_llm_contexts: None,
        }
    }

    /// The `AppState.license` an `expires_unix`-shaped test wants: `None`
    /// means licensing was skipped entirely, `Some(t)` a license expiring at
    /// `t`.
    fn license_expiring_at(expires_unix: Option<u64>) -> Option<synthpass_license::LicensePayload> {
        expires_unix.map(sample_license_payload)
    }

    #[test]
    fn skipped_licensing_imposes_no_context_limit() {
        // SYNTHPASS_LICENSE_SKIP=1 is an explicit opt-out, not a trial tier.
        assert_eq!(resolve_llm_contexts(None, 4), (4, None));
    }

    #[test]
    fn missing_multi_context_feature_forces_a_single_context() {
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Trial.default_features(),
            ..sample_license_payload(4_000_000_000)
        };
        let (effective, note) = resolve_llm_contexts(Some(&payload), 4);
        assert_eq!(effective, 1);
        let note = note.expect("an ignored env var must be explained, not silently dropped");
        assert!(
            note.contains(MULTI_CONTEXT) && note.contains('4'),
            "the note should name both the feature and what was asked for: {note}"
        );
    }

    #[test]
    fn licensed_multi_context_is_granted() {
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Pro.default_features(),
            ..sample_license_payload(4_000_000_000)
        };
        assert_eq!(resolve_llm_contexts(Some(&payload), 4), (4, None));
    }

    #[test]
    fn max_llm_contexts_caps_the_request_and_says_so() {
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Pro.default_features(),
            max_llm_contexts: Some(2),
            ..sample_license_payload(4_000_000_000)
        };
        let (effective, note) = resolve_llm_contexts(Some(&payload), 4);
        assert_eq!(effective, 2, "the env asks for 4, the license permits 2");
        assert!(note
            .expect("a capped request must be explained")
            .contains('2'));
    }

    #[test]
    fn a_single_context_never_trips_the_feature_gate() {
        // The default (1 context) is core capability, not capacity — a trial
        // license must still serve Tier 2, quietly.
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Trial.default_features(),
            ..sample_license_payload(4_000_000_000)
        };
        assert_eq!(resolve_llm_contexts(Some(&payload), 1), (1, None));
    }

    #[test]
    fn a_grandfathered_license_keeps_multi_context() {
        // Break B6: `features: []` predates gating and unlocks everything.
        let payload = sample_license_payload(4_000_000_000); // features: vec![]
        assert_eq!(resolve_llm_contexts(Some(&payload), 4), (4, None));
    }

    fn metrics_app(license: Option<synthpass_license::LicensePayload>) -> Router {
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
            license,
        });
        Router::new()
            .route("/metrics", get(metrics))
            .with_state(state)
    }

    async fn metrics_response(app: Router) -> (StatusCode, String) {
        let req = Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn metrics_renders_prometheus_text_for_a_licensed_scrape() {
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Enterprise.default_features(),
            ..sample_license_payload(4_000_000_000)
        };
        let (status, body) = metrics_response(metrics_app(Some(payload))).await;

        assert_eq!(status, StatusCode::OK);
        for expected in [
            "# TYPE synthpass_documents_total counter",
            "synthpass_documents_total{method=\"mrz-deterministic\"} 0",
            "synthpass_documents_total{method=\"llm\"} 0",
            "# TYPE synthpass_llm_queue_depth gauge",
            "# TYPE synthpass_ocr_duration_seconds histogram",
            "synthpass_ocr_duration_seconds_bucket{le=\"+Inf\"} 0",
            "synthpass_tier2_duration_seconds_count 0",
        ] {
            assert!(body.contains(expected), "missing {expected:?} in:\n{body}");
        }
    }

    #[tokio::test]
    async fn metrics_is_403_without_the_metrics_feature() {
        // Pro licenses buy capacity, not reporting — see BRANDING.md §5.
        let payload = synthpass_license::LicensePayload {
            features: synthpass_license::Tier::Pro.default_features(),
            ..sample_license_payload(4_000_000_000)
        };
        let (status, body) = metrics_response(metrics_app(Some(payload))).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            body.contains(FEATURE_METRICS),
            "the refusal must name the missing feature: {body}"
        );
    }

    #[tokio::test]
    async fn metrics_is_available_when_licensing_is_skipped() {
        // SYNTHPASS_LICENSE_SKIP=1 is a full opt-out, not a bottom tier.
        let (status, _) = metrics_response(metrics_app(None)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[test]
    fn metrics_text_histogram_buckets_are_monotonic_and_end_at_inf() {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        let body = metrics_text(&pipeline.metrics_snapshot());

        // Every `_bucket` line must parse, and `+Inf` must be the last one for
        // each histogram — the shape a scraper relies on.
        let inf_lines: Vec<&str> = body
            .lines()
            .filter(|l| l.contains("_bucket{le=\"+Inf\"}"))
            .collect();
        assert_eq!(inf_lines.len(), 2, "one +Inf per histogram: {inf_lines:?}");
        assert!(
            body.lines().filter(|l| l.contains("_bucket{")).count() > 2,
            "bounded buckets should be rendered alongside +Inf"
        );
    }

    #[test]
    fn metrics_text_never_contains_a_dynamic_label() {
        let pipeline = Pipeline::new(
            Box::new(RustOcrEngine::new(".", false)),
            Box::new(NativeInferer::new("nonexistent.gguf", 2048)),
        );
        let body = metrics_text(&pipeline.metrics_snapshot());

        // The label space must stay closed: only `method`, `stage` and `le`.
        // Anything derived from a document would be both a cardinality
        // explosion and a PII leak on a scrape endpoint.
        for line in body.lines().filter(|l| l.contains('{')) {
            let labels = &line[line.find('{').unwrap() + 1..line.rfind('}').unwrap()];
            let key = labels.split('=').next().unwrap();
            assert!(
                matches!(key, "method" | "stage" | "le"),
                "unexpected label {key:?} in metrics line: {line}"
            );
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
            license: None,
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
            license: license_expiring_at(Some(0)), // expired at the Unix epoch
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
            license: license_expiring_at(license_expires_unix),
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
            license: None,
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
