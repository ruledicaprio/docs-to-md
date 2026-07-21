//! Draws the detected MRZ band (and portrait box, if found) as a translucent
//! overlay on the image `synthpass-ocr` actually scored, and saves it next to
//! the input — a debug aid, not a shipped feature: "where does the geometry
//! code *think* the MRZ is?" is otherwise only answerable by printing numbers
//! and imagining the rectangle.
//!
//! Also appends one row to a plain JSONL ledger (`--ledger`, default
//! `docs/document-layout-survey.jsonl`) recording what was *measured*
//! (rotation, band/portrait boxes, a cheap keyword scan of the recognized
//! text) alongside an optional, separately-supplied *ground truth* — the
//! same measured/ground-truth separation `synthpass-bench` already uses.
//! This is a growing record for a document-type/page-side layout survey, not
//! a classifier: nothing here is fed back into `detect_mrz_band`/
//! `detect_portrait`'s own logic.
//!
//! Zero new dependencies, on either count:
//! - This hand-rolls an alpha blend and a stroked border over
//!   `image::RgbImage` directly rather than pulling in `rten-imageproc`'s
//!   `stroke_rect`/`fill_rect` — those exist and would work, but reaching
//!   them from an example means adding `rten-imageproc` as an explicit
//!   dependency (Cargo doesn't expose a crate's *transitive* deps to its own
//!   examples), and a dozen lines of blending code isn't worth that.
//! - The ledger is hand-written JSONL rather than using `serde_json` for the
//!   same transitive-dependency reason (`synthpass-ocr`'s dependency on
//!   `synthpass-core` doesn't make `synthpass-core`'s own `serde_json`
//!   dependency reachable here) — the schema is small and fixed, so a
//!   one-line-per-field writer with a minimal string escaper is simpler than
//!   adding a dependency edge for it.
//!
//! `mrz_band`'s coordinates are relative to the image *after*
//! `recognize_detailed`'s auto-rotation (`page.rotation`, always a multiple
//! of 90°) — not the file's raw orientation — so this example reproduces
//! that exact rotation with `image::imageops` before drawing, or the box
//! would land in the wrong place on a rotated page.
//!
//! Run from the repo root:
//! ```powershell
//! cargo run -p synthpass-ocr --release --example visualize_mrz_band -- <path-to-image> [out.png] [flags]
//!   --ledger PATH           JSONL ledger path (default: docs/document-layout-survey.jsonl)
//!   --kind K                ground truth: passport | id_card | driving_license
//!   --side S                ground truth, free text (not a closed enum — extend as new
//!                           page-layout distinctions show up in real specimens):
//!                             id_card/driving_license: front | back | face | category
//!                             passport: unfolded (flat data-page-only crop, no visible
//!                               second page or spine crease — every passport specimen
//!                               surveyed so far) | folded (data page photographed inside
//!                               the open booklet: second page, spine crease, and
//!                               non-document background all compete with the actual
//!                               MRZ/photo region in a way `unfolded` never does — a
//!                               materially different capture condition worth its own
//!                               ground-truth value once a folded specimen exists)
//!   --mrz-present true|false
//!   --mrz-icao true|false   is the MRZ (if present) ICAO 9303, vs. a different standard
//!   --photo-present true|false
//! All ground-truth flags are optional; omitted ones are recorded as `null`.
//! ```

use image::RgbImage;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use synthpass_ocr::geometry::BBox;
use synthpass_ocr::NativeOcr;

/// Green, ~35% opacity fill plus a solid 3px border — visible on both light
/// and dark document backgrounds without hiding the text underneath.
const MRZ_BAND_COLOR: [u8; 3] = [0, 220, 90];
const MRZ_BAND_ALPHA: f32 = 0.35;
const MRZ_BAND_BORDER_PX: i64 = 3;

/// Blue, so it's visually distinct from the MRZ band when both are present.
const PORTRAIT_COLOR: [u8; 3] = [60, 140, 255];
const PORTRAIT_ALPHA: f32 = 0.25;
const PORTRAIT_BORDER_PX: i64 = 2;

/// Keyword groups for the descriptive scan, grouped by what they signal.
/// Deliberately just the markers already confirmed present in this session's
/// six-specimen survey (`docs/document-layout-survey.jsonl`'s first rows) —
/// not an attempt at a complete multilingual list. Matched case-insensitively
/// as a plain substring search over the recognized text, so this is *evidence
/// for the ledger*, not a decision: a miss here doesn't mean the keyword
/// isn't on the page, only that OCR didn't recognize it cleanly.
const KEYWORD_GROUPS: &[(&str, &[&str])] = &[
    ("passport", &["PASSPORT", "PASOS", "PASOŠ", "PUTOVNICA"]),
    (
        "identity_card",
        &[
            "IDENTITY CARD",
            "LIČNA KARTA",
            "LICNA KARTA",
            "OSOBNA ISKAZNICA",
        ],
    ),
    (
        "driving_license",
        &[
            "DRIVING LICEN",
            "VOZAČKA",
            "VOZACKA",
            "RIJBEWIJS",
            "PERMIS DE CONDUIRE",
        ],
    ),
];

