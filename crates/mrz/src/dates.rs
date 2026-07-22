//! Date handling: `YYMMDD` → ISO expansion, and non-cryptographic
//! plausibility checks (expiry vs. "today", DOB-before-expiry, well-formedness).
//!
//! A valid MRZ composite check digit proves a *faithful read* of the printed
//! zone — it says nothing about whether the document is in date or whether the
//! dates are internally consistent. Those judgements live here and take an
//! explicit reference date so the crate stays deterministic and clock-free
//! (the caller supplies "today").

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Expand `YYMMDD` to ISO `YYYY-MM-DD`.
///
/// Century heuristic: birth dates after the current two-digit year roll back
/// to 19xx; expiry dates are always 20xx (no valid travel document from the
/// 1900s remains in circulation).
pub fn expand_date(yymmdd: &str, is_birth: bool) -> String {
    // Two-digit year pivot for the 19xx/20xx decision on birth dates. Kept as a
    // single constant for auditability; callers wanting an explicit pivot can
    // use [`expand_date_with_pivot`].
    expand_date_with_pivot(yymmdd, is_birth, CURRENT_YY)
}

/// The default two-digit-year pivot (2026). Birth years greater than this map
/// to the 1900s.
pub const CURRENT_YY: u32 = 26;

/// Like [`expand_date`] but with a caller-supplied two-digit-year pivot.
pub fn expand_date_with_pivot(yymmdd: &str, is_birth: bool, pivot_yy: u32) -> String {
    if yymmdd.len() != 6 || !yymmdd.chars().all(|c| c.is_ascii_digit()) {
        return yymmdd.to_string(); // leave unparseable input untouched
    }
    let yy: u32 = yymmdd[0..2].parse().unwrap();
    let century = if is_birth && yy > pivot_yy {
        "19"
    } else {
        "20"
    };
    format!(
        "{century}{}-{}-{}",
        &yymmdd[0..2],
        &yymmdd[2..4],
        &yymmdd[4..6]
    )
}

/// Gregorian leap-year rule: divisible by 4, except century years, which must
/// also be divisible by 400 (so 2000 is a leap year but 1900 is not).
pub fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Number of days in `month` of `year`; `0` for an out-of-range `month`
/// (`0` or `> 12`) so callers get an always-false range rather than a panic.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// A simple proleptic-Gregorian calendar date. Used as the "today" reference
/// for [`crate::MrzData::validity`] and to measure days-until-expiry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn new(year: i32, month: u32, day: u32) -> Self {
        Self { year, month, day }
    }

    /// Month and day fall within the true calendar range for `year` — Feb 30,
    /// Feb 29 in a non-leap year, and April 31 are all rejected rather than
    /// silently accepted the way a generous 1..=31 day check would.
    pub fn is_well_formed(self) -> bool {
        (1..=12).contains(&self.month)
            && (1..=days_in_month(self.year, self.month)).contains(&self.day)
    }

    /// Days since the Unix epoch (1970-01-01), proleptic Gregorian.
    /// Howard Hinnant's `days_from_civil` — pure integer math, no_std-friendly.
    pub fn to_epoch_days(self) -> i64 {
        let y = if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        } as i64;
        let m = self.month as i64;
        let d = self.day as i64;
        let era = (if y >= 0 { y } else { y - 399 }) / 400;
        let yoe = y - era * 400; // [0, 399]
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
        era * 146097 + doe - 719468
    }

    /// Inverse of [`to_epoch_days`]: build a date from a Unix day number.
    /// Howard Hinnant's `civil_from_days`. Lets a caller turn a system clock
    /// (days since epoch) into a [`Date`] to use as "today".
    ///
    /// [`to_epoch_days`]: Date::to_epoch_days
    pub fn from_epoch_days(days: i64) -> Date {
        let z = days + 719468;
        let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
        let doe = z - era * 146097; // [0, 146096]
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
        let mp = (5 * doy + 2) / 153; // [0, 11]
        let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
        let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
        Date {
            year: (y + i64::from(m <= 2)) as i32,
            month: m as u32,
            day: d as u32,
        }
    }
}

/// Parse an ISO `YYYY-MM-DD` string into a well-formed [`Date`].
pub(crate) fn parse_iso(date: &str) -> Option<Date> {
    let b = date.as_bytes();
    if date.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let d = Date {
        year: date[0..4].parse().ok()?,
        month: date[5..7].parse().ok()?,
        day: date[8..10].parse().ok()?,
    };
    d.is_well_formed().then_some(d)
}

/// Date-plausibility summary for an MRZ, relative to a reference "today".
/// Distinct from the check digits: a checksum-valid MRZ can still be expired
/// or carry impossible dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DateValidity {
    /// Both `date_of_birth` and `date_of_expiry` parse as real calendar dates.
    pub dates_well_formed: bool,
    /// Date of expiry is on or after the reference "today".
    pub in_date: bool,
    /// Date of birth is strictly before the date of expiry.
    pub dob_before_expiry: bool,
    /// Whole days until expiry (negative if already expired), when the expiry
    /// date is well-formed.
    pub days_until_expiry: Option<i64>,
}

impl DateValidity {
    /// Dates are well-formed, internally consistent, and the document is in date.
    pub fn all_ok(&self) -> bool {
        self.dates_well_formed && self.in_date && self.dob_before_expiry
    }
}

