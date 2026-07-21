//! Real-model end-to-end smoke test. Ignored by default (needs the two
//! `.rten` files at the repo root); run explicitly with:
//!
//! ```sh
//! cargo test -p synthpass-ocr --test native_ocr_e2e -- --ignored
//! ```

use std::path::PathBuf;
use synthpass_ocr::NativeOcr;

fn require_models() -> (PathBuf, PathBuf) {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let detection_path = repo_root.join("text-detection.rten");
    let recognition_path = repo_root.join("text-recognition.rten");
    assert!(
        detection_path.exists() && recognition_path.exists(),
        "model files not found at {} — download them first (see synthpass_ocr::download)",
        repo_root.display()
    );
    (detection_path, recognition_path)
}

#[test]
#[ignore]
fn native_ocr_recognizes_mrz_fragment_from_sample_passport() {
    let (detection_path, recognition_path) = require_models();
    let ocr = NativeOcr::load(&detection_path, &recognition_path).expect("models load");

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
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

/// M5 A3 tie-break: a page photographed/scanned fully upside-down (180°,
/// which `choose_rotation` alone cannot distinguish from upright — see its
/// doc comment) must still recover the same MRZ fragment as the upright
/// original, via the `mrz_band`-driven tie-break in `recognize_detailed`
/// (see `band_in_upper_third`). Ignored by default alongside the other
/// real-model test above; writes the rotated fixture next to the source
/// sample so this stays a single self-contained image-crate round trip, no
/// new dependency for temp-file handling.
#[test]
#[ignore]
fn native_ocr_recovers_mrz_from_a_180_degree_rotated_page() {
    let (detection_path, recognition_path) = require_models();
    let ocr = NativeOcr::load(&detection_path, &recognition_path).expect("models load");

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_path = repo_root.join("samples/Croatian_passport_data_page.jpg");
    let upright = image::open(&source_path)
        .expect("sample image opens")
        .into_rgb8();
    let flipped = image::imageops::rotate180(&upright);

    let rotated_path = std::env::temp_dir().join("synthpass_ocr_test_croatian_passport_180.png");
    flipped.save(&rotated_path).expect("rotated fixture writes");

    let page = ocr
        .recognize_detailed(&rotated_path)
        .expect("recognition succeeds on the rotated fixture");

    let _ = std::fs::remove_file(&rotated_path);

    let upper = page.text.to_uppercase();
    assert!(
        upper.contains("SPECIMEN") || upper.contains("HRV"),
        "expected the 180°-rotated fixture to recover the same MRZ fragment \
         as the upright original, got: {}",
        page.text
    );
    assert_eq!(
        page.rotation, 180,
        "expected the tie-break to report a 180° correction, got {}",
        page.rotation
    );
}
