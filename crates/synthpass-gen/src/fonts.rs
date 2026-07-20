//! Glyph loading, gated behind the `embedded-fonts` Cargo feature (off by
//! default). When off, [`load_fonts`] returns [`FontError::NotEmbedded`] and
//! `render` degrades gracefully to placeholder bars — see `render.rs`.
//!
//! When the feature is on, the two OFL fonts are baked into the binary via
//! `include_bytes!`; see `fonts/README.md` for how to supply them.

use ab_glyph::FontArc;

#[cfg(feature = "embedded-fonts")]
static OCR_B_BYTES: &[u8] = include_bytes!("../fonts/ocr-b.ttf");
#[cfg(feature = "embedded-fonts")]
static SANS_BYTES: &[u8] = include_bytes!("../fonts/sans.ttf");

/// Loaded font pair used to render the VIZ text and the MRZ band.
pub struct Fonts {
    /// Monospaced OCR-B-style font for the MRZ band.
    pub mrz: FontArc,
    /// Proportional sans font for the human-readable VIZ fields.
    pub viz: FontArc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// The `embedded-fonts` feature is off, or the embedded font bytes failed
    /// to parse. Either way, no real glyphs are available this build.
    NotEmbedded,
}

impl core::fmt::Display for FontError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FontError::NotEmbedded => write!(
                f,
                "fonts not embedded (build with --features embedded-fonts, see fonts/README.md)"
            ),
        }
    }
}

impl std::error::Error for FontError {}

/// Attempt to load the MRZ + VIZ fonts. Only succeeds when built with
/// `--features embedded-fonts` and the font files were supplied at build time.
pub fn load_fonts() -> Result<Fonts, FontError> {
    #[cfg(feature = "embedded-fonts")]
    {
        let mrz = FontArc::try_from_slice(OCR_B_BYTES).map_err(|_| FontError::NotEmbedded)?;
        let viz = FontArc::try_from_slice(SANS_BYTES).map_err(|_| FontError::NotEmbedded)?;
        Ok(Fonts { mrz, viz })
    }
    #[cfg(not(feature = "embedded-fonts"))]
    {
        Err(FontError::NotEmbedded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_embedded_by_default() {
        // Default-feature build: no font files are vendored in the repo, so
        // this must degrade gracefully rather than panic or fail to compile.
        #[cfg(not(feature = "embedded-fonts"))]
        assert!(matches!(load_fonts(), Err(FontError::NotEmbedded)));
    }
}
