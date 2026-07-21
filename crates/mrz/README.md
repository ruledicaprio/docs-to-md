# mrz

A zero-dependency [ICAO 9303](https://www.icao.int/publications/pages/publication.aspx?docnum=9303)
Machine Readable Zone (MRZ) parser and check-digit validator for Rust.

A valid composite check digit is **mathematical proof** that an OCR read is
faithful to the printed document — not a probability, not a model score. `mrz`
verifies every printed check digit under the standard 7-3-1 weighting and
exposes the result per field, so you always know *which* digit proved (or
failed) the read.

The core crate has **no runtime dependencies** and compiles to
`wasm32-unknown-unknown` as readily as to native targets.

## Supported formats

| Format | Document                          | Layout            | Status |
| ------ | --------------------------------- | ----------------- | ------ |
| TD3    | Passports                         | 2 lines × 44      | ✅ parse + emit |
| TD2    | Official travel documents / IDs   | 2 lines × 36      | ✅ parse |
| TD1    | ID cards                          | 3 lines × 30      | ✅ parse |
| MRV-A / MRV-B | Visas                      | —                 | ❌ not yet |

Emission (`format_td3`) currently covers **TD3 only**. Machine Readable Visa
(MRV-A / MRV-B) parsing and document-number overflow (numbers longer than the
9-character field) are known gaps tracked for a later release.

## Usage

Add it to `Cargo.toml`:

```toml
[dependencies]
mrz = "0.1"
```

### Scan free-form OCR text

`find_and_parse` locates an MRZ inside noisy OCR output (it tolerates
HTML-escaped fillers and lines merged onto one physical line) and drives a
check-digit-guided repair pass — a candidate reading is accepted only when its
composite check digit proves it.

```rust
let text = "## REPUBLIC OF UTOPIA\n\
            P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<\n\
            L898902C36UTO7408122F1204159ZE184226B<<<<<10";

let doc = mrz::find_and_parse(text).expect("an MRZ");
assert_eq!(doc.surname, "ERIKSSON");
assert_eq!(doc.given_names, "ANNA MARIA");
assert!(doc.valid()); // every check digit verified
```

### Parse known lines directly

```rust
let doc = mrz::parse_td3(
    "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<",
    "L898902C36UTO7408122F1204159ZE184226B<<<<<10",
).unwrap();

// Per-field proof, not a single boolean.
assert!(doc.checks.document_number);
assert!(doc.checks.date_of_birth);
assert!(doc.checks.composite);
```

`parse_td1` and `parse_td2` cover the other two formats.

### Emit a valid TD3 zone

```rust
use mrz::{format_td3, Td3Fields};

let lines = format_td3(&Td3Fields {
    issuing_country: "UTO".into(),
    document_number: "L898902C3".into(),
    surname: "ERIKSSON".into(),
    given_names: "ANNA MARIA".into(),
    nationality: "UTO".into(),
    date_of_birth: "740812".into(), // YYMMDD, MRZ-native
    sex: "F".into(),
    date_of_expiry: "120415".into(),
    personal_number: Some("ZE184226B".into()),
    ..Default::default()
});
// Round-trips: parse_td3(&lines) is valid() == true.
```

## A valid read is not an in-date document

A verified composite check digit proves the *read* is faithful — it says
nothing about whether the document is expired or its dates are internally
consistent. That separate, non-cryptographic judgement is
`MrzData::validity(today)`:

```rust
use mrz::Date;

let report = doc.validity(Date::new(2010, 1, 1));
assert!(report.in_date);
assert!(report.dob_before_expiry);
```

## Feature flags

Both are off by default, keeping the base crate zero-dependency (and
wasm-clean):

- **`serde`** — derives `Serialize` + `Deserialize` on `MrzData`, `Checks`,
  `Format`, `Td3Fields`, `Date`, and `DateValidity`.
- **`zeroize`** — derives `ZeroizeOnDrop` on `MrzData`, wiping the PII-bearing
  `String` fields from memory when the value is dropped.

## License

MIT
