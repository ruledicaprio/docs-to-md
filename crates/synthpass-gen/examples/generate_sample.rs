//! Render a few synthetic TD3 passport data pages from fixed seeds and write
//! each as a PNG next to a text dump of its ground-truth labels.
//!
//! Run: `cargo run -p synthpass-gen --example generate_sample -- <out_dir>`
//! (defaults to the current directory). Without the `embedded-fonts` feature
//! the text fields render as placeholder bars, but the layout, the checksum-
//! valid MRZ, the labels, and the mandatory synthetic watermark are all real.

use synthpass_gen::{generate_from_seed, GeneratorConfig};

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".to_string());

    for seed in [1u64, 7, 42] {
        let cfg = GeneratorConfig::new(seed);
        let (image, labels, passport) = generate_from_seed(&cfg);

        let png = format!("{out_dir}/synthpass_seed{seed}.png");
        image.save(&png).expect("write png");

        println!("seed {seed}: {} {} ({}), doc {}",
            passport.given_names, passport.surname, passport.nationality, passport.document_number);
        println!("  MRZ:\n    {}", labels.mrz_string().replace('\n', "\n    "));
        println!("  -> {png} ({}x{})\n", image.width(), image.height());
    }
}
