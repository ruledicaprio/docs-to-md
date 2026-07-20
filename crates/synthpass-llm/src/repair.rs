//! JSON repair for small quantized models, which frequently drift: code
//! fences, prose around the object, and trailing commas. A port of
//! `python/inferer/adapter.py::repair_json`, plus schema validation into the
//! canonical [`synthpass_core::Extraction`] (replacing the Python side's Pydantic
//! validation).

use synthpass_core::Extraction;

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
    let cleaned = repair_json_text(raw);
    let mut value: serde_json::Value =
        serde_json::from_str(&cleaned).map_err(|e| format!("invalid JSON after repair: {e}"))?;
    let obj = value
        .as_object_mut()
        .ok_or("model output was valid JSON but not an object")?;
    obj.insert("extraction_method".to_string(), "llm".into());
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
}
