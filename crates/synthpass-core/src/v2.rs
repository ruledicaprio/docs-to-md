//! Extraction schema v2 — the v2.0.0 keystone (`docs/V2-DESIGN.md` §3).
//!
//! v1's [`Extraction`] is MRZ-shaped: everything the MRZ can't say (per-field
//! certainty, who produced a value, portrait/barcode slots) has nowhere to
//! live, and Tier-2 output is indistinguishable in confidence from Tier-1's
//! mathematical proof. [`ExtractionV2`] fixes that without breaking the v1
//! wire format: v1 stays intact and is still served to legacy clients for one
//! major release (`?v=1` / `Accept: application/vnd.mlis.v1+json` on
//! `synthpass-serve`, `SYNTHPASS_JSON_V1=1` for on-disk artifacts — §9 B2/B3).
//!
//! The wire format keeps v1's snake_case; [`ExtractionV2::schema_version`] is
//! always serialized so a consumer can dispatch on the schema before touching
//! anything else.
//!
//! **Zeroize discipline** mirrors v1: every PII-bearing `String` (all of
//! [`ExtractionFields`], [`MrzBlock::lines`]) is wiped on drop via
//! `ZeroizeOnDrop`. Non-PII metadata — confidence scores, provenance,
//! check-digit bools, bounding boxes — is `#[zeroize(skip)]` (`Copy` numerics
//! don't implement `Zeroize`, and a confidence score identifies no one). The
//! same caveats as v1 apply: `serde_json`'s intermediate copies are not wiped
//! (see `docs/ARCHITECTURE.md` §7).

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{Extraction, Validity};

/// Wire value of [`ExtractionV2::schema_version`] for this schema generation.
pub const SCHEMA_VERSION_V2: u32 = 2;

/// Fallback `schema_version` when deserializing a payload that omits it —
/// always the current version, never 0.
fn default_schema_version() -> u32 {
    SCHEMA_VERSION_V2
}

/// Flat heuristic confidence the v1→v2 lift assigns to Tier-2 (LLM) fields.
/// Honest by construction: an LLM-extracted field is *plausible*, not proven.
/// M5 (`docs/V2-DESIGN.md` §8) replaces this flat default with scores derived
/// from GBNF parse cleanliness + field-level validators.
pub const LLM_HEURISTIC_CONFIDENCE: f32 = 0.5;

/// Extraction-confidence vocabulary: Tier-1 checksum-proven fields.
const PROVEN: f32 = 1.0;

/// A single extracted identity / travel document record, schema v2.
///
/// Relationship to v1: every v1 scalar field lives verbatim under [`fields`];
/// the additions are metadata *about* the extraction (confidence, provenance,
/// per-check-digit detail) and empty slots for capabilities landing in later
/// milestones ([`portrait`] in M2, [`barcodes`] decoding, [`documents`] in
/// M4). See `docs/V2-DESIGN.md` §3 for the full rationale.
///
/// [`fields`]: ExtractionV2::fields
/// [`portrait`]: ExtractionV2::portrait
/// [`barcodes`]: ExtractionV2::barcodes
/// [`documents`]: ExtractionV2::documents
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct ExtractionV2 {
    /// Schema generation of this record — always `2` for values built by this
    /// crate, and always serialized so consumers can version-dispatch.
    #[serde(default = "default_schema_version")]
    #[zeroize(skip)]
    pub schema_version: u32,
    /// What kind of document was read, plus the MRZ format when one was found.
    #[zeroize(skip)]
    pub document: DocumentClass,
    /// The v1 scalar fields, verbatim. Core keys always serialize (as `null`
    /// when absent), matching v1's contract; `*_name` metadata is omitted
    /// until populated.
    pub fields: ExtractionFields,
    /// Per-field certainty. `1.0` means checksum-proven (Tier 1); anything
    /// below is a heuristic model score (Tier 2). Describes extraction
    /// certainty, never document genuineness — forgery detection is an
    /// explicit non-goal (`docs/V2-DESIGN.md` §11).
    #[zeroize(skip)]
    pub confidence: FieldConfidence,
    /// Which producer created this record — the property that lets downstream
    /// consumers distinguish *proven* from *inferred*.
    #[zeroize(skip)]
    pub provenance: Provenance,
    /// The raw MRZ zone plus per-check-digit results, when an MRZ was found —
    /// present even when a digit failed, so consumers can see *which* field is
    /// suspect. [`MrzBlock::lines`] carries PII and is zeroized on drop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mrz: Option<MrzBlock>,
    /// Date-plausibility summary — unchanged semantics from v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[zeroize(skip)]
    pub validity: Option<Validity>,
    /// Bounding box of the portrait (face) region in the source image.
    /// Slot only — the cropping heuristic that populates it is M2
    /// (`docs/V2-DESIGN.md` §4). No face recognition, ever (§11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[zeroize(skip)]
    pub portrait: Option<ImageRef>,
    /// Barcode hits (PDF417 etc.). Slot only — no decoder ships in v2.0.0
    /// (`docs/V2-DESIGN.md` §12: "a slot, not a decoder"). Empty until then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub barcodes: Vec<BarcodeHit>,
    /// Which producer created this record: the same vocabulary as v1
    /// (`mrz-deterministic`, `llm`, `mrz-wasm-client`).
    pub extraction_method: String,
    /// Reserved for multi-document input (M4, `docs/V2-DESIGN.md` §3). Always
    /// empty in v2.0.0; declared now with `default` + `skip_serializing_if` so
    /// the wire format is future-proof — M4 can start emitting it without a
    /// schema bump.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<ExtractionV2>,
}

