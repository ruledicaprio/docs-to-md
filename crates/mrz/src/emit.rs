//! TD3 (passport) MRZ emission — the deterministic inverse of [`crate::parse_td3`].
//!
//! Given structured field values, [`format_td3`] produces the two 44-character
//! ICAO 9303 Part 4 lines with every check digit (document number, date of
//! birth, date of expiry, personal number, composite) computed via the same
//! [`crate::check_digit`] math the parser verifies against. Feeding the output
//! back through [`crate::parse_td3`] always yields a record with
//! `valid() == true`.
//!
//! Field widths and offsets mirror `parser::parse_td3` exactly:
//! - Line 1: document code (2) + issuing country (3) + name field (39).
//! - Line 2: document number (9) + check (1) + nationality (3) + date of
//!   birth (6) + check (1) + sex (1) + date of expiry (6) + check (1) +
//!   personal number (14) + check (1) + composite check (1).
//!
//! **Scope**: TD3 only. TD2 and TD1 emission are out of scope for this
//! milestone (M1) — only [`crate::parse_td2`] / [`crate::parse_td1`] exist for
//! those formats today.
//!
//! Input fields are taken in MRZ-native form (`YYMMDD` dates, uppercase
//! `[A-Z0-9]`) rather than the parser's output form (ISO dates, spaced given
//! names) — this keeps emission lossless and deterministic instead of forcing
//! a fragile un-parse of the parser's normalized/century-inferred output.
//! Any character outside `[A-Z0-9]` (including spaces between given names) is
//! mapped to the filler `<`, and every fixed-width field is padded with `<`
//! or truncated to its exact width — this function never panics and always
//! returns exactly 44+1+44 = 89 characters.

use crate::checksum::check_digit;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Raw TD3 sub-fields, in MRZ-native form (see module docs).
///
/// `document_number` and `personal_number` are limited to 9 and 14 characters
/// respectively (the field widths); longer input is silently truncated.
/// `date_of_birth` / `date_of_expiry` are `YYMMDD`, not ISO dates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Td3Fields {
    /// Document code, e.g. `"P"` for passport. Defaults to `"P"`.
    pub document_code: String,
    /// Issuing state (3-letter ICAO code).
    pub issuing_country: String,
    /// Document number, up to 9 characters.
    pub document_number: String,
    pub surname: String,
    pub given_names: String,
    /// Nationality (3-letter ICAO code).
    pub nationality: String,
    /// Date of birth as `YYMMDD`.
    pub date_of_birth: String,
    /// `"M"`, `"F"`, or `"X"` (unspecified — emitted as the filler `<`).
    pub sex: String,
    /// Date of expiry as `YYMMDD`.
    pub date_of_expiry: String,
    /// Optional personal number, up to 14 characters.
    pub personal_number: Option<String>,
}

impl Default for Td3Fields {
    fn default() -> Self {
        Self {
            document_code: "P".to_string(),
            issuing_country: String::new(),
            document_number: String::new(),
            surname: String::new(),
            given_names: String::new(),
            nationality: String::new(),
            date_of_birth: String::new(),
            sex: String::new(),
            date_of_expiry: String::new(),
            personal_number: None,
        }
    }
}

/// Map any character outside `[A-Z0-9]` (after uppercasing) to the filler `<`.
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| {
            let u = c.to_ascii_uppercase();
            if u.is_ascii_alphanumeric() {
                u
            } else {
                '<'
            }
        })
        .collect()
}

/// Clean and pad/truncate `s` to exactly `width` characters using `<` filler.
fn field(s: &str, width: usize) -> String {
    let cleaned = clean(s);
    let mut chars = cleaned.chars();
    let mut out = String::with_capacity(width);
    for _ in 0..width {
        out.push(chars.next().unwrap_or('<'));
    }
    out
}

/// Build the 39-char name field: `SURNAME<<GIVEN<NAMES`, filler-padded.
fn name_field(surname: &str, given_names: &str) -> String {
    let combined = format!("{}<<{}", clean(surname), clean(given_names));
    field(&combined, 39)
}

fn digit_char(field: &str) -> char {
    // `field` is always built from `clean`/`field`, i.e. only `[A-Z0-9<]`,
    // so `check_digit` can never fail here.
    char::from_digit(check_digit(field).expect("MRZ-charset field"), 10).expect("0-9")
}

/// Emit a TD3 (passport) MRZ: two 44-character lines joined by `\n`.
///
/// All four field check digits and the composite check digit are computed
/// from `fields` — the result always round-trips through
/// [`crate::parse_td3`] with `valid() == true` (see `tests/roundtrip.rs`).
pub fn format_td3(fields: &Td3Fields) -> String {
    let doc_code = field(&fields.document_code, 2);
    let issuing = field(&fields.issuing_country, 3);
    let name = name_field(&fields.surname, &fields.given_names);
    let line1 = format!("{doc_code}{issuing}{name}");

    let doc_num = field(&fields.document_number, 9);
    let doc_num_check = digit_char(&doc_num);
    let nationality = field(&fields.nationality, 3);
    let dob = field(&fields.date_of_birth, 6);
    let dob_check = digit_char(&dob);
    let sex = match fields.sex.to_ascii_uppercase().as_str() {
        "M" => 'M',
        "F" => 'F',
        _ => '<',
    };
    let expiry = field(&fields.date_of_expiry, 6);
    let expiry_check = digit_char(&expiry);
    let personal = field(fields.personal_number.as_deref().unwrap_or(""), 14);
    let personal_check = digit_char(&personal);

    // 43 chars: everything except the composite digit itself, in the same
    // layout `parser::parse_td3` expects (offsets 0..43).
    let line2_body = format!(
        "{doc_num}{doc_num_check}{nationality}{dob}{dob_check}{sex}{expiry}{expiry_check}{personal}{personal_check}"
    );
    debug_assert_eq!(line2_body.len(), 43);

    // Composite: line2[0..10] (doc number + check) + line2[13..20]
    // (DOB + check) + line2[21..43] (expiry + check + personal + check) —
    // mirrors `parser::parse_td3`'s `checks.composite` computation exactly.
    let composite_input = format!(
        "{}{}{}",
        &line2_body[0..10],
        &line2_body[13..20],
        &line2_body[21..43]
    );
    let composite_check = digit_char(&composite_input);

    let line2 = format!("{line2_body}{composite_check}");
    debug_assert_eq!(line1.len(), 44);
    debug_assert_eq!(line2.len(), 44);

    format!("{line1}\n{line2}")
}
