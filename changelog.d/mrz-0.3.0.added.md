- **`mrz` 0.3.0: robust leap-year calendar validation, first-class MRV visas, and a live browser
  demo.** `Date::is_well_formed` now runs the true Gregorian calendar instead of a generous
  `1..=31` day check — Feb 30, Feb 29 in a non-leap year, and April/June/September/November 31 are
  all rejected, while Feb 29 in a leap year is correctly accepted. The new `is_leap_year` is public;
  a property test proves `is_well_formed()` agrees with the `to_epoch_days`/`from_epoch_days`
  civil-calendar round-trip for every year 1900-2100. MRV-A and MRV-B visas (ICAO 9303 part 7) are
  now first-class citizens end to end: new `format_mrv_a`/`format_mrv_b` (with `MrvAFields`/
  `MrvBFields`) emit the two-line visa MRZ the same way `format_td3`/`format_td2` already do for
  passports and ID cards, and `find_and_parse` now scans free-form OCR text for `V`-prefixed visa
  lines — including HTML-escaped fillers and lines merged onto one physical line — running the same
  checksum-guided repair machinery as TD3/TD2. (MRV-A and MRV-B share the same `V<` document code
  with no length hint of their own, so the scanner tries the narrower MRV-B geometry before MRV-A to
  avoid a short MRV-B line being loosely padded up and misread as MRV-A.) The README now links the
  live WASM demo at the top and marks both MRV rows `✅ parse + emit`, leaving document-number
  overflow as the only remaining known gap.