impl Default for ExtractionV2 {
    /// Hand-rolled (not derived) so `schema_version` defaults to
    /// [`SCHEMA_VERSION_V2`], not 0.
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION_V2,
            document: DocumentClass::default(),
            fields: ExtractionFields::default(),
            confidence: FieldConfidence::default(),
            provenance: Provenance::default(),
            mrz: None,
            validity: None,
            portrait: None,
            barcodes: Vec::new(),
            extraction_method: String::new(),
            documents: Vec::new(),
        }
    }
}

/// What kind of document was read, plus the MRZ format when known.
///
/// A struct (not a bare enum) so the MRZ format rides alongside the class
/// without inventing per-format variants for documents that have no MRZ.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentClass {
    pub kind: DocumentKind,
    /// ICAO 9303 format of the MRZ that was read, when one was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mrz_format: Option<MrzFormat>,
}

/// Broad document class, inferred from the MRZ document code when one exists
/// (`P*` → passport, `I*` → ID card, anything else → other).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    Passport,
    IdCard,
    #[default]
    Other,
}

/// ICAO 9303 MRZ formats: TD3 (passports, 2×44), TD2 (official travel
/// documents, 2×36), TD1 (ID cards, 3×30).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MrzFormat {
    Td1,
    Td2,
    Td3,
}

impl MrzFormat {
    /// Best-effort format guess from raw MRZ text (3 lines → TD1; 2 lines
    /// longer than 36 chars → TD3; otherwise TD2). Used only by the v1→v2
    /// lift, where the exact [`mrz::Format`] was never recorded — the
    /// pipeline's Tier-1 path fills the real value in directly.
    ///
    /// [`mrz::Format`]: https://docs.rs/mrz
    pub fn guess_from_lines(lines: &str) -> Option<Self> {
        let count = lines.lines().filter(|l| !l.trim().is_empty()).count();
        let max_len = lines.lines().map(str::len).max().unwrap_or(0);
        match (count, max_len) {
            (3.., _) => Some(Self::Td1),
            (2, 37..) => Some(Self::Td3),
            (2, _) => Some(Self::Td2),
            _ => None,
        }
    }
}

/// The v1 scalar fields, moved verbatim into a nested struct so schema-level
/// metadata (confidence, provenance) sits beside them rather than among them.
///
/// Serde behavior mirrors v1 exactly: core keys always serialize (as `null`
/// when absent); the derived `*_name` metadata is omitted until populated.
/// All fields are PII and zeroized on drop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
pub struct ExtractionFields {
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
}