struct Args {
    path: PathBuf,
    out_path: Option<PathBuf>,
    ledger: PathBuf,
    kind: Option<String>,
    side: Option<String>,
    mrz_present: Option<bool>,
    mrz_icao: Option<bool>,
    photo_present: Option<bool>,
}

fn usage() -> ! {
    eprintln!(
        "usage: visualize_mrz_band <path-to-image> [out.png] [--ledger PATH] [--kind K] \
         [--side S] [--mrz-present true|false] [--mrz-icao true|false] \
         [--photo-present true|false]"
    );
    std::process::exit(2);
}

fn parse_args(raw: &[String]) -> Args {
    let mut positional = Vec::new();
    let mut ledger = None;
    let mut kind = None;
    let mut side = None;
    let mut mrz_present = None;
    let mut mrz_icao = None;
    let mut photo_present = None;

    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--ledger" => {
                ledger = Some(PathBuf::from(raw.get(i + 1).unwrap_or_else(|| usage())));
                i += 2;
            }
            "--kind" => {
                kind = Some(raw.get(i + 1).unwrap_or_else(|| usage()).clone());
                i += 2;
            }
            "--side" => {
                side = Some(raw.get(i + 1).unwrap_or_else(|| usage()).clone());
                i += 2;
            }
            "--mrz-present" => {
                mrz_present = Some(parse_bool_flag(raw.get(i + 1)));
                i += 2;
            }
            "--mrz-icao" => {
                mrz_icao = Some(parse_bool_flag(raw.get(i + 1)));
                i += 2;
            }
            "--photo-present" => {
                photo_present = Some(parse_bool_flag(raw.get(i + 1)));
                i += 2;
            }
            other if !other.starts_with("--") => {
                positional.push(other.to_string());
                i += 1;
            }
            _ => usage(),
        }
    }

    if positional.is_empty() {
        usage();
    }
    Args {
        path: PathBuf::from(&positional[0]),
        out_path: positional.get(1).map(PathBuf::from),
        ledger: ledger.unwrap_or_else(|| repo_root().join("docs/document-layout-survey.jsonl")),
        kind,
        side,
        mrz_present,
        mrz_icao,
        photo_present,
    }
}

fn parse_bool_flag(v: Option<&String>) -> bool {
    match v.map(String::as_str) {
        Some("true") => true,
        Some("false") => false,
        _ => usage(),
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&raw);

    if !args.path.is_file() {
        eprintln!("not a file: {}", args.path.display());
        std::process::exit(2);
    }
    let out_path = args.out_path.clone().unwrap_or_else(|| {
        let stem = args
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("out");
        args.path.with_file_name(format!("{stem}_mrz_band.png"))
    });

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

    let page = match ocr.recognize_detailed(&args.path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("recognize_detailed failed: {e}");
            std::process::exit(1);
        }
    };

    let keyword_hits = scan_keywords(&page.text);

    println!("rotation applied before scoring: {}°", page.rotation);
    println!("lines detected: {}", page.lines.len());
    match &page.mrz_band {
        Some(b) => println!(
            "MRZ band: x={:.1} y={:.1} w={:.1} h={:.1}",
            b.x, b.y, b.w, b.h
        ),
        None => println!("MRZ band: none (no group scored high enough to be confident)"),
    }
    match &page.portrait {
        Some(b) => println!(
            "portrait: x={:.1} y={:.1} w={:.1} h={:.1}",
            b.x, b.y, b.w, b.h
        ),
        None => println!("portrait: none"),
    }
    if keyword_hits.is_empty() {
        println!("keyword scan: no matches");
    } else {
        println!("keyword scan: {}", keyword_hits.join(", "));
    }

    let mut image = image::open(&args.path)
        .unwrap_or_else(|e| {
            eprintln!("failed to open {}: {e}", args.path.display());
            std::process::exit(1);
        })
        .into_rgb8();
    let (orig_width, orig_height) = image.dimensions();
    image = match page.rotation {
        90 => image::imageops::rotate90(&image),
        180 => image::imageops::rotate180(&image),
        270 => image::imageops::rotate270(&image),
        _ => image,
    };

    if let Some(b) = page.mrz_band {
        draw_box(
            &mut image,
            b,
            MRZ_BAND_COLOR,
            MRZ_BAND_ALPHA,
            MRZ_BAND_BORDER_PX,
        );
    }
    if let Some(b) = page.portrait {
        draw_box(
            &mut image,
            b,
            PORTRAIT_COLOR,
            PORTRAIT_ALPHA,
            PORTRAIT_BORDER_PX,
        );
    }

    if let Err(e) = image.save(&out_path) {
        eprintln!("failed to save {}: {e}", out_path.display());
        std::process::exit(1);
    }
    println!("wrote {}", out_path.display());

    if let Err(e) = append_ledger_row(&args, &page, (orig_width, orig_height), &keyword_hits) {
        eprintln!("warning: failed to append ledger row: {e}");
    } else {
        println!("appended to {}", args.ledger.display());
    }
}

