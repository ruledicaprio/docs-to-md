//! Field-level parity harness: compares `NativeLlm` extraction against the
//! known-good ground truth in `samples/*.json` (produced by the deterministic
//! MRZ pipeline, so these are exact-answer fixtures, not another model's
//! opinion). Ignored by default — needs the ~1 GB GGUF at the repo root; run
//! explicitly with:
//!
//! ```sh
//! cargo test -p synthpass-llm --test parity -- --ignored --nocapture
//! ```
//!
//! This is a regression smoke check, not a accuracy gate: a small 1.5B model
//! reading OCR'd Markdown will not hit 100% field accuracy (e.g. it may return
//! `"CROATIA"` where the fixture has the ISO code `"HRV"`), so this asserts a
//! minimum per-field match rate across the whole sample set rather than exact
//! equality per file. A regression that tanks the match rate is the signal to
//! watch for — not any single field on any single document.

use std::path::PathBuf;
use synthpass_core::Extraction;
use synthpass_llm::NativeLlm;

/// Samples with both OCR Markdown and a ground-truth extraction fixture.
const FIXTURES: &[&str] = &[
    "Croatian_passport_data_page",
    "Estonia_PASSPORT_face",
    "Passport_of_Serbia_ID_2009_version",
    "SerbianID_back",
    "Slovenian_ID_Card_2022_-_Rear",
    "2022_cetis_terra_condifea_passport_datapage3rd_inner_page",
];

/// Fields worth comparing: present in the prompt schema and stable enough
/// across documents to be a meaningful accuracy signal.
const FIELDS: &[fn(&Extraction) -> &Option<String>] = &[
    |e| &e.document_number,
    |e| &e.surname,
    |e| &e.given_names,
    |e| &e.nationality,
    |e| &e.date_of_birth,
    |e| &e.sex,
    |e| &e.date_of_expiry,
];
const FIELD_NAMES: &[&str] = &[
    "document_number",
    "surname",
    "given_names",
    "nationality",
    "date_of_birth",
    "sex",
    "date_of_expiry",
];

fn normalize(s: &str) -> String {
    s.trim().to_uppercase()
}

/// `DD.MM.YYYY` -> `YYYY-MM-DD`, the model's favorite date rendering vs. the
/// fixtures' ISO form. Falls back to the input unchanged if it isn't that shape.
fn normalize_date(s: &str) -> String {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if let [d, m, y] = parts[..] {
        if d.len() <= 2 && m.len() <= 2 && y.len() == 4 {
            return format!("{y}-{m:0>2}-{d:0>2}");
        }
    }
    normalize(s)
}

fn fields_match(a: &Option<String>, b: &Option<String>, is_date: bool) -> bool {
    match (a, b) {
        (Some(a), Some(b)) if is_date => normalize_date(a) == normalize_date(b),
        (Some(a), Some(b)) => normalize(a) == normalize(b),
        (None, None) => true,
        _ => false,
    }
}

#[test]
#[ignore]
fn native_llm_field_accuracy_over_sample_set() {
    let model_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../qwen2.5-1.5b-instruct-q4_k_m.gguf");
    assert!(
        model_path.exists(),
        "model not found at {} — download it first",
        model_path.display()
    );
    let samples_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples");

    let llm = NativeLlm::load(&model_path, 2048).expect("model loads");

    let mut total = 0usize;
    let mut matched = 0usize;

    for name in FIXTURES {
        let md_path = samples_dir.join(format!("{name}.md"));
        let json_path = samples_dir.join(format!("{name}.json"));
        if !md_path.exists() || !json_path.exists() {
            eprintln!("skipping {name}: missing fixture files");
            continue;
        }

        let markdown = std::fs::read_to_string(&md_path).expect("markdown reads");
        // Ground-truth fixtures predate `extraction_method` being required;
        // backfill it the same way `repair::parse_extraction` does for model
        // output, so this harness doesn't need its own fixture format.
        let mut expected_value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&json_path).expect("json reads"))
                .expect("fixture is valid JSON");
        expected_value
            .as_object_mut()
            .expect("fixture is a JSON object")
            .entry("extraction_method")
            .or_insert_with(|| "mrz-deterministic".into());
        let expected: Extraction =
            serde_json::from_value(expected_value).expect("fixture parses as Extraction");
        let actual = llm.extract(&markdown).expect("extraction succeeds");

        println!("--- {name} ---");
        for (i, get) in FIELDS.iter().enumerate() {
            let exp = get(&expected);
            let act = get(&actual);
            let is_date = FIELD_NAMES[i] == "date_of_birth" || FIELD_NAMES[i] == "date_of_expiry";
            let ok = fields_match(exp, act, is_date);
            total += 1;
            matched += ok as usize;
            println!(
                "  {:<16} expected={:?} actual={:?} {}",
                FIELD_NAMES[i],
                exp,
                act,
                if ok { "OK" } else { "MISMATCH" }
            );
        }
    }

    let rate = matched as f64 / total as f64;
    println!(
        "\nfield match rate: {matched}/{total} ({:.1}%)",
        rate * 100.0
    );
    // The Atlas §8 acceptance criterion, reported rather than asserted: with
    // grammar-constrained decoding on, model output should already be a valid
    // JSON object, so nothing should reach the repair path. Printing it here
    // makes "the grammar is earning its keep" a measurement instead of a
    // belief — and makes it visible when it *isn't*.
    println!(
        "repair fallbacks: {} (grammar {})",
        synthpass_llm::repair::repair_fallbacks(),
        if std::env::var("SYNTHPASS_LLM_GRAMMAR").as_deref() == Ok("0") {
            "disabled"
        } else {
            "enabled"
        }
    );
    // Measured baseline with qwen2.5-1.5b-instruct-q4_k_m: ~33% (date-format
    // normalized). The model is weak on rear-side ID cards and heavily
    // garbled MRZ blocks (SerbianID_back, Slovenian rear) — that's expected
    // for a 1.5B model and is exactly why Tier 1 (deterministic MRZ) exists;
    // this floor exists to catch a regression (e.g. a broken prompt or a
    // repair-JSON bug), not to gate on model quality.
    assert!(
        rate >= 0.25,
        "native LLM field accuracy regressed: {matched}/{total} ({:.1}%) below the 25% floor",
        rate * 100.0
    );
}
