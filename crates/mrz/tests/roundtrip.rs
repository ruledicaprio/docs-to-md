//! `format_td3`/`format_td2`/`format_td1` round-trip tests: each emitter is
//! correct iff it is the exact inverse of its matching `parse_*` function.

use mrz::{
    format_mrv_a, format_mrv_b, format_td1, format_td2, format_td3, parse_mrv_a, parse_mrv_b,
    parse_td1, parse_td2, parse_td3, MrvAFields, MrvBFields, Td1Fields, Td2Fields, Td3Fields,
};
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
    (0u32..100, 1u32..=12, 1u32..=28).prop_map(|(yy, mm, dd)| format!("{yy:02}{mm:02}{dd:02}"))
}

fn sex_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("M".to_string()),
        Just("F".to_string()),
        Just("X".to_string())
    ]
}

fn personal_number_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), "[A-Z0-9]{1,14}".prop_map(Some),]
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

// Official ICAO 9303 part 6 (TD2) specimen (Utopia / Anna Maria Eriksson) —
// same constants as the ones pinned in `src/lib.rs`'s test module.
const TD2_L1: &str = "I<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<";
const TD2_L2: &str = "D231458907UTO7408122F1204159<<<<<<<6";

#[test]
fn td2_specimen_byte_for_byte() {
    let fields = Td2Fields {
        document_code: "I".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "D23145890".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        optional_data: None,
    };

    let expected = format!("{TD2_L1}\n{TD2_L2}");
    assert_eq!(format_td2(&fields), expected);
}

#[test]
fn td2_round_trips_as_valid() {
    let fields = Td2Fields {
        document_code: "I".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "D23145890".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        optional_data: None,
    };

    let mrz = format_td2(&fields);
    let (l1, l2) = mrz.split_once('\n').unwrap();
    let d = parse_td2(l1, l2).unwrap();
    assert!(d.valid(), "checks: {:?}", d.checks);
}

// Capped so surname + "<<" + given_names never exceeds the 31-char (TD2) /
// 30-char (TD1) name field (14 + 2 + 14 = 30 <= both) — narrower than
// `name_strategy` (which is sized for TD3's wider 39-char field).
fn short_name_strategy() -> impl Strategy<Value = String> {
    "[A-Z]{1,14}"
}

fn optional_data_strategy(max_len: usize) -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        proptest::string::string_regex(&format!("[A-Z0-9]{{1,{max_len}}}"))
            .unwrap()
            .prop_map(Some),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn td2_arbitrary_fields_round_trip(
        document_number in doc_number_strategy(),
        surname in short_name_strategy(),
        given_names in short_name_strategy(),
        date_of_birth in yymmdd_strategy(),
        sex in sex_strategy(),
        date_of_expiry in yymmdd_strategy(),
        optional_data in optional_data_strategy(7),
    ) {
        let fields = Td2Fields {
            document_code: "I".to_string(),
            issuing_country: "UTO".to_string(),
            document_number: document_number.clone(),
            surname: surname.clone(),
            given_names: given_names.clone(),
            nationality: "UTO".to_string(),
            date_of_birth: date_of_birth.clone(),
            sex: sex.clone(),
            date_of_expiry: date_of_expiry.clone(),
            optional_data,
        };

        let mrz = format_td2(&fields);
        let (l1, l2) = mrz.split_once('\n').unwrap();
        prop_assert_eq!(l1.len(), 36);
        prop_assert_eq!(l2.len(), 36);

        let parsed = parse_td2(l1, l2).unwrap();
        prop_assert!(parsed.valid(), "checks: {:?}", parsed.checks);

        prop_assert_eq!(&parsed.document_number, &document_number);
        prop_assert_eq!(&parsed.surname, &surname);
        prop_assert_eq!(&parsed.given_names, &given_names);
        prop_assert_eq!(&parsed.sex, &sex);
    }
}

// Official ICAO 9303 part 5 (TD1) specimen (Utopia / Anna Maria Eriksson) —
// same constants as the ones pinned in `src/lib.rs`'s test module.
const TD1_L1: &str = "I<UTOD231458907<<<<<<<<<<<<<<<";
const TD1_L2: &str = "7408122F1204159UTO<<<<<<<<<<<6";
const TD1_L3: &str = "ERIKSSON<<ANNA<MARIA<<<<<<<<<<";

