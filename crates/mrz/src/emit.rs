//! MRZ emission for all five ICAO 9303 formats — the deterministic inverse of
//! [`crate::parse_td3`], [`crate::parse_td2`], [`crate::parse_td1`],
//! [`crate::parse_mrv_a`], and [`crate::parse_mrv_b`].
//!
//! Given structured field values, [`format_td3`], [`format_td2`],
//! [`format_td1`], [`format_mrv_a`], and [`format_mrv_b`] produce the
//! ICAO-specified lines with every check digit computed via the same
//! [`crate::check_digit`] math the parsers verify against. Feeding the output
//! back through the matching `parse_*` function always yields a record with
//! `valid() == true`.
//!
//! Field widths and offsets mirror the parsers exactly:
//! - **TD3** (two 44-char lines): document code (2) + issuing country (3) +
//!   name field (39) on line 1; document number (9) + check (1) +
//!   nationality (3) + date of birth (6) + check (1) + sex (1) + date of
//!   expiry (6) + check (1) + personal number (14) + check (1) + composite
//!   check (1) on line 2.
//! - **TD2** (two 36-char lines): document code (2) + issuing country (3) +
//!   name field (31) on line 1; document number (9) + check (1) +
//!   nationality (3) + date of birth (6) + check (1) + sex (1) + date of
//!   expiry (6) + check (1) + optional data (7) + composite check (1) on
//!   line 2. TD2 has no separate check digit over the optional-data field.
//! - **TD1** (three 30-char lines): document code (2) + issuing country (3) +
//!   document number (9) + check (1) + optional data 1 (15) on line 1; date
//!   of birth (6) + check (1) + sex (1) + date of expiry (6) + check (1) +
//!   nationality (3) + optional data 2 (11) + composite check (1) on line 2;
//!   name field (30) on line 3. TD1 has no separate check digit over either
//!   optional-data field.
//! - **MRV-A** (two 44-char lines): document code (2, `"V"`) + issuing country
//!   (3) + name field (39) on line 1; document number (9) + check (1) +
//!   nationality (3) + date of birth (6) + check (1) + sex (1) + date of
//!   expiry (6) + check (1) + optional data (16) on line 2. No personal-number
//!   check digit and no composite check digit — MRVs don't have one.
//! - **MRV-B** (two 36-char lines): document code (2, `"V"`) + issuing country
//!   (3) + name field (31) on line 1; document number (9) + check (1) +
//!   nationality (3) + date of birth (6) + check (1) + sex (1) + date of
//!   expiry (6) + check (1) + optional data (8) on line 2. Same as MRV-A: no
//!   personal-number check digit, no composite check digit.
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

/// Build the name field: `SURNAME<<GIVEN<NAMES`, padded/truncated to `width`.
fn name_field(surname: &str, given_names: &str, width: usize) -> String {
    let combined = format!("{}<<{}", clean(surname), clean(given_names));
    field(&combined, width)
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
    let name = name_field(&fields.surname, &fields.given_names, 39);
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

/// Raw TD2 sub-fields, in MRZ-native form (see module docs).
///
/// `document_number` is limited to 9 characters (the field width); longer
/// input is silently truncated. `date_of_birth` / `date_of_expiry` are
/// `YYMMDD`, not ISO dates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Td2Fields {
    /// Document code, e.g. `"I"` for identity card. Defaults to `"I"`.
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
    /// Optional data, up to 7 characters. TD2 has no check digit over this
    /// field on its own — it only feeds the composite check.
    pub optional_data: Option<String>,
}

impl Default for Td2Fields {
    fn default() -> Self {
        Self {
            document_code: "I".to_string(),
            issuing_country: String::new(),
            document_number: String::new(),
            surname: String::new(),
            given_names: String::new(),
            nationality: String::new(),
            date_of_birth: String::new(),
            sex: String::new(),
            date_of_expiry: String::new(),
            optional_data: None,
        }
    }
}

