//! Tier-1 accuracy harness: runs the in-process OCR engine over every
//! MRZ-bearing specimen in `samples/` and reports how many produce a
//! checksum-valid MRZ (the "Tier-1 hit rate" from docs/ARCHITECTURE.md §8).
//!
//! Run from the repo root (models auto-resolve there):
//! ```powershell
//! cargo run -p mlis-ocr --release --example mrz_corpus
//! ```
//! Pass `--dump` to print the raw OCR text for each miss.

use mlis_ocr::NativeOcr;
use std::path::{Path, PathBuf};

/// Every sample that physically carries an MRZ, with the expected document
/// number from its checked-in `.json` ground truth.
const CORPUS: &[(&str, &str)] = &[
    (
        "2022_cetis_terra_condifea_passport_datapage3rd_inner_page.jpg",
        "SD9990322",
    ),
    ("Croatian_passport_data_page.jpg", "007007007"),
    ("Estonia_PASSPORT_face.png", "KS0000182"),
    ("Passport_of_Serbia_ID_2009_version.jpg", "000000000"),
    ("SerbianID_back.png", "955555546"),
    ("Slovenian_ID_Card_2022_-_Rear.jpg", "IE9876543"),
    ("China_Passport_Specimen.webp", "E00000000"),
    ("Vietnam_Passport_Specimen.webp", "E00000000"),
    ("Oman_Passport_Specimen.jpg", "JL5989824"),
    ("United_Arab_Emirates_Passport_Specimen.jpg", "ZK8K81404"),
    // Known-MISS baseline (see Phase 0 of the multiscript-MRZ-robustness
    // plan), NOT a real ground-truth doc number: this specimen is a
    // publicly-posted "redacted sample" scan whose surname/given-name/
    // passport-number/ID-number/date fields — and every MRZ character
    // position — are physically blacked out with solid boxes on the source
    // image, not merely OCR-garbled. No amount of OCR/preprocessing
    // improvement can recover a checksum-valid MRZ from this file because
    // the ICAO check-digit data was never printed on the visible page in
    // the first place. It stays in the corpus as a stress test for the
    // pass-budget/graceful-degradation behavior (Phase 1) and as a
    // Hebrew-dense photographic-scan case for the row-density band
    // isolation — not for the hit-rate, which cannot reach 100% while this
    // entry is present. The placeholder value below can never match.
    (
        "Israel_Biometric_Passport.jpg",
        "REDACTED-NO-GROUND-TRUTH-MRZ",
    ),
];

/// Samples with no MRZ at all: the retry passes run in full (worst-case
/// latency) and must never produce a checksum-valid MRZ.
const NEGATIVE: &[&str] = &[
    "BulgariaID_face.png",
    "SerbianID_face.png",
    "Slovenian_ID_Card_2022_-_Front.jpg",
];

fn main() {
    let dump = std::env::args().any(|a| a == "--dump");
    let root = repo_root();
    let ocr = NativeOcr::load(
        &root.join("text-detection.rten"),
        &root.join("text-recognition.rten"),
    )
    .expect("failed to load OCR models — run from the repo root");

    let mut hits = 0usize;
    for (file, expected_doc) in CORPUS {
        let path = root.join("samples").join(file);
        let started = std::time::Instant::now();
        let text = match ocr.recognize(&path) {
            Ok(t) => t,
            Err(e) => {
                println!("MISS  {file}: OCR error: {e}");
                continue;
            }
        };
        let elapsed = started.elapsed();
        match mrz::find_and_parse(&text) {
            Ok(d) if d.valid() => {
                let doc_ok = d.document_number == *expected_doc;
                if doc_ok {
                    hits += 1;
                    println!(
                        "HIT   {file}: {} ({:?}, {elapsed:.1?})",
                        d.document_number, d.format
                    );
                } else {
                    println!(
                        "MISS  {file}: checksum-valid but WRONG doc number {} (expected {expected_doc})",
                        d.document_number
                    );
                }
            }
            Ok(d) => {
                println!(
                    "MISS  {file}: parsed but checksums failed: {:?} ({elapsed:.1?})",
                    d.checks
                );
                if dump {
                    println!(
                        "--- candidate ---\n{}\n--- ocr text ---\n{text}\n---",
                        d.mrz_lines
                    );
                }
            }
            Err(e) => {
                println!("MISS  {file}: {e} ({elapsed:.1?})");
                if dump {
                    println!("--- ocr text ---\n{text}\n---");
                }
            }
        }
    }
    let total = CORPUS.len();
    let pct = 100.0 * hits as f64 / total as f64;
    println!("\nTier-1 hit rate: {hits}/{total} = {pct:.0}%");

    println!("\nNegative controls (no MRZ present — must not validate):");
    for file in NEGATIVE {
        let path = root.join("samples").join(file);
        let started = std::time::Instant::now();
        let text = match ocr.recognize(&path) {
            Ok(t) => t,
            Err(e) => {
                println!("ERROR {file}: {e}");
                continue;
            }
        };
        let elapsed = started.elapsed();
        match mrz::find_and_parse(&text) {
            Ok(d) if d.valid() => println!(
                "FALSE-POSITIVE {file}: hallucinated valid MRZ {} ({elapsed:.1?})",
                d.document_number
            ),
            _ => println!("OK    {file}: no valid MRZ ({elapsed:.1?})"),
        }
    }
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/mlis-ocr → repo root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}
