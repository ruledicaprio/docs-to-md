//! `format_td3` round-trip tests: the emitter is correct iff it is the exact
//! inverse of `parse_td3`.

use mrz::{format_td3, parse_td3, Td3Fields};
use proptest::prelude::*;

// Official ICAO 9303 part 4 specimen (Utopia / Anna Maria Eriksson) — same
// constants as the ones pinned in `src/lib.rs`'s test module.
const TD3_L1: &str = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
const TD3_L2: &str = "L898902C36UTO7408122F1204159ZE184226B<<<<<10";

#[test]
fn specimen_byte_for_byte() {
    let fields = Td3Fields {
        document_code: "P".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "L898902C3".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        personal_number: Some("ZE184226B".to_string()),
    };

    let expected = format!("{TD3_L1}\n{TD3_L2}");
    assert_eq!(format_td3(&fields), expected);
}

#[test]
fn specimen_round_trips_as_valid() {
    let fields = Td3Fields {
        document_code: "P".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "L898902C3".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        personal_number: Some("ZE184226B".to_string()),
    };

    let mrz = format_td3(&fields);
    let (l1, l2) = mrz.split_once('\n').unwrap();
    let d = parse_td3(l1, l2).unwrap();
    assert!(d.valid(), "checks: {:?}", d.checks);
}

fn doc_number_strategy() -> impl Strategy<Value = String> {
    "[A-Z0-9]{1,9}"
}

fn name_strategy() -> impl Strategy<Value = String> {
    // Capped so surname + "<<" + given_names never exceeds the 39-char name
    // field (18 + 2 + 18 = 38 <= 39) — an over-long name is a truncation
    // concern, not a check-digit concern, and out of scope for M1.
    "[A-Z]{1,18}"
}

fn yymmdd_strategy() -> impl Strategy<Value = String> {
    // Keep month/day within always-valid ranges so `expand_date` accepts
    // them cleanly (no plausibility rejection to work around here).
    (0u32..100, 1u32..=12, 1u32..=28)
        .prop_map(|(yy, mm, dd)| format!("{yy:02}{mm:02}{dd:02}"))
}

fn sex_strategy() -> impl Strategy<Value = String> {
    prop_oneof![Just("M".to_string()), Just("F".to_string()), Just("X".to_string())]
}

fn personal_number_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        "[A-Z0-9]{1,14}".prop_map(Some),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_fields_round_trip(
        document_number in doc_number_strategy(),
        surname in name_strategy(),
        given_names in name_strategy(),
        date_of_birth in yymmdd_strategy(),
        sex in sex_strategy(),
        date_of_expiry in yymmdd_strategy(),
        personal_number in personal_number_strategy(),
    ) {
        let fields = Td3Fields {
            document_code: "P".to_string(),
            issuing_country: "UTO".to_string(),
            document_number: document_number.clone(),
            surname: surname.clone(),
            given_names: given_names.clone(),
            nationality: "UTO".to_string(),
            date_of_birth: date_of_birth.clone(),
            sex: sex.clone(),
            date_of_expiry: date_of_expiry.clone(),
            personal_number: personal_number.clone(),
        };

        let mrz = format_td3(&fields);
        let (l1, l2) = mrz.split_once('\n').unwrap();
        prop_assert_eq!(l1.len(), 44);
        prop_assert_eq!(l2.len(), 44);

        let parsed = parse_td3(l1, l2).unwrap();
        prop_assert!(parsed.valid(), "checks: {:?}", parsed.checks);

        prop_assert_eq!(&parsed.document_number, &document_number);
        prop_assert_eq!(&parsed.surname, &surname);
        prop_assert_eq!(&parsed.given_names, &given_names);
        prop_assert_eq!(&parsed.sex, &sex);

        let expected_personal = personal_number.filter(|s| !s.is_empty());
        // Clone rather than move: `MrzData` derives `ZeroizeOnDrop` when the
        // workspace unifies `mrz`'s `zeroize` feature on (e.g. via
        // `synthpass-pipeline`), and a `Drop` type forbids partial moves out of it.
        prop_assert_eq!(parsed.personal_number.clone(), expected_personal);

        // `expand_date` turns YYMMDD into ISO YYYY-MM-DD; check the tail
        // (MM-DD) and that the parsed year's last two digits match.
        let expected_mmdd = &date_of_birth[2..6];
        prop_assert_eq!(&parsed.date_of_birth[5..7], &expected_mmdd[0..2]);
        prop_assert_eq!(&parsed.date_of_birth[8..10], &expected_mmdd[2..4]);
        prop_assert_eq!(&parsed.date_of_birth[2..4], &date_of_birth[0..2]);

        let expected_mmdd_exp = &date_of_expiry[2..6];
        prop_assert_eq!(&parsed.date_of_expiry[5..7], &expected_mmdd_exp[0..2]);
        prop_assert_eq!(&parsed.date_of_expiry[8..10], &expected_mmdd_exp[2..4]);
        prop_assert_eq!(&parsed.date_of_expiry[2..4], &date_of_expiry[0..2]);
    }
}
