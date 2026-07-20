//! Deterministic fictional-identity generation.
//!
//! Every draw comes from a `ChaCha8Rng` seeded with [`crate::GeneratorConfig::seed`],
//! so the same seed always produces byte-identical field values. Names are
//! drawn from small hand-authored pools that are clearly fictional (no real
//! public figures) — this crate must never emit real PII.

use mrz::Date;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::model::{GeneratorConfig, Passport, Sex};

/// Fictional given names, split by sex so [`Sex`] and name agree.
const GIVEN_NAMES_M: &[&str] = &[
    "JOHAN", "ALEKSANDER", "MATEO", "FELIX", "OMAR", "LUCA", "KENJI", "AMARE", "DMITRI", "SOREN",
    "TARIQ", "NIKOLAI", "PIETRO", "HANS", "ELIAS",
];
const GIVEN_NAMES_F: &[&str] = &[
    "ANIKA", "SOFIA", "MAREN", "YUKI", "ZARA", "ELENA", "NADIA", "ASTRID", "LEILANI", "PRIYA",
    "IRINA", "CAMILLE", "MILENA", "TOVA", "AMARA",
];

/// Fictional surnames (never a real family name of a public figure).
const SURNAMES: &[&str] = &[
    "VANTERPOOL", "OKONKWO", "BLACKWOOD", "NYSTROM", "CASTELLANO", "WHITFIELD", "MORAVEC",
    "HALVORSEN", "DUBOISARD", "KIRSCHNER", "ADEYEMI", "STRAND", "PELLETIER", "YAMAMORI",
    "ESKANDARI",
];

/// 3-letter ICAO/ISO 3166-1 codes used for issuing state / nationality. Real
/// country codes are not PII on their own; the ICAO specimen code `UTO` is
/// included deliberately as the most "obviously synthetic" option.
const COUNTRY_CODES: &[&str] = &[
    "UTO", "UTO", "USA", "GBR", "DEU", "FRA", "CAN", "AUS", "JPN", "BRA", "ZAF", "SWE",
];

/// Alphabet for document numbers: digits + letters, excluding the MRZ filler.
const ALNUM: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

fn pick<'a, T>(rng: &mut ChaCha8Rng, pool: &'a [T]) -> &'a T {
    &pool[rng.random_range(0..pool.len())]
}

fn random_document_number(rng: &mut ChaCha8Rng) -> String {
    // TD3 document numbers are up to 9 characters; use the full width.
    (0..9)
        .map(|_| ALNUM[rng.random_range(0..ALNUM.len())] as char)
        .collect()
}

fn random_personal_number(rng: &mut ChaCha8Rng) -> String {
    // TD3 personal-number field is up to 14 characters.
    (0..14)
        .map(|_| ALNUM[rng.random_range(0..ALNUM.len())] as char)
        .collect()
}

/// A calendar date is "well-formed enough" for our purposes if the day fits
/// the month generously (see `mrz::Date::is_well_formed`); we additionally
/// clamp to 28 to dodge Feb-in-leap-years bookkeeping entirely — the exact
/// calendar day is not load-bearing for synthetic identities.
fn random_date(rng: &mut ChaCha8Rng, year_lo: i32, year_hi: i32) -> Date {
    let year = rng.random_range(year_lo..=year_hi);
    let month = rng.random_range(1..=12);
    let day = rng.random_range(1..=28);
    Date::new(year, month, day)
}

/// Generate a fictional [`Passport`] deterministically from `config.seed`.
///
/// Birth years are drawn from 1950-2008 and expiry years from 2027-2036: both
/// ranges round-trip correctly through `mrz::expand_date`'s two-digit-year
/// century pivot (`CURRENT_YY = 26`), so the identity survives an MRZ
/// render -> parse round trip unchanged. Date of birth is always strictly
/// before date of expiry by construction.
pub fn generate_passport(config: &GeneratorConfig) -> Passport {
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);

    let is_male = rng.random_bool(0.5);
    let (sex, given_names) = if is_male {
        (Sex::M, pick(&mut rng, GIVEN_NAMES_M).to_string())
    } else {
        (Sex::F, pick(&mut rng, GIVEN_NAMES_F).to_string())
    };
    let surname = pick(&mut rng, SURNAMES).to_string();

    // Issuing state and nationality are drawn together, not independently: on
    // the overwhelming majority of real passports they're the same 3-letter
    // code (a citizen's passport issued by their own state). Drawing them
    // separately let the factory emit incoherent identities like nationality
    // AUS on a USA-issued document, which is confusing ground truth for a
    // benchmark to grade against.
    let country = pick(&mut rng, COUNTRY_CODES).to_string();
    let issuing_country = country.clone();
    let nationality = country;

    let date_of_birth = random_date(&mut rng, 1950, 2008);
    let date_of_expiry = random_date(&mut rng, 2027, 2036);
    debug_assert!(date_of_birth.to_epoch_days() < date_of_expiry.to_epoch_days());

    let document_number = random_document_number(&mut rng);
    let personal_number = if config.include_personal_number {
        Some(random_personal_number(&mut rng))
    } else {
        None
    };

    Passport {
        document_type: "P".to_string(),
        issuing_country,
        surname,
        given_names,
        document_number,
        nationality,
        date_of_birth,
        sex,
        date_of_expiry,
        personal_number,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_identity() {
        let cfg = GeneratorConfig::new(42);
        let a = generate_passport(&cfg);
        let b = generate_passport(&cfg);
        assert_eq!(a, b);
    }

    #[test]
    fn different_seeds_usually_differ() {
        let a = generate_passport(&GeneratorConfig::new(1));
        let b = generate_passport(&GeneratorConfig::new(2));
        assert_ne!(a, b);
    }

    #[test]
    fn dob_before_expiry_always() {
        for seed in 0..200u64 {
            let p = generate_passport(&GeneratorConfig::new(seed));
            assert!(p.date_of_birth.to_epoch_days() < p.date_of_expiry.to_epoch_days());
        }
    }
}