/// Per-field extraction certainty, one score per ICAO field in
/// [`ExtractionFields`] (the derived `*_name` metadata carries no score of its
/// own — it inherits its code field's provenance).
///
/// Scale: `1.0` = proven by an ICAO 9303 check digit (Tier 1); anything below
/// is a heuristic model score (Tier 2). These scores describe *extraction
/// certainty*, not document authenticity — a checksum proves a faithful read,
/// not a genuine document (`docs/V2-DESIGN.md` §11). Non-PII; `#[zeroize(skip)]`
/// at the parent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldConfidence {
    pub document_type: f32,
    pub issuing_country: f32,
    pub document_number: f32,
    pub surname: f32,
    pub given_names: f32,
    pub nationality: f32,
    pub date_of_birth: f32,
    pub sex: f32,
    pub date_of_expiry: f32,
    pub personal_number: f32,
}

impl FieldConfidence {
    /// Every field at the same score.
    pub fn uniform(score: f32) -> Self {
        Self {
            document_type: score,
            issuing_country: score,
            document_number: score,
            surname: score,
            given_names: score,
            nationality: score,
            date_of_birth: score,
            sex: score,
            date_of_expiry: score,
            personal_number: score,
        }
    }

    /// All fields checksum-proven (Tier 1 / WASM demo).
    pub fn proven() -> Self {
        Self::uniform(PROVEN)
    }

    /// The flat Tier-2 heuristic used until M5 lands per-field scoring.
    pub fn llm_heuristic() -> Self {
        Self::uniform(LLM_HEURISTIC_CONFIDENCE)
    }

    /// Every field checksum-proven.
    pub fn all_proven(&self) -> bool {
        *self == Self::proven()
    }
}

/// Which producer created an [`ExtractionV2`] — the explicit form of what v1
/// encoded implicitly in `extraction_method`. Serialized tagged on `kind`:
/// `{"kind": "mrz_checksum"}`, `{"kind": "llm", "model": "…"}`,
/// `{"kind": "wasm_client"}`.
///
/// Non-PII (the `model` string names a local GGUF file, never a person);
/// `#[zeroize(skip)]` at the parent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    /// Tier 1: every field mathematically verified by ICAO 9303 check digits.
    #[default]
    MrzChecksum,
    /// Tier 2: the local LLM fallback. `model` identifies the GGUF that
    /// produced the record (path basename, e.g.
    /// `qwen2.5-1.5b-instruct-q4_k_m.gguf`), or `"unknown"` when the backend
    /// doesn't report one.
    Llm { model: String },
    /// The browser WASM demo (`mrz-wasm`), where extraction happens entirely
    /// client-side.
    WasmClient,
}

/// The raw MRZ zone plus per-check-digit verification results.
///
/// [`lines`] carries PII (names, document numbers) and is zeroized on drop;
/// the check-digit bools and format are non-PII metadata.
///
/// [`lines`]: MrzBlock::lines
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
pub struct MrzBlock {
    /// The raw MRZ lines, newline-joined, exactly as validated.
    pub lines: String,
    #[zeroize(skip)]
    pub format: MrzFormat,
    #[zeroize(skip)]
    pub checks: CheckDigits,
}

/// Per-check-digit verification results — the v2, serde-round-trippable
/// mirror of `mrz::Checks` (which is `Serialize`-only, WASM-bound).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckDigits {
    pub document_number: bool,
    pub date_of_birth: bool,
    pub date_of_expiry: bool,
    /// TD3 only; `true` for TD1/TD2 (no such check digit exists there).
    pub personal_number: bool,
    /// The composite check digit over the whole zone.
    pub composite: bool,
}

impl CheckDigits {
    /// All check digits valid — the MRZ read is mathematically verified.
    pub fn all_valid(&self) -> bool {
        self.document_number
            && self.date_of_birth
            && self.date_of_expiry
            && self.personal_number
            && self.composite
    }
}

