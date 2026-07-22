//! ICAO 9303 Machine Readable Zone parser with check-digit validation.
//!
//! Zero runtime dependencies so it compiles to native and `wasm32-unknown-unknown`
//! targets alike. Supports:
//! - **TD3** (passports): 2 lines × 44 characters
//! - **TD2** (official travel documents / ID cards): 2 lines × 36 characters
//! - **TD1** (ID cards): 3 lines × 30 characters
//!
//! Check digits use the standard 7-3-1 weighting over the value mapping
//! `0-9 → 0-9`, `A-Z → 10-35`, `< → 0`. A field checksum that validates
//! mathematically proves the OCR read is faithful to the printed document —
//! no probabilistic model involved.
//!
//! The engine is split across (private) modules:
//! - `checksum` — check-digit math and generic OCR-repair primitives
//! - `parser` — the TD1/TD2/TD3 parsers and the free-text scanner
//! - `dates` — `YYMMDD` expansion and date-plausibility checks
//! - `countries` — ICAO/ISO 3166-1 code → country name
//!
//! A valid composite check digit proves a faithful *read*; it does not prove
//! the document is in date — see [`MrzData::validity`].

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "zeroize")]
use zeroize::ZeroizeOnDrop;

mod checksum;
mod countries;
mod dates;
mod emit;
mod parser;

pub use checksum::{check_digit, verify};
pub use countries::{code_for_name, country_name};
pub use dates::{expand_date, expand_date_with_pivot, Date, DateValidity, CURRENT_YY};
pub use emit::{
    format_mrv_a, format_mrv_b, format_td1, format_td2, format_td3, MrvAFields, MrvBFields,
    Td1Fields, Td2Fields, Td3Fields,
};
pub use parser::{find_and_parse, parse_mrv_a, parse_mrv_b, parse_td1, parse_td2, parse_td3};

/// Per-field check-digit verification results.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Checks {
    pub document_number: bool,
    pub date_of_birth: bool,
    pub date_of_expiry: bool,
    /// TD3 only; `true` for TD1/TD2 (no such check digit exists there).
    pub personal_number: bool,
    /// The composite check digit over the whole zone.
    pub composite: bool,
}

impl Checks {
    /// All check digits valid — the MRZ read is mathematically verified.
    pub fn all_valid(&self) -> bool {
        self.document_number
            && self.date_of_birth
            && self.date_of_expiry
            && self.personal_number
            && self.composite
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum Format {
    Td3,
    Td2,
    Td1,
    /// Machine readable visa, type A (ICAO 9303 part 7): two 44-char lines,
    /// geometry mirrors TD3 through the expiry check digit, but there is no
    /// personal-number field and no composite check digit.
    MrvA,
    /// Machine readable visa, type B (ICAO 9303 part 7): two 36-char lines,
    /// geometry mirrors TD2 through the expiry check digit, but there is no
    /// personal-number field and no composite check digit.
    MrvB,
}

/// Parsed and validated MRZ data.
///
/// The `zeroize` feature (off by default, kept off for the `wasm32-unknown-unknown`
/// browser-demo build so this crate stays zero-dependency there) derives
/// `ZeroizeOnDrop`, wiping the PII-bearing `String` fields from memory when a
/// value is dropped. `format` and `checks` carry no PII and are `Copy`, so
/// they're `#[zeroize(skip)]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "zeroize", derive(ZeroizeOnDrop))]
pub struct MrzData {
    #[cfg_attr(feature = "zeroize", zeroize(skip))]
    pub format: Format,
    /// Document code, e.g. "P" (passport), "ID"/"I" (identity card).
    pub document_type: String,
    /// Issuing state or organization (3-letter ICAO code).
    pub issuing_country: String,
    pub document_number: String,
    pub surname: String,
    pub given_names: String,
    /// Nationality (3-letter ICAO code).
    pub nationality: String,
    /// ISO 8601 (`YYYY-MM-DD`), century inferred (see [`expand_date`]).
    pub date_of_birth: String,
    /// "M", "F" or "X" (unspecified).
    pub sex: String,
    /// ISO 8601 (`YYYY-MM-DD`).
    pub date_of_expiry: String,
    /// TD3: personal number field. TD1: optional data 1 + 2 joined.
    /// TD2: optional data field.
    pub personal_number: Option<String>,
    /// The raw MRZ lines, newline-joined, exactly as validated.
    pub mrz_lines: String,
    #[cfg_attr(feature = "zeroize", zeroize(skip))]
    pub checks: Checks,
}

impl MrzData {
    /// Shorthand for `checks.all_valid()`.
    pub fn valid(&self) -> bool {
        self.checks.all_valid()
    }

    /// Human-readable name of the issuing state, if the code is recognized.
    pub fn issuing_country_name(&self) -> Option<&'static str> {
        country_name(&self.issuing_country)
    }

    /// Human-readable name of the nationality, if the code is recognized.
    pub fn nationality_name(&self) -> Option<&'static str> {
        country_name(&self.nationality)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MrzError {
    /// Line has the wrong length for the claimed format.
    BadLength { expected: usize, got: usize },
    /// Character outside `[A-Z0-9<]`.
    BadCharacter(char),
    /// Document code not recognized for the format.
    BadDocumentCode(String),
    /// No plausible MRZ found in the supplied text.
    NotFound,
}

impl core::fmt::Display for MrzError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadLength { expected, got } => {
                write!(f, "bad MRZ line length: expected {expected}, got {got}")
            }
            Self::BadCharacter(c) => write!(f, "invalid MRZ character: {c:?}"),
            Self::BadDocumentCode(c) => write!(f, "unrecognized document code: {c:?}"),
            Self::NotFound => write!(f, "no MRZ found in text"),
        }
    }
}

