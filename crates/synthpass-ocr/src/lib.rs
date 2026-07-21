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
//! # Structured geometry (M5)
//!
//! [`NativeOcr::recognize_detailed`] is the richer sibling of `recognize`:
//! it returns an [`geometry::OcrPage`] with per-line text/bounding boxes, an
//! auto-detected page rotation, and two layout heuristics (`mrz_band`,
//! `portrait` — see [`geometry`]'s module docs). `recognize` is now defined
//! in terms of it (`recognize_detailed(..)?.text`); the retry-pass loop,
//! pass budget and time budget described below are unchanged and live
//! inside `recognize_detailed`, so `recognize`'s signature and returned text
//! are unaffected by this split. See `recognize_detailed`'s own doc comment
//! for exactly what's new versus what's verbatim.
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
//! MRZ-shaped lines it finds to the output. When the geometry pass (below)
//! found a confident `mrz_band`, [`preprocess::geometry_band_variants`]
//! contributes up to two more — a crop to that content-scored box rather
//! than a blind or row-density-searched one — chained on strictly as
//! trailing extras after `mrz_variants`'s own. The loop stops at the first
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
pub mod geometry;
pub mod preprocess;
pub mod verify;

pub use geometry::{BBox, OcrLine, OcrPage};

use image::RgbImage;
use ocrs::{DecodeMethod, ImageSource, OcrEngine as OcrsEngine, OcrEngineParams, TextItem};
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
/// blind-crop, one full-page, three trailing isolated-band) and
/// `preprocess::geometry_band_variants` appends at most 2 more (trailing
/// again, and only when the geometry-detected band differs from the blind
/// crop), so the worst case is 6 + 2 = [`MAX_RETRY_VARIANTS`] retry variants
/// plus the general pass: **9**. Note the retry loop seeds its counter at 1
/// for the *first* variant and breaks on `passes_run >= max_passes`, so the
/// number of variants that can actually run is `max_passes - 1` — an
/// off-by-one here silently truncates the last variant on exactly the
/// dense/bilingual scans the geometry-band variants exist for, which is the
/// failure this budget is sized to avoid. `max_passes_admits_every_variant`
/// pins the arithmetic so a future variant can't quietly outgrow it. The
/// `SYNTHPASS_OCR_MAX_SECONDS` wall-clock budget is what actually bounds a
/// pathological document.
const DEFAULT_MAX_PASSES: usize = MAX_RETRY_VARIANTS + 1;

