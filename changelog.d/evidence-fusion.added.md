- **`ExtractionV2` no longer claims a check-digit proof it doesn't have, and a new
  `synthpass_core::fusion` module catches the corruption this was hiding.** ICAO 9303's composite
  check digit covers only `document_number`, `date_of_birth`, `date_of_expiry`, and
  `personal_number` (verified directly against `mrz::parser`'s composite byte ranges and the
  ICAO fixture — `nationality` and `sex` are excluded too, matching the published standard).
  `document_type`, `issuing_country`, `surname`, and `given_names` carry no check digit at all.
  Yet the native Tier-1 path stamped `FieldConfidence::proven()` — `1.0` on all ten fields —
  because nothing distinguished which fields the arithmetic actually covers.

  `FieldConfidence::mrz_checksum_scope()` replaces that for the native pipeline path (`proven()`
  itself is kept, unchanged, for the WASM demo, which doesn't yet make this distinction): the
  four check-digited fields stay at `1.0`, the other six drop to a new `MRZ_STRUCTURAL` (`0.9`)
  band — a real OCR+MRZ-charset read, more reliable than a Tier-2 guess, but never allowed to
  compare equal to a proof.

  `synthpass_core::fusion::check_line1_integrity` adds the deterministic checks that partially
  make up for the missing arithmetic, using tables that already ship rather than a model:
  `issuing_country` against the ICAO code table (`mrz::country_name`), `issuing_country` against
  `nationality` (two values parsed from different MRZ lines by the same OCR pass — an honest
  `Support::CrossField`, ranked below `Support::CheckDigit`, since neither side is itself
  checksum-proven), and an empty `given_names` beside a long `surname` — the exact signature of
  the collapsed-`<`-filler-run corruption measured on the synthetic corpus
  (`P<JPNSTRAND<<ALEKSANDER<<<…` → `PJPNSTRANDALEKSANDER<<<<…`).