impl crate::MrzData {
    /// Non-cryptographic date plausibility relative to `today`.
    ///
    /// A valid MRZ composite proves a faithful *read* of the printed zone, not
    /// that the document is in date or its dates are consistent — that separate
    /// judgement is computed here from the already-expanded ISO date fields.
    pub fn validity(&self, today: Date) -> DateValidity {
        let dob = parse_iso(&self.date_of_birth);
        let exp = parse_iso(&self.date_of_expiry);
        let (in_date, days_until_expiry) = match exp {
            Some(e) => {
                let days = e.to_epoch_days() - today.to_epoch_days();
                (days >= 0, Some(days))
            }
            None => (false, None),
        };
        let dob_before_expiry = matches!(
            (dob, exp),
            (Some(b), Some(e)) if b.to_epoch_days() < e.to_epoch_days()
        );
        DateValidity {
            dates_well_formed: dob.is_some() && exp.is_some(),
            in_date,
            dob_before_expiry,
            days_until_expiry,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_td3;
    use proptest::prelude::*;

    // ICAO 9303 part 4 specimen: DOB 1974-08-12, expiry 2012-04-15.
    const TD3_L1: &str = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
    const TD3_L2: &str = "L898902C36UTO7408122F1204159ZE184226B<<<<<10";

    #[test]
    fn date_century_pivot() {
        assert_eq!(expand_date("740812", true), "1974-08-12");
        assert_eq!(expand_date("150101", true), "2015-01-01");
        assert_eq!(expand_date("301231", false), "2030-12-31");
    }

    #[test]
    fn century_pivot_boundary() {
        // Birth: yy == CURRENT_YY (26) stays in the 2000s; yy == CURRENT_YY+1
        // (27) rolls back to the 1900s; yy == 99 is always 1900s.
        assert_eq!(expand_date("260101", true), "2026-01-01");
        assert_eq!(expand_date("270101", true), "1927-01-01");
        assert_eq!(expand_date("990101", true), "1999-01-01");
        // Expiry is always 20xx, regardless of yy.
        assert_eq!(expand_date("270101", false), "2027-01-01");
    }

    #[test]
    fn leap_year_rule() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn is_well_formed_rejects_impossible_calendar_dates() {
        assert!(Date::new(2024, 2, 29).is_well_formed());
        assert!(!Date::new(2023, 2, 29).is_well_formed());
        assert!(!Date::new(2023, 2, 30).is_well_formed());
        assert!(!Date::new(2023, 4, 31).is_well_formed());
        assert!(!Date::new(2023, 6, 31).is_well_formed());
        assert!(!Date::new(2023, 11, 31).is_well_formed());
        assert!(Date::new(2023, 12, 31).is_well_formed());
        assert!(Date::new(2023, 1, 31).is_well_formed());
        assert!(!Date::new(2023, 0, 10).is_well_formed());
        assert!(!Date::new(2023, 13, 1).is_well_formed());
        assert!(!Date::new(2023, 1, 0).is_well_formed());
    }

    proptest! {
        /// A date `is_well_formed()` iff it round-trips through epoch days
        /// unchanged: a genuinely valid calendar date maps to itself, while
        /// an impossible one (Feb 30, Apr 31, ...) normalizes to a different
        /// date under `to_epoch_days`/`from_epoch_days`'s civil-calendar math.
        #[test]
        fn is_well_formed_matches_epoch_day_round_trip(
            year in 1900i32..=2100,
            month in 1u32..=13,
            day in 0u32..=32,
        ) {
            let d = Date::new(year, month, day);
            prop_assert_eq!(
                d.is_well_formed(),
                Date::from_epoch_days(d.to_epoch_days()) == d
            );
        }
    }

    #[test]
    fn epoch_days_reference_points() {
        assert_eq!(Date::new(1970, 1, 1).to_epoch_days(), 0);
        assert_eq!(Date::new(1969, 12, 31).to_epoch_days(), -1);
        assert_eq!(Date::new(2000, 1, 1).to_epoch_days(), 10957);
    }

    #[test]
    fn epoch_days_roundtrip() {
        for &(y, m, d) in &[(1970, 1, 1), (1974, 8, 12), (2012, 4, 15), (2026, 7, 17)] {
            let date = Date::new(y, m, d);
            assert_eq!(Date::from_epoch_days(date.to_epoch_days()), date);
        }
    }

    #[test]
    fn validity_tracks_expiry_and_consistency() {
        let d = parse_td3(TD3_L1, TD3_L2).unwrap();

        let before = d.validity(Date::new(2011, 1, 1));
        assert!(before.dates_well_formed);
        assert!(before.dob_before_expiry);
        assert!(before.in_date);
        assert!(before.all_ok());

        let after = d.validity(Date::new(2020, 1, 1));
        assert!(!after.in_date);
        assert!(after.days_until_expiry.unwrap() < 0);
        assert!(!after.all_ok());
    }

    #[test]
    fn days_until_expiry_is_exact() {
        // 2012-04-14 → expiry 2012-04-15 is exactly one day.
        let d = parse_td3(TD3_L1, TD3_L2).unwrap();
        let v = d.validity(Date::new(2012, 4, 14));
        assert_eq!(v.days_until_expiry, Some(1));
        assert!(v.in_date);
    }
}