/// Worst-case number of retry variants: `preprocess::mrz_variants`'s 6 plus
/// `preprocess::geometry_band_variants`'s 2. Single-sourced with
/// [`DEFAULT_MAX_PASSES`] above so the budget and the variant count cannot
/// drift apart silently — see that constant's doc comment.
const MAX_RETRY_VARIANTS: usize = 8;

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
    ///
    /// A thin wrapper over [`Self::recognize_detailed`] — this signature and
    /// its returned text are unchanged from before this crate gained a
    /// geometry API. See that method's doc comment for the "unchanged
    /// verbatim vs. new" boundary that makes this true.
    pub fn recognize(&self, image_path: &Path) -> Result<String, String> {
        Ok(self.recognize_detailed(image_path)?.text)
    }

    /// [`Self::recognize`]'s richer sibling: same recognized text, plus
    /// per-line detail, an auto-detected page rotation, and the `mrz_band`/
    /// `portrait` layout heuristics (see [`geometry`]'s module docs) —
    /// together, [`OcrPage`].
    ///
    /// **What's unchanged vs. what's new**, for anyone auditing that
    /// `recognize`'s behaviour didn't shift under this split: the pass
    /// budget / time budget / MRZ retry logic below is from before this
    /// method existed (still calling the original `run_pass`/`region_count`/
    /// `mrz_shaped_lines` helpers, and `preprocess::mrz_variants` itself is
    /// untouched), plus exactly one additive change to the retry loop's
    /// variant list (below). What's new is layered strictly around and
    /// after that core: orientation detection ([`choose_rotation`], A3) runs
    /// first and — being detection-only and conservatively biased toward "no
    /// rotation" (see its doc comment) — only ever changes the `image` this
    /// pipeline sees when it is confident the page is rotated. The
    /// structured line/word geometry ([`geometry_pass`]) is best-effort: its
    /// own detect+recognize pass never feeds back into `text`, and a failure
    /// in it degrades `OcrPage`'s structured fields to empty rather than
    /// failing this call. Its `mrz_band` output then feeds two more additive
    /// steps: the 0°/180° tie-break ([`band_in_upper_third`]) that
    /// `choose_rotation` can't resolve on its own, and — down in the retry
    /// loop — `preprocess::geometry_band_variants`, chained strictly as
    /// *trailing* extras after every existing `mrz_variants` entry. Both are
    /// gated on a *confident* `mrz_band`; absent one, `image` stays whatever
    /// `choose_rotation` produced and the variant list is exactly
    /// `mrz_variants`'s own — the same as before this milestone, so an
    /// already-passing upright specimen's `image`/`text` are byte-identical
    /// to its pre-M5 behaviour. Everything through `choose_rotation` and the
    /// geometry passes runs *before* `overall_started` is set, so none of it
    /// eats into the retry loop's own wall-clock budget
    /// (`DEFAULT_MAX_SECONDS`) — it adds to this call's total latency, not
    /// to the retry loop's.
    pub fn recognize_detailed(&self, image_path: &Path) -> Result<OcrPage, String> {
        let verbose = verbose_enabled();
        let image = image::open(image_path)
            .map_err(|e| format!("failed to open image {}: {e}", image_path.display()))?
            .into_rgb8();

        // A3: auto-rotate before the main pass (detection-only, cheap; see
        // `choose_rotation`'s doc comment, including its known 0°-vs-180°
        // limitation). `None` means "keep the image as-is", which also
        // covers detection failure — orientation is a best-effort
        // enhancement, never a reason to fail the whole call.
        let (rotation, image) = match choose_rotation(&self.engine, &image) {
            Some((angle, rotated)) => {
                if verbose {
                    eprintln!("[synthpass-ocr] orientation: auto-rotated {angle}°");
                }
                (angle, rotated)
            }
            None => (0, image),
        };

        // A1/A2/A4: structured line/word geometry for this pass, best-effort
        // (see this method's doc comment on why this is a separate pass
        // rather than threaded through `run_pass`/`region_count` below).
        let (lines, word_boxes) = geometry_pass(&self.engine, &image).unwrap_or_default();
        let mrz_band = geometry::detect_mrz_band(&lines, MRZ_CHARSET);

        // A3 continued — the 0°/180° tie-break `choose_rotation` cannot
        // resolve on its own (see its doc comment: an upside-down horizontal
        // line measures identically wide-and-short as a right-side-up one).
        // The MRZ settles it: on every ICAO layout (TD1 rear, TD2, TD3 data
        // page) it sits at the bottom of the page, so a `mrz_band` that
        // cleared `detect_mrz_band`'s own confidence floor and still landed
        // in the upper third is strong, specific evidence the page is still
        // upside-down after `choose_rotation`'s pass. Only that combination
        // — confident band *and* upper-third — triggers a flip; a `None`
        // band (nothing to disambiguate with) or a confident band anywhere
        // in the lower two-thirds (already consistent with upright) leaves
        // `image`/`rotation`/`lines`/`word_boxes`/`mrz_band` untouched, which
        // is what keeps this a pure addition rather than a second chance to
        // guess wrong. The cost is one extra geometry pass, and only on
        // documents meeting both conditions.
        let (rotation, image, lines, word_boxes, mrz_band) = match mrz_band
            .filter(|band| band_in_upper_third(*band, image.height()))
        {
            Some(_) => {
                if verbose {
                    eprintln!(
                        "[synthpass-ocr] orientation: confident MRZ band in upper third at {rotation}°; flipping 180°"
                    );
                }
                let flipped = rotate_image(&image, 180);
                let (lines, word_boxes) = geometry_pass(&self.engine, &flipped).unwrap_or_default();
                let mrz_band = geometry::detect_mrz_band(&lines, MRZ_CHARSET);
                ((rotation + 180) % 360, flipped, lines, word_boxes, mrz_band)
            }
            None => (rotation, image, lines, word_boxes, mrz_band),
        };
        let portrait = geometry::detect_portrait(&word_boxes, image.width(), image.height());

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
            return Ok(OcrPage {
                text,
                lines,
                mrz_band,
                portrait,
                rotation,
            });
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
        //
        // The geometry-detected band (A2's `mrz_band`, computed above) is
        // chained on strictly as *trailing* extras after every existing
        // `mrz_variants` entry — never modifying `mrz_variants` itself, so no
        // specimen that already validates on one of those can regress. The
        // loop below breaks on the first checksum-valid MRZ, so these are
        // only ever reached once every blind/isolated variant has already
        // failed. `geometry_band_variants` itself is the cheap part: it
        // returns nothing when there's no confident band, or when the band
        // it found is close enough to the blind crop to be a duplicate (see
        // its doc comment), so this costs nothing on the common case.
        let geometry_variants = mrz_band
            .map(|band| preprocess::geometry_band_variants(&image, band))
            .unwrap_or_default();
        let variants = preprocess::mrz_variants(&image)
            .into_iter()
            .chain(geometry_variants)
            .enumerate();
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
        Ok(OcrPage {
            text,
            lines,
            mrz_band,
            portrait,
            rotation,
        })
    }
}

