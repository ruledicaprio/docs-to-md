//! The synthetic watermark must render even with `embedded-fonts` off (the
//! default) — it is a mandatory ethics guardrail, not a font-dependent
//! nicety.

use synthpass_gen::layout::WATERMARK;
use synthpass_gen::{data::generate_passport, generate, GeneratorConfig};

#[test]
fn watermark_pixels_present_without_embedded_fonts() {
    let cfg = GeneratorConfig::new(77);
    let passport = generate_passport(&cfg);
    let (image, _labels) = generate(&passport, &cfg);
    let rgb = image.to_rgb8();

    // The background fill color used by `render` for an untouched pixel.
    const BACKGROUND: [u8; 3] = [244, 243, 236];

    let mut non_background_pixels = 0usize;
    for y in WATERMARK.y..(WATERMARK.y + WATERMARK.height) {
        for x in WATERMARK.x..(WATERMARK.x + WATERMARK.width) {
            if rgb.get_pixel(x, y).0 != BACKGROUND {
                non_background_pixels += 1;
            }
        }
    }

    assert!(
        non_background_pixels > 0,
        "expected the watermark band to contain non-background pixels"
    );
}
