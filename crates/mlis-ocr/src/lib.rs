//! In-process pure-Rust OCR for Tier 1 via `ocrs`/`rten` — the default engine
//! since v0.7.0 (it replaced the `docling-serve` Docker OCR service that
//! version), mirroring `mlis-llm`'s `NativeLlm` naming/lifecycle pattern but
//! for text detection+recognition instead of generation.
//!
//! [`NativeOcr`] loads both `.rten` weight files once and is kept warm for
//! the process lifetime; `recognize` is blocking — callers on an async
//! runtime (see `mlis-pipeline`) must run it via `spawn_blocking`, mirroring
//! how the native LLM inferer is wrapped.
//!
//! # MRZ retry passes
//!
//! A general full-page pass runs first. If its output does not contain a
//! checksum-valid MRZ (the `mrz` crate's ICAO 9303 check digits are a perfect
//! oracle for a faithful read — see docs/ARCHITECTURE.md §8), a second engine
//! constrained to the MRZ charset (`A–Z 0–9 <`, beam-search decoding) re-reads
//! preprocessed variants of the image ([`preprocess::mrz_variants`]: upscaled
//! bottom-band crops, contrast-stretched and binarized, then the upscaled full
//! page), appending any MRZ-shaped lines it finds to the output. The loop
//! stops at the first variant that validates. Retries are additive-only —
//! the general pass's text is never replaced — so Tier-2 input can only gain
//! candidate lines, and a checksum gate upstream decides what is trusted.
//!
//! Image-only: `ocrs` has no PDF parsing, and as of v0.7.5 there is no other
//! engine to route PDF input to — PDF is rejected outright at the
//! `mlis-pipeline` layer (see `crates/mlis-pipeline/src/ocr.rs`).

pub mod download;
#[cfg(feature = "embedded-models")]
pub mod embedded;
pub mod preprocess;
pub mod verify;

use image::RgbImage;
use ocrs::{DecodeMethod, ImageSource, OcrEngine as OcrsEngine, OcrEngineParams};
use rten::Model;
use std::path::Path;

/// The full ICAO 9303 MRZ character set. Constraining recognition to it makes
/// the classic filler misreads (`<` read as `«`, `?`, or lowercase noise)
/// unrepresentable at the decoder level instead of repaired after the fact.
const MRZ_CHARSET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<";

/// Beam width for the constrained pass. Greedy decoding commits to one
/// reading; a modest beam lets the decoder recover when the top character is
/// wrong but the MRZ-charset-consistent alternative was a close second.
const MRZ_BEAM_WIDTH: u32 = 24;

pub struct NativeOcr {
    /// General-purpose engine: full alphabet, greedy decode — produces the
    /// Markdown-ish page text Tier 2's prompt needs.
    engine: OcrsEngine,
    /// MRZ-targeted engine: recognition restricted to [`MRZ_CHARSET`] with
    /// beam-search decoding. Only consulted when the general pass fails the
    /// checksum oracle.
    mrz_engine: OcrsEngine,
}

fn build_general(detection: Model, recognition: Model) -> Result<OcrsEngine, String> {
    OcrsEngine::new(OcrEngineParams {
        detection_model: Some(detection),
        recognition_model: Some(recognition),
        ..Default::default()
    })
    .map_err(|e| format!("failed to build ocrs engine: {e}"))
}

fn build_mrz(detection: Model, recognition: Model) -> Result<OcrsEngine, String> {
    OcrsEngine::new(OcrEngineParams {
        detection_model: Some(detection),
        recognition_model: Some(recognition),
        allowed_chars: Some(MRZ_CHARSET.into()),
        decode_method: DecodeMethod::BeamSearch {
            width: MRZ_BEAM_WIDTH,
        },
        ..Default::default()
    })
    .map_err(|e| format!("failed to build mrz-constrained ocrs engine: {e}"))
}

impl NativeOcr {
    /// Load both `.rten` model files and build the warm, reusable engines.
    /// (`rten::Model` is not `Clone`, so each file is loaded twice — once per
    /// engine; ~12 MB each, a one-off cost at process start.)
    pub fn load(detection_path: &Path, recognition_path: &Path) -> Result<Self, String> {
        let load = |path: &Path, what: &str| {
            Model::load_file(path)
                .map_err(|e| format!("failed to load {what} model at {}: {e}", path.display()))
        };
        let engine = build_general(
            load(detection_path, "detection")?,
            load(recognition_path, "recognition")?,
        )?;
        let mrz_engine = build_mrz(
            load(detection_path, "detection")?,
            load(recognition_path, "recognition")?,
        )?;
        Ok(Self { engine, mrz_engine })
    }

