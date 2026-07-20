# Corpus coverage — comprehensive world passport-check backlog

Tracks Tier-1 MRZ corpus coverage against every ISO/ICAO country/entity code in
`crates/mrz/src/countries.rs` (230 codes). This is the concrete backlog behind the "wider
real-world corpus is the natural next accuracy milestone" note in
`docs/ARCHITECTURE.md` §8 — grown one individually-vetted specimen at a time, per the
checklist in `CONTRIBUTING.md`. **PRADO (`consilium.europa.eu/prado`) is never a source
here** — its copyright notice prohibits harvesting/redistributing its material outside
official, non-commercial use; it's consulted only as a manual human reference, never
scraped or stored.

Tier-1 MRZ checksum/parsing logic itself is ICAO-9303-generic and not special-cased per
country — a HIT here reflects real-world OCR/format validation on an actual specimen,
not new per-country code. `scripts/watch-samples.ps1` + `synthpass-ocr`'s `check_sample`
example give an instant first-pass check when a new candidate specimen is dropped into
`samples/` — see CONTRIBUTING.md.

## Summary

| Status | Countries |
|---|---|
| Tier-1 HIT in `mrz_corpus.rs` | 11 |
| Known MISS (documented, e.g. physically redacted specimen) | 1 |
| Negative-control specimen only (no MRZ on that page/document) | 1 |
| Specimen in `samples/`, not yet wired into the automated corpus | 9 |
| Candidate specimen rejected per the vetting checklist | 1 |
| No specimen yet | 207 |
| **Total tracked codes** | **230** |

## Full table

