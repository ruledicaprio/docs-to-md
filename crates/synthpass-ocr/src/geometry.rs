//! Structured OCR geometry: plain, owned bounding-box types and the pure
//! (no `ocrs`/`rten` types in their signatures) scoring heuristics that turn
//! raw detected lines into [`OcrPage`]'s `mrz_band`/`portrait` fields.
//!
//! # Why plain owned types
//!
//! `ocrs::detect_words` returns `Vec<rten_imageproc::RotatedRect>` and
//! `ocrs::TextLine::bounding_rect()` returns `rten_imageproc::Rect<i32>` —
//! neither type is re-exported by `ocrs`'s public API (confirmed by reading
//! `ocrs-0.12.2/src/lib.rs`: its `pub use` list covers `ImageSource`,
//! `DecodeMethod`, `TextChar`/`TextItem`/`TextLine`/`TextWord`, nothing from
//! `rten_imageproc`). [`BBox`] is a plain owned `x`/`y`/`w`/`h` struct
//! instead of a re-export of either, so `synthpass-pipeline` (which consumes
//! [`OcrPage`]) never needs `ocrs`/`rten`/`rten_imageproc` in its own
//! dependency graph — this crate's geometry API has zero `ocrs` types in its
//! public signatures. See `lib.rs` for how raw engine output is converted
//! into these types (via inherent methods and inferred locals only, so that
//! conversion code itself never has to name the `ocrs`/`rten_imageproc`
//! types either, keeping this a genuinely additive change with no new
//! dependency).

/// Axis-aligned bounding box in source-image pixel coordinates
/// (`x`/`y` = top-left corner, `w`/`h` = extent). Deliberately not a
/// re-export of `ocrs`'s/`rten_imageproc`'s `RotatedRect` or `Rect` — see the
/// module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BBox {
    /// Build a box from top/left/bottom/right coordinates (the shape
    /// `ocrs`'s own `Rect` exposes via `.top()`/`.left()`/`.bottom()`/
    /// `.right()`), clamping a degenerate/inverted input to zero size rather
    /// than a negative one.
    pub fn from_tlbr(top: f32, left: f32, bottom: f32, right: f32) -> Self {
        Self {
            x: left,
            y: top,
            w: (right - left).max(0.0),
            h: (bottom - top).max(0.0),
        }
    }

    /// Union bounding box of a group of boxes, or `None` if `boxes` is empty.
    pub fn union(boxes: &[BBox]) -> Option<BBox> {
        let first = boxes.first()?;
        let (mut min_x, mut min_y) = (first.x, first.y);
        let (mut max_x, mut max_y) = (first.x + first.w, first.y + first.h);
        for b in &boxes[1..] {
            min_x = min_x.min(b.x);
            min_y = min_y.min(b.y);
            max_x = max_x.max(b.x + b.w);
            max_y = max_y.max(b.y + b.h);
        }
        Some(BBox {
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        })
    }
}

/// Build an axis-aligned [`BBox`] from four corner points (`(x, y)` pairs).
/// The only place raw `ocrs`/`rten_imageproc` geometry crosses into this
/// module's owned types — callers pass `RotatedRect::corners()` mapped to
/// tuples (`.map(|c| (c.x, c.y))`), which needs no `use` of the
/// `rten_imageproc` crate at all: `Point`'s `x`/`y` fields are public, and
/// field access never requires naming the field's owner type.
pub fn bbox_from_points(points: [(f32, f32); 4]) -> BBox {
    let xs = points.map(|p| p.0);
    let ys = points.map(|p| p.1);
    let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    BBox::from_tlbr(min_y, min_x, max_y, max_x)
}

/// One recognized line of text with its location and a confidence proxy.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub bbox: BBox,
    /// Heuristic confidence in `[0, 1]` — see [`text_sanity`]'s doc comment
    /// for why this is a proxy rather than a native model score.
    pub confidence: f32,
}