/// An axis-aligned bounding box in the source image, in pixels.
/// Used for [`ExtractionV2::portrait`] and [`BarcodeHit::bbox`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A detected barcode. Slot struct only — v2.0.0 ships no decoder, so
/// [`ExtractionV2::barcodes`] stays empty; the shape is fixed now so the M-
/// milestone decoder that lands later needs no schema change. `data` is
/// reserved for the decoded payload (PII, zeroized) and never populated yet.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
pub struct BarcodeHit {
    /// Symbology vocabulary, e.g. `"pdf417"`, `"qr"`, `"code128"`.
    pub format: String,
    /// Where in the source image the barcode was found, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[zeroize(skip)]
    pub bbox: Option<ImageRef>,
    /// Reserved for the decoded payload — no decoder populates this yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Infer the broad document class from an MRZ document code (`P*` passport,
/// `I*` ID card, anything else other). Empty/unknown codes stay `Other` —
/// guessing beyond the ICAO convention would be dishonest.
fn document_kind_from_code(code: Option<&str>) -> DocumentKind {
    match code.and_then(|c| c.chars().next()) {
        Some('P') => DocumentKind::Passport,
        Some('I') => DocumentKind::IdCard,
        _ => DocumentKind::Other,
    }
}

impl From<Extraction> for ExtractionV2 {
    fn from(v1: Extraction) -> Self {
        Self::from(&v1)
    }
}

impl From<&Extraction> for ExtractionV2 {
    /// Lift a v1 record into v2, deriving what v1 never recorded:
    ///
    /// - **confidence / provenance** come from `extraction_method`:
    ///   `mrz-deterministic` → all 1.0 + [`Provenance::MrzChecksum`];
    ///   `mrz-wasm-client` → all 1.0 + [`Provenance::WasmClient`]; `llm` (and
    ///   any unrecognized value — the lift stays total rather than panicking
    ///   on producer drift) → [`LLM_HEURISTIC_CONFIDENCE`] +
    ///   [`Provenance::Llm`] with `model: "unknown"` (the lift has no access
    ///   to the backend; the pipeline re-stamps the real model id).
    /// - **document class** is inferred from the MRZ document code; the MRZ
    ///   format is guessed from the raw zone's line shape (the pipeline's
    ///   Tier-1 path overwrites both with exact values).
    /// - **check digits**: v1 kept only the aggregate `mrz_checksums_valid`
    ///   bool, so every digit inherits it — lossy, and documented as such.
    ///
    /// Fields are cloned, not moved: [`Extraction`] is a `Drop` type
    /// (`ZeroizeOnDrop`), and Rust forbids partial moves out of one.
    fn from(v1: &Extraction) -> Self {
        let (confidence, provenance) = match v1.extraction_method.as_str() {
            "mrz-deterministic" => (FieldConfidence::proven(), Provenance::MrzChecksum),
            "mrz-wasm-client" => (FieldConfidence::proven(), Provenance::WasmClient),
            _ => (
                FieldConfidence::llm_heuristic(),
                Provenance::Llm {
                    model: "unknown".to_string(),
                },
            ),
        };
        let mrz_format = v1
            .mrz_line
            .as_deref()
            .and_then(MrzFormat::guess_from_lines);
        let mrz = v1.mrz_line.as_ref().map(|lines| MrzBlock {
            lines: lines.clone(),
            // A zone whose shape we can't classify still gets recorded; TD3
            // is the least-surprising placeholder and is only ever a
            // best-effort guess on this legacy path.
            format: mrz_format.unwrap_or(MrzFormat::Td3),
            checks: {
                let ok = v1.mrz_checksums_valid.unwrap_or(false);
                CheckDigits {
                    document_number: ok,
                    date_of_birth: ok,
                    date_of_expiry: ok,
                    personal_number: ok,
                    composite: ok,
                }
            },
        });
        Self {
            schema_version: SCHEMA_VERSION_V2,
            document: DocumentClass {
                kind: document_kind_from_code(v1.document_type.as_deref()),
                mrz_format,
            },
            fields: ExtractionFields {
                document_type: v1.document_type.clone(),
                issuing_country: v1.issuing_country.clone(),
                issuing_country_name: v1.issuing_country_name.clone(),
                document_number: v1.document_number.clone(),
                surname: v1.surname.clone(),
                given_names: v1.given_names.clone(),
                nationality: v1.nationality.clone(),
                nationality_name: v1.nationality_name.clone(),
                date_of_birth: v1.date_of_birth.clone(),
                sex: v1.sex.clone(),
                date_of_expiry: v1.date_of_expiry.clone(),
                personal_number: v1.personal_number.clone(),
            },
            confidence,
            provenance,
            mrz,
            validity: v1.validity,
            portrait: None,
            barcodes: Vec::new(),
            extraction_method: v1.extraction_method.clone(),
            documents: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Build the Croatian TD3 specimen extraction as v1 — the same values the
    /// `mrz` crate's corpus tests prove (`P<HRVSPECIMEN<<SPECIMEN…`,
    /// document `007007007`). Returns the v1 record so each test can lift or
    /// serialize from one shared fixture.
    fn croatian_specimen_v1() -> Extraction {
        let mut e = Extraction::default();
        e.document_type = Some("P".into());
        e.issuing_country = Some("HRV".into());
        e.issuing_country_name = Some("Croatia".into());
        e.document_number = Some("007007007".into());
        e.surname = Some("SPECIMEN".into());
        e.given_names = Some("SPECIMEN".into());
        e.nationality = Some("HRV".into());
        e.nationality_name = Some("Croatia".into());
        e.date_of_birth = Some("1982-12-25".into());
        e.sex = Some("F".into());
        e.date_of_expiry = Some("2014-07-01".into());
        e.personal_number = None;
        e.mrz_line = Some(
            "P<HRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<<<<<<<<<<\n0070070071HRV8212258F1407019<<<<<<<<<<<<<<06"
                .into(),
        );
        e.mrz_checksums_valid = Some(true);
        e.extraction_method = "mrz-deterministic".into();
        e
    }

    #[test]
    fn lift_from_mrz_deterministic_is_fully_proven() {
        let v2 = ExtractionV2::from(croatian_specimen_v1());
        assert_eq!(v2.schema_version, 2);
        assert_eq!(v2.provenance, Provenance::MrzChecksum);
        assert!(v2.confidence.all_proven(), "Tier 1 = checksum-proven");
        assert_eq!(v2.extraction_method, "mrz-deterministic");
        assert_eq!(v2.document.kind, DocumentKind::Passport);
        assert_eq!(v2.document.mrz_format, Some(MrzFormat::Td3));
        assert_eq!(v2.fields.document_number.as_deref(), Some("007007007"));
        // Clone rather than move: `ExtractionV2` derives `ZeroizeOnDrop` when the
        // workspace unifies `synthpass-core`'s `zeroize` feature on (e.g. via
        // `synthpass-pipeline`), and a `Drop` type forbids partial moves out of it.
        let mrz = v2.mrz.clone().expect("MRZ block lifted");
        assert!(mrz.checks.all_valid());
        assert_eq!(mrz.format, MrzFormat::Td3);
        // Future-proofing slots stay empty on the lift.
        assert!(v2.portrait.is_none());
        assert!(v2.barcodes.is_empty());
        assert!(v2.documents.is_empty());
    }

    #[test]
    fn lift_from_llm_gets_heuristic_confidence() {
        let mut v1 = Extraction::default();
        v1.surname = Some("DOE".into());
        v1.extraction_method = "llm".into();
        let v2 = ExtractionV2::from(v1);
        assert_eq!(
            v2.provenance,
            Provenance::Llm {
                model: "unknown".into()
            }
        );
        assert_eq!(v2.confidence, FieldConfidence::llm_heuristic());
        assert!(!v2.confidence.all_proven());
        assert!(v2.mrz.is_none(), "no MRZ on the Tier-2 path");
    }

    #[test]
    fn lift_from_wasm_client_is_proven_browser_side() {
        let mut v1 = croatian_specimen_v1();
        v1.extraction_method = "mrz-wasm-client".into();
        let v2 = ExtractionV2::from(v1);
        assert_eq!(v2.provenance, Provenance::WasmClient);
        assert!(v2.confidence.all_proven());
    }

    #[test]
    fn lift_from_unrecognized_method_stays_total_and_honest() {
        let mut v1 = Extraction::default();
        v1.extraction_method = "some-future-producer".into();
        let v2 = ExtractionV2::from(v1);
        // Unrecognized producers degrade to the least-confident reading
        // rather than guessing provenance they don't have.
        assert!(matches!(v2.provenance, Provenance::Llm { .. }));
        assert_eq!(v2.confidence, FieldConfidence::llm_heuristic());
    }

    #[test]
    fn serde_round_trip_preserves_everything() {
        let mut v2 = ExtractionV2::from(croatian_specimen_v1());
        v2.validity = Some(Validity {
            dates_well_formed: true,
            in_date: false,
            dob_before_expiry: true,
            days_until_expiry: Some(-100),
        });
        let s = serde_json::to_string(&v2).unwrap();
        let back: ExtractionV2 = serde_json::from_str(&s).unwrap();
        assert_eq!(v2, back);
    }

    #[test]
    fn schema_version_always_serialized_and_defaults_to_two() {
        let v2 = ExtractionV2::default();
        let s = serde_json::to_string(&v2).unwrap();
        assert!(s.contains("\"schema_version\":2"));
        // A payload missing the field deserializes as v2, not 0.
        let minimal: ExtractionV2 = serde_json::from_str(
            r#"{"document":{"kind":"other"},"fields":{},"confidence":{},"provenance":{"kind":"llm","model":"x"},"extraction_method":"llm"}"#,
        )
        .unwrap();
        assert_eq!(minimal.schema_version, SCHEMA_VERSION_V2);
    }

    /// Schema snapshot: the exact v2 JSON shape for the Croatian specimen.
    /// If this test breaks, the wire format changed — that's a deliberate,
    /// reviewable event, not something to fix by editing the expected value
    /// blindly.
    #[test]
    fn v2_json_shape_snapshot_croatian_specimen() {
        let v2 = ExtractionV2::from(croatian_specimen_v1());
        let got = serde_json::to_value(&v2).unwrap();
        let expected = json!({
            "schema_version": 2,
            "document": { "kind": "passport", "mrz_format": "td3" },
            "fields": {
                "document_type": "P",
                "issuing_country": "HRV",
                "issuing_country_name": "Croatia",
                "document_number": "007007007",
                "surname": "SPECIMEN",
                "given_names": "SPECIMEN",
                "nationality": "HRV",
                "nationality_name": "Croatia",
                "date_of_birth": "1982-12-25",
                "sex": "F",
                "date_of_expiry": "2014-07-01",
                "personal_number": null
            },
            "confidence": {
                "document_type": 1.0,
                "issuing_country": 1.0,
                "document_number": 1.0,
                "surname": 1.0,
                "given_names": 1.0,
                "nationality": 1.0,
                "date_of_birth": 1.0,
                "sex": 1.0,
                "date_of_expiry": 1.0,
                "personal_number": 1.0
            },
            "provenance": { "kind": "mrz_checksum" },
            "mrz": {
                "lines": "P<HRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<<<<<<<<<<\n0070070071HRV8212258F1407019<<<<<<<<<<<<<<06",
                "format": "td3",
                "checks": {
                    "document_number": true,
                    "date_of_birth": true,
                    "date_of_expiry": true,
                    "personal_number": true,
                    "composite": true
                }
            },
            "extraction_method": "mrz-deterministic"
        });
        assert_eq!(got, expected);
    }

    #[test]
    fn empty_slots_are_omitted_from_the_wire() {
        // A Tier-2 lift: no mrz/validity/portrait/barcodes/documents keys at
        // all, so v1-shaped consumers see no noise.
        let mut v1 = Extraction::default();
        v1.extraction_method = "llm".into();
        let v2 = ExtractionV2::from(v1);
        let obj = serde_json::to_value(&v2).unwrap();
        let obj = obj.as_object().unwrap();
        for key in ["mrz", "validity", "portrait", "barcodes", "documents"] {
            assert!(!obj.contains_key(key), "empty slot leaked: {key}");
        }
        for key in [
            "schema_version",
            "document",
            "fields",
            "confidence",
            "provenance",
            "extraction_method",
        ] {
            assert!(obj.contains_key(key), "missing key: {key}");
        }
    }
}
