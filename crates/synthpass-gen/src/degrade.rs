//! Deterministic, composable image degradation.
//!
//! Real-world identity-document captures are never as clean as
//! [`crate::render::render`]'s pristine output: a phone photo has motion
//! blur and uneven lighting, a flatbed scanner has different noise and
//! perfect alignment, a worn/handled document has creases and faded ink, a
//! kiosk camera has slight skew and flash glare. This module turns a
//! pristine render into a plausibly-degraded one, so extraction pipelines
//! can be benchmarked against something closer to a real capture instead of
//! only ever the perfect render.
//!
//! The design is deliberately **composed from small, independently
//! toggleable steps** ([`Degradation`]) rather than one monolithic function
//! per [`CaptureProfile`]: a caller can inspect a profile's recipe
//! ([`profile_recipe`]), drop/reorder/replace a step, and re-apply
//! ([`apply`]) without touching this module.
//!
//! ## Determinism
//!
//! [`apply`] seeds a single `ChaCha8Rng` from its `seed` argument and
//! threads it through every step in order, so the same `(image, recipe,
//! seed)` always produces byte-identical output pixels, and a different
//! seed produces different noise/jitter within each step's declared
//! parameters. See `tests/degrade.rs`.
//!
//! ## Limitations
//!
//! These passes **simulate** degradation for benchmarking purposes; they do
//! not model any specific real capture device's exact noise transfer
//! function, lens distortion, JPEG DCT quantization tables, or paper/ink
//! physics. "Looks meaningfully different and reproducible" is the bar, not
//! photorealism.

use image::{DynamicImage, Rgb, RgbImage};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// A named, real-world capture scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureProfile {
    /// Handheld phone photo: motion blur, uneven lighting/vignette, moderate JPEG-like noise.
    Mobile,
    /// Flatbed scanner: perfect alignment, mild uniform noise, slight sharpening halo.
    Scanner,
    /// Handled/aged document: creases (localized contrast/line artifacts), scuffs, faded ink (reduced contrast).
    Worn,
    /// Border-control kiosk camera: slight skew/rotation, harsh flash glare (localized brightness blowout), moderate compression blockiness.
    BorderKiosk,
}

/// One toggleable degradation primitive, so profiles are composed from parts
/// rather than being monolithic special cases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Degradation {
    GaussianBlur {
        sigma: f32,
    },
    /// `amount` in `0.0..=1.0`; per-pixel, per-channel additive noise.
    Noise {
        amount: f32,
    },
    /// Small angles, +/- a few degrees. Rotates about the image center;
    /// pixels that would fall outside the original canvas are filled by
    /// clamping to the nearest edge pixel, so output dimensions never
    /// change.
    Rotate {
        degrees: f32,
    },
    /// `<1.0` fades (reduced contrast, as with faded ink), `>1.0` boosts.
    Contrast {
        factor: f32,
    },
    /// A radial brightness effect centered on the image. `is_glare = false`
    /// darkens the corners (vignette); `is_glare = true` blows out a
    /// localized bright spot (flash glare). `strength` in `0.0..=1.0`.
    VignetteOrGlare {
        strength: f32,
        is_glare: bool,
    },
    /// Draws `count` localized fold-line artifacts (a thin shadow/highlight
    /// pair along a random line), simulating a handled/creased document.
    Crease {
        count: u32,
    },
    /// Simulates JPEG-style compression blockiness: averages pixels within
    /// 8x8 blocks (proportionally to `strength`, in `0.0..=1.0`) and
    /// accentuates the resulting block boundaries.
    JpegBlockiness {
        strength: f32,
    },
}

/// The ordered list of [`Degradation`]s a [`CaptureProfile`] applies, so a
/// caller can inspect/toggle/reorder individual steps instead of treating a
/// profile as a black box.
pub fn profile_recipe(profile: CaptureProfile) -> Vec<Degradation> {
    match profile {
        CaptureProfile::Mobile => vec![
            Degradation::GaussianBlur { sigma: 1.3 },
            Degradation::VignetteOrGlare {
                strength: 0.35,
                is_glare: false,
            },
            Degradation::Noise { amount: 0.15 },
        ],
        CaptureProfile::Scanner => vec![
            Degradation::Noise { amount: 0.05 },
            Degradation::Contrast { factor: 1.1 },
        ],
        CaptureProfile::Worn => vec![
            Degradation::Crease { count: 3 },
            Degradation::Noise { amount: 0.1 },
            Degradation::Contrast { factor: 0.75 },
        ],
        CaptureProfile::BorderKiosk => vec![
            Degradation::Rotate { degrees: 2.5 },
            Degradation::VignetteOrGlare {
                strength: 0.6,
                is_glare: true,
            },
            Degradation::JpegBlockiness { strength: 0.5 },
        ],
    }
}

