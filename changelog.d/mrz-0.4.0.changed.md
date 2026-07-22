- **`mrz` 0.4.0 adds a public field to `MrzData`, which is a breaking change if you construct one
  literally.** `MrzData` gained `document_number_full: Option<String>`, so any code building the
  struct with a literal (rather than getting one from a `parse_*` function) now fails to compile
  with a missing-field error. Add `document_number_full: None` — that is exactly what the parsers
  produce for a document number that fits its 9-character field, so the behaviour is unchanged.
  Code that only *reads* `MrzData`, which is the overwhelmingly common case, needs no change.
  Callers matching exhaustively on `MrzError` are also affected: it is now `#[non_exhaustive]` and
  has a new `BadChecksum` variant, so a match over it needs a `_` arm.
