#![no_main]

use libfuzzer_sys::fuzz_target;

// find_and_parse scans free text (e.g. real OCR Markdown output) for an
// MRZ-shaped block, driving the checksum-verified OCR-repair machinery.
// Only valid UTF-8 input is meaningful here (OCR output is always text);
// invalid UTF-8 is skipped rather than lossily converted, since that would
// just be re-fuzzing String::from_utf8_lossy instead of this crate.
fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = mrz::find_and_parse(text);
    }
});
