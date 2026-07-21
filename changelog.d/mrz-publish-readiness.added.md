- **The `mrz` crate is now publish-ready for crates.io as a standalone `0.1.0`.** Added the
  package metadata `cargo publish` requires (`description`, `license`, `repository`, `readme`,
  `keywords`, `categories`) and decoupled its version from the workspace `1.1.0` — this is the
  crate's first *public* release and the API still has documented gaps (MRV visas, document-number
  overflow), so `0.x` states that honestly. New `crates/mrz/README.md` becomes the crates.io
  landing page, with the supported-format table (TD1/TD2/TD3; MRV-A/-B explicitly "not yet") and
  the load-bearing caveat that a valid check digit proves a faithful *read*, not an in-date
  document. The `serde` feature now derives `Deserialize` alongside `Serialize` (JSON *in*, not
  just out) on `MrzData`, `Checks`, `Format`, `Td3Fields`, `Date`, and `DateValidity`, and a new
  `tests/icao_vectors.rs` pins the check-digit primitive against ICAO Doc 9303's own worked
  examples. `cargo publish -p mrz --dry-run` is clean. No default-feature or
  `wasm32-unknown-unknown` behaviour changes — the base crate stays zero-dependency.