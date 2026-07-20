//! In-process pure-Rust OCR for Tier 1 via `ocrs`/`rten` — the default engine
//! since v0.7.0 (it replaced the `docling-serve` Docker OCR service that
//! version), mirroring `synthpass-llm`'s `NativeLlm` naming/lifecycle pattern but
//! for text detection+recognition instead of generation.
//!
//! [`NativeOcr`] loads both `.rten` weight files once and is kept warm for
//! the process lifetime; `recognize` is blocking — callers on an async
//! runtime (see `synthpass-pipeline`) must run it via `spawn_blocking`, mirroring
//! how the native LLM inferer is wrapped.
//!
//! # MRZ retry passes
//!
//! A general full-page pass runs first. If its output does not contain a
//! checksum-valid MRZ (the `mrz` crate's ICAO 9303 check digits are a perfect
//! oracle for a faithful read — see docs/ARCHITECTURE.md §8), a second engine
//! constrained to the MRZ charset (`A–Z 0–9 <`, beam-search decoding) re-reads
//! preprocessed variants of the image ([`preprocess::mrz_variants`]: a
//! row-density-isolated MRZ-band crop, contrast-stretched/binarized/locally-
//! thresholded and deskewed, then the upscaled full page), appending any
//! MRZ-shaped lines it finds to the output. The loop stops at the first
//! variant that validates. Retries are additive-only — the general pass's
//! text is never replaced — so Tier-2 input can only gain candidate lines,
//! and a checksum gate upstream decides what is trusted.
//!
//! # Pass budget
//!
//! `SYNTHPASS_OCR_MAX_PASSES` (default [`DEFAULT_MAX_PASSES`]) and
//! `SYNTHPASS_OCR_MAX_SECONDS` (default [`DEFAULT_MAX_SECONDS`]) bound the retry
//! loop so a hopeless document (dense/photographic scan with no recoverable
//! MRZ) aborts remaining variants instead of burning minutes — the general
//! pass's text is always returned regardless of where the budget cuts the
//! loop off.
//!
//! # Diagnostics
//!
//! Set `SYNTHPASS_OCR_VERBOSE=1` to log each pass's elapsed time and detected-
//! region count to stderr, and — on a Tier-1 miss — the MRZ-band candidate
//! lines each pass found. Off by default: it re-runs detection once per pass
//! purely for the region count, which `get_text` doesn't otherwise expose.
//!
//! Image-only: `ocrs` has no PDF parsing, and as of v0.7.5 there is no other
//! engine to route PDF input to — PDF is rejected outright at the
//! `synthpass-pipeline` layer (see `crates/synthpass-pipeline/src/ocr.rs`).

pub mod download;
#[cfg(feature = "embedded-models")]
pub mod embedded;
pub mod preprocess;
pub mod verify;

use image::RgbImage;
use ocrs::{DecodeMethod, ImageSource, OcrEngine as OcrsEngine, OcrEngineParams};
use rten::Model;
use std::path::Path;
use std::time::{Duration, Instant};

/// The full ICAO 9303 MRZ character set. Constraining recognition to it makes
/// the classic filler misreads (`<` read as `«`, `?`, or lowercase noise)
/// unrepresentable at the decoder level instead of repaired after the fact.
const MRZ_CHARSET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<";

/// Beam width for the constrained pass. Greedy decoding commits to one
/// reading; a modest beam lets the decoder recover when the top character is
/// wrong but the MRZ-charset-consistent alternative was a close second.
const MRZ_BEAM_WIDTH: u32 = 24;

/// Default `SYNTHPASS_OCR_MAX_PASSES` ceiling on total OCR passes per document
/// (the general pass plus retry variants) when the env var is unset or
/// invalid. `preprocess::mrz_variants` yields at most 6 variants (two
/// blind-crop, one full-page, three trailing isolated-band); 7 is those plus
/// the general pass, so none is silently truncated by the pass budget on the
/// worst case. The `SYNTHPASS_OCR_MAX_SECONDS` wall-clock budget is what actually
/// bounds a pathological document.
const DEFAULT_MAX_PASSES: usize = 7;

