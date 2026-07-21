//! Real-model end-to-end smoke test. Ignored by default (needs the ~1 GB
//! GGUF at the repo root); run explicitly with:
//!
//! ```sh
//! cargo test -p synthpass-llm --test native_llm_e2e -- --ignored
//! ```
//!
//! `LlamaBackend::init()` is a process-wide singleton (a second call errors
//! with `BackendAlreadyInitialized`), matching production usage: one model
//! load for the process lifetime. So this file loads `NativeLlm` exactly
//! once and exercises both `extract` and `extract_stream` against it, rather
//! than one `NativeLlm` per `#[test]`.

use std::path::{Path, PathBuf};
use synthpass_llm::NativeLlm;

/// Locates `name` (a bare filename, no path) anywhere under `samples/`,
/// searching recursively — so this survives `samples/` being reorganized
/// into continent/class subfolders without every call site needing the
/// exact subpath hardcoded.
fn find_sample(name: &str) -> PathBuf {
    fn search(dir: &Path, name: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = search(&path, name) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|f| f.to_str()) == Some(name) {
                return Some(path);
            }
        }
        None
    }
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    search(&repo_root.join("samples"), name)
        .unwrap_or_else(|| panic!("sample file not found anywhere under samples/: {name}"))
}

#[test]
#[ignore]
fn native_llm_extracts_via_unary_and_streaming_calls() {
    let model_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../qwen2.5-1.5b-instruct-q4_k_m.gguf");
    assert!(
        model_path.exists(),
        "model not found at {} — download it first",
        model_path.display()
    );

    let llm = NativeLlm::load(&model_path, 2048).expect("model loads");

    let markdown = std::fs::read_to_string(find_sample("Croatian_passport_data_page.md"))
        .expect("sample markdown exists");

    let extraction = llm.extract(&markdown).expect("unary extraction succeeds");
    assert_eq!(extraction.extraction_method, "llm");
    // Exact field-accuracy parity against the Python inferer is a separate,
    // dedicated check (see the parity harness); this smoke test only proves
    // the native pipeline produces schema-valid, non-empty output end to end.
    assert!(
        extraction.document_number.is_some() || extraction.mrz_line.is_some(),
        "expected at least one populated field, got {extraction:?}"
    );

    let mut deltas = Vec::new();
    let streamed = llm
        .extract_stream(
            "Name: JOHN DOE. Nationality: Utopia. Passport number A1234567.",
            |delta| deltas.push(delta.to_string()),
        )
        .expect("streaming extraction succeeds");

    assert!(!deltas.is_empty(), "expected at least one delta");
    assert_eq!(streamed.extraction_method, "llm");
}
