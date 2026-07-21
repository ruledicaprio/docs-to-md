//! Compose the rendered `DynamicImage` for a generated document.
//!
//! Two guardrails in this module are **unconditional** — they render
//! regardless of the `embedded-fonts` feature and cannot be disabled through
//! [`crate::GeneratorConfig`]:
//!
//! 1. [`draw_watermark`] stamps a "SYNTHETIC / SPECIMEN" watermark, drawn from
//!    a hand-authored 5x7 bitmap font defined in this file — it never depends
//!    on a TTF being present.
//! 2. The background composed by [`render`] is a generic, non-country
//!    template: a plain frame and neutral fill, no national emblem, coat of
//!    arms, or issuing-country branding of any kind.
//!
//! When `embedded-fonts` is off (the default), VIZ and MRZ text degrade to
//! placeholder bars drawn in the exact [`crate::layout`] rectangles, so
//! bounding boxes stay meaningful even without real glyphs.

use image::{DynamicImage, Rgb, RgbImage};

use crate::fonts::{load_fonts, Fonts};
use crate::labels::Labels;
use crate::layout::{self, Rect};
use crate::model::Passport;

const BACKGROUND: Rgb<u8> = Rgb([244, 243, 236]);
const FRAME: Rgb<u8> = Rgb([70, 72, 90]);
const PORTRAIT_FILL: Rgb<u8> = Rgb([205, 205, 210]);
const PLACEHOLDER_BAR: Rgb<u8> = Rgb([120, 122, 140]);
const MRZ_CELL: Rgb<u8> = Rgb([30, 30, 40]);
const WATERMARK_COLOR: Rgb<u8> = Rgb([176, 48, 48]);

fn fill_rect(img: &mut RgbImage, rect: Rect, color: Rgb<u8>) {
    for y in rect.y..(rect.y + rect.height).min(img.height()) {
        for x in rect.x..(rect.x + rect.width).min(img.width()) {
            img.put_pixel(x, y, color);
        }
    }
}

fn border_rect(img: &mut RgbImage, rect: Rect, color: Rgb<u8>, thickness: u32) {
    let t = thickness.max(1);
    fill_rect(img, Rect::new(rect.x, rect.y, rect.width, t), color);
    fill_rect(
        img,
        Rect::new(
            rect.x,
            (rect.y + rect.height).saturating_sub(t),
            rect.width,
            t,
        ),
        color,
    );
    fill_rect(img, Rect::new(rect.x, rect.y, t, rect.height), color);
    fill_rect(
        img,
        Rect::new(
            (rect.x + rect.width).saturating_sub(t),
            rect.y,
            t,
            rect.height,
        ),
        color,
    );
}

/// A placeholder bar occupying most of `rect`'s height, vertically centered —
/// stands in for VIZ text when no real font is embedded.
fn draw_placeholder_bar(img: &mut RgbImage, rect: Rect) {
    let inset_y = rect.height / 4;
    let bar = Rect::new(
        rect.x,
        rect.y + inset_y,
        rect.width,
        rect.height.saturating_sub(2 * inset_y).max(1),
    );
    fill_rect(img, bar, PLACEHOLDER_BAR);
}

/// Placeholder rendering of one MRZ line: each of the 44 character cells is
/// filled when the printed character is not the `<` filler, and left blank
/// otherwise — this keeps the per-character bounding boxes meaningful (filler
/// runs stay visually empty) without needing real glyphs.
fn draw_mrz_placeholder(img: &mut RgbImage, line_rect: Rect, text: &str) {
    for (i, c) in text.chars().enumerate() {
        if c == '<' {
            continue;
        }
        let cell = layout::mrz_char_rect(line_rect, i as u32);
        let inset = cell.height / 5;
        let glyph_box = Rect::new(
            cell.x + 1,
            cell.y + inset,
            cell.width.saturating_sub(2),
            cell.height.saturating_sub(2 * inset).max(1),
        );
        fill_rect(img, glyph_box, MRZ_CELL);
    }
}