#[test]
fn td1_specimen_byte_for_byte() {
    let fields = Td1Fields {
        document_code: "I".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "D23145890".to_string(),
        optional_data_1: None,
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        optional_data_2: None,
    };

    let expected = format!("{TD1_L1}\n{TD1_L2}\n{TD1_L3}");
    assert_eq!(format_td1(&fields), expected);
}

#[test]
fn td1_round_trips_as_valid() {
    let fields = Td1Fields {
        document_code: "I".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "D23145890".to_string(),
        optional_data_1: None,
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "740812".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "120415".to_string(),
        optional_data_2: None,
    };

    let mrz = format_td1(&fields);
    let mut lines = mrz.split('\n');
    let l1 = lines.next().unwrap();
    let l2 = lines.next().unwrap();
    let l3 = lines.next().unwrap();
    let d = parse_td1(l1, l2, l3).unwrap();
    assert!(d.valid(), "checks: {:?}", d.checks);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn td1_arbitrary_fields_round_trip(
        document_number in doc_number_strategy(),
        optional_data_1 in optional_data_strategy(15),
        surname in short_name_strategy(),
        given_names in short_name_strategy(),
        date_of_birth in yymmdd_strategy(),
        sex in sex_strategy(),
        date_of_expiry in yymmdd_strategy(),
        optional_data_2 in optional_data_strategy(11),
    ) {
        let fields = Td1Fields {
            document_code: "I".to_string(),
            issuing_country: "UTO".to_string(),
            document_number: document_number.clone(),
            optional_data_1,
            surname: surname.clone(),
            given_names: given_names.clone(),
            nationality: "UTO".to_string(),
            date_of_birth: date_of_birth.clone(),
            sex: sex.clone(),
            date_of_expiry: date_of_expiry.clone(),
            optional_data_2,
        };

        let mrz = format_td1(&fields);
        let mut lines = mrz.split('\n');
        let l1 = lines.next().unwrap();
        let l2 = lines.next().unwrap();
        let l3 = lines.next().unwrap();
        prop_assert_eq!(l1.len(), 30);
        prop_assert_eq!(l2.len(), 30);
        prop_assert_eq!(l3.len(), 30);

        let parsed = parse_td1(l1, l2, l3).unwrap();
        prop_assert!(parsed.valid(), "checks: {:?}", parsed.checks);

        prop_assert_eq!(&parsed.document_number, &document_number);
        prop_assert_eq!(&parsed.surname, &surname);
        prop_assert_eq!(&parsed.given_names, &given_names);
        prop_assert_eq!(&parsed.sex, &sex);
    }
}

// Verified MRV-A / MRV-B line-2 vectors (see `src/lib.rs`'s test module for
// the worked check-digit arithmetic behind these).
const MRV_A_L2: &str = "XK93054875BRA8502212F2703143R5T6U7V8W9<<<<<<";
const MRV_B_L2: &str = "L234567897DEU9201017F2706306QW12ER34";

#[test]
fn mrv_a_specimen_line2_byte_for_byte() {
    let fields = MrvAFields {
        document_code: "V".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "XK9305487".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "BRA".to_string(),
        date_of_birth: "850221".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "270314".to_string(),
        optional_data: Some("R5T6U7V8W9".to_string()),
    };

    let mrz = format_mrv_a(&fields);
    let (_, l2) = mrz.split_once('\n').unwrap();
    assert_eq!(l2, MRV_A_L2);
}

#[test]
fn mrv_b_specimen_line2_byte_for_byte() {
    let fields = MrvBFields {
        document_code: "V".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "L23456789".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "DEU".to_string(),
        date_of_birth: "920101".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "270630".to_string(),
        optional_data: Some("QW12ER34".to_string()),
    };

    let mrz = format_mrv_b(&fields);
    let (_, l2) = mrz.split_once('\n').unwrap();
    assert_eq!(l2, MRV_B_L2);
}

#[test]
fn mrv_a_round_trips_as_valid() {
    let fields = MrvAFields {
        document_code: "V".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "XK9305487".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "BRA".to_string(),
        date_of_birth: "850221".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "270314".to_string(),
        optional_data: Some("R5T6U7V8W9".to_string()),
    };

    let mrz = format_mrv_a(&fields);
    let (l1, l2) = mrz.split_once('\n').unwrap();
    let d = parse_mrv_a(l1, l2).unwrap();
    assert!(d.valid(), "checks: {:?}", d.checks);
}

