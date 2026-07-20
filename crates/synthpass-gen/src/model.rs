//! The fictional identity model and generator configuration.

pub use mrz::Date;

/// Sex marker, as printed on a TD3 MRZ (position 20 of line 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sex {
    M,
    F,
    /// Unspecified — ICAO 9303 allows `X` but [`crate::data::generate_passport`]
    /// never produces it (kept for completeness / future use).
    X,
}

impl Sex {
    /// The single MRZ character for this value.
    pub fn as_mrz_char(self) -> char {
        match self {
            Sex::M => 'M',
            Sex::F => 'F',
            Sex::X => 'X',
        }
    }
}

/// A synthetic, fictional TD3 passport identity.
///
/// Every field describes an invented person drawn deterministically from a
/// seed (see [`crate::data`]) — never a real one. Dates are internally
/// consistent (date of birth strictly precedes date of expiry) by
/// construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passport {
    /// MRZ document code, e.g. `"P"` for a passport.
    pub document_type: String,
    /// 3-letter ICAO/ISO 3166-1 issuing-state code.
    pub issuing_country: String,
    pub surname: String,
    pub given_names: String,
    /// Document number, already trimmed of MRZ filler padding.
    pub document_number: String,
    /// 3-letter ICAO/ISO 3166-1 nationality code.
    pub nationality: String,
    pub date_of_birth: Date,
    pub sex: Sex,
    pub date_of_expiry: Date,
    /// TD3 personal-number field; `None` when left blank (an all-filler field
    /// is a legitimate, checksum-valid TD3 MRZ — see `mrz::parser`).
    pub personal_number: Option<String>,
}

/// Generation parameters: the seed plus render options.
///
/// The seed is the only thing that determines the generated identity and
/// pixels — see the determinism test in `tests/`. `include_personal_number`
/// is a render/content option, not a source of extra randomness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeneratorConfig {
    /// Seed for the deterministic identity generator (`ChaCha8Rng`).
    pub seed: u64,
    /// Whether to populate the TD3 personal-number field. When `false` the
    /// field is left blank (all filler), which is a valid MRZ state.
    pub include_personal_number: bool,
}

impl GeneratorConfig {
    /// A config with the personal-number field populated.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            include_personal_number: true,
        }
    }
}
