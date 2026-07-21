- **`mrz` can now emit TD1 and TD2 MRZs, not just TD3.** New `format_td2` /
  `format_td1` functions (and their `Td2Fields` / `Td1Fields` input structs, mirroring
  `Td3Fields`'s style and `serde` derives) are the deterministic inverse of `parse_td2` /
  `parse_td1` — every check digit (document number, date of birth, date of expiry,
  composite) is computed with the same `check_digit` math the parsers verify against, so
  feeding the output back through `parse_td2` / `parse_td1` always yields `valid() ==
  true`. `crates/mrz/tests/roundtrip.rs` gained ICAO-specimen byte-for-byte tests and a
  `proptest` per format alongside the existing TD3 coverage. `crates/mrz/README.md`'s
  format table now shows "parse + emit" for all three formats. No behaviour change to
  `parse_td1` / `parse_td2` / `parse_td3` or to the base crate's zero-dependency,
  `wasm32-unknown-unknown` footprint.
