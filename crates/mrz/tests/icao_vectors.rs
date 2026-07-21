//! Check-digit vectors pinned from ICAO Doc 9303's own worked examples.
//!
//! The specimen tests in `src/lib.rs` prove whole-document parses; this file
//! proves the check-digit primitive against values the standard *publishes*,
//! so a prospective user can see the arithmetic matches ICAO exactly rather
//! than only matching this crate's own fixtures.
//!
//! Each expected digit below is independently verifiable by hand under the
//! 7-3-1 weighting (`0-9 → 0-9`, `A-Z → 10-35`, `< → 0`); the computation is
//! shown for the two Part 3 examples in the comments.

use mrz::{check_digit, verify};

/// `(field, expected_check_digit, source)` — every entry is a value ICAO 9303
/// documents, not one derived from this crate.
const VECTORS: &[(&str, u32, &str)] = &[
    // ICAO 9303 Part 3, check-digit worked examples.
    //   5·7 + 2·3 + 0·1 + 7·7 + 2·3 + 7·1 = 35+6+0+49+6+7 = 103 → 103 % 10 = 3
    ("520727", 3, "9303 pt3 numeric example"),
    //   A·7 + B·3 + 2·1 + 1·7 + 3·3 + 4·1 = 70+33+2+7+9+4 = 125 → 125 % 10 = 5
    ("AB2134<<<", 5, "9303 pt3 alphanumeric+filler example"),
    // ICAO 9303 Part 4, TD3 specimen (Utopia / Anna Maria Eriksson) field
    // check digits, as printed on the specimen's line 2.
    ("L898902C3", 6, "9303 pt4 TD3 specimen document number"),
    ("740812", 2, "9303 pt4 TD3 specimen date of birth"),
    ("120415", 9, "9303 pt4 TD3 specimen date of expiry"),
    ("ZE184226B<<<<<", 1, "9303 pt4 TD3 specimen personal number"),
];

#[test]
fn check_digit_matches_icao_worked_examples() {
    for &(field, expected, source) in VECTORS {
        let got = check_digit(field).unwrap_or_else(|e| panic!("{source}: {field:?} errored: {e}"));
        assert_eq!(got, expected, "{source}: check_digit({field:?})");
    }
}

#[test]
fn verify_accepts_the_published_digit_and_rejects_others() {
    for &(field, expected, source) in VECTORS {
        let good = char::from_digit(expected, 10).unwrap();
        assert!(
            verify(field, good),
            "{source}: verify({field:?}, {good:?}) should accept the published digit"
        );
        let wrong = char::from_digit((expected + 1) % 10, 10).unwrap();
        assert!(
            !verify(field, wrong),
            "{source}: verify({field:?}, {wrong:?}) should reject a wrong digit"
        );
    }
}
