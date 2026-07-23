//! Candidate generation for MRZ lines damaged badly enough that characters are
//! *missing*, not merely misread.
//!
//! The check digits are the oracle everywhere in this crate; nothing here
//! decides what a character is. This module only widens the set of candidates
//! the oracle gets to rule on, and reports honestly when the oracle cannot
//! separate them.
//!
//! # The gap this closes
//!
//! [`crate::checksum`]'s existing length repair inserts missing characters in
//! exactly two places: inside the longest `<` filler run, or appended at the
//! end. That covers the common OCR failure — a truncated trailing filler run —
//! and nothing else. Two failures measured on real documents fall outside it:
//!
//! - A hole punched through an ID card's MRZ. `ocrs` **drops** the destroyed
//!   glyph rather than emitting a placeholder, so a TD1 line 2 arrives 29
//!   characters wide with the deficit in the middle of the expiry field.
//! - A finger over the start of a passport's line 2, arriving 42 characters
//!   wide with the deficit at the front.
//!
//! In both cases every candidate the old repair builds still has the data
//! shifted, so every check digit correctly rejects all of them, and a
//! recoverable document falls through to a non-deterministic fallback.
//! [`width_candidates`] restores the width by inserting [`UNKNOWN`] at *every*
//! position, and [`solve_field`] resolves those unknowns against the field's
//! own check digit.
//!
//! # What "resolved" is allowed to mean
//!
//! A check digit sees only the value of a field mod 10 (ICAO 9303 part 3
//! §4.9), so a single unknown position generally admits **four** characters —
//! one residue class from [`crate::CLASSES`]. On the punched ID card the four
//! are `3`, `D`, `N`, `X`. Three of them make the expiry field `D01230`,
//! `N01230`, `X01230`, which are not dates: [`FieldKind::Date`] prunes them
//! with [`crate::Date::is_well_formed`] and the recovery is unique.
//!
//! Where the arithmetic genuinely cannot separate the candidates —
//! the finger-occluded passport leaves 138 surviving pairs — the answer is
//! [`Resolution::Ambiguous`], never a pick. A guess that happens to be wrong
//! is worse than a refusal, because it is indistinguishable from a proof.

use crate::checksum::{fit_length, is_mrz_charset, verify};
use crate::dates::Date;

/// Marks a position the recognizer could not read. Deliberately **not** `<`:
/// the filler is a legitimate MRZ value (0), so reusing it would make "no
/// character here" indistinguishable from "the character here is a filler".
/// Outside the ICAO alphabet by design, so a string still carrying one can
/// never be mistaken for a parseable line.
pub const UNKNOWN: char = '?';

/// The 37-character ICAO 9303 MRZ alphabet, in the order the solver sweeps it.
pub const MRZ_ALPHABET: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ<";

/// Most [`UNKNOWN`] positions [`solve_field`] will sweep in one field.
///
/// Two is 37² = 1369 check-digit verifications — trivial — and three would be
/// 50653 candidates of which a useful fraction survive, i.e. an `Ambiguous`
/// answer so wide it carries no information. The bound exists for the same
/// reason [`crate::checksum`]'s defiller has one: this runs inside an OCR
/// retry loop, and an unbounded sweep there is a latency bug waiting for a
/// bad photo.
const MAX_UNKNOWNS: usize = 2;

/// Most characters [`width_candidates`] will insert. Past this the position
/// sweep stops being a repair and starts being a search over lines that were
/// never read in the first place.
const MAX_WIDTH_DEFICIT: usize = 2;

/// Which ICAO field a [`solve_field`] call is resolving. Selects the
/// structural constraint applied *after* the check digit, never instead of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldKind {
    /// Left-justified, `<`-padded (ICAO 9303 part 4 §4.2.2): a filler may only
    /// appear in the trailing run, so `B<98730<` is not a document number
    /// however well its check digit verifies.
    DocumentNumber,
    /// `YYMMDD`. Must be six digits naming a real calendar date.
    Date,
    /// Left-justified and `<`-padded like [`FieldKind::DocumentNumber`].
    PersonalNumber,
    /// No constraint beyond the check digit and the MRZ alphabet.
    Other,
}

/// What the check digit could prove about a field carrying [`UNKNOWN`]s.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// Exactly one reading satisfies the check digit and the field's
    /// structural constraint. This is a proof, not a preference.
    Unique(String),
    /// Several readings satisfy both and nothing in the MRZ can separate them
    /// — the check digit's blindspot, made explicit. Sorted, so the output is
    /// stable across runs.
    Ambiguous { candidates: Vec<String> },
    /// No reading satisfies the check digit, or the input was outside what
    /// this solver will attempt (too many unknowns, an unreadable check digit,
    /// non-ASCII input).
    Unresolvable,
}

