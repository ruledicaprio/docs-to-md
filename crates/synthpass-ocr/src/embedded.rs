//! Compile-time-embedded `.rten` model bytes, present only when the
//! `embedded-models` feature is on (see `build.rs`, which verifies these
//! files' SHA-256 before they ever reach `include_bytes!`).

pub static DETECTION_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/text-detection.rten"));
pub static RECOGNITION_BYTES: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/text-recognition.rten"));