/// Right-angle rotation candidates probed by [`choose_rotation`] (A3),
/// clockwise from the page as photographed/scanned. `0°` (no rotation) is
/// the baseline scored inline in `choose_rotation` rather than listed here.
const ROTATION_CANDIDATES: [u16; 3] = [90, 180, 270];

/// How much higher a non-zero rotation's line-geometry score must be than
/// 0°'s before [`choose_rotation`] commits to it. Documents are far more
/// often already upright than not, and guessing wrong feeds the whole
/// downstream pipeline a garbled page — so ties and close calls default to
/// "no rotation", which is also the choice that keeps `recognize()`'s
/// output byte-identical to its behaviour before this module gained
/// orientation detection (see `recognize_detailed`'s doc comment).
const ROTATION_MARGIN: f64 = 1.2;

/// A3 — cheap, detection-only page-orientation heuristic. Scores how
/// "line-shaped" the detected text is at each right-angle rotation: Latin
/// text reads in wide, short horizontal lines once `find_text_lines` groups
/// detected words together; turned 90°/270° sideways, the same words end up
/// as tall, narrow single-word "lines" instead of being grouped, so the mean
/// per-line width:height ratio drops sharply. No recognition model is
/// invoked — four `detect_words`+`find_text_lines` calls (0°, 90°, 180°,
/// 270°) cost roughly what one recognition pass does, honoring this step's
/// "keep it cheap" constraint.
///
/// Known limitation: the aspect-ratio signal is symmetric under a further
/// 180° turn (an upside-down horizontal line still measures wide-and-short),
/// so this reliably catches a sideways photo but cannot on its own tell a
/// right-side-up page from a fully upside-down one — disambiguating that
/// would need a recognition-confidence signal, which this deliberately does
/// not run (see [`ROTATION_MARGIN`]'s doc comment). This function does not
/// close that gap; `recognize_detailed` does, one layer up, using the
/// content-and-geometry `mrz_band` signal it computes right after calling
/// this (see [`band_in_upper_third`]) — a content-based tie-break, not a
/// generic recognition-confidence one, and only ever a correction *after*
/// this function has already run, never a change to what this function
/// itself returns.
///
/// Returns `None` (meaning "keep the image as-is") when no candidate beats
/// 0° by [`ROTATION_MARGIN`], including whenever `ocrs` detection itself
/// fails on any candidate — orientation is a best-effort enhancement, never
/// a reason to fail the whole recognition call.
fn choose_rotation(engine: &OcrsEngine, image: &RgbImage) -> Option<(u16, RgbImage)> {
    let zero_score = orientation_score(engine, image).unwrap_or(0.0);
    let mut best: Option<(u16, RgbImage, f64)> = None;
    for &angle in &ROTATION_CANDIDATES {
        let candidate = rotate_image(image, angle);
        let Ok(score) = orientation_score(engine, &candidate) else {
            continue;
        };
        if score <= zero_score * ROTATION_MARGIN {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|&(_, _, best_score)| score > best_score)
        {
            best = Some((angle, candidate, score));
        }
    }
    best.map(|(angle, rotated, _)| (angle, rotated))
}

