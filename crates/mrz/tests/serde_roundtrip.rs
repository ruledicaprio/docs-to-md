//! `serde` symmetry: a parsed `MrzData` survives a JSON round-trip unchanged.
//!
//! Compiled only when the `serde` feature is on (which pulls in the
//! `Serialize`/`Deserialize` derives this test exercises).

#![cfg(feature = "serde")]

use mrz::{parse_td3, MrzData};

// Official ICAO 9303 Part 4 specimen (Utopia / Anna Maria Eriksson).
const TD3_L1: &str = "P<UTOERIKSSON<<ANNA<MARIA<<<<<<<<<<<<<<<<<<<";
const TD3_L2: &str = "L898902C36UTO7408122F1204159ZE184226B<<<<<10";

#[test]
fn mrzdata_json_round_trip_is_identity() {
    let original = parse_td3(TD3_L1, TD3_L2).unwrap();

    let json = serde_json::to_string(&original).expect("serialize");
    let restored: MrzData = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(original, restored);
    assert!(restored.valid(), "checks should survive the round-trip");
}
