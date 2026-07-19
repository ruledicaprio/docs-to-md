//! Property-based "never panics on arbitrary input" regression suite.
//!
//! `mrz` is a zero-dependency parser that does raw byte-index surgery in its
//! checksum-verified OCR repair (reinserting dropped fillers, padding short
//! lines) and also compiles to WASM for the public browser demo — a panic
//! here is a real remote DoS in two places at once. This suite runs under
//! `cargo test --workspace` on every PR (both the Linux and macOS CI jobs),
//! giving an always-on regression net. Deeper coverage-guided fuzzing lives
//! in `fuzz/` (opt-in, `cargo fuzz run ...`).
//!
//! These properties assert only that the parser returns `Ok`/`Err` and never
//! panics — they say nothing about correctness (the existing unit tests in
//! `src/lib.rs`/`src/parser.rs` already cover that against real specimens).

use mrz::{find_and_parse, parse_td1, parse_td2, parse_td3};
use proptest::prelude::*;

/// The real MRZ charset plus the OCR-noise characters this crate's own unit
/// tests are seeded with (`«`, lowercase, HTML-escaped fillers, stray
/// spaces) — biases the generator toward inputs that actually exercise the
/// repair heuristics instead of bouncing off the character-set check.
fn mrz_ish_char() -> impl Strategy<Value = char> {
    prop_oneof![
        // Valid MRZ charset: the common case, filler weighted heavily since
        // it's what most of a real MRZ line actually consists of.
        8 => prop::char::range('A', 'Z'),
        8 => prop::char::range('0', '9'),
        12 => Just('<'),
        // OCR-noise lookalikes the repair heuristics target.
        3 => prop::char::range('a', 'z'),
        2 => Just('«'),
        2 => Just(' '),
        1 => Just('&'),
        1 => Just(';'),
        1 => Just('l'),
        1 => Just('K'),
        1 => any::<char>(),
    ]
}

/// A string of `mrz_ish_char`s at roughly the given length (± a few, so both
/// exact-length and length-mismatch paths get exercised).
fn near_length_string(target: usize) -> impl Strategy<Value = String> {
    let lo = target.saturating_sub(3);
    let hi = target + 3;
    prop::collection::vec(mrz_ish_char(), lo..=hi).prop_map(|chars| chars.into_iter().collect())
}

/// Any string at all — the most adversarial case for `find_and_parse`, which
/// must scan free text for MRZ-shaped lines.
fn arbitrary_text() -> impl Strategy<Value = String> {
    prop::collection::vec(mrz_ish_char(), 0..200).prop_map(|chars| chars.into_iter().collect())
}

/// Mutates one of the crate's own valid specimen lines at a single random
/// position — reaches the repair heuristics (which assume "almost valid"
/// input) far more often than pure-random strings do.
fn mutated_specimen() -> impl Strategy<Value = String> {
    let specimens = [
        "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<".to_string(),
        "L898902C36UTO7408122F1204159ZE184226B<<<<<10".to_string(),
        "I<UTOD231458907<<<<<<<<<<<<<<<".to_string(),
        "7408122F1204159UTO<<<<<<<<<<<6".to_string(),
        "ERIKSSON<<ANNA<MARIA<<<<<<<<<<".to_string(),
        "I<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<".to_string(),
        "D231458907UTO7408122F1204159<<<<<<<6".to_string(),
    ];
    (0..specimens.len()).prop_flat_map(move |i| {
        let base = specimens[i].clone();
        let len = base.len();
        (Just(base), 0..len.max(1), mrz_ish_char()).prop_map(|(mut s, pos, c)| {
            if pos < s.len() {
                let mut chars: Vec<char> = s.chars().collect();
                chars[pos] = c;
                s = chars.into_iter().collect();
            }
            s
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// `find_and_parse` must never panic on arbitrary free text, however
    /// garbled — it always returns `Ok`/`Err`.
    #[test]
    fn find_and_parse_never_panics_on_arbitrary_text(text in arbitrary_text()) {
        let _ = find_and_parse(&text);
    }

    /// Same, but over multi-line text combining several arbitrary strings —
    /// closer to real OCR Markdown output than a single line.
    #[test]
    fn find_and_parse_never_panics_on_multiline_text(
        lines in prop::collection::vec(arbitrary_text(), 0..6)
    ) {
        let text = lines.join("\n");
        let _ = find_and_parse(&text);
    }

    /// `find_and_parse` over a mutated real specimen — reaches the OCR-repair
    /// machinery (dropped/extra fillers, lookalike digits) far more often.
    #[test]
    fn find_and_parse_never_panics_on_mutated_specimen(
        lines in prop::collection::vec(mutated_specimen(), 1..4)
    ) {
        let text = lines.join("\n");
        let _ = find_and_parse(&text);
    }

    /// `parse_td3` must never panic regardless of line length or content.
    #[test]
    fn parse_td3_never_panics(l1 in near_length_string(44), l2 in near_length_string(44)) {
        let _ = parse_td3(&l1, &l2);
    }

    /// `parse_td2` must never panic regardless of line length or content.
    #[test]
    fn parse_td2_never_panics(l1 in near_length_string(36), l2 in near_length_string(36)) {
        let _ = parse_td2(&l1, &l2);
    }

    /// `parse_td1` must never panic regardless of line length or content.
    #[test]
    fn parse_td1_never_panics(
        l1 in near_length_string(30),
        l2 in near_length_string(30),
        l3 in near_length_string(30),
    ) {
        let _ = parse_td1(&l1, &l2, &l3);
    }

    /// Exact-length (no length-mismatch early return) inputs over the full
    /// MRZ-ish charset — the case most likely to reach deep index math.
    #[test]
    fn parse_td3_never_panics_exact_length(
        l1 in prop::collection::vec(mrz_ish_char(), 44).prop_map(|c| c.into_iter().collect::<String>()),
        l2 in prop::collection::vec(mrz_ish_char(), 44).prop_map(|c| c.into_iter().collect::<String>()),
    ) {
        let _ = parse_td3(&l1, &l2);
    }
}