// ---------------------------------------------------------------------
// Real glyph rendering (only compiled with `embedded-fonts`).
// ---------------------------------------------------------------------

/// Alpha-blend `fg` over `bg` by the rasterizer's per-pixel coverage — using
/// the full coverage gradient (instead of a hard on/off threshold) preserves
/// the sub-pixel edge shape that OCR models rely on to distinguish
/// similarly-shaped glyphs (e.g. `7`/`Z`, `O`/`0`).
fn blend(bg: Rgb<u8>, fg: Rgb<u8>, coverage: f32) -> Rgb<u8> {
    let a = coverage.clamp(0.0, 1.0);
    let mut out = [0u8; 3];
    for k in 0..3 {
        out[k] = (bg[k] as f32 * (1.0 - a) + fg[k] as f32 * a).round() as u8;
    }
    Rgb(out)
}

#[cfg(feature = "embedded-fonts")]
fn draw_one_glyph(img: &mut RgbImage, font: &ab_glyph::FontArc, glyph: ab_glyph::Glyph) {
    use ab_glyph::Font;

    let Some(outlined) = font.outline_glyph(glyph) else {
        return;
    };
    let bounds = outlined.px_bounds();
    outlined.draw(|gx, gy, coverage| {
        if coverage <= 0.0 {
            return;
        }
        let px = bounds.min.x as i32 + gx as i32;
        let py = bounds.min.y as i32 + gy as i32;
        if px >= 0 && py >= 0 {
            let (px, py) = (px as u32, py as u32);
            if px < img.width() && py < img.height() {
                let bg = *img.get_pixel(px, py);
                img.put_pixel(px, py, blend(bg, MRZ_CELL, coverage));
            }
        }
    });
}

/// Flows `text` left-to-right using the font's own advance widths — fine for
/// VIZ fields, which aren't checksum-validated and just need to look
/// plausible within `rect`.
#[cfg(feature = "embedded-fonts")]
fn draw_glyph_text(
    img: &mut RgbImage,
    font: &ab_glyph::FontArc,
    text: &str,
    rect: Rect,
    px_scale: f32,
) {
    use ab_glyph::{point, Font, ScaleFont};

    let scaled = font.as_scaled(px_scale);
    let mut x = rect.x as f32;
    let y = rect.y as f32 + scaled.ascent();
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        let glyph = id.with_scale_and_position(px_scale, point(x, y));
        draw_one_glyph(img, font, glyph);
        x += scaled.h_advance(id);
    }
}

/// Draws one MRZ character per fixed-width cell from
/// [`layout::mrz_char_rect`], centering each glyph within its own cell
/// instead of flowing by the font's natural advance. MRZ text is checksum-
/// validated after OCR, so cross-character drift from an advance/cell-width
/// mismatch (which compounds over all 44 columns) must not be allowed to
/// merge or overlap adjacent glyphs.
#[cfg(feature = "embedded-fonts")]
fn draw_mrz_glyphs(img: &mut RgbImage, font: &ab_glyph::FontArc, text: &str, line_rect: Rect) {
    use ab_glyph::{point, Font, ScaleFont};

    let px_scale = line_rect.height as f32 * 0.8;
    let scaled = font.as_scaled(px_scale);
    let y = line_rect.y as f32 + scaled.ascent();
    for (i, c) in text.chars().enumerate() {
        let cell = layout::mrz_char_rect(line_rect, i as u32);
        let id = scaled.glyph_id(c);
        let advance = scaled.h_advance(id);
        let x = cell.x as f32 + ((cell.width as f32 - advance) / 2.0).max(0.0);
        let glyph = id.with_scale_and_position(px_scale, point(x, y));
        draw_one_glyph(img, font, glyph);
    }
}

fn draw_text_field(img: &mut RgbImage, rect: Rect, text: &str, fonts: Option<&Fonts>) {
    #[cfg(feature = "embedded-fonts")]
    if let Some(fonts) = fonts {
        draw_glyph_text(img, &fonts.viz, text, rect, rect.height as f32 * 0.7);
        return;
    }
    #[cfg(not(feature = "embedded-fonts"))]
    let _ = (fonts, text);
    draw_placeholder_bar(img, rect);
}

