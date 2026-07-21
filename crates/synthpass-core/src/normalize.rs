//! Deterministic field normalizers (M5 §8).
//!
//! Tier-2 (LLM) extractions read the *visual* zone of a document, which is
//! free-form printed text — not the constrained MRZ dialect Tier 1 produces.
//! A parity miss like `"CROATIA"` vs `HRV`, or `"JAAK-KRISTJAN"` vs
//! `JAAK KRISTJAN`, is not the LLM failing to *understand* the document; the
//! model read the field correctly and simply reproduced the visual zone's
//! own formatting. These are normalization failures, fixable deterministically
//! with no model and no new dependency — estimated worth +5–7 accuracy
//! points against the Tier-1/Tier-2 parity suite.
//!
//! Every function here is **pure and idempotent**: same input always gives
//! the same output, and normalizing an already-normalized value is a no-op
//! (`f(f(x)) == f(x)`). None of them invent data — input this module can't
//! confidently resolve passes through unchanged rather than guessing.
//!
//! **Not wired into the pipeline.** `synthpass-pipeline` decides whether and
//! where to call these; wiring them into the extraction flow is a
//! `synthpass-pipeline` change, deliberately left for that integration step
//! (see this module's originating task).

/// Normalize an `issuing_country`/`nationality` field: a full country name
/// (any case) resolves to its 3-letter ICAO/ISO 3166-1 code via
/// [`mrz::code_for_name`] — the *same* table [`mrz::country_name`] already
/// uses, so this can never drift from what the rest of the workspace
/// considers a valid code. A string that already looks like a valid code (3
/// ASCII letters, or the legacy single-letter `"D"`) passes through
/// unchanged except for uppercasing. Anything unresolved passes through
/// unchanged, verbatim.
pub fn country_code(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return trimmed.to_string();
    }
    if looks_like_a_code(trimmed) {
        return trimmed.to_uppercase();
    }
    match mrz::code_for_name(trimmed) {
        Some(code) => code.to_string(),
        None => trimmed.to_string(),
    }
}

fn looks_like_a_code(s: &str) -> bool {
    (s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic())) || s.eq_ignore_ascii_case("D")
}

/// Normalize `Extraction::issuing_country`. See [`country_code`].
pub fn issuing_country(input: &str) -> String {
    country_code(input)
}

/// Normalize `Extraction::nationality`. See [`country_code`].
pub fn nationality(input: &str) -> String {
    country_code(input)
}