| Code | Country/Entity | Document type(s) | Status | Note |
|---|---|---|---|---|
| DZA | Algeria | -- | No specimen yet | -- |
| AGO | Angola | -- | No specimen yet | -- |
| BEN | Benin | -- | No specimen yet | -- |
| BWA | Botswana | -- | No specimen yet | -- |
| BFA | Burkina Faso | -- | No specimen yet | -- |
| BDI | Burundi | -- | No specimen yet | -- |
| CPV | Cabo Verde | -- | No specimen yet | -- |
| CMR | Cameroon | -- | No specimen yet | -- |
| CAF | Central African Republic | -- | No specimen yet | -- |
| TCD | Chad | -- | No specimen yet | -- |
| COM | Comoros | -- | No specimen yet | -- |
| COG | Congo | -- | No specimen yet | -- |
| COD | Congo (Democratic Republic of the) | -- | No specimen yet | -- |
| CIV | Côte d'Ivoire | -- | No specimen yet | -- |
| DJI | Djibouti | -- | No specimen yet | -- |
| EGY | Egypt | -- | No specimen yet | -- |
| GNQ | Equatorial Guinea | -- | No specimen yet | -- |
| ERI | Eritrea | -- | No specimen yet | -- |
| SWZ | Eswatini | -- | No specimen yet | -- |
| ETH | Ethiopia | -- | No specimen yet | -- |
| GAB | Gabon | -- | No specimen yet | -- |
| GMB | Gambia | -- | No specimen yet | -- |
| GHA | Ghana | -- | No specimen yet | -- |
| GIN | Guinea | -- | No specimen yet | -- |
| GNB | Guinea-Bissau | -- | No specimen yet | -- |
| KEN | Kenya | -- | No specimen yet | -- |
| LSO | Lesotho | -- | No specimen yet | -- |
| LBR | Liberia | -- | No specimen yet | -- |
| LBY | Libya | -- | No specimen yet | -- |
| MDG | Madagascar | -- | No specimen yet | -- |
| MWI | Malawi | -- | No specimen yet | -- |
| MLI | Mali | -- | No specimen yet | -- |
| MRT | Mauritania | -- | No specimen yet | -- |
| MUS | Mauritius | -- | No specimen yet | -- |
| MAR | Morocco | -- | No specimen yet | -- |
| MOZ | Mozambique | -- | No specimen yet | -- |
| NAM | Namibia | -- | No specimen yet | -- |
| NER | Niger | -- | No specimen yet | -- |
| NGA | Nigeria | -- | No specimen yet | -- |
| RWA | Rwanda | -- | No specimen yet | -- |
| STP | Sao Tome and Principe | -- | No specimen yet | -- |
| SEN | Senegal | -- | No specimen yet | -- |
| SYC | Seychelles | -- | No specimen yet | -- |
| SLE | Sierra Leone | -- | No specimen yet | -- |
| SOM | Somalia | -- | No specimen yet | -- |
| ZAF | South Africa | -- | No specimen yet | -- |
| SSD | South Sudan | -- | No specimen yet | -- |
| SDN | Sudan | Passport | Candidate rejected | No SPECIMEN watermark, read as real personal data -- excluded per vetting checklist |
| TZA | Tanzania | -- | No specimen yet | -- |
| TGO | Togo | -- | No specimen yet | -- |
| TUN | Tunisia | -- | No specimen yet | -- |
| UGA | Uganda | -- | No specimen yet | -- |
| ZMB | Zambia | -- | No specimen yet | -- |
| ZWE | Zimbabwe | -- | No specimen yet | -- |
| ESH | Western Sahara | -- | No specimen yet | -- |
| ATG | Antigua and Barbuda | -- | No specimen yet | -- |
| ARG | Argentina | -- | No specimen yet | -- |
| BHS | Bahamas | -- | No specimen yet | -- |
| BRB | Barbados | -- | No specimen yet | -- |
| BLZ | Belize | -- | No specimen yet | -- |
| BOL | Bolivia | -- | No specimen yet | -- |
| BRA | Brazil | -- | No specimen yet | -- |
| CAN | Canada | Passport | HIT (x3 specimens) | Contributor-supplied specimens (SPECIMEN watermark) |
| CHL | Chile | -- | No specimen yet | -- |
| COL | Colombia | -- | No specimen yet | -- |
| CRI | Costa Rica | -- | No specimen yet | -- |
| CUB | Cuba | -- | No specimen yet | -- |
| DMA | Dominica | -- | No specimen yet | -- |
| DOM | Dominican Republic | -- | No specimen yet | -- |
| ECU | Ecuador | -- | No specimen yet | -- |
| SLV | El Salvador | -- | No specimen yet | -- |
| GRD | Grenada | -- | No specimen yet | -- |
| GTM | Guatemala | -- | No specimen yet | -- |
| GUY | Guyana | -- | No specimen yet | -- |
| HTI | Haiti | -- | No specimen yet | -- |
| HND | Honduras | -- | No specimen yet | -- |
| JAM | Jamaica | -- | No specimen yet | -- |
| MEX | Mexico | -- | No specimen yet | -- |
| NIC | Nicaragua | -- | No specimen yet | -- |
| PAN | Panama | -- | No specimen yet | -- |
| PRY | Paraguay | -- | No specimen yet | -- |
| PER | Peru | -- | No specimen yet | -- |
| KNA | Saint Kitts and Nevis | -- | No specimen yet | -- |
| LCA | Saint Lucia | -- | No specimen yet | -- |
| VCT | Saint Vincent and the Grenadines | -- | No specimen yet | -- |
| SUR | Suriname | -- | No specimen yet | -- |
| TTO | Trinidad and Tobago | -- | No specimen yet | -- |
| USA | United States of America | -- | No specimen yet | -- |
| URY | Uruguay | -- | No specimen yet | -- |
| VEN | Venezuela | -- | No specimen yet | -- |
| AFG | Afghanistan | -- | No specimen yet | -- |
| ARM | Armenia | -- | No specimen yet | -- |
| AZE | Azerbaijan | -- | No specimen yet | -- |
| BHR | Bahrain | -- | No specimen yet | -- |
| BGD | Bangladesh | -- | No specimen yet | -- |
| BTN | Bhutan | -- | No specimen yet | -- |
| BRN | Brunei Darussalam | -- | No specimen yet | -- |
| KHM | Cambodia | -- | No specimen yet | -- |
| CHN | China | Passport | HIT | Contributor-supplied specimen (SPECIMEN watermark) |
| CYP | Cyprus | -- | No specimen yet | -- |
| GEO | Georgia | -- | No specimen yet | -- |
| HKG | Hong Kong | -- | No specimen yet | -- |
| IND | India | -- | No specimen yet | -- |
| IDN | Indonesia | -- | No specimen yet | -- |
| IRN | Iran | -- | No specimen yet | -- |
| IRQ | Iraq | -- | No specimen yet | -- |
| ISR | Israel | Passport | Known MISS (documented) | Public specimen, physically redacted MRZ -- kept local-only, not committed |
| JPN | Japan | -- | No specimen yet | -- |
| JOR | Jordan | -- | No specimen yet | -- |
| KAZ | Kazakhstan | Passport | Specimen in samples/, not wired | Public-domain specimens |
| PRK | Korea (Democratic People's Republic of) | -- | No specimen yet | -- |
| KOR | Korea (Republic of) | -- | No specimen yet | -- |
| KWT | Kuwait | -- | No specimen yet | -- |
| KGZ | Kyrgyzstan | -- | No specimen yet | -- |
| LAO | Lao People's Democratic Republic | -- | No specimen yet | -- |
| LBN | Lebanon | -- | No specimen yet | -- |
| MAC | Macao | -- | No specimen yet | -- |
| MYS | Malaysia | -- | No specimen yet | -- |
| MDV | Maldives | -- | No specimen yet | -- |
| MNG | Mongolia | -- | No specimen yet | -- |
| MMR | Myanmar | -- | No specimen yet | -- |
| NPL | Nepal | -- | No specimen yet | -- |
| OMN | Oman | Passport | HIT | Contributor-supplied specimen (SPECIMEN watermark) |
| PAK | Pakistan | -- | No specimen yet | -- |
| PSE | Palestine | -- | No specimen yet | -- |
| PHL | Philippines | -- | No specimen yet | -- |
| QAT | Qatar | -- | No specimen yet | -- |
| SAU | Saudi Arabia | -- | No specimen yet | -- |
| SGP | Singapore | -- | No specimen yet | -- |
| LKA | Sri Lanka | -- | No specimen yet | -- |
| SYR | Syrian Arab Republic | -- | No specimen yet | -- |
| TWN | Taiwan | -- | No specimen yet | -- |
| TJK | Tajikistan | -- | No specimen yet | -- |
| THA | Thailand | -- | No specimen yet | -- |
| TLS | Timor-Leste | -- | No specimen yet | -- |
| TUR | Türkiye | -- | No specimen yet | -- |
| TKM | Turkmenistan | -- | No specimen yet | -- |
| ARE | United Arab Emirates | Passport | HIT | Contributor-supplied specimen (watermarked reference) |
| UZB | Uzbekistan | -- | No specimen yet | -- |
| VNM | Viet Nam | Passport | HIT | Contributor-supplied specimen (placeholder MRZ number) |
| YEM | Yemen | -- | No specimen yet | -- |
| ALB | Albania | -- | No specimen yet | -- |
| AND | Andorra | -- | No specimen yet | -- |
| AUT | Austria | ID card front | Specimen in samples/, not wired | MRZ is on the card back, not supplied yet |
| BLR | Belarus | -- | No specimen yet | -- |
| BEL | Belgium | -- | No specimen yet | -- |
| BIH | Bosnia and Herzegovina | ID card, Passport | Specimen in samples/, not wired | Public-domain specimens |
| BGR | Bulgaria | ID card front | Negative control (no MRZ on this page) | Public-domain specimen |
| HRV | Croatia | Passport | HIT | Public-domain specimen |
| CZE | Czechia | -- | No specimen yet | -- |
| DNK | Denmark | Passport | Specimen in samples/, not wired | Public-domain specimens |
| EST | Estonia | Passport | HIT | Public-domain specimen |
| FIN | Finland | -- | No specimen yet | -- |
| FRA | France | -- | No specimen yet | -- |
| DEU | Germany | -- | No specimen yet | -- |
| GRC | Greece | -- | No specimen yet | -- |
| HUN | Hungary | -- | No specimen yet | -- |
| ISL | Iceland | -- | No specimen yet | -- |
| IRL | Ireland | ID card front | Specimen in samples/, not wired | Public-domain specimen |
| ITA | Italy | -- | No specimen yet | -- |
| XKX | Kosovo | Passport | Specimen in samples/, not wired | Public-domain specimens |
| LVA | Latvia | -- | No specimen yet | -- |
| LIE | Liechtenstein | -- | No specimen yet | -- |
| LTU | Lithuania | -- | No specimen yet | -- |
| LUX | Luxembourg | -- | No specimen yet | -- |
| MLT | Malta | -- | No specimen yet | -- |
| MDA | Moldova | -- | No specimen yet | -- |
| MCO | Monaco | ID card, Passport | Specimen in samples/, not wired | Public-domain specimens |
| MNE | Montenegro | -- | No specimen yet | -- |
| NLD | Netherlands | Driving license (no MRZ) | Specimen in samples/, not wired | Public-domain specimen |
| MKD | North Macedonia | Passport | Specimen in samples/, not wired | Public-domain specimens |
| NOR | Norway | -- | No specimen yet | -- |
| POL | Poland | -- | No specimen yet | -- |
| PRT | Portugal | -- | No specimen yet | -- |
| ROU | Romania | -- | No specimen yet | -- |
| RUS | Russian Federation | -- | No specimen yet | -- |
| SMR | San Marino | -- | No specimen yet | -- |
| SRB | Serbia | Passport, ID card (TD1) | HIT (+ negative control) | Public-domain specimens |
| SVK | Slovakia | Passport, Service Passport | HIT (x2) + negative control | Contributor-supplied specimens (Specimen/Vzorka placeholder name) |
| SVN | Slovenia | ID card (TD1) | HIT (+ negative control) | Public-domain specimen |
| ESP | Spain | Passport | HIT (x2) | Contributor-supplied specimens (ESPECIMEN watermark / placeholder name) |
| SWE | Sweden | -- | No specimen yet | -- |
| CHE | Switzerland | -- | No specimen yet | -- |
| UKR | Ukraine | -- | No specimen yet | -- |
| GBR | United Kingdom | -- | No specimen yet | -- |
| VAT | Holy See (Vatican City State) | -- | No specimen yet | -- |
| AUS | Australia | -- | No specimen yet | -- |
| FJI | Fiji | -- | No specimen yet | -- |
| KIR | Kiribati | -- | No specimen yet | -- |
| MHL | Marshall Islands | -- | No specimen yet | -- |
| FSM | Micronesia (Federated States of) | -- | No specimen yet | -- |
| NRU | Nauru | -- | No specimen yet | -- |
| NZL | New Zealand | -- | No specimen yet | -- |
| PLW | Palau | -- | No specimen yet | -- |
| PNG | Papua New Guinea | -- | No specimen yet | -- |
| WSM | Samoa | -- | No specimen yet | -- |
| SLB | Solomon Islands | -- | No specimen yet | -- |
| TON | Tonga | -- | No specimen yet | -- |
| TUV | Tuvalu | -- | No specimen yet | -- |
| VUT | Vanuatu | -- | No specimen yet | -- |
| ABW | Aruba | -- | No specimen yet | -- |
| BMU | Bermuda | -- | No specimen yet | -- |
| CYM | Cayman Islands | -- | No specimen yet | -- |
| CUW | Curaçao | -- | No specimen yet | -- |
| FRO | Faroe Islands | -- | No specimen yet | -- |
| GIB | Gibraltar | -- | No specimen yet | -- |
| GRL | Greenland | -- | No specimen yet | -- |
| SXM | Sint Maarten (Dutch part) | -- | No specimen yet | -- |
| UTO | Utopia (ICAO specimen) | -- | No specimen yet | -- |
| EUE | European Union | -- | No specimen yet | -- |
| RKS | Kosovo | -- | No specimen yet | -- |
| GBD | British Overseas Territories Citizen | -- | No specimen yet | -- |
| GBN | British National (Overseas) | -- | No specimen yet | -- |
| GBO | British Overseas Citizen | -- | No specimen yet | -- |
| GBP | British Protected Person | -- | No specimen yet | -- |
| GBS | British Subject | -- | No specimen yet | -- |
| XXA | Stateless person (1954 Convention) | -- | No specimen yet | -- |
| XXB | Refugee (1951 Convention) | -- | No specimen yet | -- |
| XXC | Refugee (other) | -- | No specimen yet | -- |
| XXX | Unspecified nationality | -- | No specimen yet | -- |
| UNO | United Nations Organization | -- | No specimen yet | -- |
| UNA | United Nations specialized agency | -- | No specimen yet | -- |
| UNK | United Nations Interim Administration Mission in Kosovo | -- | No specimen yet | -- |
| XOM | Sovereign Military Order of Malta | -- | No specimen yet | -- |
| XBA | African Development Bank | -- | No specimen yet | -- |
| XIM | African Export-Import Bank | -- | No specimen yet | -- |
| XCC | Caribbean Community (CARICOM) | -- | No specimen yet | -- |
| XCO | Common Market for Eastern and Southern Africa (COMESA) | -- | No specimen yet | -- |
| XEC | Economic Community of West African States (ECOWAS) | -- | No specimen yet | -- |
| XPO | International Criminal Police Organization (INTERPOL) | -- | No specimen yet | -- |
