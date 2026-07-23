//! MRZ line parsers (TD1/TD2/TD3) and the free-text scanner [`find_and_parse`].
//!
//! Field offsets follow ICAO 9303 parts 4 (TD3), 5 (TD1) and 6 (TD2). Each
//! parser verifies every printed check digit; the scanner drives the OCR-repair
//! machinery in [`crate::checksum`] and accepts a candidate reading only when
//! its composite check digit proves the read.

use crate::checksum::{
    aggressive_defiller, char_value, defiller, digitize, fix_doc_code, fix_name_separator,
    is_mrz_charset, letterize, normalize_line, repair_positions, variants, verify,
};
use crate::dates::expand_date_with_pivot;
use crate::{Checks, Format, MrzData, MrzError, ParseOptions};

/// A document number that overflowed its 9-character field.
struct Overflow {
    /// First 8 printed characters plus the remainder read from the optional field.
    full: String,
    /// Whether the remainder's own check digit validates the reassembly.
    check_ok: bool,
    /// Characters of the optional field consumed by the overflow encoding
    /// (remainder + its check digit + the terminating filler).
    consumed: usize,
}

/// Decode the ICAO 9303 part 4 §4.2.2.2 long-document-number encoding.
///
/// When the number exceeds the 9-character field, the document is printed with
/// its first 8 characters followed by a filler, the field's check-digit
/// position set to a filler, and the *remainder* plus a check digit computed
/// over the whole number written at the start of the optional/personal-number
/// field, terminated by a filler.
///
/// Returns `None` when the zone does not carry that signature, in which case
/// the caller keeps the ordinary fixed-width reading. MRV-A/MRV-B never use
/// this — ICAO 9303 part 7 defines no overflow encoding.
fn read_overflow(number_field: &str, check: char, optional: &str) -> Option<Overflow> {
    if check != '<' || !number_field.ends_with('<') {
        return None;
    }
    let head = number_field[0..8].trim_end_matches('<');
    if head.is_empty() {
        // A blank document-number field is a blank field, not an overflow.
        return None;
    }
    // The remainder runs to the terminating filler and ends with its check digit.
    let end = optional.find('<')?;
    if end < 2 {
        return None; // need at least one remainder character plus a check digit
    }
    let (remainder, check_digit) = optional[..end].split_at(end - 1);
    let check_digit = check_digit.chars().next()?;
    if !check_digit.is_ascii_digit() {
        return None;
    }
    let full = format!("{head}{remainder}");
    Some(Overflow {
        check_ok: verify(&full, check_digit),
        full,
        consumed: end + 1,
    })
}

/// Trim an optional-data field to what remains after any overflow encoding.
fn optional_tail<'a>(optional: &'a str, overflow: &Option<Overflow>) -> &'a str {
    match overflow {
        Some(o) => optional[o.consumed..].trim_end_matches('<'),
        None => optional.trim_end_matches('<'),
    }
}

