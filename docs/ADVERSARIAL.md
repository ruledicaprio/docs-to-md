# ADVERSARIAL.md — degraded capture profiles as the adversarial/stress corpus

`synthpass-gen` renders a pristine "clean" data page by default. Real-world identity-document
captures are never that clean: a phone photo has motion blur and uneven lighting, a flatbed
scanner has different noise and perfect alignment, a worn/handled document has creases and faded
ink, a kiosk camera has slight skew and flash glare. `crates/synthpass-gen/src/degrade.rs` turns
a pristine render into a plausibly-degraded one via `CaptureProfile`, so the extraction pipeline
can be benchmarked against something closer to a real capture — this is SynthPass's adversarial /
stress corpus.

## The four profiles

| Profile | Simulates | Recipe (`profile_recipe`) |
|---|---|---|
| **Mobile** | Handheld phone photo: motion blur, uneven lighting/vignette, moderate JPEG-like noise. | Gaussian blur (σ=1.3) → vignette (strength 0.35) → noise (amount 0.15) |
| **Scanner** | Flatbed scanner: perfect alignment, mild uniform noise, slight sharpening halo. | Noise (amount 0.05) → contrast boost (factor 1.1) |
| **Worn** | Handled/aged document: creases, scuffs, faded ink (reduced contrast). | 3 crease artifacts → noise (amount 0.1) → contrast fade (factor 0.75) |
| **Border-kiosk** | Border-control kiosk camera: slight skew/rotation, harsh flash glare, moderate compression blockiness. | Rotate (2.5°) → glare (strength 0.6) → JPEG blockiness (strength 0.5) |

Each profile is composed from small, independently toggleable [`Degradation`] primitives
(`GaussianBlur`, `Noise`, `Rotate`, `Contrast`, `VignetteOrGlare`, `Crease`, `JpegBlockiness`) —
a caller can inspect a profile's recipe, or drop/reorder/replace a step, without touching the
degradation engine itself.

These passes **simulate** degradation for benchmarking purposes; they do not model any specific
real capture device's exact noise transfer function, lens distortion, JPEG DCT quantization
tables, or paper/ink physics. "Looks meaningfully different and reproducible" is the bar, not
photorealism.

## Determinism

`apply`/`apply_profile` seed a single `ChaCha8Rng` from the caller's `seed` argument and thread it
through every step in order, so the same `(image, recipe, seed)` always produces byte-identical
output pixels, and a different seed produces different noise/blur/rotation jitter within each
step's declared parameters. See `crates/synthpass-gen/tests/degrade.rs` for the round-trip and
determinism regression tests.

## Status: measured, not (yet) merge-blocking

Run `synthpass-bench --profile all` (or an individual profile name) to measure the Tier-1 hit rate
against each degraded variant — see [`SYNTHPASS.md`](SYNTHPASS.md) for the runner's flags and
report format. **Only the `clean` profile is CI-gated today** (`.github/workflows/ci.yml`'s M4
Tier-1 hit-rate gate). The four degraded profiles are measured and reported for visibility, but
don't block merges yet — there isn't enough accumulated data to set a realistic, non-flaky floor
per profile the way there is for `clean`. Once real numbers exist across enough runs, individual
degraded-profile gates are natural follow-up work (tracked as part of M5/future roadmap work, not
this document).
