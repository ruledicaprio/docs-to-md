//! JSON repair for small quantized models, which frequently drift: code
//! fences, prose around the object, and trailing commas. A port of
//! `python/inferer/adapter.py::repair_json`, plus schema validation into the
//! canonical [`synthpass_core::Extraction`] (replacing the Python side's Pydantic
//! validation).
//!
//! Since [`crate::grammar`] landed, this is a **fallback**, not the main path:
//! grammar-constrained decoding can't produce the drift these routines undo.
//! [`parse_extraction`] therefore tries a plain parse first and only reaches
//! for repair when that fails, counting each such occasion in
//! [`repair_fallbacks`] — so "the grammar is working" is a number, not a
//! belief. The routines stay because the grammar can legitimately be
//! unavailable (see [`crate::build_sampler`]) and because a model can still
//! stop mid-object at the token cap.

use std::sync::atomic::{AtomicU64, Ordering};
use synthpass_core::Extraction;

/// Count of [`parse_extraction`] calls that needed the repair path. Process-
/// global and monotonic: the corpus harness reads it once at the end of a run,
/// so there's nothing to reset and no per-call ownership to thread through.
static REPAIR_FALLBACKS: AtomicU64 = AtomicU64::new(0);

/// How many extractions so far have fallen back to JSON repair.
///
/// The Atlas §8 acceptance criterion is that this stays at zero across the
/// corpus once grammar-constrained decoding is on. It counts *attempts*, not
/// successes — a fallback that then fails the schema is still a fallback.
pub fn repair_fallbacks() -> u64 {
    REPAIR_FALLBACKS.load(Ordering::Relaxed)
}

/// `true` when `raw` isn't already a single JSON object and would have to go
/// through [`repair_json_text`] to become one.
///
/// Pure — unlike [`repair_fallbacks`], which aggregates across a whole process
/// — so a caller can ask the question about one document, and a test can
/// assert on it without racing other tests through a shared counter.
pub fn needs_repair(raw: &str) -> bool {
    !matches!(
        serde_json::from_str::<serde_json::Value>(raw.trim()),
        Ok(value) if value.is_object()
    )
}

/// Best-effort cleanup of a model's raw JSON output: strips ```json fences,
/// narrows to the outermost `{...}`, and removes trailing commas before a
/// closing brace/bracket. Does not itself validate the result is valid JSON.
pub fn repair_json_text(raw: &str) -> String {
    let mut s = raw.trim();

    // Strip a leading ```json / ``` fence (language tag up to the newline).
    if let Some(rest) = s.strip_prefix("```") {
        s = match rest.find('\n') {
            Some(nl) => &rest[nl + 1..],
            None => rest,
        };
    }
    // Strip a trailing ``` fence, only if nothing but whitespace follows it.
    if let Some(pos) = s.rfind("```") {
        if s[pos + 3..].trim().is_empty() {
            s = &s[..pos];
        }
    }
    let s = s.trim();

    // Narrow to the outermost JSON object if there is surrounding prose.
    let narrowed = match (s.find('{'), s.rfind('}')) {
        (Some(start), Some(end)) if end > start => &s[start..=end],
        _ => s,
    };

    // Remove trailing commas: `,}` / `,]` (optionally with whitespace between).
    strip_trailing_commas(narrowed)
}

/// Removes a comma that appears (possibly followed by whitespace) directly
/// before a closing `}` or `]`.
fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1; // drop the comma, keep scanning from the whitespace
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Repair and parse a model's raw output into the canonical [`Extraction`]
/// schema, then force `extraction_method` to `"llm"` regardless of what the
/// model echoed (mirrors `extraction_from_response` in synthpass-pipeline). The
/// model is never asked for `extraction_method` (see `prompt::FIELDS`), so it
/// is injected into the JSON *before* deserialization — `Extraction` requires
/// the field, unlike the other, optional, ICAO columns.
pub fn parse_extraction(raw: &str) -> Result<Extraction, String> {
    // Fast path: grammar-constrained output is already exactly one JSON
    // object, so repairing it would be a no-op on every byte. The re-parse
    // costs microseconds against a generation that took ~a second, and buys a
    // race-free predicate ([`needs_repair`]) that callers and tests can use
    // without reading a global counter.
    if !needs_repair(raw) {
        let value = serde_json::from_str(raw.trim()).expect("needs_repair just parsed this");
        return into_extraction(value);
    }

    REPAIR_FALLBACKS.fetch_add(1, Ordering::Relaxed);
    let cleaned = repair_json_text(raw);
    let value: serde_json::Value =
        serde_json::from_str(&cleaned).map_err(|e| format!("invalid JSON after repair: {e}"))?;
    if !value.is_object() {
        return Err("model output was valid JSON but not an object".to_string());
    }
    into_extraction(value)
}

