//! Fixed pixel geometry for a TD3 passport data page.
//!
//! Every rectangle here is a deterministic constant — the same layout is used
//! for every render, which is what lets [`crate::labels::Labels`] be 100%
//! accurate by construction: the generator knows exactly where it is about to
//! draw a field before it draws it.

/// A pixel-space bounding box, `(x, y)` top-left plus `width`/`height`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether this rectangle lies entirely within a `width x height` image.
    pub fn within_bounds(self, width: u32, height: u32) -> bool {
        self.x + self.width <= width && self.y + self.height <= height
    }
}

/// Overall data-page canvas size, roughly the ID-3 aspect ratio (125mm x 88mm).
pub const IMAGE_WIDTH: u32 = 1200;
pub const IMAGE_HEIGHT: u32 = 840;

/// Portrait photo placeholder box.
pub const PORTRAIT: Rect = Rect::new(860, 100, 260, 340);

/// VIZ (visual inspection zone) text fields, top half of the page.
pub const DOCUMENT_TYPE: Rect = Rect::new(60, 60, 120, 34);
pub const ISSUING_COUNTRY: Rect = Rect::new(220, 60, 120, 34);
pub const SURNAME: Rect = Rect::new(60, 120, 700, 34);
pub const GIVEN_NAMES: Rect = Rect::new(60, 170, 700, 34);
pub const DOCUMENT_NUMBER: Rect = Rect::new(60, 220, 300, 34);
pub const NATIONALITY: Rect = Rect::new(60, 270, 200, 34);
pub const DATE_OF_BIRTH: Rect = Rect::new(280, 270, 240, 34);
pub const SEX: Rect = Rect::new(540, 270, 80, 34);
pub const DATE_OF_EXPIRY: Rect = Rect::new(60, 320, 240, 34);
pub const PERSONAL_NUMBER: Rect = Rect::new(60, 370, 400, 34);

/// The unconditional "SYNTHETIC / SPECIMEN" watermark band (see
/// `render::draw_watermark`) — always drawn, independent of the
/// `embedded-fonts` feature.
pub const WATERMARK: Rect = Rect::new(60, 470, 1080, 60);

/// The 2-line MRZ band at the bottom of the page. Each line is 44 MRZ
/// characters wide.
pub const MRZ_CHARS: u32 = 44;
pub const MRZ_LINE1: Rect = Rect::new(60, 720, 1080, 50);
pub const MRZ_LINE2: Rect = Rect::new(60, 775, 1080, 50);

/// Pixel rectangle for MRZ character `index` (0-based) within `line`.
pub fn mrz_char_rect(line: Rect, index: u32) -> Rect {
    debug_assert!(index < MRZ_CHARS);
    let cell_width = line.width / MRZ_CHARS;
    Rect::new(line.x + index * cell_width, line.y, cell_width, line.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_named_rects_fit_the_canvas() {
        let rects = [
            PORTRAIT,
            DOCUMENT_TYPE,
            ISSUING_COUNTRY,
            SURNAME,
            GIVEN_NAMES,
            DOCUMENT_NUMBER,
            NATIONALITY,
            DATE_OF_BIRTH,
            SEX,
            DATE_OF_EXPIRY,
            PERSONAL_NUMBER,
            WATERMARK,
            MRZ_LINE1,
            MRZ_LINE2,
        ];
        for r in rects {
            assert!(
                r.within_bounds(IMAGE_WIDTH, IMAGE_HEIGHT),
                "{r:?} escapes the {IMAGE_WIDTH}x{IMAGE_HEIGHT} canvas"
            );
        }
    }

    #[test]
    fn mrz_char_cells_fit_within_the_line() {
        for i in 0..MRZ_CHARS {
            let cell = mrz_char_rect(MRZ_LINE1, i);
            assert!(cell.x + cell.width <= MRZ_LINE1.x + MRZ_LINE1.width);
        }
    }
}
