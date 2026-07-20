//! Instant single-file check for a candidate `samples/` specimen: does it OCR
//! to a checksum-valid MRZ, and does the raw OCR text contain the word
//! "specimen" anywhere (a rough, non-authoritative signal only)?
//!
//! This does NOT replace the human PII/provenance review described in
//! `CONTRIBUTING.md`'s "Adding a corpus specimen" checklist -- a clean OCR
//! hit plus a "specimen" match is not proof the document is a genuine
//! template rather than a real person's document, and a miss on either is
//! not proof it isn't. It's a fast first pass; `scripts/watch-samples.ps1`
//! also opens the image so you can make the actual call.
//!
//! Run from the repo root:
//! ```powershell
//! cargo run -p synthpass-ocr --release --example check_sample -- <path-to-image>
//! ```

use synthpass_ocr::NativeOcr;
use std::path::{Path, PathBuf};

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: check_sample <path-to-image>");
            std::process::exit(2);
        }
    };
    if !path.is_file() {
        eprintln!("not a file: {}", path.display());
        std::process::exit(2);
    }

    let root = repo_root();
    let ocr = match NativeOcr::load(
        &root.join("text-detection.rten"),
        &root.join("text-recognition.rten"),
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("failed to load OCR models (run from the repo root): {e}");
            std::process::exit(1);
        }
    };

    let text = match ocr.recognize(&path) {
        Ok(t) => t,
        Err(e) => {
            println!("OCR-ERROR  {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let has_specimen_text = text.to_ascii_lowercase().contains("specimen");

    match mrz::find_and_parse(&text) {
        Ok(d) if d.valid() => {
            println!(
                "MRZ HIT    doc_number={} format={:?}",
                d.document_number, d.format
            );
        }
        Ok(d) => {
            println!("MRZ MISS   parsed but checksums failed: {:?}", d.checks);
        }
        Err(e) => {
            println!("MRZ MISS   {e}");
        }
    }

    if has_specimen_text {
        println!("WATERMARK  OCR text contains \"specimen\" (case-insensitive) -- weak positive signal only");
    } else {
        println!(
            "WATERMARK  no \"specimen\" text found in OCR output -- review the image yourself before adding this to the corpus"
        );
    }

    println!(
        "\nThis is a fast first pass, not a verdict -- see CONTRIBUTING.md's \"Adding a corpus\n\
         specimen\" checklist before committing this file. Look at the image: is there a genuine\n\
         SPECIMEN watermark or an established placeholder MRZ number (E00000000, 000000000,\n\
         007007007)? Does it read as a real person's document, or carry a novelty/fake-ID-vendor\n\
         watermark? When in doubt, don't add it."
    );
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/synthpass-ocr -> repo root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}