/// Emit a TD2 (identity-card) MRZ: two 36-character lines joined by `\n`.
///
/// The document number, date-of-birth, date-of-expiry, and composite check
/// digits are computed from `fields` — the result always round-trips through
/// [`crate::parse_td2`] with `valid() == true` (see `tests/roundtrip.rs`).
/// TD2 has no separate check digit over the optional-data field.
pub fn format_td2(fields: &Td2Fields) -> String {
    let doc_code = field(&fields.document_code, 2);
    let issuing = field(&fields.issuing_country, 3);
    let name = name_field(&fields.surname, &fields.given_names, 31);
    let line1 = format!("{doc_code}{issuing}{name}");
    debug_assert_eq!(line1.len(), 36);

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
    let optional = field(fields.optional_data.as_deref().unwrap_or(""), 7);

    // 35 chars: everything except the composite digit itself, in the same
    // layout `parser::parse_td2` expects (offsets 0..35).
    let line2_body = format!(
        "{doc_num}{doc_num_check}{nationality}{dob}{dob_check}{sex}{expiry}{expiry_check}{optional}"
    );
    debug_assert_eq!(line2_body.len(), 35);

    // Composite: line2[0..10] (doc number + check) + line2[13..20]
    // (DOB + check) + line2[21..35] (expiry + check + optional data) —
    // mirrors `parser::parse_td2`'s `checks.composite` computation exactly.
    let composite_input = format!(
        "{}{}{}",
        &line2_body[0..10],
        &line2_body[13..20],
        &line2_body[21..35]
    );
    let composite_check = digit_char(&composite_input);

    let line2 = format!("{line2_body}{composite_check}");
    debug_assert_eq!(line2.len(), 36);

    format!("{line1}\n{line2}")
}

/// Raw TD1 sub-fields, in MRZ-native form (see module docs).
///
/// `document_number` is limited to 9 characters (the field width); longer
/// input is silently truncated. `date_of_birth` / `date_of_expiry` are
/// `YYMMDD`, not ISO dates.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Td1Fields {
    /// Document code, e.g. `"I"` for identity card. Defaults to `"I"`.
    pub document_code: String,
    /// Issuing state (3-letter ICAO code).
    pub issuing_country: String,
    /// Document number, up to 9 characters.
    pub document_number: String,
    /// Optional data on line 1, up to 15 characters. TD1 has no separate
    /// check digit over this field on its own — it only feeds the composite.
    pub optional_data_1: Option<String>,
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
    /// Optional data on line 2, up to 11 characters. TD1 has no separate
    /// check digit over this field on its own — it only feeds the composite.
    pub optional_data_2: Option<String>,
}

impl Default for Td1Fields {
    fn default() -> Self {
        Self {
            document_code: "I".to_string(),
            issuing_country: String::new(),
            document_number: String::new(),
            optional_data_1: None,
            surname: String::new(),
            given_names: String::new(),
            nationality: String::new(),
            date_of_birth: String::new(),
            sex: String::new(),
            date_of_expiry: String::new(),
            optional_data_2: None,
        }
    }
}

/// Emit a TD1 (ID-card) MRZ: three 30-character lines joined by `\n`.
///
/// The document number, date-of-birth, date-of-expiry, and composite check
/// digits are computed from `fields` — the result always round-trips through
/// [`crate::parse_td1`] with `valid() == true` (see `tests/roundtrip.rs`).
/// TD1 has no separate check digit over either optional-data field.
pub fn format_td1(fields: &Td1Fields) -> String {
    let doc_code = field(&fields.document_code, 2);
    let issuing = field(&fields.issuing_country, 3);
    let doc_num = field(&fields.document_number, 9);
    let doc_num_check = digit_char(&doc_num);
    let optional1 = field(fields.optional_data_1.as_deref().unwrap_or(""), 15);
    let line1 = format!("{doc_code}{issuing}{doc_num}{doc_num_check}{optional1}");
    debug_assert_eq!(line1.len(), 30);

    let dob = field(&fields.date_of_birth, 6);
    let dob_check = digit_char(&dob);
    let sex = match fields.sex.to_ascii_uppercase().as_str() {
        "M" => 'M',
        "F" => 'F',
        _ => '<',
    };
    let expiry = field(&fields.date_of_expiry, 6);
    let expiry_check = digit_char(&expiry);
    let nationality = field(&fields.nationality, 3);
    let optional2 = field(fields.optional_data_2.as_deref().unwrap_or(""), 11);

    // 29 chars: everything except the composite digit itself, in the same
    // layout `parser::parse_td1` expects (offsets 0..29).
    let line2_body = format!("{dob}{dob_check}{sex}{expiry}{expiry_check}{nationality}{optional2}");
    debug_assert_eq!(line2_body.len(), 29);

    // Composite: line1[5..30] (doc number + check + optional-1) +
    // line2[0..7] (DOB + check) + line2[8..15] (expiry + check) +
    // line2[18..29] (optional-2) — mirrors `parser::parse_td1`'s
    // `checks.composite` computation exactly.
    let composite_input = format!(
        "{}{}{}{}",
        &line1[5..30],
        &line2_body[0..7],
        &line2_body[8..15],
        &line2_body[18..29]
    );
    let composite_check = digit_char(&composite_input);

    let line2 = format!("{line2_body}{composite_check}");
    debug_assert_eq!(line2.len(), 30);

    let line3 = name_field(&fields.surname, &fields.given_names, 30);
    debug_assert_eq!(line3.len(), 30);

    format!("{line1}\n{line2}\n{line3}")
}