fn draw_mrz_line(img: &mut RgbImage, rect: Rect, text: &str, fonts: Option<&Fonts>) {
    #[cfg(feature = "embedded-fonts")]
    if let Some(fonts) = fonts {
        draw_mrz_glyphs(img, &fonts.mrz, text, rect);
        return;
    }
    #[cfg(not(feature = "embedded-fonts"))]
    let _ = fonts;
    draw_mrz_placeholder(img, rect, text);
}

// ---------------------------------------------------------------------
// Hand-authored 5x7 bitmap font — used ONLY for the mandatory watermark, so
// the "SYNTHETIC / SPECIMEN" stamp never depends on a TTF being available.
// ---------------------------------------------------------------------

/// 7 rows x 5 columns, `1` = ink. Covers exactly the characters needed to
/// spell "SYNTHETIC / SPECIMEN".
fn glyph_5x7(c: char) -> [[u8; 5]; 7] {
    match c {
        'S' => [
            [0, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [0, 1, 1, 1, 0],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [1, 1, 1, 1, 0],
        ],
        'Y' => [
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [0, 1, 0, 1, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
        ],
        'N' => [
            [1, 0, 0, 0, 1],
            [1, 1, 0, 0, 1],
            [1, 0, 1, 0, 1],
            [1, 0, 0, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
        ],
        'T' => [
            [1, 1, 1, 1, 1],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
        ],
        'H' => [
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
        ],
        'E' => [
            [1, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 1, 1, 1, 1],
        ],
        'I' => [
            [0, 1, 1, 1, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 0, 1, 0, 0],
            [0, 1, 1, 1, 0],
        ],
        'C' => [
            [0, 1, 1, 1, 1],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [0, 1, 1, 1, 1],
        ],
        'P' => [
            [1, 1, 1, 1, 0],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 1, 1, 1, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
        ],
        'M' => [
            [1, 0, 0, 0, 1],
            [1, 1, 0, 1, 1],
            [1, 0, 1, 0, 1],
            [1, 0, 1, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
            [1, 0, 0, 0, 1],
        ],
        '/' => [
            [0, 0, 0, 0, 1],
            [0, 0, 0, 0, 1],
            [0, 0, 0, 1, 0],
            [0, 0, 1, 0, 0],
            [0, 1, 0, 0, 0],
            [1, 0, 0, 0, 0],
            [1, 0, 0, 0, 0],
        ],
        _ => [[0; 5]; 7], // space and anything unrecognized: blank
    }
}

fn draw_bitmap_char(img: &mut RgbImage, c: char, x: u32, y: u32, scale: u32, color: Rgb<u8>) {
    let glyph = glyph_5x7(c);
    for (row_idx, row) in glyph.iter().enumerate() {
        for (col_idx, &on) in row.iter().enumerate() {
            if on == 0 {
                continue;
            }
            let px0 = x + col_idx as u32 * scale;
            let py0 = y + row_idx as u32 * scale;
            for dy in 0..scale {
                for dx in 0..scale {
                    let (px, py) = (px0 + dx, py0 + dy);
                    if px < img.width() && py < img.height() {
                        img.put_pixel(px, py, color);
                    }
                }
            }
        }
    }
}

/// Draw "SYNTHETIC / SPECIMEN" from the hand-authored bitmap font in
/// [`layout::WATERMARK`], tiled to fill the band. This is one of the two
/// mandatory ethics guardrails: it renders unconditionally, independent of
/// the `embedded-fonts` feature and not configurable via [`crate::GeneratorConfig`].
fn draw_watermark(img: &mut RgbImage) {
    const TEXT: &str = "SYNTHETIC / SPECIMEN ";
    let rect = layout::WATERMARK;
    let scale = 4u32;
    let glyph_w = 5 * scale;
    let advance = glyph_w + scale;
    let glyph_h = 7 * scale;
    let y = rect.y + rect.height.saturating_sub(glyph_h) / 2;

    let mut x = rect.x;
    'tiles: loop {
        for c in TEXT.chars() {
            if x + glyph_w > rect.x + rect.width {
                break 'tiles;
            }
            draw_bitmap_char(img, c, x, y, scale, WATERMARK_COLOR);
            x += advance;
        }
    }
}

/// Compose the full data-page image for `passport`, using `labels` as the
/// single source of truth for text content and placement.
pub fn render(passport: &Passport, labels: &Labels) -> DynamicImage {
    // `labels` is the single source of drawn text (kept in sync with
    // `passport` by construction, see `labels::build_labels`); `passport` is
    // accepted for API symmetry with `crate::generate` and future per-field
    // render options (e.g. photo synthesis keyed off sex/nationality).
    let _ = passport;
    let mut img = RgbImage::from_pixel(layout::IMAGE_WIDTH, layout::IMAGE_HEIGHT, BACKGROUND);

    // Generic, non-country template: a plain frame only. Deliberately no
    // national emblem, coat of arms, or issuing-country branding — guardrail
    // #2, unconditional regardless of `embedded-fonts`.
    border_rect(
        &mut img,
        Rect::new(0, 0, layout::IMAGE_WIDTH, layout::IMAGE_HEIGHT),
        FRAME,
        6,
    );

    // Portrait placeholder: a plain filled box, never a rendered likeness.
    fill_rect(&mut img, layout::PORTRAIT, PORTRAIT_FILL);
    border_rect(&mut img, layout::PORTRAIT, FRAME, 2);

    let fonts: Option<Fonts> = load_fonts().ok();
    let fonts_ref = fonts.as_ref();

    draw_text_field(
        &mut img,
        layout::DOCUMENT_TYPE,
        &labels.document_type.value,
        fonts_ref,
    );
    draw_text_field(
        &mut img,
        layout::ISSUING_COUNTRY,
        &labels.issuing_country.value,
        fonts_ref,
    );
    draw_text_field(&mut img, layout::SURNAME, &labels.surname.value, fonts_ref);
    draw_text_field(
        &mut img,
        layout::GIVEN_NAMES,
        &labels.given_names.value,
        fonts_ref,
    );
    draw_text_field(
        &mut img,
        layout::DOCUMENT_NUMBER,
        &labels.document_number.value,
        fonts_ref,
    );
    draw_text_field(
        &mut img,
        layout::NATIONALITY,
        &labels.nationality.value,
        fonts_ref,
    );
    draw_text_field(
        &mut img,
        layout::DATE_OF_BIRTH,
        &labels.date_of_birth.value,
        fonts_ref,
    );
    draw_text_field(&mut img, layout::SEX, &labels.sex.value, fonts_ref);
    draw_text_field(
        &mut img,
        layout::DATE_OF_EXPIRY,
        &labels.date_of_expiry.value,
        fonts_ref,
    );
    if let Some(pn) = &labels.personal_number {
        draw_text_field(&mut img, layout::PERSONAL_NUMBER, &pn.value, fonts_ref);
    }

    draw_mrz_line(&mut img, layout::MRZ_LINE1, &labels.mrz_line1, fonts_ref);
    draw_mrz_line(&mut img, layout::MRZ_LINE2, &labels.mrz_line2, fonts_ref);

    // Guardrail #1: unconditional synthetic watermark, drawn last so it stays
    // on top of every other element.
    draw_watermark(&mut img);

    DynamicImage::ImageRgb8(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_renders_without_embedded_fonts() {
        let img = RgbImage::from_pixel(layout::IMAGE_WIDTH, layout::IMAGE_HEIGHT, BACKGROUND);
        let mut watermarked = img.clone();
        draw_watermark(&mut watermarked);

        let rect = layout::WATERMARK;
        let mut differs = false;
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                if watermarked.get_pixel(x, y) != img.get_pixel(x, y) {
                    differs = true;
                    break;
                }
            }
        }
        assert!(
            differs,
            "watermark region must differ from a blank template"
        );
    }
}
