- **`synthpass-bench` now measures what fraction of Tier-1 hits still have a structurally
  suspect line 1**, not just the binary hit rate. `check_document`'s `HitResult` carries the new
  `synthpass_core::fusion::check_line1_integrity` verdict alongside its per-field CER, and the
  corpus runner reports it: **of 29 Tier-1 hits on the 50-seed clean profile, 28 (96.6%) are
  still flagged** — the check-digit gate proves the read is faithful over a bit more than a third
  of the record and says nothing about the rest, and this is the first automated measurement of
  how often that gap actually bites, rather than the one hand-counted specimen in the per-field
  CER note. `hit`/`hit_rate` themselves are unchanged, so the CI gate keeps its meaning.
