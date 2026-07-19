#![no_main]

use libfuzzer_sys::fuzz_target;

// Drives the TD1/TD2/TD3 line parsers directly (bypassing find_and_parse's
// scanning step) so the fuzzer spends its budget on the parsers' own
// length/index/checksum logic rather than re-discovering "which lines look
// MRZ-shaped" every run. Splits the input on newlines and feeds however many
// lines are available to whichever parsers that count supports.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();

    if lines.len() >= 2 {
        let _ = mrz::parse_td3(lines[0], lines[1]);
        let _ = mrz::parse_td2(lines[0], lines[1]);
    }
    if lines.len() >= 3 {
        let _ = mrz::parse_td1(lines[0], lines[1], lines[2]);
    }
});
