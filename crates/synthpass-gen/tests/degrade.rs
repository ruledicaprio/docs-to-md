//! Same seed + same image + same recipe -> byte-identical degraded pixels;
//! different seeds jitter; every [`CaptureProfile`] visibly changes the
//! pristine render.

use image::GenericImageView;
use synthpass_gen::degrade::{apply, apply_profile, CaptureProfile, Degradation};
use synthpass_gen::{generate_from_seed, GeneratorConfig};

fn sample_image() -> image::DynamicImage {
    let cfg = GeneratorConfig::new(99);
    let (image, _labels, _passport) = generate_from_seed(&cfg);
    image
}

#[test]
fn same_seed_same_image_same_recipe_is_byte_identical() {
    let image = sample_image();
    let recipe = synthpass_gen::degrade::profile_recipe(CaptureProfile::Mobile);

    let out_a = apply(&image, &recipe, 42);
    let out_b = apply(&image, &recipe, 42);

    assert_eq!(out_a.to_rgb8().into_raw(), out_b.to_rgb8().into_raw());
}

#[test]
fn different_seeds_jitter_pixels() {
    let image = sample_image();
    let recipe = synthpass_gen::degrade::profile_recipe(CaptureProfile::Mobile);

    let out_a = apply(&image, &recipe, 1);
    let out_b = apply(&image, &recipe, 2);
    assert_ne!(out_a.to_rgb8().into_raw(), out_b.to_rgb8().into_raw());

    let recipe_kiosk = synthpass_gen::degrade::profile_recipe(CaptureProfile::BorderKiosk);
    let out_c = apply(&image, &recipe_kiosk, 10);
    let out_d = apply(&image, &recipe_kiosk, 20);
    assert_ne!(out_c.to_rgb8().into_raw(), out_d.to_rgb8().into_raw());
}

#[test]
fn every_profile_visibly_changes_the_pristine_render() {
    let image = sample_image();
    let pristine = image.to_rgb8().into_raw();

    for profile in [
        CaptureProfile::Mobile,
        CaptureProfile::Scanner,
        CaptureProfile::Worn,
        CaptureProfile::BorderKiosk,
    ] {
        let degraded = apply_profile(&image, profile, 7);
        assert_ne!(
            pristine,
            degraded.to_rgb8().into_raw(),
            "{profile:?} was a no-op"
        );
        assert_eq!(image.dimensions(), degraded.dimensions());
    }
}

#[test]
fn empty_degradation_list_is_a_no_op() {
    let image = sample_image();
    let out = apply(&image, &[], 5);
    assert_eq!(image.to_rgb8().into_raw(), out.to_rgb8().into_raw());
}

#[test]
fn single_degradation_applies_in_isolation() {
    let image = sample_image();
    let out = apply(&image, &[Degradation::Rotate { degrees: 3.0 }], 3);
    assert_eq!(image.dimensions(), out.dimensions());
    assert_ne!(image.to_rgb8().into_raw(), out.to_rgb8().into_raw());
}