impl std::error::Error for MrzError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Official ICAO 9303 part 4 specimen (Utopia / Anna Maria Eriksson).
    const TD3_L1: &str = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
    const TD3_L2: &str = "L898902C36UTO7408122F1204159ZE184226B<<<<<10";

    #[test]
    fn td3_specimen_fully_valid() {
        let d = parse_td3(TD3_L1, TD3_L2).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.document_type, "P");
        assert_eq!(d.issuing_country, "UTO");
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.given_names, "ANNA MARIA");
        assert_eq!(d.document_number, "L898902C3");
        assert_eq!(d.nationality, "UTO");
        assert_eq!(d.date_of_birth, "1974-08-12");
        assert_eq!(d.sex, "F");
        assert_eq!(d.date_of_expiry, "2012-04-15");
        assert_eq!(d.personal_number.as_deref(), Some("ZE184226B"));
    }

    #[test]
    fn td3_tampered_dob_fails_checksum() {
        // Change one digit of the date of birth: 740812 → 750812.
        let tampered = TD3_L2.replacen("740812", "750812", 1);
        let d = parse_td3(TD3_L1, &tampered).unwrap();
        assert!(!d.checks.date_of_birth);
        assert!(!d.checks.composite);
        assert!(!d.valid());
    }

    #[test]
    fn td3_empty_personal_number_with_filler_check() {
        // Personal number all fillers and check digit '<' is valid (value 0).
        let l2 = "L898902C36UTO7408122F1204159<<<<<<<<<<<<<<06";
        let d = parse_td3(TD3_L1, l2).unwrap();
        assert!(d.checks.personal_number);
        assert_eq!(d.personal_number, None);
    }

    // Official ICAO 9303 part 5 TD1 specimen.
    const TD1_L1: &str = "I<UTOD231458907<<<<<<<<<<<<<<<";
    const TD1_L2: &str = "7408122F1204159UTO<<<<<<<<<<<6";
    const TD1_L3: &str = "ERIKSSON<<ANNA<MARIA<<<<<<<<<<";

    #[test]
    fn td1_specimen_fully_valid() {
        let d = parse_td1(TD1_L1, TD1_L2, TD1_L3).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::Td1);
        assert_eq!(d.document_type, "I");
        assert_eq!(d.document_number, "D23145890");
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.given_names, "ANNA MARIA");
        assert_eq!(d.date_of_birth, "1974-08-12");
        assert_eq!(d.date_of_expiry, "2012-04-15");
    }

    // Official ICAO 9303 part 6 TD2 specimen (Utopia / Anna Maria Eriksson).
    const TD2_L1: &str = "I<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<";
    const TD2_L2: &str = "D231458907UTO7408122F1204159<<<<<<<6";

    #[test]
    fn td2_specimen_fully_valid() {
        let d = parse_td2(TD2_L1, TD2_L2).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::Td2);
        assert_eq!(d.document_type, "I");
        assert_eq!(d.issuing_country, "UTO");
        assert_eq!(d.document_number, "D23145890");
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.given_names, "ANNA MARIA");
        assert_eq!(d.nationality, "UTO");
        assert_eq!(d.date_of_birth, "1974-08-12");
        assert_eq!(d.sex, "F");
        assert_eq!(d.date_of_expiry, "2012-04-15");
    }

    #[test]
    fn td2_tampered_expiry_fails_checksum() {
        let tampered = TD2_L2.replacen("120415", "120416", 1);
        let d = parse_td2(TD2_L1, &tampered).unwrap();
        assert!(!d.checks.date_of_expiry);
        assert!(!d.checks.composite);
        assert!(!d.valid());
    }

    #[test]
    fn td2_found_in_ocr_text() {
        let text = format!("## IDENTITY CARD\n\nnoise\n\n{TD2_L1}\n{TD2_L2}\n\nfooter");
        let d = find_and_parse(&text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::Td2);
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.document_number, "D23145890");
    }

    #[test]
    fn find_in_ocr_noise() {
        let text = format!(
            "## REPUBLIC OF UTOPIA\n\nSome OCR noise here\n\n{}\n{}\n\nfooter",
            // OCR quirks: lowercase, stray spaces, « for <<, dropped fillers.
            "p<utoeriksson«anna<maria<<<<<<<<<<<<<<<<<",
            "L898902C36UTO7408122F1204159ZE184226B<<<<<10"
        );
        let d = find_and_parse(&text).unwrap();
        assert!(d.valid());
        assert_eq!(d.surname, "ERIKSSON");
    }

    #[test]
    fn find_html_escaped_and_merged_lines() {
        // Real docling output shape: fillers escaped as &lt; and both TD3
        // lines on one physical markdown line (Croatian specimen).
        let text = "## PUTOVNICA\n\nP&lt;HRVSPECIMEN&lt;&lt;SPECIMEN&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt; 0070070071HRV8212258F1407019&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;06\n";
        let d = find_and_parse(text).unwrap();
        assert_eq!(d.surname, "SPECIMEN");
        assert_eq!(d.document_number, "007007007");
        assert_eq!(d.issuing_country, "HRV");
        assert!(d.checks.document_number);
        assert!(d.checks.date_of_birth);
        assert!(d.checks.date_of_expiry);
    }

    #[test]
    fn checksum_verified_ocr_repair() {
        // Verbatim tesseract.js output for the Croatian specimen at low
        // resolution: trailing fillers read as K/L runs, a hallucinated
        // leading '1' on line 2 (45 chars), and 'B' where '8' is printed.
        // The check digits prove which repaired variant is the true read.
        let text = "I 01072009 PUJZAGREB 0\n\nBIDFD WH5SS A 2\n\n01072014\nP<HRVSPECIMEN<<SPECIMEN<KLLLLLLLLLLLLLLLLLKLKL\n10070070071HRVB212258F1407019<<<<<<<<<<<<<<06\n";
        let d = find_and_parse(text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.surname, "SPECIMEN");
        assert_eq!(d.given_names, "SPECIMEN");
        assert_eq!(d.document_number, "007007007");
        assert_eq!(d.date_of_birth, "1982-12-25");
    }

    #[test]
    fn ocr_repair_dropped_filler_mid_line() {
        // Second verbatim tesseract.js reading of the same specimen: an
        // L-run inside the personal-number field and one filler DROPPED
        // (43 chars) — the missing character must be re-inserted inside the
        // filler run, not appended, or the check digits shift.
        let text = "RF 01072009 PUZAGREB\n01072014\nP<HRVSPECIMEN<<SPECIMEN<<K<KLLLLLLLLLLLLLLLLKLKL\n0070070071HRVB212258F1407019<<<<LLLLLLL<<06\n";
        let d = find_and_parse(text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.surname, "SPECIMEN");
        assert_eq!(d.document_number, "007007007");
        assert_eq!(d.personal_number, None);
    }

    #[test]
    fn td1_from_single_docling_line_with_k_misreads() {
        // Verbatim docling OCR of the Slovenian 2022 specimen ID card rear:
        // all three TD1 lines in ONE paragraph, `<` escaped as &lt;, and the
        // usual K-for-filler misreads (IK→I<, 145K<→145<<, VZORECKK→VZOREC<<).
        let text = "1F9874543\n\nIKSVNIE987654302806985505145K&lt; 8506287F3203282SVN&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;&lt;2 VZORECKKJANAKKKKKKKKK&lt;&lt;KK";
        let d = find_and_parse(text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::Td1);
        assert_eq!(d.document_type, "I");
        assert_eq!(d.issuing_country, "SVN");
        assert_eq!(d.document_number, "IE9876543");
        assert_eq!(d.surname, "VZOREC");
        assert_eq!(d.given_names, "JANA");
        assert_eq!(d.date_of_birth, "1985-06-28");
        assert_eq!(d.date_of_expiry, "2032-03-28");
        // The trailing K in the EMŠO field is a filler misread that check
        // digits cannot catch (K ≡ < mod 10) — heuristic cleanup handles it.
        assert_eq!(d.personal_number.as_deref(), Some("2806985505145"));
    }

    #[test]
    fn ocr_repair_deeply_truncated_name_line() {
        // Verbatim ocrs output for the Croatian specimen at 600×421: line 2 is
        // read perfectly, but line 1 loses NINE trailing fillers (35/44 chars)
        // and its `<` document-code filler is misread as `K`. The name line
        // carries no check digit of its own, so padding the filler run back is
        // safe — line 2's check digits still prove the read.
        let text = "PUTOVNICA\nPKHRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<\n0070070071HRV8212258F1407019<<<<<<<<<<<<<<06\n";
        let d = find_and_parse(text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.surname, "SPECIMEN");
        assert_eq!(d.given_names, "SPECIMEN");
        assert_eq!(d.document_number, "007007007");
    }

    #[test]
    fn invalid_checksums_still_reported() {
        // A tampered MRZ parses but is flagged invalid rather than dropped.
        let tampered = TD3_L2.replacen("740812", "750812", 1);
        let text = format!("{TD3_L1}\n{tampered}");
        let d = find_and_parse(&text).unwrap();
        assert!(!d.valid());
        assert!(!d.checks.date_of_birth);
    }

    #[test]
    fn find_nothing_in_plain_text() {
        assert_eq!(
            find_and_parse("just a regular paragraph\nwith two lines"),
            Err(MrzError::NotFound)
        );
    }

    // MRV-A specimen line 2 (44 chars); check-digit arithmetic (7-3-1, values
    // A-Z=10-35, <=0):
    //   doc#   XK9305487: X=33,K=20,9,3,0,5,4,8,7 * 7,3,1,7,3,1,7,3,1
    //          = 231+60+9+21+0+5+28+24+7 = 385 -> 385 mod 10 = 5
    //   DOB    850221: 8,5,0,2,2,1 * 7,3,1,7,3,1
    //          = 56+15+0+14+6+1 = 92 -> 92 mod 10 = 2
    //   expiry 270314: 2,7,0,3,1,4 * 7,3,1,7,3,1
    //          = 14+21+0+21+3+4 = 63 -> 63 mod 10 = 3
    const MRV_A_L1: &str = "V<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
    const MRV_A_L2: &str = "XK93054875BRA8502212F2703143R5T6U7V8W9<<<<<<";

    #[test]
    fn mrv_a_specimen_fully_valid() {
        let d = parse_mrv_a(MRV_A_L1, MRV_A_L2).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvA);
        assert_eq!(d.document_type, "V");
        assert_eq!(d.issuing_country, "UTO");
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.given_names, "ANNA MARIA");
        assert_eq!(d.nationality, "BRA");
        assert_eq!(d.date_of_birth, "1985-02-21");
        assert_eq!(d.sex, "F");
        assert_eq!(d.date_of_expiry, "2027-03-14");
        assert_eq!(d.document_number, "XK9305487");
        assert_eq!(d.personal_number.as_deref(), Some("R5T6U7V8W9"));
    }

    // MRV-B specimen line 2 (36 chars); check-digit arithmetic (7-3-1, values
    // A-Z=10-35, <=0):
    //   doc#   L23456789: L=21,2,3,4,5,6,7,8,9 * 7,3,1,7,3,1,7,3,1
    //          = 147+6+3+28+15+6+49+24+9 = 287 -> 287 mod 10 = 7
    //   DOB    920101: 9,2,0,1,0,1 * 7,3,1,7,3,1
    //          = 63+6+0+7+0+1 = 77 -> 77 mod 10 = 7
    //   expiry 270630: 2,7,0,6,3,0 * 7,3,1,7,3,1
    //          = 14+21+0+42+9+0 = 86 -> 86 mod 10 = 6
    const MRV_B_L1: &str = "V<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<";
    const MRV_B_L2: &str = "L234567897DEU9201017F2706306QW12ER34";

    #[test]
    fn mrv_b_specimen_fully_valid() {
        let d = parse_mrv_b(MRV_B_L1, MRV_B_L2).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvB);
        assert_eq!(d.nationality, "DEU");
        assert_eq!(d.date_of_birth, "1992-01-01");
        assert_eq!(d.date_of_expiry, "2027-06-30");
        assert_eq!(d.document_number, "L23456789");
        assert_eq!(d.personal_number.as_deref(), Some("QW12ER34"));
    }

    #[test]
    fn mrv_a_tampered_dob_fails_checksum() {
        let tampered = MRV_A_L2.replacen("850221", "860221", 1);
        let d = parse_mrv_a(MRV_A_L1, &tampered).unwrap();
        assert!(!d.checks.date_of_birth);
        assert!(!d.valid());
    }

    #[test]
    fn mrv_rejects_non_v_document_code() {
        let line1 = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
        let result = parse_mrv_a(line1, MRV_A_L2);
        assert!(matches!(result, Err(MrzError::BadDocumentCode(_))));
    }

    #[test]
    fn mrv_a_found_in_ocr_text() {
        let text = format!("## VISA\n\nnoise\n\n{MRV_A_L1}\n{MRV_A_L2}\n\nfooter");
        let d = find_and_parse(&text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvA);
        assert_eq!(d.surname, "ERIKSSON");
        assert_eq!(d.nationality, "BRA");
    }

    #[test]
    fn mrv_b_found_in_ocr_text() {
        let text = format!("## VISA\n\nnoise\n\n{MRV_B_L1}\n{MRV_B_L2}\n\nfooter");
        let d = find_and_parse(&text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvB);
        assert_eq!(d.nationality, "DEU");
    }

    #[test]
    fn mrv_found_html_escaped_or_merged() {
        // HTML-escaped fillers.
        let escaped_l1 = MRV_A_L1.replace('<', "&lt;");
        let escaped_l2 = MRV_A_L2.replace('<', "&lt;");
        let text = format!("## VISA\n\n{escaped_l1}\n{escaped_l2}\n");
        let d = find_and_parse(&text).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvA);

        // Both lines merged onto one physical line with no separator at all
        // (the ~88-char merged-line case, like docling's single-paragraph
        // OCR output for a TD3 specimen).
        let merged = format!("## VISA\n\n{MRV_A_L1}{MRV_A_L2}\n");
        let d = find_and_parse(&merged).unwrap();
        assert!(d.valid(), "checks: {:?}", d.checks);
        assert_eq!(d.format, Format::MrvA);
    }

    #[test]
    fn country_names_surface_on_mrzdata() {
        let d = parse_td1(TD1_L1, TD1_L2, TD1_L3).unwrap();
        assert_eq!(d.issuing_country_name(), Some("Utopia (ICAO specimen)"));
        assert_eq!(d.nationality_name(), Some("Utopia (ICAO specimen)"));
    }
}
