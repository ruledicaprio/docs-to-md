//! Keystone correctness test: the MRZ extracted from `Labels` must parse back
//! through `mrz::parse_td3` as fully checksum-valid, and every parsed field
//! must equal the generated `Passport` field it came from.

use synthpass_gen::{data::generate_passport, generate, GeneratorConfig};

#[test]
fn generated_mrz_round_trips_through_mrz_crate() {
    for seed in 0..100u64 {
        let cfg = GeneratorConfig::new(seed);
        let passport = generate_passport(&cfg);
        let (_image, labels) = generate(&passport, &cfg);

        let mrz_string = labels.mrz_string();
        let mut lines = mrz_string.lines();
        let line1 = lines.next().expect("line1");
        let line2 = lines.next().expect("line2");

        let parsed = mrz::parse_td3(line1, line2)
            .unwrap_or_else(|e| panic!("seed {seed}: MRZ failed to parse: {e}"));
        assert!(
            parsed.valid(),
            "seed {seed}: MRZ parsed but not checksum-valid: {:?}",
            parsed.checks
        );

        assert_eq!(parsed.document_type, passport.document_type, "seed {seed}");
        assert_eq!(parsed.issuing_country, passport.issuing_country, "seed {seed}");
        assert_eq!(parsed.surname, passport.surname, "seed {seed}");
        assert_eq!(parsed.given_names, passport.given_names, "seed {seed}");
        assert_eq!(parsed.document_number, passport.document_number, "seed {seed}");
        assert_eq!(parsed.nationality, passport.nationality, "seed {seed}");
        assert_eq!(
            parsed.date_of_birth,
            format!(
                "{:04}-{:02}-{:02}",
                passport.date_of_birth.year, passport.date_of_birth.month, passport.date_of_birth.day
            ),
            "seed {seed}"
        );
        assert_eq!(parsed.sex, passport.sex.as_mrz_char().to_string(), "seed {seed}");
        assert_eq!(
            parsed.date_of_expiry,
            format!(
                "{:04}-{:02}-{:02}",
                passport.date_of_expiry.year, passport.date_of_expiry.month, passport.date_of_expiry.day
            ),
            "seed {seed}"
        );
        assert_eq!(parsed.personal_number, passport.personal_number, "seed {seed}");
    }
}

#[test]
fn generated_mrz_round_trips_without_personal_number() {
    let cfg = GeneratorConfig {
        seed: 999,
        include_personal_number: false,
    };
    let passport = generate_passport(&cfg);
    assert!(passport.personal_number.is_none());
    let (_image, labels) = generate(&passport, &cfg);
    let mrz_string = labels.mrz_string();
    let mut lines = mrz_string.lines();
    let parsed = mrz::parse_td3(lines.next().unwrap(), lines.next().unwrap()).unwrap();
    assert!(parsed.valid(), "checks: {:?}", parsed.checks);
    assert_eq!(parsed.personal_number, None);
}