/// Structured OCR output for one page/image: the full text (identical to
/// what [`crate::NativeOcr::recognize`] has always returned), per-line
/// detail, and two layout-heuristic regions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OcrPage {
    pub text: String,
    pub lines: Vec<OcrLine>,
    /// The line group scored as the MRZ zone by [`detect_mrz_band`] (A2), if
    /// any scored high enough to be confident.
    pub mrz_band: Option<BBox>,
    /// The region scored as the ID photo by [`detect_portrait`] (A4), if
    /// any scored high enough to be confident. **Crop coordinates only** —
    /// see [`detect_portrait`]'s doc comment for the VISION §2 boundary this
    /// crate must never cross.
    pub portrait: Option<BBox>,
    /// The rotation (degrees, clockwise) applied before the main OCR pass —
    /// `0` if the page was used as-is. See [`crate::choose_rotation`].
    pub rotation: u16,
}

/// Heuristic confidence proxy in `[0, 1]` for a recognized line's text.
///
/// `ocrs`'s public API exposes no native per-character or per-line
/// probability/confidence score — confirmed by reading `ocrs-0.12.2`'s
/// source: [`TextChar`](ocrs::TextChar) carries only `char` and `rect`,
/// [`TextLine`](ocrs::TextLine) is a sequence of those, and
/// `OcrEngine::recognize_text`'s CTC decode probabilities
/// (`recognition.rs`) are computed internally but never returned. This
/// substitutes the fraction of non-whitespace characters that are
/// alphanumeric or common MRZ/document punctuation: a garbled decode (wrong
/// glyph substitutions, stray symbol noise) tends to produce a higher
/// proportion of unlikely characters than a clean read. It is a real, if
/// indirect, signal — not a substitute for the ICAO checksum oracle the MRZ
/// retry loop already uses as its actual correctness proof.
pub fn text_sanity(text: &str) -> f32 {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if chars.is_empty() {
        return 0.0;
    }
    let plausible = chars
        .iter()
        .filter(|c| c.is_alphanumeric() || ".,-/<'".contains(**c))
        .count();
    plausible as f32 / chars.len() as f32
}

/// TD1/TD2/TD3 MRZ line lengths (ICAO 9303), used by [`mrz_line_score`]'s
/// length-match component.
const MRZ_LINE_LENGTHS: [f64; 3] = [30.0, 36.0, 44.0];

/// How many characters a line's length may differ from the nearest
/// [`MRZ_LINE_LENGTHS`] target before [`mrz_line_score`]'s length component
/// hits zero. ICAO MRZ lines have an *exact* reproducible length, unlike
/// prose — this needs to be tight enough that a short header/caption line
/// (which, being all-uppercase, can otherwise score a deceptively high
/// charset-density component) doesn't sneak past on length alone.
const MRZ_LENGTH_TOLERANCE: f64 = 6.0;

/// Typical OCR-B glyph width:height ratio on ICAO documents — MRZ glyphs run
/// close to a fixed per-character aspect once monospaced, used by
/// [`mrz_line_score`]'s aspect-ratio component. Not exact (fonts/scans
/// vary); [`mrz_line_score`] scores closeness rather than requiring a match.
const MRZ_GLYPH_ASPECT: f64 = 0.62;

/// A group of MRZ lines must average at least this per-line score (each
/// component is in `[0, 1]` and [`mrz_line_score`] multiplies them, so a
/// genuine MRZ line — dense in-charset, right length, right-ish aspect —
/// comfortably clears it) for [`detect_mrz_band`] to report a band at all.
/// Keeps a document with no MRZ (e.g. a photo-only front side) from getting
/// a spurious `mrz_band` out of whatever text happens to score highest.
const MRZ_BAND_MIN_AVG_SCORE: f64 = 0.15;