/// Normalize `Extraction::given_names` to MRZ convention: `<` (a leaked MRZ
/// filler, when a Tier-2 read pulls formatting from the machine-readable
/// zone instead of the visual zone) and `-` (a hyphenated given name, e.g.
/// `"JAAK-KRISTJAN"`, which the MRZ encodes as a filler/space rather than a
/// hyphen — ICAO 9303 part 3 §4.6.1) both become a single space; runs of
/// whitespace collapse to one; the result is uppercased to match every other
/// MRZ-sourced name field in this schema.
pub fn given_names(input: &str) -> String {
    let spaced: String = input
        .chars()
        .map(|c| if c == '<' || c == '-' { ' ' } else { c })
        .collect();
    spaced
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// Plausible calendar-year bounds a normalized date must fall within to be
/// accepted — mirrors `v2::looks_like_a_date`'s own range (see that
/// function's doc comment in `v2.rs`) so this module's judgement of
/// "plausible enough to normalize" agrees with the confidence scorer that
/// already ships in this crate, rather than inventing a second opinion.
const PLAUSIBLE_YEAR_RANGE: std::ops::RangeInclusive<u32> = 1900..=2999;

/// Normalize a date field to the strict ISO `YYYY-MM-DD` form
/// `mrz::dates::parse_iso` requires (4-digit year, zero-padded 2-digit month
/// and day, `-`-separated, exactly 10 characters) — read that parser first;
/// this targets exactly its dialect rather than inventing a new one, since
/// it is what every date-plausibility check in this workspace (`Validity`,
/// `MrzData::validity`) is ultimately built on.
///
/// Recognizes:
/// - already-correct or loosely-padded ISO order (`"2014-7-1"` →
///   `"2014-07-01"`);
/// - day-first `DD.MM.YYYY` / `DD/MM/YYYY` / `DD-MM-YYYY` — the common
///   non-US format a Tier-2 read of a visual zone tends to reproduce;
/// - unseparated `YYYYMMDD` (8 digits, the raw shape once an MRZ `YYMMDD`
///   has been century-expanded).
///
/// Genuinely ambiguous input (e.g. `MM/DD/YYYY`-shaped, where neither
/// outer component is a 4-digit year) passes through unchanged rather than
/// guessing which of two readings is right.
pub fn date(input: &str) -> String {
    let trimmed = input.trim();
    match parse_date_parts(trimmed) {
        Some((y, m, d)) => format!("{y:04}-{m:02}-{d:02}"),
        None => trimmed.to_string(),
    }
}

fn parse_date_parts(s: &str) -> Option<(u32, u32, u32)> {
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        let (y, m, d) = (
            s[0..4].parse().ok()?,
            s[4..6].parse().ok()?,
            s[6..8].parse().ok()?,
        );
        return valid_date(y, m, d).then_some((y, m, d));
    }
    let parts: Vec<&str> = s.split(['-', '.', '/']).map(str::trim).collect();
    let [a, b, c] = parts[..] else {
        return None;
    };
    let (y, m, d) = if a.len() == 4 {
        // ISO order: YYYY-MM-DD.
        (a.parse().ok()?, b.parse().ok()?, c.parse().ok()?)
    } else if c.len() == 4 {
        // Day-first order: DD-MM-YYYY.
        (c.parse().ok()?, b.parse().ok()?, a.parse().ok()?)
    } else {
        return None; // ambiguous (neither end names a 4-digit year) — don't guess
    };
    valid_date(y, m, d).then_some((y, m, d))
}

fn valid_date(y: u32, m: u32, d: u32) -> bool {
    PLAUSIBLE_YEAR_RANGE.contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d)
}

/// Normalize `Extraction::sex` to the single-letter ICAO 9303 code
/// (`M`/`F`/`X`): long forms map to their code, anything already a single
/// letter is uppercased and passed through unchanged (so an already-correct
/// `M`/`F` — or any other single letter a document might print — survives
/// untouched, rather than this function silently coercing every unknown
/// value to `X`).
pub fn sex(input: &str) -> String {
    let upper = input.trim().to_uppercase();
    match upper.as_str() {
        "MALE" => "M".to_string(),
        "FEMALE" => "F".to_string(),
        "UNSPECIFIED" | "UNKNOWN" | "OTHER" => "X".to_string(),
        _ => upper,
    }
}

