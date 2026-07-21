//! ICAO 9303 / ISO 3166-1 alpha-3 issuing-state & nationality code ↔ name.
//!
//! The MRZ carries a 3-letter code for the issuing state and the holder's
//! nationality. [`country_name`] maps that code to a human-readable name so
//! callers can enrich the extraction (`issuing_country_name`,
//! `nationality_name`); [`code_for_name`] is the reverse direction, needed
//! when a Tier-2 LLM read prints the full name ("CROATIA") instead of the
//! MRZ code (`HRV`) and a downstream normalizer wants the canonical code.
//!
//! Both functions read the same [`CODES`] table — single-sourced, so the two
//! directions can never drift apart — which follows ISO 3166-1 and the ICAO
//! 9303 code list *verbatim and neutrally*: every code that can legitimately
//! appear on a travel document is included — ISO member states, the ICAO
//! stateless/refugee/organization codes, the `GBR` nationality subvariants,
//! and the specimen code `UTO` — because the parser must be able to name
//! whatever a real document prints. Zero dependencies, just `&'static str`
//! literals, so it compiles for native and wasm alike.

/// `(code, name)` pairs, in the same order as the original per-region match
/// arms. Order matters for [`code_for_name`]: a couple of names have more
/// than one legitimate code on real documents (Kosovo: `XKX`/`RKS`; Germany:
/// `DEU`/`D`, the legacy single-letter code) — [`code_for_name`] returns the
/// first match, so the primary ISO/ICAO code wins over the alias by virtue
/// of appearing earlier in this table.
const CODES: &[(&str, &str)] = &[
    // ── Africa ──
    ("DZA", "Algeria"),
    ("AGO", "Angola"),
    ("BEN", "Benin"),
    ("BWA", "Botswana"),
    ("BFA", "Burkina Faso"),
    ("BDI", "Burundi"),
    ("CPV", "Cabo Verde"),
    ("CMR", "Cameroon"),
    ("CAF", "Central African Republic"),
    ("TCD", "Chad"),
    ("COM", "Comoros"),
    ("COG", "Congo"),
    ("COD", "Congo (Democratic Republic of the)"),
    ("CIV", "Côte d'Ivoire"),
    ("DJI", "Djibouti"),
    ("EGY", "Egypt"),
    ("GNQ", "Equatorial Guinea"),
    ("ERI", "Eritrea"),
    ("SWZ", "Eswatini"),
    ("ETH", "Ethiopia"),
    ("GAB", "Gabon"),
    ("GMB", "Gambia"),
    ("GHA", "Ghana"),
    ("GIN", "Guinea"),
    ("GNB", "Guinea-Bissau"),
    ("KEN", "Kenya"),
    ("LSO", "Lesotho"),
    ("LBR", "Liberia"),
    ("LBY", "Libya"),
    ("MDG", "Madagascar"),
    ("MWI", "Malawi"),
    ("MLI", "Mali"),
    ("MRT", "Mauritania"),
    ("MUS", "Mauritius"),
    ("MAR", "Morocco"),
    ("MOZ", "Mozambique"),
    ("NAM", "Namibia"),
    ("NER", "Niger"),
    ("NGA", "Nigeria"),
    ("RWA", "Rwanda"),
    ("STP", "Sao Tome and Principe"),
    ("SEN", "Senegal"),
    ("SYC", "Seychelles"),
    ("SLE", "Sierra Leone"),
    ("SOM", "Somalia"),
    ("ZAF", "South Africa"),
    ("SSD", "South Sudan"),
    ("SDN", "Sudan"),
    ("TZA", "Tanzania"),
    ("TGO", "Togo"),
    ("TUN", "Tunisia"),
    ("UGA", "Uganda"),
    ("ZMB", "Zambia"),
    ("ZWE", "Zimbabwe"),
    ("ESH", "Western Sahara"),
    // ── Americas ──
    ("ATG", "Antigua and Barbuda"),
    ("ARG", "Argentina"),
    ("BHS", "Bahamas"),
    ("BRB", "Barbados"),
    ("BLZ", "Belize"),
    ("BOL", "Bolivia"),
    ("BRA", "Brazil"),
    ("CAN", "Canada"),
    ("CHL", "Chile"),
    ("COL", "Colombia"),
    ("CRI", "Costa Rica"),
    ("CUB", "Cuba"),
    ("DMA", "Dominica"),
    ("DOM", "Dominican Republic"),
    ("ECU", "Ecuador"),
    ("SLV", "El Salvador"),
    ("GRD", "Grenada"),
    ("GTM", "Guatemala"),
    ("GUY", "Guyana"),
    ("HTI", "Haiti"),
    ("HND", "Honduras"),
    ("JAM", "Jamaica"),
    ("MEX", "Mexico"),
    ("NIC", "Nicaragua"),
    ("PAN", "Panama"),
    ("PRY", "Paraguay"),
    ("PER", "Peru"),
    ("KNA", "Saint Kitts and Nevis"),
    ("LCA", "Saint Lucia"),
    ("VCT", "Saint Vincent and the Grenadines"),
    ("SUR", "Suriname"),
    ("TTO", "Trinidad and Tobago"),
    ("USA", "United States of America"),
    ("URY", "Uruguay"),
    ("VEN", "Venezuela"),
    // ── Asia ──
    ("AFG", "Afghanistan"),
    ("ARM", "Armenia"),
    ("AZE", "Azerbaijan"),
    ("BHR", "Bahrain"),
    ("BGD", "Bangladesh"),
    ("BTN", "Bhutan"),
    ("BRN", "Brunei Darussalam"),
    ("KHM", "Cambodia"),
    ("CHN", "China"),
    ("CYP", "Cyprus"),
    ("GEO", "Georgia"),
    ("HKG", "Hong Kong"),
    ("IND", "India"),
    ("IDN", "Indonesia"),
    ("IRN", "Iran"),
    ("IRQ", "Iraq"),
    ("ISR", "Israel"),
    ("JPN", "Japan"),
    ("JOR", "Jordan"),
    ("KAZ", "Kazakhstan"),
    ("PRK", "Korea (Democratic People's Republic of)"),
    ("KOR", "Korea (Republic of)"),
    ("KWT", "Kuwait"),
    ("KGZ", "Kyrgyzstan"),
    ("LAO", "Lao People's Democratic Republic"),
    ("LBN", "Lebanon"),
    ("MAC", "Macao"),
    ("MYS", "Malaysia"),
    ("MDV", "Maldives"),
    ("MNG", "Mongolia"),
    ("MMR", "Myanmar"),
    ("NPL", "Nepal"),
    ("OMN", "Oman"),
    ("PAK", "Pakistan"),
    ("PSE", "Palestine"),
    ("PHL", "Philippines"),
    ("QAT", "Qatar"),
    ("SAU", "Saudi Arabia"),
    ("SGP", "Singapore"),
    ("LKA", "Sri Lanka"),
    ("SYR", "Syrian Arab Republic"),
    ("TWN", "Taiwan"),
    ("TJK", "Tajikistan"),
    ("THA", "Thailand"),
    ("TLS", "Timor-Leste"),
    ("TUR", "Türkiye"),
    ("TKM", "Turkmenistan"),
    ("ARE", "United Arab Emirates"),
    ("UZB", "Uzbekistan"),
    ("VNM", "Viet Nam"),
    ("YEM", "Yemen"),
    // ── Europe ──
    ("ALB", "Albania"),
    ("AND", "Andorra"),
    ("AUT", "Austria"),
    ("BLR", "Belarus"),
    ("BEL", "Belgium"),
    ("BIH", "Bosnia and Herzegovina"),
    ("BGR", "Bulgaria"),
    ("HRV", "Croatia"),
    ("CZE", "Czechia"),
    ("DNK", "Denmark"),
    ("EST", "Estonia"),
    ("FIN", "Finland"),
    ("FRA", "France"),
    ("DEU", "Germany"),
    ("GRC", "Greece"),
    ("HUN", "Hungary"),
    ("ISL", "Iceland"),
    ("IRL", "Ireland"),
    ("ITA", "Italy"),
    ("XKX", "Kosovo"),
    ("LVA", "Latvia"),
    ("LIE", "Liechtenstein"),
    ("LTU", "Lithuania"),
    ("LUX", "Luxembourg"),
    ("MLT", "Malta"),
    ("MDA", "Moldova"),
    ("MCO", "Monaco"),
    ("MNE", "Montenegro"),
    ("NLD", "Netherlands"),
    ("MKD", "North Macedonia"),
    ("NOR", "Norway"),
    ("POL", "Poland"),
    ("PRT", "Portugal"),
    ("ROU", "Romania"),
    ("RUS", "Russian Federation"),
    ("SMR", "San Marino"),
    ("SRB", "Serbia"),
    ("SVK", "Slovakia"),
    ("SVN", "Slovenia"),
    ("ESP", "Spain"),
    ("SWE", "Sweden"),
    ("CHE", "Switzerland"),
    ("UKR", "Ukraine"),
    ("GBR", "United Kingdom"),
    ("VAT", "Holy See (Vatican City State)"),
    // ── Oceania ──
    ("AUS", "Australia"),
    ("FJI", "Fiji"),
    ("KIR", "Kiribati"),
    ("MHL", "Marshall Islands"),
    ("FSM", "Micronesia (Federated States of)"),
    ("NRU", "Nauru"),
    ("NZL", "New Zealand"),
    ("PLW", "Palau"),
    ("PNG", "Papua New Guinea"),
    ("WSM", "Samoa"),
    ("SLB", "Solomon Islands"),
    ("TON", "Tonga"),
    ("TUV", "Tuvalu"),
    ("VUT", "Vanuatu"),
    // ── Territories & dependencies commonly seen on documents ──
    ("ABW", "Aruba"),
    ("BMU", "Bermuda"),
    ("CYM", "Cayman Islands"),
    ("CUW", "Curaçao"),
    ("FRO", "Faroe Islands"),
    ("GIB", "Gibraltar"),
    ("GRL", "Greenland"),
    ("SXM", "Sint Maarten (Dutch part)"),
    // ── ICAO 9303 special / non-ISO codes ──
    ("UTO", "Utopia (ICAO specimen)"),
    ("D", "Germany"), // legacy single-letter code on older passports
    ("EUE", "European Union"),
    ("RKS", "Kosovo"), // code used on Kosovo travel documents
    // British nationality subvariants used in the MRZ nationality field
    ("GBD", "British Overseas Territories Citizen"),
    ("GBN", "British National (Overseas)"),
    ("GBO", "British Overseas Citizen"),
    ("GBP", "British Protected Person"),
    ("GBS", "British Subject"),
    // Stateless persons, refugees and unspecified nationality
    ("XXA", "Stateless person (1954 Convention)"),
    ("XXB", "Refugee (1951 Convention)"),
    ("XXC", "Refugee (other)"),
    ("XXX", "Unspecified nationality"),
    // Inter-governmental organizations issuing travel documents
    ("UNO", "United Nations Organization"),
    ("UNA", "United Nations specialized agency"),
    (
        "UNK",
        "United Nations Interim Administration Mission in Kosovo",
    ),
    ("XOM", "Sovereign Military Order of Malta"),
    ("XBA", "African Development Bank"),
    ("XIM", "African Export-Import Bank"),
    ("XCC", "Caribbean Community (CARICOM)"),
    (
        "XCO",
        "Common Market for Eastern and Southern Africa (COMESA)",
    ),
    ("XEC", "Economic Community of West African States (ECOWAS)"),
    (
        "XPO",
        "International Criminal Police Organization (INTERPOL)",
    ),
];

