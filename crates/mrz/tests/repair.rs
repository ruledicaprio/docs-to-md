//! Regressions for the two damage shapes measured on real captures: a hole
//! punched through a TD1 card's MRZ, and a finger over the start of a TD3
//! passport's line 2.
//!
//! **The fixtures are emitted, not transcribed.** Each test builds its own
//! zone with `mrz::format_td1`/`format_td3` from placeholder field values and
//! then damages it, so the suite reproduces the real failure *geometry* — a
//! character deleted mid-field, a character deleted at a line start — with no
//! real document's data in the repository. `format_*` computes the check
//! digits, so each fixture is self-verifying: if the emitter is wrong the
//! undamaged assertions fail first.

use mrz::{
    format_td1, format_td3, solve_field, width_candidates, FieldKind, Resolution, Td1Fields,
    Td3Fields, UNKNOWN,
};

/// A synthetic TD1 card. Expiry `301230` is what makes this fixture useful:
/// destroying its first character leaves a residue class whose other members
/// are letters, so the date constraint has something to prune.
fn td1_zone() -> (String, String, String) {
    let mrz = format_td1(&Td1Fields {
        document_code: "ID".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "E00000000".to_string(),
        surname: "SPECIMEN".to_string(),
        given_names: "TEST".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "800101".to_string(),
        sex: "M".to_string(),
        date_of_expiry: "301230".to_string(),
        optional_data_1: None,
        optional_data_2: None,
    });
    let lines: Vec<&str> = mrz.lines().collect();
    (
        lines[0].to_string(),
        lines[1].to_string(),
        lines[2].to_string(),
    )
}

/// A synthetic TD3 passport.
fn td3_zone() -> (String, String) {
    let mrz = format_td3(&Td3Fields {
        document_code: "P".to_string(),
        issuing_country: "UTO".to_string(),
        document_number: "E00000000".to_string(),
        surname: "SPECIMEN".to_string(),
        given_names: "TEST".to_string(),
        nationality: "UTO".to_string(),
        date_of_birth: "800101".to_string(),
        sex: "F".to_string(),
        date_of_expiry: "301230".to_string(),
        personal_number: None,
    });
    let lines: Vec<&str> = mrz.lines().collect();
    (lines[0].to_string(), lines[1].to_string())
}

/// Delete the character at `at`, the way `ocrs` drops a glyph it cannot
/// recognize instead of emitting a placeholder — the behaviour that makes a
/// damaged line arrive narrow rather than merely wrong.
fn delete_at(line: &str, at: usize) -> String {
    let mut s = line.to_string();
    s.remove(at);
    s
}

/// The punched-hole case: one character destroyed inside the expiry field.
///
/// TD1 line 2 is `YYMMDD C S YYMMDD C NNN ... C`; the expiry field is
/// `[8..14]` and its check digit is at `[14]`.
#[test]
fn a_hole_through_the_expiry_field_resolves_uniquely() {
    let (l1, l2, l3) = td1_zone();
    assert!(
        mrz::parse_td1(&l1, &l2, &l3)
            .expect("emitted TD1 parses")
            .valid(),
        "fixture must be checksum-valid before it is damaged"
    );

    let damaged = delete_at(&l2, 8);
    assert_eq!(damaged.len(), 29, "a dropped glyph narrows the line");

    // Restoring the width has to put the unknown back where the glyph was.
    let restored = width_candidates(&damaged, 30);
    let target = format!("{}{}{}", &l2[..8], UNKNOWN, &l2[9..]);
    assert!(
        restored.contains(&target),
        "position sweep must offer the true insertion point"
    );

    let expiry = &target[8..14];
    let check = target.as_bytes()[14] as char;
    assert_eq!(
        solve_field(expiry, check, FieldKind::Date),
        Resolution::Unique(l2[8..14].to_string()),
        "the date constraint must leave exactly one reading"
    );
}

