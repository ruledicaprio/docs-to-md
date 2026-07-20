//! Same seed -> identical identity and byte-identical rendered image.

use synthpass_gen::{data::generate_passport, generate, GeneratorConfig};

#[test]
fn same_seed_yields_identical_identity_and_pixels() {
    let cfg = GeneratorConfig::new(1234);

    let passport_a = generate_passport(&cfg);
    let passport_b = generate_passport(&cfg);
    assert_eq!(passport_a, passport_b, "identity must be deterministic");

    let (image_a, labels_a) = generate(&passport_a, &cfg);
    let (image_b, labels_b) = generate(&passport_b, &cfg);

    assert_eq!(labels_a, labels_b, "labels must be deterministic");
    assert_eq!(
        image_a.to_rgb8().into_raw(),
        image_b.to_rgb8().into_raw(),
        "rendered pixels must be byte-identical for the same seed"
    );
}

#[test]
fn different_seeds_yield_different_pixels() {
    let a = GeneratorConfig::new(1);
    let b = GeneratorConfig::new(2);
    let pa = generate_passport(&a);
    let pb = generate_passport(&b);
    let (image_a, _) = generate(&pa, &a);
    let (image_b, _) = generate(&pb, &b);
    assert_ne!(image_a.to_rgb8().into_raw(), image_b.to_rgb8().into_raw());
}
