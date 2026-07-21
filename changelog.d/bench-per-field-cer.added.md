- **`synthpass-bench` now measures per-field character error rate, not just a binary hit — and
  the first run found that the Tier-1 gate is blind to half the MRZ.**

  `HitResult` gains a `fields: Vec<FieldOutcome>` breakdown (expected, got, CER) and its
  free-text `reason` becomes a `MissReason` enum, so misses aggregate by kind instead of having
  to be read one line at a time. CER is Levenshtein distance over expected length, hand-written
  in ~20 lines of `std` — adding `strsim` for one textbook function would spend a dependency
  against `docs/VISION.md` §1. Ground truth is the generator's own MRZ lines parsed back through
  `mrz::parse_td3`, so both sides are `MrzData` in identical formats and the CER measures the
  *read* rather than a formatting difference. `run_check` was restructured so a checksum failure
  still yields the breakdown — the old version returned early and discarded exactly the read that
  says which field broke the checksum.

  **`hit` is deliberately unchanged**, since the M4 CI gate is defined on it: the 50-seed clean
  profile re-measures at **54%**, consistent with the 55% baseline, confirming the instrument is
  additive.

  What it found, over that same 50-seed run:

  | | |
  |---|---|
  | Documents passing the Tier-1 gate | 27 / 50 |
  | …with **both names** read correctly | **1** |
  | …with **issuing country** read correctly | **4** |
  | MRZ line 1 wrong in its first 5 characters | 38 / 42 parsed |
  | MRZ line 2 character-perfect | 24 / 42 parsed |

  The cause is structural, not statistical. ICAO 9303 TD3 check digits cover **line 2 only** —
  document number, date of birth, date of expiry, personal number, and a composite over those.
  **Line 1 carries no check digit at all**, so document type, issuing country, surname and given
  names are unverified by the oracle the whole Tier-1 thesis rests on. A document can therefore
  be checksum-proven and still return the wrong name.

  The mechanism behind the line-1 errors is a single recurring misread: OCR **collapses interior
  runs of the `<` filler**, and the trailing filler run absorbs the loss, so the line stays 44
  characters and looks structurally valid while every field boundary after the first shifts left.
  `P<JPNSTRAND<<ALEKSANDER<<<…` reads as `PJPNSTRANDALEKSANDER<<<<…` — a 7.9% character error
  rate that yields `document_type` `"PJ"`, `issuing_country` `"PNS"`, `surname`
  `"TRANDALEKSANDER"` and an empty `given_names`. The per-field CER table is dominated by this
  cascade rather than by per-character noise, which is why `mrz_lines` CER (24.7% mean) is the
  honest per-character read quality and the line-1 field rates (66–92%) measure parse fragility.

  This **refutes** the hypothesis recorded in `docs/ROADMAP.md` after PR #30 that misses cluster
  on the 14-character `personal_number` field; that field's CER is 25%, mid-pack, and the roadmap
  note is corrected accordingly. It was a reasonable inference from a binary signal — it is just
  not what the data says once the signal stops being binary.
