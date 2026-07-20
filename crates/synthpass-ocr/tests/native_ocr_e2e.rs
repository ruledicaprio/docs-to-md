//! Real-model end-to-end smoke test. Ignored by default (needs the two
//! `.rten` files at the repo root); run explicitly with:
//!
//! ```sh
//! cargo test -p synthpass-ocr --test native_ocr_e2e -- --ignored
//! ```

use synthpass_ocr::NativeOcr;
use std::path::PathBuf;

#[test]
#[ignore]
fn native_ocr_recognizes_mrz_fragment_from_sample_passport() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let detection_path = repo_root.join("text-detection.rten");
    let recognition_path = repo_root.join("text-recognition.rten");
    assert!(
        detection_path.exists() && recognition_path.exists(),
        "model files not found at {} — download them first (see synthpass_ocr::download)",
        repo_root.display()
    );

    let ocr = NativeOcr::load(&detection_path, &recognition_path).expect("models load");

    let image_path = repo_root.join("samples/Croatian_passport_data_page.jpg");
    let text = ocr.recognize(&image_path).expect("recognition succeeds");

    assert!(!text.is_empty(), "expected non-empty OCR output");
    // The sample is a published specimen passport; its MRZ line contains
    // "HRV" (Croatia's ICAO nationality code) and "SPECIMEN".
    let upper = text.to_uppercase();
    assert!(
        upper.contains("SPECIMEN") || upper.contains("HRV"),
        "expected a recognizable MRZ fragment in OCR output, got: {text}"
    );
}
