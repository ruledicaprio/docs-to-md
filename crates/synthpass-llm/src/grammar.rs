//! GBNF grammar for Tier-2 decoding (Atlas §8).
//!
//! llama.cpp can constrain sampling to a grammar, masking every token that
//! couldn't continue a conforming string. Applied to the extraction schema,
//! that makes malformed output *unrepresentable* rather than something
//! [`crate::repair`] has to salvage after the fact: no code fences, no prose
//! preamble, no trailing commas, no truncated object, no invented keys.
//!
//! **Structure only, deliberately.** The grammar pins the JSON *shape* — the
//! exact ten keys of [`crate::prompt::FIELDS`], in order, each holding a
//! string or `null`. It does **not** constrain the values themselves (date
//! layout, `sex` vocabulary, country-code casing), because
//! [`synthpass_core::Extraction`] accepts any string in those slots. Encoding
//! a narrower dialect here would be a semantic change to what the model may
//! say about a document, not the syntactic guarantee this module is for, and
//! it belongs behind its own measurement rather than riding along with this
//! one.
//!
//! The grammar is *generated from* [`crate::prompt::FIELDS`] rather than
//! written out by hand, so the fields the model is asked for and the fields it
//! is permitted to emit cannot drift apart.

use crate::prompt::FIELDS;

/// The entry-point rule name, as passed to `LlamaSampler::grammar`.
pub const GRAMMAR_ROOT: &str = "root";

/// The value/​string/​whitespace rules, kept byte-compatible with the reference
/// `json.gbnf` that ships with `llama-cpp-2` — same escape handling, same
/// negated character class. `value` is narrowed to `string | "null"` because
/// every field of [`synthpass_core::Extraction`] this grammar covers is an
/// `Option<String>`: a number or nested object there would parse as JSON and
/// then fail the schema, which is exactly the failure class this exists to
/// remove.
const VALUE_RULES: &str = r#"
value  ::= string | "null"
string ::= "\"" ( [^"\\] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F]) )* "\""
ws     ::= [ \t\n]*
"#;

/// Build the GBNF constraining Tier-2 output to the extraction schema.
///
/// Field order is fixed (it mirrors [`FIELDS`]) rather than free: a fixed
/// order is a strictly smaller search space, and nothing downstream cares
/// about key order since `serde` matches by name.
pub fn extraction_gbnf() -> String {
    let mut gbnf = String::from("root ::= \"{\" ws");
    for (i, field) in FIELDS.iter().enumerate() {
        if i > 0 {
            gbnf.push_str(" \",\" ws");
        }
        // Emits e.g.  "\"surname\"" ws ":" ws value ws
        gbnf.push_str(&format!(" \"\\\"{field}\\\"\" ws \":\" ws value ws"));
    }
    gbnf.push_str(" \"}\"\n");
    gbnf.push_str(VALUE_RULES);
    gbnf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_names_every_prompted_field_in_order() {
        let gbnf = extraction_gbnf();
        let mut cursor = 0;
        for field in FIELDS {
            let needle = format!("\\\"{field}\\\"");
            let at = gbnf[cursor..]
                .find(&needle)
                .unwrap_or_else(|| panic!("grammar is missing the '{field}' key:\n{gbnf}"));
            cursor += at + needle.len();
        }
    }

    #[test]
    fn grammar_defines_every_rule_it_references() {
        let gbnf = extraction_gbnf();
        for rule in ["root", "value", "string", "ws"] {
            assert!(
                gbnf.contains(&format!("{rule} ")),
                "grammar references '{rule}' but never defines it:\n{gbnf}"
            );
        }
    }

    #[test]
    fn a_value_is_only_ever_a_string_or_null() {
        let gbnf = extraction_gbnf();
        let value_rule = gbnf
            .lines()
            .find(|l| l.starts_with("value"))
            .expect("the grammar defines a `value` rule");
        let alternatives: Vec<&str> = value_rule
            .split("::=")
            .nth(1)
            .expect("a rule has a right-hand side")
            .split('|')
            .map(str::trim)
            .collect();
        assert_eq!(
            alternatives,
            ["string", "\"null\""],
            "every field this grammar covers is an Option<String>; anything \
             else (a number, an object) would parse as JSON and then fail the \
             schema — which is the failure class this grammar exists to remove"
        );
    }

    /// The drift guard that matters: a document in exactly the shape this
    /// grammar permits must land in `Extraction` with **every** field
    /// populated. `serde` ignores unknown keys, so a typo in a grammar field
    /// name would otherwise pass silently — here it shows up as a `None`.
    #[test]
    fn grammar_shaped_output_populates_every_extraction_field() {
        // Distinct value per field so a mix-up can't hide behind a match.
        let body: Vec<String> = FIELDS
            .iter()
            .map(|f| format!("\"{f}\":\"value-of-{f}\""))
            .collect();
        let doc = format!("{{{}}}", body.join(","));

        let e = crate::repair::parse_extraction(&doc)
            .expect("grammar-shaped output must satisfy the Extraction schema");

        assert_eq!(e.document_type.as_deref(), Some("value-of-document_type"));
        assert_eq!(
            e.issuing_country.as_deref(),
            Some("value-of-issuing_country")
        );
        assert_eq!(
            e.document_number.as_deref(),
            Some("value-of-document_number")
        );
        assert_eq!(e.surname.as_deref(), Some("value-of-surname"));
        assert_eq!(e.given_names.as_deref(), Some("value-of-given_names"));
        assert_eq!(e.nationality.as_deref(), Some("value-of-nationality"));
        assert_eq!(e.date_of_birth.as_deref(), Some("value-of-date_of_birth"));
        assert_eq!(e.sex.as_deref(), Some("value-of-sex"));
        assert_eq!(e.date_of_expiry.as_deref(), Some("value-of-date_of_expiry"));
        assert_eq!(e.mrz_line.as_deref(), Some("value-of-mrz_line"));
        assert_eq!(e.extraction_method, "llm");
    }

    /// ...and it must arrive without the repair layer having to touch it.
    /// This is the Atlas §8 acceptance criterion in miniature.
    #[test]
    fn grammar_shaped_output_needs_no_repair() {
        let body: Vec<String> = FIELDS.iter().map(|f| format!("\"{f}\":null")).collect();
        let doc = format!("{{{}}}", body.join(","));

        assert!(
            !crate::repair::needs_repair(&doc),
            "grammar-conforming output must take the clean parse path"
        );
        crate::repair::parse_extraction(&doc).expect("an all-null document is still schema-valid");
    }

    #[test]
    fn grammar_permits_whitespace_a_model_would_naturally_emit() {
        // The grammar allows optional whitespace around structural tokens, so
        // pretty-printed output is conforming too — and must still parse.
        let doc = "{\n  \"document_type\": \"P\",\n  \"surname\": null\n}";
        assert!(
            serde_json::from_str::<serde_json::Value>(doc).is_ok(),
            "the whitespace shape the grammar permits must be valid JSON"
        );
    }
}