/// Per-line MRZ-likelihood score (A2), combining three independent signals
/// — each in `[0, 1]`, multiplied together so a line must be strong on all
/// three rather than merely acceptable on one (a long run of digits is
/// charset-dense but the wrong length; a short random uppercase word is the
/// wrong length too):
/// - **charset density**: fraction of non-whitespace characters in
///   `mrz_charset` (the crate's [`crate::MRZ_CHARSET`]).
/// - **length match**: how close the line's character count is to one of
///   the three ICAO MRZ line lengths ([`MRZ_LINE_LENGTHS`]).
/// - **glyph aspect**: how close the bounding box's width-per-character is
///   to [`MRZ_GLYPH_ASPECT`], i.e. how monospace-OCR-B-shaped the line looks
///   geometrically (independent of what the recognizer actually read).
pub fn mrz_line_score(line: &OcrLine, mrz_charset: &str) -> f64 {
    let stripped: Vec<char> = line.text.chars().filter(|c| !c.is_whitespace()).collect();
    if stripped.is_empty() {
        return 0.0;
    }
    let len = stripped.len() as f64;

    let mrz_count = stripped
        .iter()
        .filter(|c| mrz_charset.contains(**c))
        .count();
    let density = mrz_count as f64 / len;

    let length_score = MRZ_LINE_LENGTHS
        .iter()
        .map(|&target| (1.0 - (len - target).abs() / MRZ_LENGTH_TOLERANCE).max(0.0))
        .fold(0.0_f64, f64::max);

    let aspect_score = if line.bbox.h > 0.0 {
        let glyph_w = f64::from(line.bbox.w) / len;
        let ratio = glyph_w / f64::from(line.bbox.h);
        (1.0 - (ratio - MRZ_GLYPH_ASPECT).abs() / MRZ_GLYPH_ASPECT).max(0.0)
    } else {
        0.0
    };

    density * length_score * aspect_score
}

/// A2 — score every detected line ([`mrz_line_score`]) and return the
/// bounding box of whichever adjacent group of 2 or 3 lines (TD2/TD3 are
/// 2-line MRZs, TD1 is 3-line) scores highest on average, provided that
/// average clears [`MRZ_BAND_MIN_AVG_SCORE`]. `lines` is assumed to already
/// be in reading order (top-to-bottom), which is what `ocrs::find_text_lines`
/// guarantees.
///
/// This is content-and-geometry based, unlike `preprocess::BAND_FRACTION`'s
/// blind bottom-45%-of-the-page crop — that blind crop **stays** as the
/// retry loop's fallback (see `preprocess.rs`'s module docs); this is a
/// separate, additive signal surfaced on [`OcrPage::mrz_band`], not a
/// replacement for it.
pub fn detect_mrz_band(lines: &[OcrLine], mrz_charset: &str) -> Option<BBox> {
    let scores: Vec<f64> = lines
        .iter()
        .map(|l| mrz_line_score(l, mrz_charset))
        .collect();

    let mut best: Option<(f64, usize, usize)> = None; // (avg_score, start, end_exclusive)
    for group_len in [2usize, 3usize] {
        if group_len > lines.len() {
            continue;
        }
        for start in 0..=(lines.len() - group_len) {
            let end = start + group_len;
            let avg = scores[start..end].iter().sum::<f64>() / group_len as f64;
            if best.is_none_or(|(b, _, _)| avg > b) {
                best = Some((avg, start, end));
            }
        }
    }

    let (avg, start, end) = best?;
    if avg < MRZ_BAND_MIN_AVG_SCORE {
        return None;
    }
    let boxes: Vec<BBox> = lines[start..end].iter().map(|l| l.bbox).collect();
    BBox::union(&boxes)
}

/// Grid resolution (cells per axis) [`detect_portrait`] uses for its coarse
/// occupied/empty search over the upper-left quadrant. Fine enough to
/// resolve a meaningfully-sized photo block; coarse enough that the
/// exhaustive candidate-rectangle scan (backed by a 2-D prefix sum, so each
/// candidate is checked in O(1)) stays on the order of `N^4` ≈ 1.6e5 cheap
/// integer comparisons, not a real performance concern.
const PORTRAIT_GRID_N: usize = 20;