/// Case-insensitive substring scan for this tool's known document-type
/// signal words. Returns the *group names* that matched (e.g.
/// `"identity_card"`), not the individual keyword, since the ledger only
/// needs "does this page look like an ID card" — which specific translation
/// matched is incidental.
fn scan_keywords(text: &str) -> Vec<&'static str> {
    let upper = text.to_uppercase();
    KEYWORD_GROUPS
        .iter()
        .filter(|(_, keywords)| keywords.iter().any(|k| upper.contains(k)))
        .map(|(group, _)| *group)
        .collect()
}

/// Alpha-blends `color` at `alpha` over `bbox`'s interior, then stamps a
/// fully opaque `border_px`-wide border on top so the box's edge stays
/// crisp even where the translucent fill would be hard to see against a
/// similarly-colored background.
fn draw_box(image: &mut RgbImage, bbox: BBox, color: [u8; 3], alpha: f32, border_px: i64) {
    let (width, height) = image.dimensions();
    let x0 = bbox.x.max(0.0) as i64;
    let y0 = bbox.y.max(0.0) as i64;
    let x1 = ((bbox.x + bbox.w).max(0.0) as i64).min(width as i64);
    let y1 = ((bbox.y + bbox.h).max(0.0) as i64).min(height as i64);

    for y in y0..y1 {
        for x in x0..x1 {
            let on_border = x < x0 + border_px
                || x >= x1 - border_px
                || y < y0 + border_px
                || y >= y1 - border_px;
            let px = image.get_pixel_mut(x as u32, y as u32);
            if on_border {
                px.0 = color;
            } else {
                for (channel, blend) in px.0.iter_mut().zip(color) {
                    *channel = (*channel as f32 * (1.0 - alpha) + blend as f32 * alpha) as u8;
                }
            }
        }
    }
}

/// Appends one JSON object per line to `args.ledger`, creating it (and its
/// parent directory) if it doesn't exist yet. Hand-rolled rather than
/// `serde_json` — see the module doc comment.
fn append_ledger_row(
    args: &Args,
    page: &synthpass_ocr::geometry::OcrPage,
    (width, height): (u32, u32),
    keyword_hits: &[&str],
) -> std::io::Result<()> {
    use std::io::Write;

    let filename = args.path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    let mut row = String::new();
    let _ = write!(row, "{{");
    let _ = write!(row, "\"filename\":{}", json_string(filename));
    let _ = write!(row, ",\"width\":{width},\"height\":{height}");
    let _ = write!(row, ",\"rotation\":{}", page.rotation);
    let _ = write!(row, ",\"lines_detected\":{}", page.lines.len());
    let _ = write!(row, ",\"mrz_band\":{}", json_bbox(&page.mrz_band));
    let _ = write!(row, ",\"portrait\":{}", json_bbox(&page.portrait));
    let _ = write!(row, ",\"keyword_hits\":[");
    for (i, hit) in keyword_hits.iter().enumerate() {
        if i > 0 {
            let _ = write!(row, ",");
        }
        let _ = write!(row, "{}", json_string(hit));
    }
    let _ = write!(row, "]");
    let _ = write!(row, ",\"ground_truth\":{{");
    let _ = write!(row, "\"document_kind\":{}", json_opt_string(&args.kind));
    let _ = write!(row, ",\"page_side\":{}", json_opt_string(&args.side));
    let _ = write!(row, ",\"mrz_present\":{}", json_opt_bool(args.mrz_present));
    let _ = write!(row, ",\"mrz_is_icao\":{}", json_opt_bool(args.mrz_icao));
    let _ = write!(
        row,
        ",\"photo_present\":{}",
        json_opt_bool(args.photo_present)
    );
    let _ = write!(row, "}}");
    let _ = write!(row, "}}");

    if let Some(parent) = args.ledger.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.ledger)?;
    writeln!(file, "{row}")
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_opt_string(s: &Option<String>) -> String {
    match s {
        Some(v) => json_string(v),
        None => "null".to_string(),
    }
}

fn json_opt_bool(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    }
}

fn json_bbox(b: &Option<BBox>) -> String {
    match b {
        Some(b) => format!(
            "{{\"x\":{:.1},\"y\":{:.1},\"w\":{:.1},\"h\":{:.1}}}",
            b.x, b.y, b.w, b.h
        ),
        None => "null".to_string(),
    }
}

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/synthpass-ocr -> repo root is two levels up.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}
