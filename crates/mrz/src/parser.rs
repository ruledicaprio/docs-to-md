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
use crate::dates::expand_date;
use crate::{Checks, Format, MrzData, MrzError};

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

/// Parse a TD3 (passport) MRZ: two lines of exactly 44 characters.
pub fn parse_td3(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
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
    let personal = personal_raw.trim_end_matches('<');

    let checks = Checks {
        document_number: verify(&line2[0..9], line2.as_bytes()[9] as char),
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
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date(&line2[13..19], true),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date(&line2[21..27], false),
        personal_number: if personal.is_empty() {
            None
        } else {
            Some(personal.to_string())
        },
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse a TD2 MRZ: two lines of exactly 36 characters (ICAO 9303 part 6).
/// Covers identity-card document codes (`I`/`A`/`C`); MRV-B visas share the
/// geometry but lack a composite check digit and are not handled here.
pub fn parse_td2(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
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
    let optional = line2[28..35].trim_end_matches('<');

    let checks = Checks {
        document_number: verify(&line2[0..9], line2.as_bytes()[9] as char),
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
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date(&line2[13..19], true),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date(&line2[21..27], false),
        personal_number: if optional.is_empty() {
            None
        } else {
            Some(optional.to_string())
        },
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse a TD1 (ID card) MRZ: three lines of exactly 30 characters.
pub fn parse_td1(line1: &str, line2: &str, line3: &str) -> Result<MrzData, MrzError> {
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

    let optional1 = line1[15..30].trim_end_matches('<');
    let optional2 = line2[18..29].trim_end_matches('<');
    let personal = [optional1, optional2]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    let checks = Checks {
        document_number: verify(&line1[5..14], line1.as_bytes()[14] as char),
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
        surname,
        given_names,
        nationality: line2[15..18].trim_end_matches('<').to_string(),
        date_of_birth: expand_date(&line2[0..6], true),
        sex: clean_sex(line2.as_bytes()[7] as char),
        date_of_expiry: expand_date(&line2[8..14], false),
        personal_number: if personal.is_empty() {
            None
        } else {
            Some(personal)
        },
        mrz_lines: format!("{line1}\n{line2}\n{line3}"),
        checks,
    })
}

/// Parse an MRV-A machine readable visa: two lines of exactly 44 characters
/// (ICAO 9303 part 7). Geometry mirrors TD3 through the expiry check digit,
/// but there is no personal-number field and no composite check digit.
pub fn parse_mrv_a(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
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
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date(&line2[13..19], true),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date(&line2[21..27], false),
        personal_number: if optional.is_empty() {
            None
        } else {
            Some(optional.to_string())
        },
        mrz_lines: format!("{line1}\n{line2}"),
        checks,
    })
}

/// Parse an MRV-B machine readable visa: two lines of exactly 36 characters
/// (ICAO 9303 part 7). Geometry mirrors TD2 through the expiry check digit,
/// but there is no personal-number field and no composite check digit.
pub fn parse_mrv_b(line1: &str, line2: &str) -> Result<MrzData, MrzError> {
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
        surname,
        given_names,
        nationality: line2[10..13].trim_end_matches('<').to_string(),
        date_of_birth: expand_date(&line2[13..19], true),
        sex: clean_sex(line2.as_bytes()[20] as char),
        date_of_expiry: expand_date(&line2[21..27], false),
        personal_number: if optional.is_empty() {
            None
        } else {
            Some(optional.to_string())
        },
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
pub fn find_and_parse(text: &str) -> Result<MrzData, MrzError> {
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

    // First parseable-but-checksum-failed hit, reported when nothing better
    // is found so callers can show which check digits failed.
    let mut fallback: Option<MrzData> = None;
    let mut consider = |data: MrzData| -> Option<MrzData> {
        if data.valid() {
            return Some(data);
        }
        fallback.get_or_insert(data);
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
                    if let Ok(data) = parse_td3(&l1, &l2) {
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
                    if let Ok(data) = parse_td3(&l1, &l2) {
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
                    if let Ok(data) = parse_mrv_b(&l1, &l2) {
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
                    if let Ok(data) = parse_mrv_b(&l1, &l2) {
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
                    if let Ok(data) = parse_mrv_a(&l1, &l2) {
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
                    if let Ok(data) = parse_mrv_a(&l1, &l2) {
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
                    if let Ok(data) = parse_td1(&l1, &l2, &l3) {
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
                    if let Ok(data) = parse_td2(&l1, &l2) {
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
                if let Ok(data) = parse_td2(&l1, &l2) {
                    if let Some(valid) = consider(data) {
                        return Ok(valid);
                    }
                }
            }
        }
    }

    fallback.ok_or(MrzError::NotFound)
}
