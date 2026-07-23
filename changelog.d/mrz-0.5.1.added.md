- **`mrz` 0.5.1: MRZ lines that lost a character to physical damage now resolve deterministically
  instead of falling through.** When a recognizer meets a glyph destroyed by a punched hole or
  hidden by a finger it typically *drops* it rather than emitting a placeholder, so the line
  arrives one character too narrow and every field after the damage is shifted — a read that
  decodes to plausible nonsense (an expiry of `2011-22-49`) rather than to a detectable error. The
  existing length repair could not help, because it only ever reinserted missing characters inside
  the longest `<` filler run or at the end of the line, and a punched hole lands in the middle of a
  digit field. `find_and_parse` now runs a bounded damaged-capture pass when nothing else validates:
  it sweeps the missing character across *every* insertion point and lets the check digits rule.
  Measured on a real ID card with a hole punched through its MRZ, the expiry recovers exactly
  (15 ms; a clean zone is unaffected at 180 µs and a page with no MRZ at 45 µs). The primitives are
  public — `width_candidates(line, target)` for the position sweep and `solve_field(field, check,
  kind) -> Resolution` (`Unique` / `Ambiguous` / `Unresolvable`) for resolving unknown positions
  against a field's own check digit, with `UNKNOWN`, `MRZ_ALPHABET` and `FieldKind` alongside.
  Two properties are deliberate and pinned by tests. First, a check digit sees a field only mod 10
  and so does the composite, so one destroyed position admits a whole residue class — four readings
  that *every* check digit accepts; only the calendar (`Date::is_well_formed`, via
  `FieldKind::Date`) separates them, which is why a recovered record must have well-formed dates
  before it is accepted. Second, two destroyed positions leave hundreds of readings that all
  verify, so the answer is `Ambiguous` and the scanner does not attempt a recovery at all: a guess
  that happens to be wrong is indistinguishable from a proof.