#[test]
fn mrv_b_round_trips_as_valid() {
    let fields = MrvBFields {
        document_code: "V".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "L23456789".to_string(),
        surname: "ERIKSSON".to_string(),
        given_names: "ANNA MARIA".to_string(),
        nationality: "DEU".to_string(),
        date_of_birth: "920101".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "270630".to_string(),
        optional_data: Some("QW12ER34".to_string()),
    };

    let mrz = format_mrv_b(&fields);
    let (l1, l2) = mrz.split_once('\n').unwrap();
    let d = parse_mrv_b(l1, l2).unwrap();
    assert!(d.valid(), "checks: {:?}", d.checks);
}

// A name strategy narrow enough for MRV-A's 39-wide and MRV-B's 31-wide name
// field (14 + 2 + 14 = 30, comfortably under both) — a wider strategy like
// `name_strategy()` (sized for TD3) can overflow MRV-B's field and truncate,
// breaking round-trip.
fn mrv_name_strategy() -> impl Strategy<Value = String> {
    "[A-Z]{1,14}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn mrv_a_arbitrary_fields_round_trip(
        document_number in doc_number_strategy(),
        surname in mrv_name_strategy(),
        given_names in mrv_name_strategy(),
        nationality in "[A-Z]{3}",
        date_of_birth in yymmdd_strategy(),
        sex in sex_strategy(),
        date_of_expiry in yymmdd_strategy(),
        optional_data in optional_data_strategy(16),
    ) {
        let fields = MrvAFields {
            document_code: "V".to_string(),
            issuing_country: "UTO".to_string(),
            document_number: document_number.clone(),
            surname: surname.clone(),
            given_names: given_names.clone(),
            nationality: nationality.clone(),
            date_of_birth,
            sex: sex.clone(),
            date_of_expiry,
            optional_data,
        };

        let mrz = format_mrv_a(&fields);
        let (l1, l2) = mrz.split_once('\n').unwrap();
        prop_assert_eq!(l1.len(), 44);
        prop_assert_eq!(l2.len(), 44);

        let parsed = parse_mrv_a(l1, l2).unwrap();
        prop_assert!(parsed.valid(), "checks: {:?}", parsed.checks);

        prop_assert_eq!(&parsed.document_number, &document_number);
        prop_assert_eq!(&parsed.surname, &surname);
        prop_assert_eq!(&parsed.given_names, &given_names);
        prop_assert_eq!(&parsed.nationality, &nationality);
        prop_assert_eq!(&parsed.sex, &sex);
    }

    #[test]
    fn mrv_b_arbitrary_fields_round_trip(
        document_number in doc_number_strategy(),
        surname in mrv_name_strategy(),
        given_names in mrv_name_strategy(),
        nationality in "[A-Z]{3}",
        date_of_birth in yymmdd_strategy(),
        sex in sex_strategy(),
        date_of_expiry in yymmdd_strategy(),
        optional_data in optional_data_strategy(8),
    ) {
        let fields = MrvBFields {
            document_code: "V".to_string(),
            issuing_country: "UTO".to_string(),
            document_number: document_number.clone(),
            surname: surname.clone(),
            given_names: given_names.clone(),
            nationality: nationality.clone(),
            date_of_birth,
            sex: sex.clone(),
            date_of_expiry,
            optional_data,
        };

        let mrz = format_mrv_b(&fields);
        let (l1, l2) = mrz.split_once('\n').unwrap();
        prop_assert_eq!(l1.len(), 36);
        prop_assert_eq!(l2.len(), 36);

        let parsed = parse_mrv_b(l1, l2).unwrap();
        prop_assert!(parsed.valid(), "checks: {:?}", parsed.checks);

        prop_assert_eq!(&parsed.document_number, &document_number);
        prop_assert_eq!(&parsed.surname, &surname);
        prop_assert_eq!(&parsed.given_names, &given_names);
        prop_assert_eq!(&parsed.nationality, &nationality);
        prop_assert_eq!(&parsed.sex, &sex);
    }
}
