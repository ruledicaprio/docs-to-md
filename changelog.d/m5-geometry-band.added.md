- **The detected MRZ band now drives a recognition pass, and 0°/180° is no longer a coin flip.**
  `preprocess::geometry_band_variants` crops to the content-scored band that `detect_mrz_band`
  finds (MRZ-charset density, ICAO line length, OCR-B aspect ratio) and runs the two proven
  treatments over it. These are chained strictly as **trailing** extras after every existing
  `mrz_variants` entry, and `mrz_variants` itself is untouched — since the retry loop breaks on
  the first checksum-valid MRZ, a new variant is only ever reached when every existing one already
  failed, which makes "no currently-passing specimen can regress" provable rather than asserted.
  `SYNTHPASS_OCR_MAX_PASSES`'s default rises 7 → 9 to admit the two new variants: the worst case
  is 6 + 2 retry variants plus the general pass, and the retry loop admits `max_passes - 1` of
  them. That arithmetic is now derived from a single constant and pinned by a test rather than
  described in a comment.

- **The 0°/180° tie-break is a comparison between orientations, not a guess about layout.**
  Orientation detection is detection-only and cannot distinguish 0° from 180° — both give
  identical horizontal line geometry — so `recognize_detailed` scores the MRZ band on the page
  *and* on its 180° flip (`geometry::detect_mrz_band_scored`) and keeps the better one.

  The first attempt used the obvious layout rule instead — the MRZ sits at the bottom on
  TD1/TD2/TD3, so a confident band in the upper third means the page is upside down — and it is
  recorded here because it **does not work**, for a reason that generalises: on a genuinely
  upside-down page the real MRZ is garbled, so it scores *low*, and unrelated mid-page noise
  routinely wins `detect_mrz_band` instead. The band's position then describes the noise. On the
  180°-rotated Croatian specimen the winning band sat mid-page, the upper-third test was false,
  and the flip never fired. Comparing orientations sidesteps this: it never has to locate the MRZ
  on the page it cannot read.

  **Measured over the 42-image `samples/` corpus**, each page scored upright and rotated 180°:
  the comparison gets the direction right on **41 of 42, with zero false flips**. Two constants
  come out of that sweep rather than out of intuition. `BAND_FLIP_MARGIN` (1.2×, mirroring
  `ROTATION_MARGIN`'s existing "ties default to leaving it alone" bias) is cleared comfortably by
  every genuine correction — narrowest real win 1.27×, most 2–5× — while suppressing the corpus's
  single wrong-direction vote (`Passport_of_Serbia_ID_2009_version.jpg`, a 1.18× lead) and all
  four exact ties. Serbia 2009 is consequently the one page here that stays mis-oriented: an
  honest miss rather than a silent wrong answer. `geometry::MRZ_BAND_CONFIDENT_SCORE` (0.75)
  skips the probe entirely when the upright band is already strong — no upside-down page in the
  corpus scored above 0.7132, so the flip could not have won — which keeps the common case at
  exactly its previous cost.

  The sweep also rules out the cheaper design of thresholding a single orientation: upside-down
  scores overlap genuine upright ones across most of the range (plenty of real, correctly
  oriented documents score 0.23–0.48), so no absolute cutoff separates them. Only running both
  and comparing does.
