- **Structured OCR geometry API in `synthpass-ocr` (M5).** `NativeOcr::recognize_detailed` returns
  a new `OcrPage` (plain owned `BBox`/`OcrLine` types — no `ocrs`/`rten` types leak into the public
  API) with per-line text/bounding boxes, an auto-detected page rotation, and two layout
  heuristics: `mrz_band` (content-and-geometry scored — MRZ-charset density, TD1/TD2/TD3 line
  length, OCR-B glyph aspect ratio — as an additive signal alongside `preprocess.rs`'s existing
  blind bottom-band crop, which stays as the retry loop's fallback, untouched) and `portrait`
  (largest text-free region in the upper-left quadrant with a ~3:4 aspect ratio — **crop
  coordinates only**; this and every future portrait feature stays bounding-box cropping, never
  face recognition or biometric matching, per `VISION.md` §2). Orientation detection
  (`choose_rotation`) is a cheap, detection-only 0°/90°/180°/270° heuristic, conservatively biased
  toward "no rotation" so it never fires on an already-upright page. `NativeOcr::recognize`'s
  signature and returned text are unchanged — it is now `recognize_detailed(..)?.text`, with the
  original retry-pass loop, pass budget, and time budget moved verbatim (unedited) inside
  `recognize_detailed`; the new orientation/geometry steps run before that loop's timer starts, so
  they add to this call's total latency but never eat into the retry loop's own wall-clock budget.
  Zero new dependencies — `ocrs`'s detected-word/line geometry (`RotatedRect`/`Rect`, from its
  `rten-imageproc` transitive dependency, not re-exported by `ocrs` itself) is consumed via
  inherent methods and inferred locals only, so this crate's `Cargo.toml` is unchanged.
- **Deterministic field normalizers in `synthpass-core` (M5).** A new `normalize` module of pure,
  idempotent functions fixes normalization-only parity misses between Tier-1 (MRZ) and Tier-2
  (LLM) extractions — e.g. `"CROATIA"` vs `HRV`, `"JAAK-KRISTJAN"` vs `JAAK KRISTJAN` — with no
  model and no new dependency: `issuing_country`/`nationality` (full country name → ICAO/ISO
  3166-1 code, via a new `mrz::code_for_name` reverse lookup added over the *same* table
  `mrz::country_name` already uses, so the two directions can't drift apart), `given_names`
  (MRZ-convention `<`/`-` → space, whitespace collapse), `date` (day-first/unseparated/loosely-
  padded input → the strict `YYYY-MM-DD` form `mrz::dates::parse_iso` requires), and `sex`/
  `document_type` (long forms → single-letter ICAO codes). Table-driven unit tests only — not yet
  wired into `synthpass-pipeline`, which is a separate integration step.