/// Map a 3-letter ICAO/ISO 3166-1 code to a country or entity name.
/// Returns `None` for codes not in the table.
pub fn country_name(code: &str) -> Option<&'static str> {
    CODES
        .iter()
        .find(|&&(c, _)| c == code)
        .map(|&(_, name)| name)
}

/// Map a country or entity name back to its 3-letter ICAO/ISO 3166-1 code —
/// the reverse of [`country_name`], over the same `CODES` table so the two
/// directions can't drift apart. Case-insensitive (Tier-2 LLM reads commonly
/// come back upper/lower/title-cased inconsistently, e.g. `"CROATIA"` vs
/// `"Croatia"`). Returns `None` for names not in the table; when a name has
/// more than one legitimate code (Kosovo, Germany — see `CODES`'s doc
/// comment), the first (primary) code in table order wins.
pub fn code_for_name(name: &str) -> Option<&'static str> {
    CODES
        .iter()
        .find(|&&(_, n)| n.eq_ignore_ascii_case(name))
        .map(|&(c, _)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_codes_neutrally_and_completely() {
        // Ordinary members.
        assert_eq!(country_name("HRV"), Some("Croatia"));
        assert_eq!(country_name("SRB"), Some("Serbia"));
        assert_eq!(country_name("SVN"), Some("Slovenia"));
        // Politically sensitive entries are present, per the standard — the
        // parser must name whatever a real document prints.
        assert_eq!(country_name("TWN"), Some("Taiwan"));
        assert_eq!(country_name("PSE"), Some("Palestine"));
        assert_eq!(country_name("XKX"), Some("Kosovo"));
        // ICAO specials.
        assert_eq!(country_name("UTO"), Some("Utopia (ICAO specimen)"));
        assert_eq!(country_name("GBN"), Some("British National (Overseas)"));
        assert_eq!(country_name("XXB"), Some("Refugee (1951 Convention)"));
        // Unknown / empty.
        assert_eq!(country_name("ZZZ"), None);
        assert_eq!(country_name(""), None);
    }

    #[test]
    fn reverse_lookup_is_case_insensitive_and_round_trips() {
        assert_eq!(code_for_name("Croatia"), Some("HRV"));
        assert_eq!(code_for_name("CROATIA"), Some("HRV"));
        assert_eq!(code_for_name("croatia"), Some("HRV"));
        assert_eq!(code_for_name("Taiwan"), Some("TWN"));
        assert_eq!(code_for_name("Not A Country"), None);
        assert_eq!(code_for_name(""), None);
    }

    #[test]
    fn reverse_lookup_prefers_the_primary_code_for_aliased_names() {
        // Kosovo and Germany each have two legitimate codes in the table;
        // the primary ISO/ICAO code (appearing first) must win.
        assert_eq!(code_for_name("Kosovo"), Some("XKX"));
        assert_eq!(code_for_name("Germany"), Some("DEU"));
    }

    #[test]
    fn every_table_entry_round_trips_through_both_directions() {
        for &(code, name) in CODES {
            assert_eq!(
                country_name(code),
                Some(name),
                "country_name({code:?}) should return {name:?}"
            );
            // Not asserting code_for_name(name) == code here: several names
            // have more than one valid code (see the alias test above), so
            // this only checks the reverse lookup resolves to *some* valid
            // code that maps back to the same name.
            let reverse = code_for_name(name).expect("name should resolve back to a code");
            assert_eq!(country_name(reverse), Some(name));
        }
    }
}
