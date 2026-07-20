//! ICAO 9303 check-digit math and generic OCR-repair primitives.
//!
//! The check digit is deterministic proof of a faithful read; the repair
//! helpers generate candidate readings of noisy OCR whose correctness is then
//! *proven* (or rejected) by that same math — see [`crate::find_and_parse`].

use crate::MrzError;

/// ICAO 9303 character value: `0-9 → 0-9`, `A-Z → 10-35`, `< → 0`.
pub(crate) fn char_value(c: char) -> Result<u32, MrzError> {
    match c {
        '0'..='9' => Ok(c as u32 - '0' as u32),
        'A'..='Z' => Ok(c as u32 - 'A' as u32 + 10),
        '<' => Ok(0),
        other => Err(MrzError::BadCharacter(other)),
    }
}

/// Compute the ICAO 9303 check digit (7-3-1 repeating weights, mod 10).
pub fn check_digit(field: &str) -> Result<u32, MrzError> {
    const WEIGHTS: [u32; 3] = [7, 3, 1];
    let mut sum = 0u32;
    for (i, c) in field.chars().enumerate() {
        sum += char_value(c)? * WEIGHTS[i % 3];
    }
    Ok(sum % 10)
}

/// Verify `field` against its printed check digit character.
/// A `<` check digit counts as 0 (used for empty optional fields).
pub fn verify(field: &str, digit: char) -> bool {
    match (check_digit(field), char_value(digit)) {
        (Ok(expected), Ok(got)) => expected == got && (digit.is_ascii_digit() || digit == '<'),
        _ => false,
    }
}

/// Normalize one line of OCR output toward the MRZ charset:
/// uppercase, strip whitespace, map common OCR confusions of the filler —
/// `«`/`≪` are how OCR typically renders the double filler `<<`, while `‹`
/// is a single `<`.
pub(crate) fn normalize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars().filter(|c| !c.is_whitespace()) {
        match c {
            '«' | '≪' => out.push_str("<<"),
            '‹' => out.push('<'),
            c => out.push(c.to_ascii_uppercase()),
        }
    }
    out
}

pub(crate) fn is_mrz_charset(s: &str) -> bool {
    s.chars().all(|c| matches!(c, 'A'..='Z' | '0'..='9' | '<'))
}

/// OCR lookalike corrections for positions that must be digits.
pub(crate) fn digitize(c: char) -> char {
    match c {
        'O' | 'Q' | 'D' => '0',
        'I' | 'L' => '1',
        'Z' => '2',
        'S' => '5',
        'G' => '6',
        'B' => '8',
        c => c,
    }
}

/// OCR lookalike corrections for positions that must be letters (or filler).
pub(crate) fn letterize(c: char) -> char {
    match c {
        '0' => 'O',
        '1' => 'I',
        '2' => 'Z',
        '5' => 'S',
        '6' => 'G',
        '8' => 'B',
        c => c,
    }
}

/// Replace runs of ≥ `min_run` consecutive `K`/`L` characters with fillers —
/// OCR persistently misreads the `<` filler as K or L, and no transliterated
/// ICAO name contains four K/L in a row.
pub(crate) fn defiller(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'K' | b'L') {
            let mut j = i;
            while j < bytes.len() && matches!(bytes[j], b'K' | b'L' | b'<') {
                j += 1;
            }
            let kl = bytes[i..j]
                .iter()
                .filter(|b| matches!(b, b'K' | b'L'))
                .count();
            if j - i >= 4 && kl >= 3 {
                out.extend(std::iter::repeat_n('<', j - i));
            } else {
                out.push_str(&s[i..j]);
            }
            i = j;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// One per-index repair rule: apply `fix` to any character at an index inside `range`.
pub(crate) type RepairRule = (core::ops::Range<usize>, fn(char) -> char);

/// Apply a per-index repair map: `spec` lists `(range, fix)` pairs.
pub(crate) fn repair_positions(line: &str, spec: &[RepairRule]) -> String {
    line.chars()
        .enumerate()
        .map(|(i, c)| {
            if c == '<' {
                return c; // fillers are always legitimate
            }
            for (range, fix) in spec {
                if range.contains(&i) {
                    return fix(c);
                }
            }
            c
        })
        .collect()
}

/// Fix a document-code second character misread: no ICAO document code has
/// `K` there, but OCR reads the `<` filler as `K` constantly.
pub(crate) fn fix_doc_code(l: &str) -> String {
    if l.as_bytes().get(1) == Some(&b'K') {
        format!("{}<{}", &l[0..1], &l[2..])
    } else {
        l.to_string()
    }
}

/// MRZ name fields separate surname from given names with `<<`. When a name
/// field has no `<<` at all but contains `KK`, that pair is a misread
/// separator. Fields that already contain a real `<<` (e.g. MIKKO<<HEIKKI)
/// are left untouched.
pub(crate) fn fix_name_separator(s: &str) -> String {
    let trimmed = s.trim_end_matches('<');
    if !trimmed.contains("<<") {
        if let Some(pos) = trimmed.find("KK") {
            return format!("{}<<{}", &s[..pos], &s[pos + 2..]);
        }
    }
    s.to_string()
}

/// Last-resort variant: any `K`/`L` touching a `<` becomes `<` (to fixpoint).
/// Only ever accepted when the composite check digit validates the result.
pub(crate) fn aggressive_defiller(s: &str) -> String {
    let mut b: Vec<u8> = s.bytes().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..b.len() {
            if matches!(b[i], b'K' | b'L')
                && ((i > 0 && b[i - 1] == b'<') || (i + 1 < b.len() && b[i + 1] == b'<'))
            {
                b[i] = b'<';
                changed = true;
            }
        }
    }
    String::from_utf8(b).expect("ascii in, ascii out")
}

