//! `synthpass-gen` — a deterministic, pure-Rust synthetic TD3 passport
//! data-page generator.
//!
//! Given a seed and a small set of parameters, [`generate`] produces a
//! rendered document-style image plus perfectly accurate ground-truth labels
//! for every field, including a checksum-valid MRZ. This is **synthetic-data
//! infrastructure for testing and benchmarking identity-document extraction
//! pipelines** — it is not a tool for imitating genuine documents, and that
//! posture is enforced at the artifact level, not just in this doc comment:
//!
//! - **Unconditional watermark.** Every render carries a "SYNTHETIC /
//!   SPECIMEN" watermark, drawn from a hand-authored bitmap font baked into
//!   the binary (see [`render`]). It cannot be disabled through
//!   [`GeneratorConfig`] and does not depend on any font file being present.
//! - **Generic, non-country template.** The background is a plain frame with
//!   no national emblem, coat of arms, or issuing-country branding of any
//!   kind — regardless of which issuing-country *code* a given identity
//!   happens to carry in its MRZ/VIZ text.
//! - **No real PII, ever.** Identities are drawn deterministically from a
//!   seed out of small, hand-authored pools of clearly fictional names (see
//!   [`data`]) — never real people, never sourced from real documents.
//!
//! ## MRZ correctness
//!
//! The rendered MRZ is assembled locally in [`mrz_line`] by reusing
//! `mrz::check_digit` (the standalone `mrz` crate's checksum oracle) rather
//! than duplicating ICAO 9303 checksum math. The keystone test in
//! `tests/mrz_roundtrip.rs` parses generated output back through
//! `mrz::parse_td3` and asserts every check digit is valid.
//!
//! ## Determinism
//!
//! [`generate`] is a pure function of `(passport, config)`, and
//! [`data::generate_passport`] is a pure function of `config.seed`: the same
//! seed always produces byte-identical identity data and pixels. See
//! `tests/determinism.rs`.

pub mod data;
pub mod degrade;
pub mod fonts;
pub mod labels;
pub mod layout;
pub mod model;
mod mrz_line;
pub mod render;

pub use labels::{FieldLabel, Labels};
pub use model::{GeneratorConfig, Passport, Sex};

/// Generate a synthetic TD3 passport data page: a fictional identity drawn
/// from `config.seed`, rendered into an image, alongside its ground-truth
/// [`Labels`].
///
/// The two ethics guardrails described in the module docs (the synthetic
/// watermark and the generic template) render unconditionally as part of
/// this call — there is no configuration path that skips them.
pub fn generate(passport: &Passport, config: &GeneratorConfig) -> (image::DynamicImage, Labels) {
    let _ = config; // reserved for future render options; seed already consumed by data::generate_passport
    let labels = labels::build_labels(passport);
    let image = render::render(passport, &labels);
    (image, labels)
}

/// Convenience: generate a fictional passport from `config.seed` and render
/// it in one call.
pub fn generate_from_seed(config: &GeneratorConfig) -> (image::DynamicImage, Labels, Passport) {
    let passport = data::generate_passport(config);
    let (image, labels) = generate(&passport, config);
    (image, labels, passport)
}
