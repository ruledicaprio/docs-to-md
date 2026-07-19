//! Image preprocessing variants for the MRZ-targeted retry passes.
//!
//! The failure modes these address (measured on `samples/` via
//! `examples/mrz_corpus.rs`, see docs/ARCHITECTURE.md §8): low-resolution
//! scans whose MRZ glyphs are too small for clean recognition, and
//! low-contrast photos where the detector fragments an MRZ line into
//! disconnected pieces. Each helper is deterministic and pure-Rust (`image`
//! crate only); correctness of any retry built on them is proven or rejected
//! by the ICAO check digits upstream, never assumed.

use image::imageops::FilterType;
use image::{GrayImage, RgbImage};

/// Upscale target for the shorter side of a full-page retry pass. Below
/// roughly this, `ocrs` recognition quality on MRZ glyphs degrades sharply
/// (the ~300-DPI-equivalent heuristic `ocr-daemon`'s Tesseract path also
/// uses, scaled down: `ocrs` normalizes internally, so past this point extra
/// pixels stop helping).
const FULL_PAGE_MIN_DIM: u32 = 1000;

/// Upscale target for the width of the bottom-band crop.
const BAND_MIN_WIDTH: u32 = 1600;

/// Never upscale beyond this factor — past it there is no new signal, only
/// interpolation blur and slower inference.
const MAX_SCALE: f64 = 5.0;

/// The MRZ sits in the bottom portion of every ICAO layout (TD1 rear, TD2,
/// TD3 data page). 45% keeps all three MRZ lines with margin even on rear
/// sides where the zone starts near mid-card.
const BAND_FRACTION: f64 = 0.45;

/// The ordered preprocessing variants for the MRZ retry passes, cheapest
/// first. Callers run OCR over each until the checksum oracle validates.
pub fn mrz_variants(image: &RgbImage) -> Vec<RgbImage> {
    let band = bottom_band(image);
    vec![
        contrast_stretched(&upscale_to_width(&band, BAND_MIN_WIDTH)),
        binarized(&upscale_to_width(&band, BAND_MIN_WIDTH)),
        upscale_to_min_dim(image, FULL_PAGE_MIN_DIM),
    ]
}

/// Crop the bottom [`BAND_FRACTION`] of the image (full width).
fn bottom_band(image: &RgbImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let band_h = ((f64::from(h) * BAND_FRACTION).round() as u32).clamp(1, h);
    image::imageops::crop_imm(image, 0, h - band_h, w, band_h).to_image()
}

fn scale_by(image: &RgbImage, scale: f64) -> RgbImage {
    if scale <= 1.0 {
        return image.clone();
    }
    let scale = scale.min(MAX_SCALE);
    let (w, h) = image.dimensions();
    image::imageops::resize(
        image,
        (f64::from(w) * scale).round() as u32,
        (f64::from(h) * scale).round() as u32,
        FilterType::Lanczos3,
    )
}

fn upscale_to_width(image: &RgbImage, min_width: u32) -> RgbImage {
    let w = image.width().max(1);
    scale_by(image, f64::from(min_width) / f64::from(w))
}

fn upscale_to_min_dim(image: &RgbImage, min_dim: u32) -> RgbImage {
    let shorter = image.width().min(image.height()).max(1);
    scale_by(image, f64::from(min_dim) / f64::from(shorter))
}

/// Grayscale + linear contrast stretch mapping the 1st..99th intensity
/// percentiles to 0..255, returned as RGB (what `ocrs`'s input path expects).
/// Robust to the washed-out look of guilloche-patterned document backgrounds
/// without the all-or-nothing commitment of hard binarization.
fn contrast_stretched(image: &RgbImage) -> RgbImage {
    let gray = to_gray(image);
    let (lo, hi) = percentile_bounds(&gray, 0.01, 0.99);
    let range = f64::from(hi.saturating_sub(lo)).max(1.0);
    gray_to_rgb_map(&gray, |v| {
        ((f64::from(v.saturating_sub(lo)) / range * 255.0).round() as i64).clamp(0, 255) as u8
    })
}

/// Grayscale + Otsu global threshold, returned as RGB. The strongest variant
/// on clean-but-tiny scans, the weakest on uneven lighting — which is why it
/// runs as one candidate among several rather than unconditionally (the same
/// method `ocr-daemon`'s Tesseract path applies unconditionally).
fn binarized(image: &RgbImage) -> RgbImage {
    let gray = to_gray(image);
    let t = otsu_threshold(&gray);
    gray_to_rgb_map(&gray, |v| if v > t { 255 } else { 0 })
}

fn to_gray(image: &RgbImage) -> GrayImage {
    image::imageops::grayscale(image)
}