/// Does `band`'s vertical centre sit in the page's upper third? The signal
/// `recognize_detailed`'s post-`choose_rotation` MRZ tie-break acts on (see
/// there). "Upper third" rather than "upper half": the MRZ is generally the
/// bottom-most content block on an ICAO layout, so even a photo/data-heavy
/// front side rarely pushes a correctly-oriented MRZ band's centre past the
/// halfway line — a stricter third keeps the tie-break from firing on a page
/// that's merely MRZ-heavy in its layout rather than genuinely upside-down.
fn band_in_upper_third(band: BBox, image_h: u32) -> bool {
    if image_h == 0 {
        return false;
    }
    let center_y = f64::from(band.y) + f64::from(band.h) / 2.0;
    center_y < f64::from(image_h) / 3.0
}

/// Detection-only orientation score for one candidate rotation — see
/// [`choose_rotation`]'s doc comment for what this measures and why.
fn orientation_score(engine: &OcrsEngine, image: &RgbImage) -> Result<f64, String> {
    let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())
        .map_err(|e| format!("failed to prepare image source: {e}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|e| format!("failed to prepare ocr input: {e}"))?;
    let words = engine
        .detect_words(&input)
        .map_err(|e| format!("ocr word detection failed: {e}"))?;
    if words.is_empty() {
        return Ok(0.0);
    }
    let lines = engine.find_text_lines(&input, &words);
    if lines.is_empty() {
        return Ok(0.0);
    }
    let mut total = 0.0;
    for line_words in &lines {
        if line_words.is_empty() {
            continue;
        }
        let width_sum: f64 = line_words.iter().map(|w| f64::from(w.width())).sum();
        let height_mean: f64 = line_words
            .iter()
            .map(|w| f64::from(w.height()))
            .sum::<f64>()
            / line_words.len() as f64;
        if height_mean > 0.0 {
            total += width_sum / height_mean;
        }
    }
    Ok(total / lines.len() as f64)
}

/// Rotate `image` clockwise by `angle` degrees (must be 0/90/180/270 — any
/// other value is treated as a no-op copy). Lossless pixel rotation via the
/// `image` crate (no new dependency): exact for right angles, unlike
/// `preprocess::deskew`'s bilinear arbitrary-angle rotation for small-tilt
/// correction.
fn rotate_image(image: &RgbImage, angle: u16) -> RgbImage {
    match angle {
        90 => image::imageops::rotate90(image),
        180 => image::imageops::rotate180(image),
        270 => image::imageops::rotate270(image),
        _ => image.clone(),
    }
}

/// One detect+group+recognize pass over `image`, used only to surface
/// structured line/word geometry for [`OcrPage`] (A1/A2/A4) — run
/// separately from (and in addition to) [`run_pass`]/[`region_count`] below
/// rather than threaded through them, so the general pass's `text` output
/// and error messages stay byte-for-byte what `recognize` has always
/// produced (see `recognize_detailed`'s doc comment). The cost is one extra
/// detect+recognize pass on the general image; correctness of `recognize`'s
/// output was judged worth more than avoiding it.
///
/// Best-effort: a failure here (mapped to `Err` and discarded by the caller
/// via `.unwrap_or_default()`) degrades `OcrPage`'s structured fields to
/// empty, never the returned `text`.
fn geometry_pass(
    engine: &OcrsEngine,
    image: &RgbImage,
) -> Result<(Vec<OcrLine>, Vec<BBox>), String> {
    let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())
        .map_err(|e| format!("failed to prepare image source: {e}"))?;
    let input = engine
        .prepare_input(source)
        .map_err(|e| format!("failed to prepare ocr input: {e}"))?;
    let words = engine
        .detect_words(&input)
        .map_err(|e| format!("ocr word detection failed: {e}"))?;
    let word_boxes: Vec<BBox> = words
        .iter()
        .map(|w| geometry::bbox_from_points(w.corners().map(|c| (c.x, c.y))))
        .collect();
    let line_groups = engine.find_text_lines(&input, &words);
    let recognized = engine
        .recognize_text(&input, &line_groups)
        .map_err(|e| format!("ocr text extraction failed: {e}"))?;
    let lines = recognized
        .into_iter()
        .flatten()
        .map(|line| {
            let r = line.bounding_rect();
            let bbox = BBox::from_tlbr(
                r.top() as f32,
                r.left() as f32,
                r.bottom() as f32,
                r.right() as f32,
            );
            let text = line.to_string();
            let confidence = geometry::text_sanity(&text);
            OcrLine {
                text,
                bbox,
                confidence,
            }
        })
        .collect();
    Ok((lines, word_boxes))
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
    fn band_in_upper_third_true_only_near_the_top() {
        let image_h = 900;
        let top_band = BBox {
            x: 0.0,
            y: 50.0,
            w: 400.0,
            h: 60.0,
        }; // centre at 80, well under 300 (a third of 900)
        assert!(band_in_upper_third(top_band, image_h));

        let bottom_band = BBox {
            x: 0.0,
            y: 750.0,
            w: 400.0,
            h: 60.0,
        }; // centre at 780, the ordinary upright-MRZ location
        assert!(!band_in_upper_third(bottom_band, image_h));

        assert!(
            !band_in_upper_third(top_band, 0),
            "zero-height image must not divide by zero"
        );
    }

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

    /// The default pass budget must admit *every* retry variant, including
    /// the last one. This is arithmetic, not behaviour, and it is a test
    /// rather than a comment because a comment is exactly what failed here:
    /// the budget was first raised to 8 alongside a doc comment asserting
    /// "6 + 2 more, plus the general pass" — which is 9. The truncation is
    /// silent (the loop just stops early) and lands only on documents
    /// producing the full variant set, i.e. the dense/bilingual scans the
    /// geometry-band variants were added for, so it would have shown up as
    /// the feature looking weaker than it is rather than as a failure.
    #[test]
    fn max_passes_admits_every_variant() {
        // The loop seeds `passes_run` at 1 for the *first* variant and breaks
        // on `passes_run >= max_passes`, so the count it actually admits is
        // one less than the budget. See the retry loop in `recognize_detailed`.
        let admitted = DEFAULT_MAX_PASSES - 1;
        assert_eq!(
            admitted, MAX_RETRY_VARIANTS,
            "default budget admits {admitted} variants but {MAX_RETRY_VARIANTS} can be produced — \
             the last one(s) would be silently truncated"
        );
    }

    /// Guards the other half of the arithmetic above: `MAX_RETRY_VARIANTS` is
    /// only meaningful if the variant producers actually stay within it. Both
    /// are counted against a real (if tiny) image so adding a variant to
    /// either function without updating the constant fails here rather than
    /// silently eating the budget.
    #[test]
    fn variant_producers_stay_within_the_budgeted_count() {
        let image = image::RgbImage::from_pixel(400, 600, image::Rgb([255, 255, 255]));
        let blind = preprocess::mrz_variants(&image);
        let band = BBox {
            x: 0.0,
            y: 20.0,
            w: 400.0,
            h: 60.0,
        };
        let geometry = preprocess::geometry_band_variants(&image, band);
        assert!(
            blind.len() + geometry.len() <= MAX_RETRY_VARIANTS,
            "producers yielded {} + {} variants, over the budgeted {MAX_RETRY_VARIANTS}",
            blind.len(),
            geometry.len()
        );
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