/// Default `SYNTHPASS_OCR_MAX_SECONDS` wall-clock ceiling on the whole
/// `recognize` call when the env var is unset or invalid. Measured
/// (`examples/mrz_corpus.rs`, post-Phase-1 variants, reference hardware): a
/// full negative-control sweep (all retry variants run, none validate) costs
/// ~15-27s, and `Slovenian_ID_Card_2022_-_Rear.jpg` — whose checksum-valid
/// MRZ only assembles once enough variants have each contributed a line —
/// needs close to 30s of its own. 45s keeps clearance above that measured
/// worst case while still bounding the multi-minute-per-document blowup
/// that motivated this budget in the first place (see the
/// `Israel_Biometric_Passport.jpg` corpus entry, whose multi-minute cost was
/// dominated by Tier 2's LLM generation on garbage OCR text, not the OCR
/// passes themselves — this budget caps the OCR side of that problem).
const DEFAULT_MAX_SECONDS: u64 = 45;

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
        let verbose = verbose_enabled();
        let image = image::open(image_path)
            .map_err(|e| format!("failed to open image {}: {e}", image_path.display()))?
            .into_rgb8();

        let overall_started = Instant::now();
        let general_started = Instant::now();
        let mut text = run_pass(&self.engine, &image)?;
        if verbose {
            let regions = region_count(&self.engine, &image).unwrap_or(0);
            eprintln!(
                "[synthpass-ocr] general pass: {:?} elapsed, {regions} region(s) detected",
                general_started.elapsed()
            );
        }
        if has_valid_mrz(&text) {
            return Ok(text);
        }
        if verbose {
            eprintln!("[synthpass-ocr] Tier-1 miss on general pass; MRZ-band candidate lines:");
            for line in mrz_shaped_lines(&text).lines() {
                eprintln!("[synthpass-ocr]   {line}");
            }
        }

        let max_passes = max_passes();
        let max_duration = max_duration();

        // `passes_run` counts total passes including the general one above
        // (seeded at 1) — a `zip` counter rather than a manually incremented
        // one so clippy's `explicit_counter_loop` stays clean; its value at
        // the top of each iteration is exactly "passes run so far".
        let variants = preprocess::mrz_variants(&image).into_iter().enumerate();
        for (passes_run, (i, variant)) in (1usize..).zip(variants) {
            if passes_run >= max_passes {
                if verbose {
                    eprintln!(
                        "[synthpass-ocr] pass budget ({max_passes}) reached before variant {i}; stopping retries"
                    );
                }
                break;
            }
            if overall_started.elapsed() >= max_duration {
                if verbose {
                    eprintln!(
                        "[synthpass-ocr] time budget ({max_duration:?}) reached before variant {i}; stopping retries"
                    );
                }
                break;
            }

            let variant_started = Instant::now();
            // A failed retry pass must never fail the whole OCR — the general
            // pass's text is already in hand and Tier 2 can still run on it.
            let Ok(pass_text) = run_pass(&self.mrz_engine, &variant) else {
                if verbose {
                    eprintln!("[synthpass-ocr] variant {i}: pass failed, skipping");
                }
                continue;
            };
            if verbose {
                let regions = region_count(&self.mrz_engine, &variant).unwrap_or(0);
                eprintln!(
                    "[synthpass-ocr] variant {i}: {:?} elapsed, {regions} region(s) detected",
                    variant_started.elapsed()
                );
            }
            let candidates = mrz_shaped_lines(&pass_text);
            if candidates.is_empty() {
                if verbose {
                    eprintln!("[synthpass-ocr] variant {i}: no MRZ-shaped lines");
                }
                continue;
            }
            text.push('\n');
            text.push_str(&candidates);
            // Check just this pass's lines: a valid MRZ appended means Tier 1
            // will find it — later (costlier) variants have nothing to add.
            if has_valid_mrz(&candidates) {
                if verbose {
                    eprintln!("[synthpass-ocr] variant {i}: valid MRZ found, stopping retries");
                }
                break;
            } else if verbose {
                eprintln!("[synthpass-ocr] variant {i}: MRZ-shaped but checksum-invalid lines:");
                for line in candidates.lines() {
                    eprintln!("[synthpass-ocr]   {line}");
                }
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

/// Is `SYNTHPASS_OCR_VERBOSE=1` set? Gates the per-pass diagnostic logging
/// described in the module docs.
fn verbose_enabled() -> bool {
    std::env::var("SYNTHPASS_OCR_VERBOSE").as_deref() == Ok("1")
}

/// `SYNTHPASS_OCR_MAX_PASSES`, or [`DEFAULT_MAX_PASSES`] if unset/invalid/zero.
fn max_passes() -> usize {
    std::env::var("SYNTHPASS_OCR_MAX_PASSES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_PASSES)
}

/// `SYNTHPASS_OCR_MAX_SECONDS`, or [`DEFAULT_MAX_SECONDS`] if unset/invalid/zero.
fn max_duration() -> Duration {
    std::env::var("SYNTHPASS_OCR_MAX_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_MAX_SECONDS))
}

/// Detected text-region count for one pass, for `SYNTHPASS_OCR_VERBOSE` logging
/// only — `get_text` already runs detection internally but doesn't expose
/// the count, so this re-runs it; callers must only invoke this when
/// verbose logging is on.
fn region_count(engine: &OcrsEngine, image: &RgbImage) -> Result<usize, String> {
    let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())
        .map_err(|e| format!("failed to prepare image source: {e}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|e| format!("failed to prepare ocr input: {e}"))?;
    let words = engine
        .detect_words(&input)
        .map_err(|e| format!("ocr word detection failed: {e}"))?;
    Ok(words.len())
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

    // Env-var tests mutate process-global state (unavoidable for `std::env`
    // config in Rust 2024, hence the `unsafe` blocks — mirrors the existing
    // pattern in `verify.rs`). `cargo test` runs this binary's tests in
    // parallel threads by default, so each var gets exactly one test
    // (covering every case serially within it) rather than being spread
    // across multiple tests, which would race on the shared process env.
    #[test]
    fn max_passes_reads_env_with_fallback() {
        unsafe { std::env::remove_var("SYNTHPASS_OCR_MAX_PASSES") };
        assert_eq!(max_passes(), DEFAULT_MAX_PASSES, "unset falls back");

        unsafe { std::env::set_var("SYNTHPASS_OCR_MAX_PASSES", "2") };
        assert_eq!(max_passes(), 2, "valid override honored");

        unsafe { std::env::set_var("SYNTHPASS_OCR_MAX_PASSES", "not-a-number") };
        assert_eq!(max_passes(), DEFAULT_MAX_PASSES, "invalid falls back");

        unsafe { std::env::set_var("SYNTHPASS_OCR_MAX_PASSES", "0") };
        assert_eq!(max_passes(), DEFAULT_MAX_PASSES, "zero falls back");

        unsafe { std::env::remove_var("SYNTHPASS_OCR_MAX_PASSES") };
    }

    #[test]
    fn max_duration_reads_env_with_fallback() {
        unsafe { std::env::remove_var("SYNTHPASS_OCR_MAX_SECONDS") };
        assert_eq!(
            max_duration(),
            Duration::from_secs(DEFAULT_MAX_SECONDS),
            "unset falls back"
        );

        unsafe { std::env::set_var("SYNTHPASS_OCR_MAX_SECONDS", "5") };
        assert_eq!(
            max_duration(),
            Duration::from_secs(5),
            "valid override honored"
        );

        unsafe { std::env::set_var("SYNTHPASS_OCR_MAX_SECONDS", "0") };
        assert_eq!(
            max_duration(),
            Duration::from_secs(DEFAULT_MAX_SECONDS),
            "zero falls back"
        );

        unsafe { std::env::remove_var("SYNTHPASS_OCR_MAX_SECONDS") };
    }

    #[test]
    fn verbose_enabled_only_on_exact_flag() {
        unsafe { std::env::remove_var("SYNTHPASS_OCR_VERBOSE") };
        assert!(!verbose_enabled());
        unsafe { std::env::set_var("SYNTHPASS_OCR_VERBOSE", "true") };
        assert!(
            !verbose_enabled(),
            "only the literal \"1\" should enable it"
        );
        unsafe { std::env::set_var("SYNTHPASS_OCR_VERBOSE", "1") };
        assert!(verbose_enabled());
        unsafe { std::env::remove_var("SYNTHPASS_OCR_VERBOSE") };
    }
}