fn gray_to_rgb_map(gray: &GrayImage, f: impl Fn(u8) -> u8) -> RgbImage {
    RgbImage::from_fn(gray.width(), gray.height(), |x, y| {
        let v = f(gray.get_pixel(x, y)[0]);
        image::Rgb([v, v, v])
    })
}

/// Intensity values at the given cumulative-histogram fractions.
fn percentile_bounds(gray: &GrayImage, lo_frac: f64, hi_frac: f64) -> (u8, u8) {
    let mut hist = [0u64; 256];
    for p in gray.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return (0, 255);
    }
    let lo_target = (total as f64 * lo_frac) as u64;
    let hi_target = (total as f64 * hi_frac) as u64;
    let (mut lo, mut hi, mut cum) = (0u8, 255u8, 0u64);
    let mut lo_set = false;
    for (v, &count) in hist.iter().enumerate() {
        cum += count;
        if !lo_set && cum > lo_target {
            lo = v as u8;
            lo_set = true;
        }
        if cum >= hi_target {
            hi = v as u8;
            break;
        }
    }
    (lo, hi.max(lo))
}

/// Otsu's global threshold from the 256-bin intensity histogram (same method
/// as `ocr-daemon::preprocess::otsu_threshold`; duplicated because that crate
/// only compiles on Linux/WSL against the Tesseract C stack).
fn otsu_threshold(gray: &GrayImage) -> u8 {
    let mut hist = [0u32; 256];
    for p in gray.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total = gray.width() * gray.height();
    if total == 0 {
        return 127;
    }
    let sum: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &c)| i as f64 * c as f64)
        .sum();

    let mut sum_b = 0f64;
    let mut w_b = 0u32;
    let mut max_var = -1f64;
    let mut thresh = 0u8;
    for (t, &count) in hist.iter().enumerate() {
        w_b += count;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * count as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum - sum_b) / w_f as f64;
        let var = w_b as f64 * w_f as f64 * (m_b - m_f).powi(2);
        if var > max_var {
            max_var = var;
            thresh = t as u8;
        }
    }
    thresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    fn solid(w: u32, h: u32, v: u8) -> RgbImage {
        RgbImage::from_pixel(w, h, Rgb([v, v, v]))
    }

    #[test]
    fn bottom_band_keeps_full_width_and_bottom_rows() {
        let mut img = solid(100, 100, 255);
        // Mark the bottom row so we can prove the crop kept the *bottom*.
        for x in 0..100 {
            img.put_pixel(x, 99, Rgb([0, 0, 0]));
        }
        let band = bottom_band(&img);
        assert_eq!(band.width(), 100);
        assert_eq!(band.height(), 45);
        assert_eq!(band.get_pixel(0, 44)[0], 0, "bottom row survives the crop");
    }

    #[test]
    fn upscale_caps_at_max_scale() {
        let img = solid(100, 50, 128);
        let out = upscale_to_width(&img, 1600);
        // 16× requested, capped at MAX_SCALE (5×).
        assert_eq!(out.width(), 500);
        assert_eq!(out.height(), 250);
    }

    #[test]
    fn upscale_never_downscales() {
        let img = solid(2000, 1500, 128);
        assert_eq!(upscale_to_min_dim(&img, 1000).dimensions(), (2000, 1500));
    }

    #[test]
    fn contrast_stretch_expands_a_narrow_range() {
        // Two clusters squeezed into 100..140 must spread toward 0/255.
        let img = RgbImage::from_fn(64, 64, |x, _| {
            let v = if x < 32 { 100 } else { 140 };
            Rgb([v, v, v])
        });
        let out = contrast_stretched(&img);
        let dark = out.get_pixel(0, 0)[0];
        let light = out.get_pixel(63, 0)[0];
        assert!(dark < 30, "dark cluster should map near 0, got {dark}");
        assert!(
            light > 225,
            "light cluster should map near 255, got {light}"
        );
    }

    #[test]
    fn binarized_is_pure_black_and_white() {
        let img = RgbImage::from_fn(32, 32, |x, _| {
            let v = if x < 16 { 60 } else { 200 };
            Rgb([v, v, v])
        });
        for p in binarized(&img).pixels() {
            assert!(p[0] == 0 || p[0] == 255);
        }
    }

    #[test]
    fn variants_are_nonempty_and_ordered_cheapest_first() {
        let v = mrz_variants(&solid(600, 400, 200));
        assert_eq!(v.len(), 3);
        // Band crops (45% height, upscaled) come before the full-page pass.
        assert!(v[0].height() < v[2].height() * 3 / 4);
    }
}