/// Apply `degradations` to `image` in order, deterministically seeded by
/// `seed` (same seed + same image + same recipe => byte-identical output;
/// different seeds => different noise/blur/rotation jitter within each
/// degradation's parameters).
pub fn apply(image: &DynamicImage, degradations: &[Degradation], seed: u64) -> DynamicImage {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut buf = image.to_rgb8();
    for degradation in degradations {
        buf = apply_one(&buf, *degradation, &mut rng);
    }
    DynamicImage::ImageRgb8(buf)
}

/// Convenience: apply a profile's full recipe.
pub fn apply_profile(image: &DynamicImage, profile: CaptureProfile, seed: u64) -> DynamicImage {
    apply(image, &profile_recipe(profile), seed)
}

fn apply_one(img: &RgbImage, degradation: Degradation, rng: &mut ChaCha8Rng) -> RgbImage {
    match degradation {
        Degradation::GaussianBlur { sigma } => gaussian_blur(img, sigma),
        Degradation::Noise { amount } => noise(img, amount, rng),
        Degradation::Rotate { degrees } => rotate(img, degrees),
        Degradation::Contrast { factor } => contrast(img, factor),
        Degradation::VignetteOrGlare { strength, is_glare } => {
            vignette_or_glare(img, strength, is_glare, rng)
        }
        Degradation::Crease { count } => crease(img, count, rng),
        Degradation::JpegBlockiness { strength } => jpeg_blockiness(img, strength),
    }
}

fn gaussian_blur(img: &RgbImage, sigma: f32) -> RgbImage {
    if sigma <= 0.0 {
        return img.clone();
    }
    image::imageops::blur(img, sigma)
}