fn opt_string(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

fn clean_name(field: &str) -> (String, String) {
    let trimmed = field.trim_end_matches('<');
    let (surname, given) = match trimmed.split_once("<<") {
        Some((s, g)) => (s, g),
        None => (trimmed, ""),
    };
    (
        surname.replace('<', " ").trim().to_string(),
        given.replace('<', " ").trim().to_string(),
    )
}

fn clean_sex(c: char) -> String {
    match c {
        'M' => "M".into(),
        'F' => "F".into(),
        _ => "X".into(),
    }
}

fn ensure_charset(line: &str) -> Result<(), MrzError> {
    for c in line.chars() {
        char_value(c)?;
    }
    Ok(())
}

/// Parse a TD3 (passport) MRZ: two lines of exactly 44 characters
/// (ICAO 9303 part 4 §4.2.2; document-number overflow is §4.2.2.2).
pub fn parse_td3(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
    parse_td3_with(line1, line2, &ParseOptions::default())
}

/// [`parse_td3`] with an explicit [`ParseOptions`].
pub fn parse_td3_with(line1: &str, line2: &str, opts: &ParseOptions) -> Result<MrzData, MrzError> {
    for line in [line1, line2] {
        if line.len() != 44 {
            return Err(MrzError::BadLength {
                expected: 44,
                got: line.len(),
            });
        }
        ensure_charset(line)?;
    }
    if !line1.starts_with('P') {
        return Err(MrzError::BadDocumentCode(line1[0..2].to_string()));
    }

    let (surname, given_names) = clean_name(&line1[5..44]);

    let document_number = line2[0..9].trim_end_matches('<').to_string();
    let personal_raw = &line2[28..42];
    let overflow = read_overflow(&line2[0..9], line2.as_bytes()[9] as char, personal_raw);
    let personal = optional_tail(personal_raw, &overflow);

    let checks = Checks {
        document_number: match &overflow {
            Some(o) => o.check_ok,
            None => verify(&line2[0..9], line2.as_bytes()[9] as char),
        },
        date_of_birth: verify(&line2[13..19], line2.as_bytes()[19] as char),
        date_of_expiry: verify(&line2[21..27], line2.as_bytes()[27] as char),
        personal_number: verify(personal_raw, line2.as_bytes()[42] as char),
        // Composite: doc number + check, DOB + check, expiry + check +
        // personal number + check (line 2 positions 1-10, 14-20, 22-43).
        composite: verify(
            &format!("{}{}{}", &line2[0..10], &line2[13..20], &line2[21..43]),
            line2.as_bytes()[43] as char,
        ),
    };

    Ok(MrzData {
        format: Format::Td3,
        document_type: line1[0..2].trim_end_matches('<').to_string(),
        issuing_country: line1[2..5].trim_end_matches('<').to_string(),
        document_number,
        document_number_full: overflow.map(|o| o.full),
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date_with_pivot(&line2[13..19], true, opts.pivot_yy),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date_with_pivot(&line2[21..27], false, opts.pivot_yy),
        personal_number: opt_string(personal),
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse a TD2 MRZ: two lines of exactly 36 characters (ICAO 9303 part 6).
/// Covers identity-card document codes (`I`/`A`/`C`); MRV-B visas share the
/// geometry but lack a composite check digit and are not handled here.
pub fn parse_td2(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
    parse_td2_with(line1, line2, &ParseOptions::default())
}

/// [`parse_td2`] with an explicit [`ParseOptions`].
pub fn parse_td2_with(line1: &str, line2: &str, opts: &ParseOptions) -> Result<MrzData, MrzError> {
    for line in [line1, line2] {
        if line.len() != 36 {
            return Err(MrzError::BadLength {
                expected: 36,
                got: line.len(),
            });
        }
        ensure_charset(line)?;
    }
    let code = line1[0..2].trim_end_matches('<');
    if !matches!(code.as_bytes().first(), Some(b'I' | b'A' | b'C')) {
        return Err(MrzError::BadDocumentCode(code.to_string()));
    }

    let (surname, given_names) = clean_name(&line1[5..36]);

    let document_number = line2[0..9].trim_end_matches('<').to_string();
    let optional_raw = &line2[28..35];
    let overflow = read_overflow(&line2[0..9], line2.as_bytes()[9] as char, optional_raw);
    let optional = optional_tail(optional_raw, &overflow);

    let checks = Checks {
        document_number: match &overflow {
            Some(o) => o.check_ok,
            None => verify(&line2[0..9], line2.as_bytes()[9] as char),
        },
        date_of_birth: verify(&line2[13..19], line2.as_bytes()[19] as char),
        date_of_expiry: verify(&line2[21..27], line2.as_bytes()[27] as char),
        personal_number: true, // TD2 has no separate personal-number check digit
        // Composite: line 2 positions 1-10, 14-20, 22-35 (doc number + check,
        // DOB + check, expiry + check + optional data).
        composite: verify(
            &format!("{}{}{}", &line2[0..10], &line2[13..20], &line2[21..35]),
            line2.as_bytes()[35] as char,
        ),
    };

    Ok(MrzData {
        format: Format::Td2,
        document_type: code.to_string(),
        issuing_country: line1[2..5].trim_end_matches('<').to_string(),
        document_number,
        document_number_full: overflow.map(|o| o.full),
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date_with_pivot(&line2[13..19], true, opts.pivot_yy),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date_with_pivot(&line2[21..27], false, opts.pivot_yy),
        personal_number: opt_string(optional),
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse a TD1 (ID card) MRZ: three lines of exactly 30 characters
/// (ICAO 9303 part 5).
pub fn parse_td1(line1: &str, line2: &str, line3: &str) -> Result<MrzData, MrzError> {
    parse_td1_with(line1, line2, line3, &ParseOptions::default())
}

/// [`parse_td1`] with an explicit [`ParseOptions`].
pub fn parse_td1_with(
    line1: &str,
    line2: &str,
    line3: &str,
    opts: &ParseOptions,
) -> Result<MrzData, MrzError> {
    for line in [line1, line2, line3] {
        if line.len() != 30 {
            return Err(MrzError::BadLength {
                expected: 30,
                got: line.len(),
            });
        }
        ensure_charset(line)?;
    }
    let code = line1[0..2].trim_end_matches('<');
    if !matches!(code.as_bytes().first(), Some(b'I' | b'A' | b'C')) {
        return Err(MrzError::BadDocumentCode(code.to_string()));
    }

    let (surname, given_names) = clean_name(line3);

    let optional1_raw = &line1[15..30];
    let overflow = read_overflow(&line1[5..14], line1.as_bytes()[14] as char, optional1_raw);
    let optional1 = optional_tail(optional1_raw, &overflow);
    let optional2 = line2[18..29].trim_end_matches('<');
    let personal = [optional1, optional2]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    let checks = Checks {
        document_number: match &overflow {
            Some(o) => o.check_ok,
            None => verify(&line1[5..14], line1.as_bytes()[14] as char),
        },
        date_of_birth: verify(&line2[0..6], line2.as_bytes()[6] as char),
        date_of_expiry: verify(&line2[8..14], line2.as_bytes()[14] as char),
        personal_number: true, // TD1 has no personal-number check digit
        // Composite: line1 positions 6-30, line2 positions 1-7, 9-15, 19-29.
        composite: verify(
            &format!(
                "{}{}{}{}",
                &line1[5..30],
                &line2[0..7],
                &line2[8..15],
                &line2[18..29]
            ),
            line2.as_bytes()[29] as char,
        ),
    };

    Ok(MrzData {
        format: Format::Td1,
        document_type: code.to_string(),
        issuing_country: line1[2..5].trim_end_matches('<').to_string(),
        document_number: line1[5..14].trim_end_matches('<').to_string(),
        document_number_full: overflow.map(|o| o.full),
        surname,
        given_names,
        nationality: line2[15..18].trim_end_matches('<').to_string(),
        date_of_birth: expand_date_with_pivot(&line2[0..6], true, opts.pivot_yy),
        sex: clean_sex(line2.as_bytes()[7] as char),
        date_of_expiry: expand_date_with_pivot(&line2[8..14], false, opts.pivot_yy),
        personal_number: opt_string(&personal),
        mrz_lines: format!("{line1}\n{line2}\n{line3}"),
        checks,
    })
}

/// Parse an MRV-A machine readable visa: two lines of exactly 44 characters
/// (ICAO 9303 part 7). Geometry mirrors TD3 through the expiry check digit,
/// but there is no personal-number field and no composite check digit.
pub fn parse_mrv_a(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
    parse_mrv_a_with(line1, line2, &ParseOptions::default())
}

/// [`parse_mrv_a`] with an explicit [`ParseOptions`].
pub fn parse_mrv_a_with(
    line1: &str,
    line2: &str,
    opts: &ParseOptions,
) -> Result<MrzData, MrzError> {
    for line in [line1, line2] {
        if line.len() != 44 {
            return Err(MrzError::BadLength {
                expected: 44,
                got: line.len(),
            });
        }
        ensure_charset(line)?;
    }
    if !line1.starts_with('V') {
        return Err(MrzError::BadDocumentCode(line1[0..2].to_string()));
    }

    let (surname, given_names) = clean_name(&line1[5..44]);

    let document_number = line2[0..9].trim_end_matches('<').to_string();
    let optional = line2[28..44].trim_end_matches('<');

    let checks = Checks {
        document_number: verify(&line2[0..9], line2.as_bytes()[9] as char),
        date_of_birth: verify(&line2[13..19], line2.as_bytes()[19] as char),
        date_of_expiry: verify(&line2[21..27], line2.as_bytes()[27] as char),
        // MRV-A has no personal-number or composite check digit at all;
        // vacuously true, same convention TD1/TD2 use for personal_number.
        personal_number: true,
        composite: true,
    };

    Ok(MrzData {
        format: Format::MrvA,
        document_type: line1[0..2].trim_end_matches('<').to_string(),
        issuing_country: line1[2..5].trim_end_matches('<').to_string(),
        document_number,
        // ICAO 9303 part 7 defines no overflow encoding for visas.
        document_number_full: None,
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date_with_pivot(&line2[13..19], true, opts.pivot_yy),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date_with_pivot(&line2[21..27], false, opts.pivot_yy),
        personal_number: opt_string(optional),
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse an MRV-B machine readable visa: two lines of exactly 36 characters
/// (ICAO 9303 part 7). Geometry mirrors TD2 through the expiry check digit,
/// but there is no personal-number field and no composite check digit.
pub fn parse_mrv_b(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
    parse_mrv_b_with(line1, line2, &ParseOptions::default())
}

/// [`parse_mrv_b`] with an explicit [`ParseOptions`].
pub fn parse_mrv_b_with(
    line1: &str,
    line2: &str,
    opts: &ParseOptions,
) -> Result<MrzData, MrzError> {
    for line in [line1, line2] {
        if line.len() != 36 {
            return Err(MrzError::BadLength {
                expected: 36,
                got: line.len(),
            });
        }
        ensure_charset(line)?;
    }
    if !line1.starts_with('V') {
        return Err(MrzError::BadDocumentCode(line1[0..2].to_string()));
    }

    let (surname, given_names) = clean_name(&line1[5..36]);

    let document_number = line2[0..9].trim_end_matches('<').to_string();
    let optional = line2[28..36].trim_end_matches('<');

    let checks = Checks {
        document_number: verify(&line2[0..9], line2.as_bytes()[9] as char),
        date_of_birth: verify(&line2[13..19], line2.as_bytes()[19] as char),
        date_of_expiry: verify(&line2[21..27], line2.as_bytes()[27] as char),
        // MRV-B has no personal-number or composite check digit at all;
        // vacuously true, same convention TD1/TD2 use for personal_number.
        personal_number: true,
        composite: true,
    };

    Ok(MrzData {
        format: Format::MrvB,
        document_type: line1[0..2].trim_end_matches('<').to_string(),
        issuing_country: line1[2..5].trim_end_matches('<').to_string(),
        document_number,
        // ICAO 9303 part 7 defines no overflow encoding for visas.
        document_number_full: None,
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date_with_pivot(&line2[13..19], true, opts.pivot_yy),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date_with_pivot(&line2[21..27], false, opts.pivot_yy),
        personal_number: opt_string(optional),
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

fn repair_td3_line1(l: &str) -> String {
    // Issuing state must be letters; the name field suffers filler misreads.
    let l = fix_doc_code(&repair_positions(l, &[(2..5, letterize)]));
    format!("{}{}", &l[0..5], fix_name_separator(&defiller(&l[5..])))
}

fn repair_td3_line2(l: &str) -> String {
    let l = repair_positions(
        l,
        &[
            (9..10, digitize),   // doc number check digit
            (10..13, letterize), // nationality
            (13..20, digitize),  // DOB + check digit
            (21..28, digitize),  // expiry + check digit
            (42..44, digitize),  // personal-number check + composite
        ],
    );
    // The personal-number field is filler-dominated on most passports.
    // Note: `K` ≡ `<` (both value 20 ≡ 0 mod 10) under every 7-3-1 weight, so
    // check digits are provably blind to this misread — heuristics must do it.
    format!(
        "{}{}{}",
        &l[0..28],
        aggressive_defiller(&defiller(&l[28..42])),
        &l[42..44]
    )
}

fn repair_td2_line1(l: &str) -> String {
    let l = fix_doc_code(&repair_positions(l, &[(2..5, letterize)]));
    format!("{}{}", &l[0..5], fix_name_separator(&defiller(&l[5..])))
}

fn repair_td2_line2(l: &str) -> String {
    let l = repair_positions(
        l,
        &[
            (9..10, digitize),   // doc number check digit
            (10..13, letterize), // nationality
            (13..20, digitize),  // DOB + check digit
            (21..28, digitize),  // expiry + check digit
            (35..36, digitize),  // composite check digit
        ],
    );
    format!(
        "{}{}{}",
        &l[0..28],
        aggressive_defiller(&defiller(&l[28..35])),
        &l[35..36]
    )
}

fn repair_td1_line1(l: &str) -> String {
    let l = fix_doc_code(&repair_positions(
        l,
        &[(2..5, letterize), (14..15, digitize)],
    ));
    format!("{}{}", &l[0..15], aggressive_defiller(&defiller(&l[15..])))
}

fn repair_td1_line2(l: &str) -> String {
    let l = repair_positions(
        l,
        &[
            (0..7, digitize),    // DOB + check digit
            (8..15, digitize),   // expiry + check digit
            (15..18, letterize), // nationality
            (29..30, digitize),  // composite
        ],
    );
    format!(
        "{}{}{}",
        &l[0..18],
        aggressive_defiller(&defiller(&l[18..29])),
        &l[29..30]
    )
}

fn repair_td1_line3(l: &str) -> String {
    fix_name_separator(&defiller(l))
}

/// MRV name/document-code line repair, shared shape by MRV-A (44 chars) and
/// MRV-B (36 chars). Modeled on `repair_td3_line1`/`repair_td2_line1`: letterize
/// the issuing-state field, fix the doc-code filler, defiller + fix the name
/// separator. Neither `repair_positions` (range starts at index 2) nor
/// `fix_doc_code` (only ever rewrites index 1) can touch index 0, so the
/// leading `V` document code always survives repair unchanged — the caller's
/// `l1.starts_with('V')` check after repair is exactly as strict as before.
fn repair_mrv_line1(l: &str) -> String {
    let l = fix_doc_code(&repair_positions(l, &[(2..5, letterize)]));
    format!("{}{}", &l[0..5], fix_name_separator(&defiller(&l[5..])))
}

fn repair_mrv_a_line1(l: &str) -> String {
    repair_mrv_line1(l)
}

fn repair_mrv_b_line1(l: &str) -> String {
    repair_mrv_line1(l)
}

/// MRV-A line-2 repair: same geometry as TD3 through the expiry check digit,
/// but positions 28..44 are free-form optional data with no check digit of
/// its own — unlike `repair_td3_line2`, we must NOT digitize/defiller the
/// tail as if it were a personal-number + composite check field, since that
/// data is never checked and unconstrained.
fn repair_mrv_a_line2(l: &str) -> String {
    repair_positions(
        l,
        &[
            (9..10, digitize),   // doc number check digit
            (10..13, letterize), // nationality
            (13..20, digitize),  // DOB + check digit
            (21..28, digitize),  // expiry + check digit
        ],
    )
}

/// MRV-B line-2 repair: same geometry as TD2 through the expiry check digit;
/// positions 28..36 are free-form optional data, left untouched.
fn repair_mrv_b_line2(l: &str) -> String {
    repair_positions(
        l,
        &[
            (9..10, digitize),   // doc number check digit
            (10..13, letterize), // nationality
            (13..20, digitize),  // DOB + check digit
            (21..28, digitize),  // expiry + check digit
        ],
    )
}

/// Scan free-form text (e.g. OCR output) for an MRZ and parse it.
///
/// Tries TD3 (two 44-char lines starting with `P`), then TD1 (three 30-char
/// lines starting with `I`/`A`/`C`), then TD2 (two 36-char lines starting with
/// `I`/`A`/`C`). Tolerates HTML-escaped fillers (`&lt;`, as produced by
/// docling's Markdown) and MRZ lines merged onto a single physical line.
///
/// A reading whose check digits all validate is returned immediately. When no
/// candidate fully validates, the *best-scoring* one — the reading with the
/// most passing check digits — is returned with its honest (partially `false`)
/// [`Checks`], so callers can see how close the read came and decide whether to
/// escalate. [`MrzError::NotFound`] means nothing MRZ-shaped was found at all.
pub fn find_and_parse(text: &str) -> Result<MrzData, MrzError> {
    find_and_parse_with(text, &ParseOptions::default())
}

/// [`find_and_parse`] with an explicit [`ParseOptions`].
pub fn find_and_parse_with(text: &str, opts: &ParseOptions) -> Result<MrzData, MrzError> {
    // Markdown/HTML pipelines escape the filler character.
    let text = text.replace("&lt;", "<");
    // OCR often emits several MRZ lines as ONE physical line, space-separated
    // (docling renders the whole zone as a single paragraph) — treat long
    // whitespace-separated tokens as individual candidate lines.
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().filter(|t| t.len() >= 20).collect();
        if tokens.len() >= 2 {
            lines.extend(tokens);
        } else {
            lines.push(line);
        }
    }

    // Best parseable-but-checksum-failed hit, reported when nothing fully
    // validates so callers can see which check digits failed and how close the
    // read came. Ties keep the earlier candidate: the repair pipeline emits its
    // most conservative variants first, so the first reading at a given score
    // is the one that assumed least about the OCR noise.
    let mut fallback: Option<MrzData> = None;
    let mut consider = |data: MrzData| -> Option<MrzData> {
        if data.valid() {
            return Some(data);
        }
        match &fallback {
            Some(best) if best.checks.score() >= data.checks.score() => {}
            _ => fallback = Some(data),
        }
        None
    };

    // TD3: a line starting with 'P' followed by a candidate line — or both
    // 44-char lines merged into one ~88-char physical line.
    for i in 0..lines.len() {
        let merged = normalize_line(lines[i]);
        if merged.starts_with('P') && (84..=92).contains(&merged.len()) && is_mrz_charset(&merged) {
            let head = &merged[0..44];
            let tail = &merged[44..];
            for l1 in [repair_td3_line1(head), head.to_string()] {
                for l2 in variants(tail, 44, repair_td3_line2) {
                    if let Ok(data) = parse_td3_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }

        for l1 in variants(lines[i], 44, repair_td3_line1) {
            if !l1.starts_with('P') {
                continue;
            }
            for l2_raw in lines.iter().skip(i + 1).take(3) {
                for l2 in variants(l2_raw, 44, repair_td3_line2) {
                    if let Ok(data) = parse_td3_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }
    }

    // MRV-B: a line starting with 'V' followed by a candidate line — or both
    // 36-char lines merged into one ~68-76 char physical line. Tried before
    // MRV-A: both share the 'V' document-code prefix (no length hint from the
    // code itself), and `variants`'s padding tolerance (+14) is far more
    // generous than its truncation tolerance (+4) — trying the *narrower*
    // format first means a genuine MRV-B line never gets loosely padded up
    // and misparsed as MRV-A before MRV-B gets a chance at its exact length.
    for i in 0..lines.len() {
        let merged = normalize_line(lines[i]);
        if merged.starts_with('V') && (68..=76).contains(&merged.len()) && is_mrz_charset(&merged) {
            let head = &merged[0..36];
            let tail = &merged[36..];
            for l1 in [repair_mrv_b_line1(head), head.to_string()] {
                for l2 in variants(tail, 36, repair_mrv_b_line2) {
                    if let Ok(data) = parse_mrv_b_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }

        for l1 in variants(lines[i], 36, repair_mrv_b_line1) {
            if !l1.starts_with('V') {
                continue;
            }
            for l2_raw in lines.iter().skip(i + 1).take(3) {
                for l2 in variants(l2_raw, 36, repair_mrv_b_line2) {
                    if let Ok(data) = parse_mrv_b_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }
    }

    // MRV-A: a line starting with 'V' followed by a candidate line — or both
    // 44-char lines merged into one ~84-92 char physical line. Disjoint from
    // TD3 ('P'-prefixed) and TD1/TD2 (I/A/C-prefixed), so no cannibalization
    // against those; see the MRV-B comment above for why MRV-B runs first.
    for i in 0..lines.len() {
        let merged = normalize_line(lines[i]);
        if merged.starts_with('V') && (84..=92).contains(&merged.len()) && is_mrz_charset(&merged) {
            let head = &merged[0..44];
            let tail = &merged[44..];
            for l1 in [repair_mrv_a_line1(head), head.to_string()] {
                for l2 in variants(tail, 44, repair_mrv_a_line2) {
                    if let Ok(data) = parse_mrv_a_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }

        for l1 in variants(lines[i], 44, repair_mrv_a_line1) {
            if !l1.starts_with('V') {
                continue;
            }
            for l2_raw in lines.iter().skip(i + 1).take(3) {
                for l2 in variants(l2_raw, 44, repair_mrv_a_line2) {
                    if let Ok(data) = parse_mrv_a_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }
    }

    // TD1: three consecutive candidate lines, first starting with I/A/C.
    for i in 0..lines.len().saturating_sub(2) {
        for l1 in variants(lines[i], 30, repair_td1_line1) {
            if !matches!(l1.as_bytes().first(), Some(b'I' | b'A' | b'C')) {
                continue;
            }
            for l2 in variants(lines[i + 1], 30, repair_td1_line2) {
                for l3 in variants(lines[i + 2], 30, repair_td1_line3) {
                    if let Ok(data) = parse_td1_with(&l1, &l2, &l3, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }
    }

    // TD2: two 36-char lines starting with I/A/C — or both merged into one
    // ~72-char physical line.
    for &line in &lines {
        let merged = normalize_line(line);
        if (68..=76).contains(&merged.len())
            && is_mrz_charset(&merged)
            && matches!(merged.as_bytes().first(), Some(b'I' | b'A' | b'C'))
        {
            let head = &merged[0..36];
            let tail = &merged[36..];
            for l1 in [repair_td2_line1(head), head.to_string()] {
                for l2 in variants(tail, 36, repair_td2_line2) {
                    if let Ok(data) = parse_td2_with(&l1, &l2, opts) {
                        if let Some(valid) = consider(data) {
                            return Ok(valid);
                        }
                    }
                }
            }
        }
    }
    for i in 0..lines.len().saturating_sub(1) {
        for l1 in variants(lines[i], 36, repair_td2_line1) {
            if !matches!(l1.as_bytes().first(), Some(b'I' | b'A' | b'C')) {
                continue;
            }
            for l2 in variants(lines[i + 1], 36, repair_td2_line2) {
                if let Ok(data) = parse_td2_with(&l1, &l2, opts) {
                    if let Some(valid) = consider(data) {
                        return Ok(valid);
                    }
                }
            }
        }
    }

    // Nothing validated on any ordinary variant. Before giving up, try the
    // damaged-capture case (see `crate::repair`): a line that arrived *narrow*
    // because the recognizer dropped a destroyed glyph instead of emitting a
    // placeholder, so every field after the damage is shifted and no amount of
    // lookalike repair can help. Runs only here, at the end, and only when
    // some line already matched a format's shape — a document with no MRZ at
    // all never pays for it.
    if fallback.is_some() {
        if let Some(data) = damaged_pass(&lines, opts) {
            return Ok(data);
        }
    }

    fallback.ok_or(MrzError::NotFound)
}

/// A two-line format's parse entry point, as [`damaged_pass`]'s table stores it.
type TwoLineParse = fn(&str, &str, &ParseOptions) -> Result<MrzData, MrzError>;

/// One row of [`damaged_pass`]'s two-line format table: the ICAO line width,
/// the line-1 document-code bytes that identify the format, the per-line
/// lookalike repairs, and the parser. Adding a two-line format is a row here
/// rather than another nested loop.
type TwoLineFormat = (
    usize,
    &'static [u8],
    fn(&str) -> String,
    fn(&str) -> String,
    TwoLineParse,
);

/// Upper bound on parse attempts across the whole damaged pass.
///
/// The pass is a search, and a search inside an OCR retry loop needs a ceiling
/// rather than good intentions — the same reasoning as
/// `checksum::MAX_DEFILL_PASSES`. Generous enough for one narrow line crossed
/// against the ordinary variants of its neighbours; far below anything a user
/// would notice.
const MAX_DAMAGED_ATTEMPTS: usize = 200_000;

/// Concrete candidate readings for a line that is **exactly one** character
/// too narrow, with the missing character swept across every insertion point.
///
/// One is not an arbitrary cutoff. A single destroyed position admits one
/// residue class, which the calendar constraint in [`accept_damaged`] can cut
/// to a unique reading; two destroyed positions leave hundreds of readings that
/// every check digit accepts (`tests/repair.rs` pins this), so no amount of
/// searching can return an answer — it can only return a guess. Attempting it
/// would cost 37² candidates per insertion point for a result this function is
/// required to throw away.
///
/// Empty for any other line, which is what keeps this off the happy path.
fn restored(raw: &str, target: usize, repair: fn(&str) -> String) -> Vec<String> {
    let n = normalize_line(raw);
    if n.len() + 1 != target || !is_mrz_charset(&n) {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for shaped in crate::repair::width_candidates(&n, target) {
        if !shaped.contains(crate::repair::UNKNOWN) {
            continue;
        }
        for concrete in crate::repair::concrete_fillings(&shaped) {
            for form in [repair(&concrete), concrete] {
                if seen.insert(form.clone()) {
                    out.push(form);
                }
            }
        }
    }
    out
}

/// A reading recovered from damage has to clear a higher bar than one read
/// cleanly: every check digit valid **and** both dates real calendar dates.
///
/// The dates are load-bearing, not belt-and-braces. A check digit sees a field
/// only mod 10 and the composite sees the same characters, so a single
/// destroyed position admits an entire residue class that *both* digits
/// accept — four readings on the card this was measured against. Three of them
/// are not dates. Without this filter the pass would have to pick one, and a
/// wrong pick is indistinguishable from a proof.
fn accept_damaged(data: &MrzData) -> bool {
    data.valid()
        && data
            .validity(crate::Date::new(2000, 1, 1))
            .dates_well_formed
}

/// Try to recover a record in which exactly one line arrived too narrow.
///
/// Only one line is damaged in the captures this exists for (a punched hole, a
/// finger over one end), so each shape below restores a single line and
/// crosses it against the ordinary variants of the others — linear in the
/// number of restored candidates rather than a product of three searches.
///
/// Returns `Some` only when the surviving readings all agree on the fields
/// that matter. Several genuinely different readings means the MRZ cannot
/// distinguish them, and the honest answer is the ordinary checksum-failed
/// fallback, not the first candidate off the list.
fn damaged_pass(lines: &[&str], opts: &ParseOptions) -> Option<MrzData> {
    let mut budget = MAX_DAMAGED_ATTEMPTS;
    let mut hits: Vec<MrzData> = Vec::new();

    let record = |data: MrzData, hits: &mut Vec<MrzData>| {
        if accept_damaged(&data) && !hits.iter().any(|h| h.mrz_lines == data.mrz_lines) {
            hits.push(data);
        }
    };

    // TD1: three consecutive lines, any one of them narrow.
    for i in 0..lines.len().saturating_sub(2) {
        let (a, b, c) = (lines[i], lines[i + 1], lines[i + 2]);
        let shapes: [(Vec<String>, Vec<String>, Vec<String>); 3] = [
            (
                restored(a, 30, repair_td1_line1),
                variants(b, 30, repair_td1_line2),
                variants(c, 30, repair_td1_line3),
            ),
            (
                variants(a, 30, repair_td1_line1),
                restored(b, 30, repair_td1_line2),
                variants(c, 30, repair_td1_line3),
            ),
            (
                variants(a, 30, repair_td1_line1),
                variants(b, 30, repair_td1_line2),
                restored(c, 30, repair_td1_line3),
            ),
        ];
        for (v1, v2, v3) in shapes {
            for l1 in &v1 {
                if !matches!(l1.as_bytes().first(), Some(b'I' | b'A' | b'C')) {
                    continue;
                }
                for l2 in &v2 {
                    for l3 in &v3 {
                        if budget == 0 {
                            return single(hits);
                        }
                        budget -= 1;
                        if let Ok(data) = parse_td1_with(l1, l2, l3, opts) {
                            record(data, &mut hits);
                        }
                    }
                }
            }
        }
    }

    // Two-line formats: TD3/MRV-A at 44, TD2/MRV-B at 36. Each entry is the
    // line-1 prefix that identifies the format plus its parse function, so a
    // new two-line format is one row rather than another nested loop.
    let two_line: [TwoLineFormat; 4] = [
        (44, b"P", repair_td3_line1, repair_td3_line2, parse_td3_with),
        (
            44,
            b"V",
            repair_mrv_a_line1,
            repair_mrv_a_line2,
            parse_mrv_a_with,
        ),
        (
            36,
            b"IAC",
            repair_td2_line1,
            repair_td2_line2,
            parse_td2_with,
        ),
        (
            36,
            b"V",
            repair_mrv_b_line1,
            repair_mrv_b_line2,
            parse_mrv_b_with,
        ),
    ];
    for i in 0..lines.len().saturating_sub(1) {
        let (a, b) = (lines[i], lines[i + 1]);
        for (width, prefixes, rep1, rep2, parse) in two_line {
            for (v1, v2) in [
                (restored(a, width, rep1), variants(b, width, rep2)),
                (variants(a, width, rep1), restored(b, width, rep2)),
            ] {
                for l1 in &v1 {
                    if !l1.bytes().next().is_some_and(|c| prefixes.contains(&c)) {
                        continue;
                    }
                    for l2 in &v2 {
                        if budget == 0 {
                            return single(hits);
                        }
                        budget -= 1;
                        if let Ok(data) = parse(l1, l2, opts) {
                            record(data, &mut hits);
                        }
                    }
                }
            }
        }
    }

    single(hits)
}

/// The one recovered reading, or `None` if the damage left more than one
/// record standing. Readings that differ only in their raw zone but agree on
/// every extracted field are the same answer reached twice (two insertion
/// points inside one filler run, say) and count as one.
fn single(mut hits: Vec<MrzData>) -> Option<MrzData> {
    let first = hits.first()?.clone();
    let same = |a: &MrzData, b: &MrzData| {
        a.document_number == b.document_number
            && a.date_of_birth == b.date_of_birth
            && a.date_of_expiry == b.date_of_expiry
            && a.surname == b.surname
            && a.given_names == b.given_names
            && a.nationality == b.nationality
    };
    if hits.iter().all(|h| same(h, &first)) {
        return Some(hits.remove(0));
    }
    None
}