/// Target portrait-photo aspect ratio (width:height) — the common ID-photo
/// proportions.
const PORTRAIT_ASPECT: f64 = 3.0 / 4.0;

/// How far a candidate rectangle's aspect ratio may drift from
/// [`PORTRAIT_ASPECT`] (as a fraction of it, either direction) and still be
/// considered.
const PORTRAIT_ASPECT_TOLERANCE: f64 = 0.35;

/// Minimum fraction of the searched quadrant's cell area a candidate must
/// cover to be reported — filters out page margin/gutter slivers that
/// happen to be text-free and roughly the right shape.
const PORTRAIT_MIN_AREA_FRACTION: f64 = 0.03;

/// A4 — the largest text-free region in the image's upper-left quadrant
/// whose aspect ratio is close to a portrait photo's, found via a coarse
/// occupancy grid (cells touched by any detected word box are marked
/// "occupied") and an exhaustive candidate-rectangle scan over it. Purely a
/// layout heuristic: it never looks at pixel content, only at where text
/// *isn't*, which is why an ID-card front page's photo block (word-free,
/// roughly 3:4, sitting in the upper-left with the MRZ/data fields to its
/// right and below) is the kind of region this finds — and also why a
/// blank margin the right size and shape can produce a false positive; it
/// is a heuristic, not a proof.
///
/// **VISION §2 permanent non-goal**: the returned [`BBox`] is a *crop
/// coordinate only* — where a downstream caller may crop the source image
/// if it wants to isolate/redact the portrait region. This function (and
/// this crate) never inspects pixel content to decide whether a face is
/// present, extracts no biometric features, and performs no face detection
/// or recognition. Bounding-box cropping by layout heuristic alone is the
/// permanent ceiling for portrait handling in this codebase — it must never
/// grow into face recognition or biometric matching.
pub fn detect_portrait(word_boxes: &[BBox], image_w: u32, image_h: u32) -> Option<BBox> {
    if image_w == 0 || image_h == 0 {
        return None;
    }
    let (qw, qh) = (f64::from(image_w) / 2.0, f64::from(image_h) / 2.0);
    let cell_w = qw / PORTRAIT_GRID_N as f64;
    let cell_h = qh / PORTRAIT_GRID_N as f64;
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return None;
    }

    let mut occupied = vec![false; PORTRAIT_GRID_N * PORTRAIT_GRID_N];
    for b in word_boxes {
        let (bx0, by0) = (f64::from(b.x), f64::from(b.y));
        let (bx1, by1) = (f64::from(b.x + b.w), f64::from(b.y + b.h));
        if bx1 <= 0.0 || by1 <= 0.0 || bx0 >= qw || by0 >= qh {
            continue; // entirely outside the upper-left quadrant
        }
        let c0 = ((bx0.max(0.0)) / cell_w).floor() as usize;
        let r0 = ((by0.max(0.0)) / cell_h).floor() as usize;
        let c1 = (((bx1.min(qw)) / cell_w).ceil() as usize).clamp(c0 + 1, PORTRAIT_GRID_N);
        let r1 = (((by1.min(qh)) / cell_h).ceil() as usize).clamp(r0 + 1, PORTRAIT_GRID_N);
        for r in r0..r1 {
            for c in c0..c1 {
                occupied[r * PORTRAIT_GRID_N + c] = true;
            }
        }
    }

    // 2-D prefix sum over `occupied` so "is this rectangle entirely empty"
    // is an O(1) check during the candidate scan below.
    let stride = PORTRAIT_GRID_N + 1;
    let mut prefix = vec![0u32; stride * stride];
    for r in 0..PORTRAIT_GRID_N {
        for c in 0..PORTRAIT_GRID_N {
            let v = u32::from(occupied[r * PORTRAIT_GRID_N + c]);
            prefix[(r + 1) * stride + (c + 1)] = prefix[r * stride + (c + 1)]
                + prefix[(r + 1) * stride + c]
                - prefix[r * stride + c]
                + v;
        }
    }
    // Order matters for unsigned arithmetic: grouping the two largest terms
    // first (`d + a`) before subtracting the two "medium" ones keeps every
    // intermediate step non-negative, even though `d - b - c + a` (the more
    // obvious inclusion-exclusion order) is mathematically identical — it
    // can underflow `u32` at the `- c` step despite the final result always
    // being >= 0. Same ordering `preprocess::Integral::sum` already uses for
    // exactly this reason.
    let occupied_count = |r0: usize, c0: usize, r1: usize, c1: usize| -> u32 {
        prefix[r1 * stride + c1] + prefix[r0 * stride + c0]
            - prefix[r0 * stride + c1]
            - prefix[r1 * stride + c0]
    };

    let mut best: Option<(f64, usize, usize, usize, usize)> = None; // (area, r0, c0, r1, c1)
    for r0 in 0..PORTRAIT_GRID_N {
        for c0 in 0..PORTRAIT_GRID_N {
            if occupied[r0 * PORTRAIT_GRID_N + c0] {
                continue;
            }
            for r1 in (r0 + 1)..=PORTRAIT_GRID_N {
                for c1 in (c0 + 1)..=PORTRAIT_GRID_N {
                    if occupied_count(r0, c0, r1, c1) != 0 {
                        continue;
                    }
                    let (w, h) = ((c1 - c0) as f64, (r1 - r0) as f64);
                    let ratio = w / h;
                    if (ratio - PORTRAIT_ASPECT).abs() > PORTRAIT_ASPECT * PORTRAIT_ASPECT_TOLERANCE
                    {
                        continue;
                    }
                    let area = w * h;
                    if best.is_none_or(|(a, ..)| area > a) {
                        best = Some((area, r0, c0, r1, c1));
                    }
                }
            }
        }
    }

    let (area, r0, c0, r1, c1) = best?;
    if area < (PORTRAIT_GRID_N * PORTRAIT_GRID_N) as f64 * PORTRAIT_MIN_AREA_FRACTION {
        return None;
    }
    Some(BBox::from_tlbr(
        r0 as f32 * cell_h as f32,
        c0 as f32 * cell_w as f32,
        r1 as f32 * cell_h as f32,
        c1 as f32 * cell_w as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MRZ_CHARSET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789<";

    fn line(text: &str, x: f32, y: f32, w: f32, h: f32) -> OcrLine {
        OcrLine {
            text: text.to_string(),
            bbox: BBox { x, y, w, h },
            confidence: 0.0,
        }
    }

    #[test]
    fn bbox_from_points_takes_min_max_regardless_of_order() {
        let b = bbox_from_points([(10.0, 5.0), (2.0, 5.0), (2.0, 20.0), (10.0, 20.0)]);
        assert_eq!(b, BBox::from_tlbr(5.0, 2.0, 20.0, 10.0));
    }

    #[test]
    fn bbox_union_covers_all_boxes() {
        let boxes = [
            BBox {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 5.0,
            },
            BBox {
                x: 20.0,
                y: 10.0,
                w: 5.0,
                h: 5.0,
            },
        ];
        let u = BBox::union(&boxes).unwrap();
        assert_eq!(
            u,
            BBox {
                x: 0.0,
                y: 0.0,
                w: 25.0,
                h: 15.0
            }
        );
    }

    #[test]
    fn bbox_union_of_empty_is_none() {
        assert!(BBox::union(&[]).is_none());
    }

    #[test]
    fn text_sanity_scores_clean_text_higher_than_symbol_noise() {
        assert!(text_sanity("ANNA MARIA") > text_sanity("#@%^&*()!!"));
        assert_eq!(text_sanity(""), 0.0);
        assert_eq!(text_sanity("   "), 0.0);
    }

    #[test]
    fn mrz_line_score_favors_a_real_td3_line_over_prose() {
        // Real TD3 line 2 (44 chars), monospace-ish bbox matching that length.
        let mrz = line(
            "L898902C36UTO7408122F1204159ZE184226B<<<<<10",
            0.0,
            0.0,
            44.0 * 20.0,
            30.0,
        );
        let prose = line(
            "Republic of Utopia issues this document",
            0.0,
            40.0,
            300.0,
            20.0,
        );
        assert!(mrz_line_score(&mrz, MRZ_CHARSET) > mrz_line_score(&prose, MRZ_CHARSET));
    }

    #[test]
    fn detect_mrz_band_finds_the_two_line_group_and_ignores_header_text() {
        let header = line("REPUBLIC OF UTOPIA PASSPORT", 0.0, 0.0, 300.0, 20.0);
        let mrz1 = line(
            "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<",
            0.0,
            100.0,
            44.0 * 20.0,
            30.0,
        );
        let mrz2 = line(
            "L898902C36UTO7408122F1204159ZE184226B<<<<<10",
            0.0,
            135.0,
            44.0 * 20.0,
            30.0,
        );
        let lines = vec![header, mrz1.clone(), mrz2.clone()];
        let band = detect_mrz_band(&lines, MRZ_CHARSET).expect("should find a band");
        let expected = BBox::union(&[mrz1.bbox, mrz2.bbox]).unwrap();
        assert_eq!(band, expected);
    }

    #[test]
    fn detect_mrz_band_returns_none_with_no_mrz_shaped_lines() {
        let lines = vec![
            line("FRONT OF CARD", 0.0, 0.0, 200.0, 20.0),
            line("SOME OTHER TEXT HERE", 0.0, 30.0, 200.0, 20.0),
        ];
        assert!(detect_mrz_band(&lines, MRZ_CHARSET).is_none());
    }

    #[test]
    fn detect_mrz_band_returns_none_on_too_few_lines() {
        let lines = vec![line("ONLY ONE LINE HERE OK", 0.0, 0.0, 100.0, 10.0)];
        assert!(detect_mrz_band(&lines, MRZ_CHARSET).is_none());
    }

    #[test]
    fn detect_portrait_finds_a_word_free_3_to_4_block_in_the_upper_left() {
        // 400x400 image; a photo-shaped empty block roughly (0,0)-(120,160)
        // sits in the upper-left quadrant, with text words everywhere else
        // in that quadrant's remaining space.
        let word_boxes = vec![
            BBox {
                x: 150.0,
                y: 0.0,
                w: 40.0,
                h: 10.0,
            },
            BBox {
                x: 0.0,
                y: 170.0,
                w: 190.0,
                h: 10.0,
            },
            BBox {
                x: 150.0,
                y: 60.0,
                w: 40.0,
                h: 10.0,
            },
        ];
        let portrait = detect_portrait(&word_boxes, 400, 400).expect("should find a portrait box");
        assert!(portrait.x < 130.0 && portrait.y < 20.0);
        let ratio = f64::from(portrait.w) / f64::from(portrait.h);
        assert!((ratio - PORTRAIT_ASPECT).abs() < PORTRAIT_ASPECT * PORTRAIT_ASPECT_TOLERANCE);
    }

    #[test]
    fn detect_portrait_none_when_quadrant_is_fully_occupied() {
        let word_boxes = vec![BBox {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 200.0,
        }];
        assert!(detect_portrait(&word_boxes, 400, 400).is_none());
    }

    #[test]
    fn detect_portrait_none_on_zero_sized_image() {
        assert!(detect_portrait(&[], 0, 0).is_none());
    }
}