fn noise(img: &RgbImage, amount: f32, rng: &mut ChaCha8Rng) -> RgbImage {
    let amount = amount.clamp(0.0, 1.0);
    let max_delta = amount * 60.0; // amount=1.0 -> +/-60 per channel, a visibly noisy image
    let mut out = img.clone();
    for pixel in out.pixels_mut() {
        for channel in pixel.0.iter_mut() {
            let delta = rng.random_range(-max_delta..=max_delta);
            *channel = (*channel as f32 + delta).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn rotate(img: &RgbImage, degrees: f32) -> RgbImage {
    if degrees == 0.0 {
        return img.clone();
    }
    let (width, height) = img.dimensions();
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let theta = -degrees.to_radians(); // rotate the sampled source by -degrees
    let (sin_t, cos_t) = theta.sin_cos();

    let mut out = RgbImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let src_x = cos_t * dx - sin_t * dy + cx;
            let src_y = sin_t * dx + cos_t * dy + cy;
            let sx = src_x.round().clamp(0.0, (width - 1) as f32) as u32;
            let sy = src_y.round().clamp(0.0, (height - 1) as f32) as u32;
            out.put_pixel(x, y, *img.get_pixel(sx, sy));
        }
    }
    out
}

fn contrast(img: &RgbImage, factor: f32) -> RgbImage {
    let mut out = img.clone();
    for pixel in out.pixels_mut() {
        for channel in pixel.0.iter_mut() {
            let v = (*channel as f32 - 128.0) * factor + 128.0;
            *channel = v.clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn vignette_or_glare(
    img: &RgbImage,
    strength: f32,
    is_glare: bool,
    rng: &mut ChaCha8Rng,
) -> RgbImage {
    let strength = strength.clamp(0.0, 1.0);
    let (width, height) = img.dimensions();
    let mut out = img.clone();

    // Vignette is centered on the image; glare is a localized off-center
    // hotspot (jittered by `rng` so different seeds glare in slightly
    // different places), matching a handheld flash rather than a lens
    // vignette.
    let (center_x, center_y, max_dist) = if is_glare {
        let cx = width as f32 * rng.random_range(0.3..=0.7);
        let cy = height as f32 * rng.random_range(0.3..=0.7);
        (cx, cy, (width.min(height) as f32) * 0.35)
    } else {
        (width as f32 / 2.0, height as f32 / 2.0, {
            let dx = width as f32 / 2.0;
            let dy = height as f32 / 2.0;
            (dx * dx + dy * dy).sqrt()
        })
    };

    for (x, y, pixel) in out.enumerate_pixels_mut() {
        let dx = x as f32 - center_x;
        let dy = y as f32 - center_y;
        let dist = (dx * dx + dy * dy).sqrt();
        let t = (dist / max_dist).clamp(0.0, 1.0);
        let factor = if is_glare {
            // Bright at the center, fading to no effect at the edge of the
            // hotspot radius: blow out toward white.
            1.0 - t
        } else {
            // Dark at the corners, no effect near the center.
            -t
        };
        let adjustment = factor * strength * 255.0;
        for channel in pixel.0.iter_mut() {
            *channel = (*channel as f32 + adjustment).clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn crease(img: &RgbImage, count: u32, rng: &mut ChaCha8Rng) -> RgbImage {
    let (width, height) = img.dimensions();
    let mut out = img.clone();

    for _ in 0..count {
        // A random line through the image, defined by a point and an angle.
        let px = rng.random_range(0.0..width as f32);
        let py = rng.random_range(0.0..height as f32);
        let angle: f32 = rng.random_range(0.0..std::f32::consts::PI);
        let (sin_a, cos_a) = angle.sin_cos();
        // Line direction is (cos_a, sin_a); the normal is (-sin_a, cos_a).
        let band_half_width = 2.5_f32;

        for (x, y, pixel) in out.enumerate_pixels_mut() {
            let dx = x as f32 - px;
            let dy = y as f32 - py;
            // Signed perpendicular distance from the crease line.
            let perp = dx * (-sin_a) + dy * cos_a;
            if perp.abs() <= band_half_width {
                // A shadow on one side of the fold, a thin highlight on the
                // other: the classic look of a crease in paper/laminate.
                let shade = if perp < 0.0 { -28.0 } else { 18.0 };
                let falloff = 1.0 - (perp.abs() / band_half_width);
                let delta = shade * falloff;
                for channel in pixel.0.iter_mut() {
                    *channel = (*channel as f32 + delta).clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    out
}

fn jpeg_blockiness(img: &RgbImage, strength: f32) -> RgbImage {
    let strength = strength.clamp(0.0, 1.0);
    const BLOCK: u32 = 8;
    let (width, height) = img.dimensions();
    let mut out = img.clone();

    let mut bx = 0;
    while bx < width {
        let mut by = 0;
        while by < height {
            let bw = BLOCK.min(width - bx);
            let bh = BLOCK.min(height - by);

            let mut sums = [0u64; 3];
            let mut n = 0u64;
            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y);
                    for (c, sum) in sums.iter_mut().enumerate() {
                        *sum += p.0[c] as u64;
                    }
                    n += 1;
                }
            }
            let avg = [
                (sums[0] / n.max(1)) as f32,
                (sums[1] / n.max(1)) as f32,
                (sums[2] / n.max(1)) as f32,
            ];

            for y in by..by + bh {
                for x in bx..bx + bw {
                    let p = img.get_pixel(x, y);
                    let mut blended = [0u8; 3];
                    for c in 0..3 {
                        let v = p.0[c] as f32 * (1.0 - strength) + avg[c] * strength;
                        blended[c] = v.clamp(0.0, 255.0) as u8;
                    }
                    out.put_pixel(x, y, Rgb(blended));
                }
            }

            by += BLOCK;
        }
        bx += BLOCK;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn checkerboard(width: u32, height: u32) -> RgbImage {
        RgbImage::from_fn(width, height, |x, y| {
            if (x / 4 + y / 4) % 2 == 0 {
                Rgb([230, 230, 230])
            } else {
                Rgb([30, 30, 30])
            }
        })
    }

    #[test]
    fn empty_recipe_is_a_no_op() {
        let img = DynamicImage::ImageRgb8(checkerboard(64, 64));
        let out = apply(&img, &[], 1);
        assert_eq!(img.to_rgb8().into_raw(), out.to_rgb8().into_raw());
    }

    #[test]
    fn contrast_changes_spread_but_not_dimensions() {
        let img = DynamicImage::ImageRgb8(checkerboard(64, 64));
        let out = apply(&img, &[Degradation::Contrast { factor: 0.5 }], 1);
        assert_eq!(img.dimensions(), out.dimensions());

        let in_pixels = img.to_rgb8().into_raw();
        let out_pixels = out.to_rgb8().into_raw();
        assert_ne!(in_pixels, out_pixels);

        // Contrast 0.5 pulls values toward 128, so the spread must shrink.
        let spread = |buf: &[u8]| {
            let max = *buf.iter().max().unwrap() as i32;
            let min = *buf.iter().min().unwrap() as i32;
            max - min
        };
        assert!(spread(&out_pixels) < spread(&in_pixels));
    }

    #[test]
    fn dimensions_unchanged_by_every_degradation() {
        let img = DynamicImage::ImageRgb8(checkerboard(80, 60));
        let all = [
            Degradation::GaussianBlur { sigma: 1.5 },
            Degradation::Noise { amount: 0.2 },
            Degradation::Rotate { degrees: 3.0 },
            Degradation::Contrast { factor: 1.3 },
            Degradation::VignetteOrGlare {
                strength: 0.4,
                is_glare: true,
            },
            Degradation::Crease { count: 2 },
            Degradation::JpegBlockiness { strength: 0.5 },
        ];
        for degradation in all {
            let out = apply(&img, &[degradation], 7);
            assert_eq!(
                img.dimensions(),
                out.dimensions(),
                "{degradation:?} changed dimensions"
            );
        }
    }
}
