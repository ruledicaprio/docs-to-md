//! Cross-language E2E test: drives the **real** Python gRPC inferer (mock mode)
//! from the **real** Rust `InfererClient`, proving the wire contract holds
//! across the process boundary.
//!
//! Every other test mocks one side of the bridge: `crates/mlis-pipeline`'s unit
//! tests mock the *server* with a Rust `tonic` service, and `python/smoke_test.py`
//! mocks the *client* with a generated Python stub. Neither catches drift
//! between `proto/inferer.proto`, the Rust `tonic::include_proto!` output, and
//! the Python `grpcio` servicer (e.g. an incompatible protobuf/grpcio version,
//! a stub regenerated against a stale `.proto`, or a wire-format mismatch) — this
//! test does, by actually crossing the boundary.
//!
//! Requires a Python interpreter with the inferer's gRPC dependencies installed
//! and the stubs generated (`cd python && python generate_grpc.py`); needs no
//! GGUF model since `MLIS_INFERER_MOCK=1` short-circuits model loading. Ignored
//! by default so a plain `cargo test` never tries to spawn a Python process;
//! CI's `bridge` job prepares the interpreter and passes `--ignored`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use mlis_pipeline::inferer::inferer_client::InfererClient;
use mlis_pipeline::inferer::{ExtractRequest, HealthRequest};

/// Kills the spawned Python process even if an assertion panics mid-test.
struct InfererGuard(Child);

impl Drop for InfererGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn python_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python")
}

fn python_exe() -> String {
    std::env::var("PYTHON_EXE").unwrap_or_else(|_| "python".into())
}

#[tokio::test]
#[ignore = "spawns a real python process; needs the inferer's gRPC deps + \
            generated stubs — see python/pyproject.toml and generate_grpc.py, \
            or run CI's `bridge` job"]
async fn rust_client_round_trips_with_real_python_inferer() {
    let bind = "127.0.0.1:50199";
    let child = Command::new(python_exe())
        .args(["-m", "inferer"])
        .current_dir(python_dir())
        .env("MLIS_INFERER_MOCK", "1")
        .env("MLIS_INFERER_BIND", bind)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect(
            "failed to spawn `python -m inferer` — is python on PATH (or PYTHON_EXE set) \
             with grpcio/grpcio-tools/pydantic/protobuf installed and the stubs generated \
             (`cd python && python generate_grpc.py`)?",
        );
    let _guard = InfererGuard(child);

    // Mock mode starts fast (no model load); poll instead of a fixed sleep.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if tokio::net::TcpStream::connect(bind).await.is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "python inferer did not start listening on {bind} within 15s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let mut client = InfererClient::connect(format!("http://{bind}"))
        .await
        .expect("connect to the real python inferer over gRPC");

    let extract = client
        .extract(ExtractRequest {
            markdown: "P<UTO passport specimen".into(),
            image_roi: Vec::new(),
        })
        .await
        .expect("Extract RPC to the real python inferer")
        .into_inner();

    assert_eq!(extract.surname.as_deref(), Some("MOCK"));
    assert_eq!(extract.document_number.as_deref(), Some("M0"));
    assert!(extract.raw_json.contains("\"extraction_method\""));
    assert!(extract.raw_json.contains("\"llm\""));

    let health = client
        .health(HealthRequest {})
        .await
        .expect("Health RPC to the real python inferer")
        .into_inner();
    assert!(
        health.model_loaded,
        "mock mode should report model_loaded=true"
    );
}
