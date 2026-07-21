//! Canonical extraction schema shared across the synthpass workspace.
//!
//! Every producer — Tier 1 (deterministic ICAO 9303 MRZ), Tier 2 (in-process
//! LLM), and the browser WASM demo — emits this same [`Extraction`] shape, so
//! every consumer (CLI, web app, on-disk JSON artifacts) sees one contract
//! instead of several ad-hoc field lists.
//!
//! Later phases add crypto/audit helpers to this crate; the schema lives here
//! because it is the one type the whole system agrees on.

use serde::{Deserialize, Serialize};
use zeroize::ZeroizeOnDrop;

#[cfg(feature = "security")]
pub mod audit;
#[cfg(feature = "security")]
pub mod crypt;
pub mod fusion;
pub mod normalize;
pub mod v2;

/// A single extracted identity / travel document record.
///
/// **Superseded (v2.0.0 / M1):** [`v2::ExtractionV2`] adds per-field
/// confidence, provenance, and per-check-digit detail. This type stays fully
/// intact for the v1 wire format — legacy clients (`?v=1` on `synthpass-serve`,
/// `SYNTHPASS_JSON_V1=1` for on-disk JSON) are served this shape for one major
/// release (see `docs/V2-DESIGN.md` §9, B2/B3). New code should produce and
/// consume [`v2::ExtractionV2`]; `From<Extraction>` lifts v1 → v2.
///
/// The core ICAO fields are `Option` because the LLM tier may legitimately fail
/// to find a value and emit `null`; the deterministic MRZ tier fills them
/// non-null. Core fields always serialize (as `null` when absent) so consumers
/// can rely on the keys existing; added metadata (`*_name`, [`validity`]) is
/// omitted from JSON until populated, keeping artifacts stable.
///
/// [`validity`]: Extraction::validity
///
/// `ZeroizeOnDrop`: best-effort wipe of the PII-bearing `String` fields when
/// this value is dropped, shrinking the window a swap file or crash dump
/// could leak identity data. `mrz_checksums_valid` and `validity` carry no
/// PII (plain `bool`/`i64` summaries) and are `#[zeroize(skip)]`, since
/// `Copy` types don't implement `Zeroize`. This does not cover copies made
/// by `serde_json` during (de)serialization — see `docs/ARCHITECTURE.md` §7.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ZeroizeOnDrop)]
pub struct Extraction {
    #[serde(default)]
    pub document_type: Option<String>,
    #[serde(default)]
    pub issuing_country: Option<String>,
    /// Human-readable country name for `issuing_country` (ICAO/ISO 3166-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuing_country_name: Option<String>,
    #[serde(default)]
    pub document_number: Option<String>,
    #[serde(default)]
    pub surname: Option<String>,
    #[serde(default)]
    pub given_names: Option<String>,
    #[serde(default)]
    pub nationality: Option<String>,
    /// Human-readable country name for `nationality` (ICAO/ISO 3166-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nationality_name: Option<String>,
    #[serde(default)]
    pub date_of_birth: Option<String>,
    #[serde(default)]
    pub sex: Option<String>,
    #[serde(default)]
    pub date_of_expiry: Option<String>,
    #[serde(default)]
    pub personal_number: Option<String>,
    /// The raw MRZ zone (newline-joined lines), when one was found.
    #[serde(default)]
    pub mrz_line: Option<String>,
    /// `true` when every ICAO 9303 check digit validated (Tier 1 only).
    #[serde(default)]
    #[zeroize(skip)]
    pub mrz_checksums_valid: Option<bool>,
    /// Date-plausibility summary — populated by the MRZ tier (see Phase 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[zeroize(skip)]
    pub validity: Option<Validity>,
    /// Which producer created this record: `mrz-deterministic`, `llm`, or
    /// `mrz-wasm-client`.
    pub extraction_method: String,
}

/// Date-plausibility summary for an MRZ. A valid composite check digit proves a
/// *faithful read* of the printed zone — it says nothing about whether the
/// document is *in date* or the dates are internally consistent. This captures
/// that separate, non-cryptographic judgement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validity {
    /// Both dates parse as real calendar dates (`YYYY-MM-DD`, month/day in range).
    pub dates_well_formed: bool,
    /// Date of expiry is on or after the reference "today".
    pub in_date: bool,
    /// Date of birth is strictly before the date of expiry.
    pub dob_before_expiry: bool,
    /// Whole-number days until expiry (negative if already expired), when both
    /// the reference date and the expiry date are well-formed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_until_expiry: Option<i64>,
}

impl Validity {
    /// All plausibility checks pass and the document is in date.
    pub fn all_ok(&self) -> bool {
        self.dates_well_formed && self.in_date && self.dob_before_expiry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_keys_always_present_metadata_omitted() {
        // A record with only core fields set: the ICAO keys must all appear
        // (null when absent), while unpopulated metadata is omitted entirely.
        // Struct-update syntax (`..Extraction::default()`) can't be used here:
        // `Extraction` implements `Drop` (via `ZeroizeOnDrop`), and Rust
        // disallows partial moves out of a base value of a `Drop` type.
        let mut e = Extraction::default();
        e.document_type = Some("P".into());
        e.issuing_country = Some("UTO".into());
        e.surname = Some("ERIKSSON".into());
        e.mrz_checksums_valid = Some(true);
        e.extraction_method = "mrz-deterministic".into();
        let v = serde_json::to_value(&e).unwrap();
        let obj = v.as_object().unwrap();

        // Core fields present (even when null).
        for key in [
            "document_type",
            "issuing_country",
            "document_number",
            "surname",
            "given_names",
            "nationality",
            "date_of_birth",
            "sex",
            "date_of_expiry",
            "personal_number",
            "mrz_line",
            "mrz_checksums_valid",
            "extraction_method",
        ] {
            assert!(obj.contains_key(key), "missing core key: {key}");
        }
        assert_eq!(obj["document_number"], serde_json::Value::Null);
        assert_eq!(obj["mrz_checksums_valid"], serde_json::json!(true));

        // Unpopulated metadata omitted.
        for key in ["issuing_country_name", "nationality_name", "validity"] {
            assert!(!obj.contains_key(key), "metadata should be omitted: {key}");
        }
    }

    #[test]
    fn roundtrips_through_json() {
        // See the comment above `core_keys_always_present_metadata_omitted`
        // for why struct-update syntax isn't used here.
        let mut e = Extraction::default();
        e.document_type = Some("I".into());
        e.issuing_country = Some("SVN".into());
        e.issuing_country_name = Some("Slovenia".into());
        e.validity = Some(Validity {
            dates_well_formed: true,
            in_date: true,
            dob_before_expiry: true,
            days_until_expiry: Some(1234),
        });
        e.extraction_method = "llm".into();
        let s = serde_json::to_string(&e).unwrap();
        let back: Extraction = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert!(back.validity.unwrap().all_ok());
    }
}