/// Inject `extraction_method` and deserialize into the canonical schema.
///
/// The model is never asked for `extraction_method` (see [`crate::prompt`]),
/// so it's set here rather than trusted from the output — `Extraction`
/// requires the field, unlike the other, optional, ICAO columns.
fn into_extraction(mut value: serde_json::Value) -> Result<Extraction, String> {
    value
        .as_object_mut()
        .expect("callers check is_object() first")
        .insert("extraction_method".to_string(), "llm".into());
    serde_json::from_value(value).map_err(|e| format!("JSON did not match Extraction schema: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_code_fences() {
        let raw = "```json\n{\"surname\": \"DOE\"}\n```";
        assert_eq!(repair_json_text(raw), "{\"surname\": \"DOE\"}");
    }

    #[test]
    fn strips_bare_fences() {
        let raw = "```\n{\"surname\": \"DOE\"}\n```";
        assert_eq!(repair_json_text(raw), "{\"surname\": \"DOE\"}");
    }

    #[test]
    fn narrows_to_outermost_object_with_surrounding_prose() {
        let raw = "Sure, here is the JSON:\n{\"surname\": \"DOE\"}\nHope that helps!";
        assert_eq!(repair_json_text(raw), "{\"surname\": \"DOE\"}");
    }

    #[test]
    fn strips_trailing_commas() {
        let raw = r#"{"a": 1, "b": [1, 2,], "c": 3,}"#;
        assert_eq!(repair_json_text(raw), r#"{"a": 1, "b": [1, 2], "c": 3}"#);
    }

    #[test]
    fn parse_extraction_forces_method_to_llm() {
        let raw = r#"{"surname": "DOE", "document_number": "X1", "extraction_method": "will-be-overwritten"}"#;
        let e = parse_extraction(raw).expect("parses");
        assert_eq!(e.surname.as_deref(), Some("DOE"));
        assert_eq!(e.extraction_method, "llm");
    }

    #[test]
    fn parse_extraction_rejects_garbage() {
        assert!(parse_extraction("not json at all").is_err());
    }

    #[test]
    fn clean_json_does_not_need_repair() {
        assert!(!needs_repair(r#"{"surname":"DOE"}"#));
        assert!(
            !needs_repair("  {\"surname\": \"DOE\"}\n"),
            "surrounding whitespace alone is not drift"
        );
    }

    #[test]
    fn the_drift_this_module_exists_for_needs_repair() {
        for drifted in [
            "```json\n{\"surname\": \"DOE\"}\n```",
            "Sure, here is the JSON:\n{\"surname\": \"DOE\"}",
            r#"{"surname": "DOE",}"#,
            "not json at all",
        ] {
            assert!(
                needs_repair(drifted),
                "should have been routed to repair: {drifted}"
            );
        }
    }

    #[test]
    fn a_bare_json_scalar_is_not_an_extraction() {
        // Valid JSON, wrong shape. It must not sneak through the fast path as
        // if it were an object.
        assert!(needs_repair("\"just a string\""));
        assert!(needs_repair("[1, 2, 3]"));
        assert!(parse_extraction("[1, 2, 3]").is_err());
    }

    #[test]
    fn repair_fallbacks_counts_only_the_drifted() {
        // Monotonic and process-global, so this asserts direction rather than
        // an exact delta — other tests share the counter.
        let before = repair_fallbacks();
        let _ = parse_extraction("```json\n{\"surname\": \"DOE\"}\n```");
        assert!(
            repair_fallbacks() > before,
            "a fenced document must be counted as a repair fallback"
        );
    }
}