/// Longest run of consecutive `<` fillers: `(start, len)`.
fn longest_filler_run(s: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'<' {
                i += 1;
            }
            if best.is_none_or(|(_, l)| i - start > l) {
                best = Some((start, i - start));
            }
        } else {
            i += 1;
        }
    }
    best
}

/// Candidate alignments of a wrong-length line. OCR drops or hallucinates
/// characters most often inside filler runs, so besides trimming/padding at
/// the ends we also inflate/deflate the longest `<` run.
fn fit_length(n: &str, target: usize) -> Vec<String> {
    use core::cmp::Ordering;
    match n.len().cmp(&target) {
        Ordering::Equal => vec![n.to_string()],
        Ordering::Less => {
            let missing = target - n.len();
            let mut v = Vec::new();
            if let Some((start, len)) = longest_filler_run(n) {
                let mut s = String::with_capacity(target);
                s.push_str(&n[..start + len]);
                s.extend(core::iter::repeat_n('<', missing));
                s.push_str(&n[start + len..]);
                v.push(s);
            }
            let mut padded = n.to_string();
            padded.extend(core::iter::repeat_n('<', missing));
            v.push(padded);
            v
        }
        Ordering::Greater => {
            let extra = n.len() - target;
            let mut v = Vec::new();
            if let Some((start, len)) = longest_filler_run(n) {
                if len > extra {
                    let mut s = String::with_capacity(target);
                    s.push_str(&n[..start]);
                    s.extend(core::iter::repeat_n('<', len - extra));
                    s.push_str(&n[start + len..]);
                    v.push(s);
                }
            }
            v.push(n[0..target].to_string());
            v.push(n[n.len() - target..].to_string());
            v
        }
    }
}

/// Candidate readings of one OCR line for a given MRZ line length:
/// normalized, length-adjusted, each both verbatim and lookalike-repaired.
/// The check digits decide which variant — if any — is the true read.
pub(crate) fn variants(raw: &str, target: usize, repair: fn(&str) -> String) -> Vec<String> {
    let n = normalize_line(raw);
    // OCR drops trailing fillers wholesale — ocrs truncates a TD3 name line's
    // filler run by 9+ characters on low-resolution scans — so tolerate short
    // lines generously (they get padded back); hallucinated extra characters
    // are rarer. Every padded candidate still has to prove itself against the
    // check digits, so a wider net costs candidates, not correctness.
    if n.len() + 14 < target || n.len() > target + 4 || !is_mrz_charset(&n) {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for fitted in fit_length(&n, target) {
        let repaired = repair(&fitted);
        let last_resort = aggressive_defiller(&repaired);
        for form in [repaired, fitted, last_resort] {
            if !out.contains(&form) {
                out.push(form);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_digit_icao_vectors() {
        assert_eq!(check_digit("L898902C3").unwrap(), 6);
        assert_eq!(check_digit("740812").unwrap(), 2);
        assert_eq!(check_digit("120415").unwrap(), 9);
        assert_eq!(check_digit("ZE184226B<<<<<").unwrap(), 1);
    }

    #[test]
    fn name_separator_fix_spares_real_kk_names() {
        // MIKKONEN<<HEIKKI already contains a real `<<` separator — the KK
        // pairs inside the names must survive.
        assert_eq!(
            fix_name_separator("MIKKONEN<<HEIKKI<<<<<<"),
            "MIKKONEN<<HEIKKI<<<<<<"
        );
        // No separator at all + a KK pair → it was the separator.
        assert_eq!(fix_name_separator("VZORECKKJANA<<<"), "VZOREC<<JANA<<<");
    }
}