impl Resolution {
    /// The proven reading, if there is exactly one. `Ambiguous` deliberately
    /// yields `None` — a caller that wants "the first candidate" has to reach
    /// into the variant and own that decision explicitly.
    pub fn unique(&self) -> Option<&str> {
        match self {
            Resolution::Unique(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// Every way `line` could be restored to exactly `target` characters.
///
/// Combines [`crate::checksum`]'s existing filler-run and end-padding
/// candidates with a sweep that inserts [`UNKNOWN`] at each of the `len + 1`
/// positions — the case a punched hole or an occluded line start produces.
/// Candidates containing [`UNKNOWN`] are not parseable as they stand; resolve
/// them field-by-field with [`solve_field`] first.
///
/// Bounded and deterministic: at most `target + 4` candidates, each exactly
/// `target` characters, in a stable order. Returns empty for non-ASCII input
/// or a deficit wider than the module's insertion bound.
pub fn width_candidates(line: &str, target: usize) -> Vec<String> {
    let n: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    if !n.is_ascii() || target == 0 {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();

    // The pre-existing behaviour first, so a line this module could already
    // handle keeps resolving through the same candidate it always did.
    for candidate in fit_length(&n, target) {
        push_unique(&mut out, candidate, target);
    }

    let len = n.len();
    if len < target {
        let deficit = target - len;
        if deficit <= MAX_WIDTH_DEFICIT {
            let bytes = n.as_bytes();
            for at in 0..=len {
                let mut s = String::with_capacity(target);
                s.push_str(&n[..at]);
                for _ in 0..deficit {
                    s.push(UNKNOWN);
                }
                s.push_str(&n[at..]);
                debug_assert_eq!(bytes.len() + deficit, s.len());
                push_unique(&mut out, s, target);
            }
        }
    }
    out
}

/// Append `candidate` if it is exactly `target` characters and not already
/// present — the de-duplication [`width_candidates`] needs because the
/// existing filler-run repair and the position sweep overlap whenever the
/// deficit happens to sit inside a filler run.
fn push_unique(out: &mut Vec<String>, candidate: String, target: usize) {
    if candidate.len() == target && !out.contains(&candidate) {
        out.push(candidate);
    }
}

/// Resolve the [`UNKNOWN`] positions in one check-digited field.
///
/// Sweeps [`MRZ_ALPHABET`] over every unknown position and keeps the readings
/// whose own check digit verifies *and* which satisfy `kind`'s structural
/// constraint. A field with no unknowns is simply verified.
///
/// Returns [`Resolution::Unresolvable`] rather than attempting a solve when
/// `check` is itself [`UNKNOWN`]: a check digit that was not read cannot prove
/// anything, and recomputing it from the candidate would only prove the
/// candidate agrees with itself.
pub fn solve_field(field: &str, check: char, kind: FieldKind) -> Resolution {
    if !field.is_ascii() || check == UNKNOWN {
        return Resolution::Unresolvable;
    }
    let chars: Vec<char> = field.chars().collect();
    let unknowns: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == UNKNOWN)
        .map(|(i, _)| i)
        .collect();
    if unknowns.len() > MAX_UNKNOWNS {
        return Resolution::Unresolvable;
    }
    // Anything that is neither a known MRZ character nor an explicit unknown
    // is corruption this module has no business guessing around.
    if chars
        .iter()
        .any(|c| *c != UNKNOWN && !is_mrz_charset(&c.to_string()))
    {
        return Resolution::Unresolvable;
    }

    let alphabet: Vec<char> = MRZ_ALPHABET.chars().collect();
    let mut hits: Vec<String> = Vec::new();
    let total = alphabet.len().pow(unknowns.len() as u32);
    for combo in 0..total {
        let mut candidate = chars.clone();
        let mut rest = combo;
        for &pos in &unknowns {
            candidate[pos] = alphabet[rest % alphabet.len()];
            rest /= alphabet.len();
        }
        let s: String = candidate.into_iter().collect();
        if verify(&s, check) && satisfies(&s, kind) {
            hits.push(s);
        }
    }

    hits.sort();
    match hits.len() {
        0 => Resolution::Unresolvable,
        1 => Resolution::Unique(hits.remove(0)),
        _ => Resolution::Ambiguous { candidates: hits },
    }
}

/// Every concrete reading of a line still carrying [`UNKNOWN`]s, for callers
/// that would rather let a whole-record parse be the oracle than resolve field
/// by field.
///
/// Bounded by [`MAX_UNKNOWNS`] exactly as [`solve_field`] is — a line with more
/// unknowns than that yields nothing rather than a combinatorial expansion.
/// A line with no unknowns yields itself, so this is safe to call
/// unconditionally.
pub(crate) fn concrete_fillings(line: &str) -> Vec<String> {
    if !line.is_ascii() {
        return Vec::new();
    }
    let chars: Vec<char> = line.chars().collect();
    let unknowns: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == UNKNOWN)
        .map(|(i, _)| i)
        .collect();
    if unknowns.is_empty() {
        return vec![line.to_string()];
    }
    if unknowns.len() > MAX_UNKNOWNS {
        return Vec::new();
    }
    let alphabet: Vec<char> = MRZ_ALPHABET.chars().collect();
    let mut out = Vec::with_capacity(alphabet.len().pow(unknowns.len() as u32));
    for combo in 0..alphabet.len().pow(unknowns.len() as u32) {
        let mut candidate = chars.clone();
        let mut rest = combo;
        for &pos in &unknowns {
            candidate[pos] = alphabet[rest % alphabet.len()];
            rest /= alphabet.len();
        }
        out.push(candidate.into_iter().collect());
    }
    out
}

/// The structural constraint for `kind` — applied only to readings the check
/// digit has already accepted, so it can never rescue a value the arithmetic
/// rejected, only narrow a set the arithmetic could not separate.
fn satisfies(field: &str, kind: FieldKind) -> bool {
    match kind {
        FieldKind::Other => true,
        FieldKind::DocumentNumber | FieldKind::PersonalNumber => left_justified(field),
        FieldKind::Date => is_plausible_yymmdd(field),
    }
}

/// No `<` appears before a non-filler character: ICAO left-justifies these
/// fields and pads them on the right only.
fn left_justified(field: &str) -> bool {
    !field.trim_end_matches('<').contains('<')
}

/// Six digits naming a real calendar date.
///
/// The century is unknown at field level — the pivot lives in
/// [`crate::ParseOptions`] and belongs to the parser, not here — so a date is
/// accepted when it is well-formed under **either** century. That only matters
/// for `yy = 00` on 29 February (1900 is not a leap year, 2000 is), and
/// accepting the union is the conservative choice: this function's job is to
/// discard the impossible, never to narrow by guessing an era.
fn is_plausible_yymmdd(field: &str) -> bool {
    if field.len() != 6 || !field.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let yy: i32 = field[0..2].parse().expect("two ascii digits");
    let month: u32 = field[2..4].parse().expect("two ascii digits");
    let day: u32 = field[4..6].parse().expect("two ascii digits");
    Date::new(1900 + yy, month, day).is_well_formed()
        || Date::new(2000 + yy, month, day).is_well_formed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_candidates_are_all_target_width_and_deduplicated() {
        for target in [30usize, 36, 44] {
            let short: String = "A".repeat(target - 1);
            let candidates = width_candidates(&short, target);
            assert!(!candidates.is_empty());
            for c in &candidates {
                assert_eq!(c.len(), target, "candidate {c:?} is not {target} wide");
            }
            let mut sorted = candidates.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), candidates.len(), "duplicate candidates");
        }
    }

    #[test]
    fn width_candidates_sweep_every_insertion_point() {
        // A deficit of one in a line with no filler run at all: the old
        // filler-run repair has nothing to inflate, so every candidate here
        // comes from the position sweep.
        let candidates = width_candidates("ABCDE", 6);
        assert!(candidates.contains(&"?ABCDE".to_string()));
        assert!(candidates.contains(&"AB?CDE".to_string()));
        assert!(candidates.contains(&"ABCDE?".to_string()));
    }

    #[test]
    fn width_candidates_refuses_a_deficit_it_cannot_bound() {
        let candidates = width_candidates("AB", 30);
        assert!(
            candidates.iter().all(|c| !c.contains(UNKNOWN)),
            "a 28-character deficit must not be swept position by position"
        );
    }

    #[test]
    fn a_field_with_no_unknowns_is_just_verified() {
        // ICAO 9303 part 3 worked example.
        assert_eq!(
            solve_field("L898902C<", '3', FieldKind::DocumentNumber),
            Resolution::Unique("L898902C<".to_string())
        );
        assert_eq!(
            solve_field("L898902C<", '4', FieldKind::DocumentNumber),
            Resolution::Unresolvable
        );
    }

    #[test]
    fn an_unreadable_check_digit_proves_nothing() {
        assert_eq!(
            solve_field("6908?22", UNKNOWN, FieldKind::Other),
            Resolution::Unresolvable
        );
    }

    #[test]
    fn too_many_unknowns_is_refused_not_attempted() {
        assert_eq!(
            solve_field("???????", '0', FieldKind::Other),
            Resolution::Unresolvable
        );
    }

    #[test]
    fn left_justified_rejects_an_interior_filler() {
        assert!(left_justified("B1987309"));
        assert!(left_justified("B198730<"));
        assert!(!left_justified("B<987309"));
    }

    #[test]
    fn plausible_dates_reject_impossible_calendars() {
        assert!(!is_plausible_yymmdd("890229")); // 1989/2089 both non-leap
        assert!(is_plausible_yymmdd("000229")); // 2000 is a leap year
        assert!(is_plausible_yymmdd("301230"));
        assert!(!is_plausible_yymmdd("301332"));
        assert!(!is_plausible_yymmdd("D01230"));
        assert!(!is_plausible_yymmdd("30123"));
    }
}
