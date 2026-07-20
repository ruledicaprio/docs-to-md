// Known-good SHA-256 hashes for the two `.rten` model files. Kept in this
// standalone, dependency-free file and pulled in via `include!` by both
// runtime verification (crate::verify) and the `embedded-models` build
// script (build.rs), so the two can never drift out of sync. Plain `//`
// comments only (not `//!`) — build.rs includes this inside a fn body, where
// inner doc comments aren't valid.

// `text-detection.rten` from the fixed `ocrs-models` S3 bucket this workspace
// ships against.
pub const KNOWN_GOOD_SHA256_DETECTION: &str =
    "f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca";
// `text-recognition.rten` from the fixed `ocrs-models` S3 bucket this
// workspace ships against.
pub const KNOWN_GOOD_SHA256_RECOGNITION: &str =
    "e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e";