/// The same field, same damage, *without* the date constraint — the control
/// that proves the constraint is load-bearing rather than decorative.
///
/// A check digit sees a field only mod 10, so one unknown position admits a
/// whole residue class. Without `FieldKind::Date` the answer is honestly
/// ambiguous; with it, three of the four members are not calendar dates.
#[test]
fn without_date_pruning_the_same_hole_is_ambiguous() {
    let (_, l2, _) = td1_zone();
    let damaged_field = format!("{}{}", UNKNOWN, &l2[9..14]);
    let check = l2.as_bytes()[14] as char;

    let Resolution::Ambiguous { candidates } = solve_field(&damaged_field, check, FieldKind::Other)
    else {
        panic!("check digits alone cannot separate a residue class");
    };
    assert_eq!(
        candidates.len(),
        4,
        "one unknown position admits exactly one residue class: {candidates:?}"
    );
    assert!(candidates.contains(&l2[8..14].to_string()));
    // And exactly one of those four survives the calendar.
    let dates: Vec<&String> = candidates
        .iter()
        .filter(|c| {
            solve_field(&damaged_field, check, FieldKind::Date)
                .unique()
                .is_some_and(|u| u == c.as_str())
        })
        .collect();
    assert_eq!(dates.len(), 1);
}

/// The finger case: two characters gone from the very start of line 2, inside
/// the document-number field.
///
/// Two unknowns is past what one check digit can separate, so the correct
/// outcome is `Ambiguous` — asserted here as a *requirement*. A version of
/// this solver that returned a single answer would be guessing, and a guess
/// that happens to be wrong is indistinguishable from a proof.
#[test]
fn an_occluded_line_start_stays_ambiguous_but_contains_the_truth() {
    let (l1, l2) = td3_zone();
    assert!(mrz::parse_td3(&l1, &l2)
        .expect("emitted TD3 parses")
        .valid());

    let damaged = delete_at(&delete_at(&l2, 0), 0);
    assert_eq!(damaged.len(), 42);

    let restored = width_candidates(&damaged, 44);
    let target = format!("{}{}{}", UNKNOWN, UNKNOWN, &l2[2..]);
    assert!(restored.contains(&target));

    let doc_field = &target[0..9];
    let check = target.as_bytes()[9] as char;
    match solve_field(doc_field, check, FieldKind::DocumentNumber) {
        Resolution::Ambiguous { candidates } => {
            assert!(
                candidates.len() > 1,
                "two unknowns cannot be resolved by one check digit"
            );
            assert!(
                candidates.contains(&l2[0..9].to_string()),
                "the true reading must survive among the candidates"
            );
        }
        other => panic!("expected an ambiguous answer, got {other:?}"),
    }
}

/// An undamaged zone must be untouched by any of this: same parse, same
/// fields, and the width candidates for a correct line are just that line.
#[test]
fn an_undamaged_zone_is_unaffected() {
    let (l1, l2, l3) = td1_zone();
    let parsed = mrz::parse_td1(&l1, &l2, &l3).expect("parses");
    assert!(parsed.valid());
    assert_eq!(parsed.date_of_expiry, "2030-12-30");

    for line in [&l1, &l2, &l3] {
        assert_eq!(width_candidates(line, 30), vec![line.to_string()]);
    }
}

/// The document-number field is left-justified and `<`-padded, so a reading
/// with an interior filler is structurally impossible however well its check
/// digit verifies. Guards against the solver "resolving" a shifted field.
#[test]
fn an_interior_filler_is_never_a_document_number() {
    let (_, l2) = td3_zone();
    let check = l2.as_bytes()[9] as char;
    let field = format!("{}{}", UNKNOWN, &l2[1..9]);

    if let Resolution::Ambiguous { candidates } =
        solve_field(&field, check, FieldKind::DocumentNumber)
    {
        for c in candidates {
            assert!(
                !c.trim_end_matches('<').contains('<'),
                "{c:?} has an interior filler"
            );
        }
    }
}