    /// Build warm engines from the models baked into the binary at compile
    /// time (`embedded-models` feature) — no filesystem or network access,
    /// for a true single-file air-gapped deployment (see
    /// docs/ARCHITECTURE.md §10).
    #[cfg(feature = "embedded-models")]
    pub fn load_embedded() -> Result<Self, String> {
        let load = |bytes: &'static [u8], what: &str| {
            Model::load_static_slice(bytes)
                .map_err(|e| format!("failed to load embedded {what} model: {e}"))
        };
        let engine = build_general(
            load(embedded::DETECTION_BYTES, "detection")?,
            load(embedded::RECOGNITION_BYTES, "recognition")?,
        )?;
        let mrz_engine = build_mrz(
            load(embedded::DETECTION_BYTES, "detection")?,
            load(embedded::RECOGNITION_BYTES, "recognition")?,
        )?;
        Ok(Self { engine, mrz_engine })
    }

    /// Run OCR on the image at `image_path`, returning all recognized text as
    /// a single string — exactly what Tier 1's MRZ pattern search and the
    /// Tier-2 LLM prompt both need; no requirement for structured layout.
    ///
    /// If the general pass's text lacks a checksum-valid MRZ, the constrained
    /// retry passes run (see the module docs) and their MRZ-shaped lines are
    /// appended to the returned text.
    pub fn recognize(&self, image_path: &Path) -> Result<String, String> {
        let image = image::open(image_path)
            .map_err(|e| format!("failed to open image {}: {e}", image_path.display()))?
            .into_rgb8();

        let mut text = run_pass(&self.engine, &image)?;
        if has_valid_mrz(&text) {
            return Ok(text);
        }

        for variant in preprocess::mrz_variants(&image) {
            // A failed retry pass must never fail the whole OCR — the general
            // pass's text is already in hand and Tier 2 can still run on it.
            let Ok(pass_text) = run_pass(&self.mrz_engine, &variant) else {
                continue;
            };
            let candidates = mrz_shaped_lines(&pass_text);
            if candidates.is_empty() {
                continue;
            }
            text.push('\n');
            text.push_str(&candidates);
            // Check just this pass's lines: a valid MRZ appended means Tier 1
            // will find it — later (costlier) variants have nothing to add.
            if has_valid_mrz(&candidates) {
                break;
            }
        }
        Ok(text)
    }
}

/// Run one detection+recognition pass over an in-memory image.
fn run_pass(engine: &OcrsEngine, image: &RgbImage) -> Result<String, String> {
    let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())
        .map_err(|e| format!("failed to prepare image source: {e}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|e| format!("failed to prepare ocr input: {e}"))?;
    engine
        .get_text(&input)
        .map_err(|e| format!("ocr text extraction failed: {e}"))
}

/// Does this text already contain a checksum-valid MRZ? The oracle that
/// decides whether the retry passes are worth their CPU.
fn has_valid_mrz(text: &str) -> bool {
    mrz::find_and_parse(text).is_ok_and(|d| d.valid())
}

/// Keep only lines long enough to be MRZ material (a TD1 line is 30 chars;
/// 20 tolerates truncation, matching `mrz::find_and_parse`'s own token
/// threshold). The constrained pass reads the whole crop, so this drops the
/// non-MRZ text it garbles (it can only emit `A–Z 0–9 <`) instead of feeding
/// that noise to Tier 2's prompt.
fn mrz_shaped_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| l.chars().filter(|c| !c.is_whitespace()).count() >= 20)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mrz_shaped_lines_keeps_mrz_and_drops_noise() {
        let text = "REPUBLIKA\nP<HRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<<<<<<<<<<\nZAGREB\n\
                    0070070071HRV8212258F1407019<<<<<<<<<<<<<<06\nOK";
        let kept = mrz_shaped_lines(text);
        assert_eq!(kept.lines().count(), 2);
        assert!(kept.lines().all(|l| l.len() >= 20));
    }

    #[test]
    fn has_valid_mrz_accepts_specimen_and_rejects_prose() {
        let valid = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<\n\
                     L898902C36UTO7408122F1204159ZE184226B<<<<<10";
        assert!(has_valid_mrz(valid));
        assert!(!has_valid_mrz("just some regular text\nwith two lines"));
    }
}