/// Raw MRV-A sub-fields, in MRZ-native form (see module docs).
///
/// `document_number` is limited to 9 characters (the field width); longer
/// input is silently truncated. `date_of_birth` / `date_of_expiry` are
/// `YYMMDD`, not ISO dates. MRV-A has no personal-number or composite check
/// digit — `optional_data` is free-form data up to 16 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MrvAFields {
    /// Document code, always `"V"` for a visa. Defaults to `"V"`.
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
    /// Optional free-form data, up to 16 characters. No check digit covers
    /// this field — MRVs have neither a personal-number nor composite check.
    pub optional_data: Option<String>,
}

impl Default for MrvAFields {
    fn default() -> Self {
        Self {
            document_code: "V".to_string(),
            issuing_country: String::new(),
            document_number: String::new(),
            surname: String::new(),
            given_names: String::new(),
            nationality: String::new(),
            date_of_birth: String::new(),
            sex: String::new(),
            date_of_expiry: String::new(),
            optional_data: None,
        }
    }
}

/// Emit an MRV-A (machine readable visa) MRZ: two 44-character lines joined
/// by `\n`.
///
/// The document number, date-of-birth, and date-of-expiry check digits are
/// computed from `fields` — the result always round-trips through
/// [`crate::parse_mrv_a`] with `valid() == true` (see `tests/roundtrip.rs`).
/// MRV-A has no personal-number check digit and no composite check digit.
pub fn format_mrv_a(fields: &MrvAFields) -> String {
    let doc_code = field(&fields.document_code, 2);
    let issuing = field(&fields.issuing_country, 3);
    let name = name_field(&fields.surname, &fields.given_names, 39);
    let line1 = format!("{doc_code}{issuing}{name}");
    debug_assert_eq!(line1.len(), 44);

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
    let optional = field(fields.optional_data.as_deref().unwrap_or(""), 16);

    let line2 = format!(
        "{doc_num}{doc_num_check}{nationality}{dob}{dob_check}{sex}{expiry}{expiry_check}{optional}"
    );
    debug_assert_eq!(line2.len(), 44);

    format!("{line1}\n{line2}")
}

/// Raw MRV-B sub-fields, in MRZ-native form (see module docs).
///
/// `document_number` is limited to 9 characters (the field width); longer
/// input is silently truncated. `date_of_birth` / `date_of_expiry` are
/// `YYMMDD`, not ISO dates. MRV-B has no personal-number or composite check
/// digit — `optional_data` is free-form data up to 8 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MrvBFields {
    /// Document code, always `"V"` for a visa. Defaults to `"V"`.
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
    /// Optional free-form data, up to 8 characters. No check digit covers
    /// this field — MRVs have neither a personal-number nor composite check.
    pub optional_data: Option<String>,
}

impl Default for MrvBFields {
    fn default() -> Self {
        Self {
            document_code: "V".to_string(),
            issuing_country: String::new(),
            document_number: String::new(),
            surname: String::new(),
            given_names: String::new(),
            nationality: String::new(),
            date_of_birth: String::new(),
            sex: String::new(),
            date_of_expiry: String::new(),
            optional_data: None,
        }
    }
}

/// Emit an MRV-B (machine readable visa) MRZ: two 36-character lines joined
/// by `\n`.
///
/// The document number, date-of-birth, and date-of-expiry check digits are
/// computed from `fields` — the result always round-trips through
/// [`crate::parse_mrv_b`] with `valid() == true` (see `tests/roundtrip.rs`).
/// MRV-B has no personal-number check digit and no composite check digit.
pub fn format_mrv_b(fields: &MrvBFields) -> String {
    let doc_code = field(&fields.document_code, 2);
    let issuing = field(&fields.issuing_country, 3);
    let name = name_field(&fields.surname, &fields.given_names, 31);
    let line1 = format!("{doc_code}{issuing}{name}");
    debug_assert_eq!(line1.len(), 36);

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
    let optional = field(fields.optional_data.as_deref().unwrap_or(""), 8);

    let line2 = format!(
        "{doc_num}{doc_num_check}{nationality}{dob}{dob_check}{sex}{expiry}{expiry_check}{optional}"
    );
    debug_assert_eq!(line2.len(), 36);

    format!("{line1}\n{line2}")
}
