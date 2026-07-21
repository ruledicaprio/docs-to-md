- **The detected MRZ band now drives a recognition pass, and 0°/180° is no longer a coin flip.**
  `preprocess::geometry_band_variants` crops to the content-scored band that `detect_mrz_band`
  finds (MRZ-charset density, ICAO line length, OCR-B aspect ratio) and runs the two proven
  treatments over it. These are chained strictly as **trailing** extras after every existing
  `mrz_variants` entry, and `mrz_variants` itself is untouched — since the retry loop breaks on
  the first checksum-valid MRZ, a new variant is only ever reached when every existing one already
  failed, which makes "no currently-passing specimen can regress" provable rather than asserted.
  Orientation detection is detection-only and cannot distinguish 0° from 180° (both give identical
  horizontal line geometry); the MRZ band settles it, since on TD1/TD2/TD3 the zone sits at the
  bottom, so a confident band in the upper third means the page is upside down. Absent a confident
  band, nothing changes. `SYNTHPASS_OCR_MAX_PASSES`'s default rises 7 → 9 to admit the two new
  variants: the worst case is 6 + 2 retry variants plus the general pass, and the retry loop
  admits `max_passes - 1` of them. That arithmetic is now derived from a single constant and
  pinned by a test rather than described in a comment.