/// Normalize `Extraction::document_type` to the ICAO 9303 single-letter
/// document code (`P` passport, `I` ID card, `V` visa): long forms map to
/// their code; anything else is uppercased and passed through unchanged.
pub fn document_type(input: &str) -> String {
    let upper = input.trim().to_uppercase();
    match upper.as_str() {
        "PASSPORT" => "P".to_string(),
        "IDENTITY CARD" | "ID CARD" | "NATIONAL IDENTITY CARD" | "NATIONAL ID" => "I".to_string(),
        "VISA" => "V".to_string(),
        _ => upper,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── country_code / issuing_country / nationality ──

    #[test]
    fn country_code_maps_full_names_case_insensitively() {
        let cases = [
            ("CROATIA", "HRV"),
            ("Croatia", "HRV"),
            ("croatia", "HRV"),
            ("Slovenia", "SVN"),
            ("United States of America", "USA"),
        ];
        for (input, expected) in cases {
            assert_eq!(country_code(input), expected, "input: {input:?}");
        }
    }

    #[test]
    fn country_code_passes_through_valid_codes_unchanged_except_case() {
        assert_eq!(country_code("HRV"), "HRV");
        assert_eq!(country_code("hrv"), "HRV");
        assert_eq!(country_code("D"), "D");
    }

    #[test]
    fn country_code_passes_through_unrecognized_input() {
        assert_eq!(country_code("Narnia"), "Narnia");
        assert_eq!(country_code(""), "");
    }

    #[test]
    fn country_code_is_idempotent() {
        for input in ["CROATIA", "HRV", "hrv", "Narnia", ""] {
            let once = country_code(input);
            let twice = country_code(&once);
            assert_eq!(once, twice, "not idempotent for input: {input:?}");
        }
    }

    #[test]
    fn issuing_country_and_nationality_are_the_same_normalizer() {
        assert_eq!(issuing_country("Croatia"), nationality("Croatia"));
    }

    // ── given_names ──

    #[test]
    fn given_names_converts_hyphen_to_space_matching_mrz_convention() {
        assert_eq!(given_names("JAAK-KRISTJAN"), "JAAK KRISTJAN");
    }

    #[test]
    fn given_names_converts_mrz_filler_to_space() {
        assert_eq!(given_names("ANNA<MARIA"), "ANNA MARIA");
    }

    #[test]
    fn given_names_collapses_whitespace_and_uppercases() {
        assert_eq!(given_names("  anna   maria  "), "ANNA MARIA");
    }

    #[test]
    fn given_names_is_idempotent() {
        for input in ["JAAK-KRISTJAN", "ANNA<MARIA", "  anna   maria  ", "SINGLE"] {
            let once = given_names(input);
            let twice = given_names(&once);
            assert_eq!(once, twice, "not idempotent for input: {input:?}");
        }
    }

    // ── date ──

    #[test]
    fn date_pads_a_loosely_formatted_iso_date() {
        assert_eq!(date("2014-7-1"), "2014-07-01");
    }

    #[test]
    fn date_passes_through_a_well_formed_iso_date() {
        assert_eq!(date("1974-08-12"), "1974-08-12");
    }

    #[test]
    fn date_converts_day_first_formats() {
        assert_eq!(date("12.08.1974"), "1974-08-12");
        assert_eq!(date("12/08/1974"), "1974-08-12");
        assert_eq!(date("12-08-1974"), "1974-08-12");
    }

    #[test]
    fn date_converts_unseparated_yyyymmdd() {
        assert_eq!(date("19740812"), "1974-08-12");
    }

    #[test]
    fn date_passes_through_ambiguous_and_garbage_input() {
        // Neither end names a 4-digit year: genuinely ambiguous, don't guess.
        assert_eq!(date("01/02/03"), "01/02/03");
        assert_eq!(date("not a date"), "not a date");
        assert_eq!(date(""), "");
    }

    #[test]
    fn date_rejects_out_of_range_components_without_guessing() {
        assert_eq!(date("1974-13-40"), "1974-13-40");
    }

    #[test]
    fn date_is_idempotent() {
        for input in ["2014-7-1", "12.08.1974", "19740812", "not a date", ""] {
            let once = date(input);
            let twice = date(&once);
            assert_eq!(once, twice, "not idempotent for input: {input:?}");
        }
    }

    // ── sex ──

    #[test]
    fn sex_maps_long_forms() {
        assert_eq!(sex("MALE"), "M");
        assert_eq!(sex("Female"), "F");
        assert_eq!(sex("unspecified"), "X");
    }

    #[test]
    fn sex_passes_through_single_letters_uppercased() {
        assert_eq!(sex("m"), "M");
        assert_eq!(sex("F"), "F");
    }

    #[test]
    fn sex_is_idempotent() {
        for input in ["MALE", "female", "m", "X", ""] {
            let once = sex(input);
            let twice = sex(&once);
            assert_eq!(once, twice, "not idempotent for input: {input:?}");
        }
    }

    // ── document_type ──

    #[test]
    fn document_type_maps_long_forms() {
        assert_eq!(document_type("PASSPORT"), "P");
        assert_eq!(document_type("Identity Card"), "I");
        assert_eq!(document_type("national id"), "I");
        assert_eq!(document_type("visa"), "V");
    }

    #[test]
    fn document_type_passes_through_short_codes_uppercased() {
        assert_eq!(document_type("p"), "P");
        assert_eq!(document_type("IR"), "IR");
    }

    #[test]
    fn document_type_is_idempotent() {
        for input in ["PASSPORT", "identity card", "p", "IR", ""] {
            let once = document_type(input);
            let twice = document_type(&once);
            assert_eq!(once, twice, "not idempotent for input: {input:?}");
        }
    }
}
