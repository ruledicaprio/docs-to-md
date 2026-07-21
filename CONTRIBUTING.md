# Contributing

Thanks for your interest in `multi-level-id-strip` (SynthPass). This is a pure-Rust workspace (no
Python, no Docker required for any functional path as of v0.7.5) plus a WASM browser demo.

## Layout

```
crates/mrz          zero-dep ICAO 9303 engine (TD1/TD2/TD3, checksum-verified OCR repair)
crates/mrz-wasm     wasm-bindgen wrapper for the GitHub Pages demo
crates/synthpass-gen     synthetic passport factory: seeded identities, TD3 MRZ, layout/render/
                    labels, capture-degradation profiles (M1-M3 of docs/ROADMAP.md)
crates/synthpass-core    canonical Extraction schema + Tier-3 audit/crypto (feature `security`)
crates/synthpass-llm     in-process Tier-2 inference: Qwen GGUF via `llama-cpp-2`
crates/synthpass-ocr     in-process pure-Rust OCR: `ocrs`/`rten`
crates/synthpass-pipeline  OcrEngine trait → Tier 1 MRZ → Tier 2 InferBackend → JSON
crates/synthpass-cli     CLI (binary `synthpass`; extract, `generate`, `decrypt`, `doctor`, ...)
crates/synthpass-serve   axum web app (auth, TLS)
```

## Building & testing

The default toolchain target is `x86_64-pc-windows-msvc`; if you don't have the MSVC linker (or on
any OS), the simplest reproducible path is a Linux Rust container. `docker/Dockerfile.builder`
provides a reproducible one (replaces an earlier ad-hoc `docker commit`-built image), matching
`.github/workflows/ci.yml`'s `rust` job's system deps plus the musl/Zig toolchain from v1.0.0:

**PowerShell (Windows, no Git Bash needed):**
```powershell
docker build -f docker/Dockerfile.builder -t synthpass-builder:latest .
docker run --rm -v "${PWD}:/work" `
  -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry `
  -w /work synthpass-builder:latest cargo test --workspace
```

**bash (Linux / macOS / Git Bash on Windows):**
```bash
docker build -f docker/Dockerfile.builder -t synthpass-builder:latest .

# Git Bash on Windows needs MSYS_NO_PATHCONV=1 so `-w /work` isn't mangled into a Windows path;
# harmless (and unnecessary) on Linux/macOS.
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" \
  -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry \
  -w /work synthpass-builder:latest bash -c "cargo test --workspace"
```

### Cross-compiling to musl locally

`synthpass-builder` ships `x86_64-unknown-linux-musl`'s Rust std, a pinned Zig (the `CC`/`CXX` for
`llama-cpp-2`'s C++ build under musl), and `cargo-zigbuild` — see docs/ARCHITECTURE.md §10 for why
Zig was chosen over `cross-rs`/manual `musl-gcc`:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" \
  -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry \
  -w /work synthpass-builder:latest \
  cargo zigbuild --release --target x86_64-unknown-linux-musl -p synthpass-cli -p synthpass-serve \
    --features ocr-embedded
```

`ocr-embedded` bakes both `.rten` OCR models into the binary at compile time (needs
`text-detection.rten`/`text-recognition.rten` already present under `SYNTHPASS_OCR_MODEL_DIR`, default
`.` — see `crates/synthpass-ocr/build.rs`); omit it to keep the regular runtime-download OCR path. Verify
the result actually links static: `file target/x86_64-unknown-linux-musl/release/synthpass` should say
"statically linked".

### Fuzzing (`mrz`)

`cargo test --workspace` already runs an always-on `proptest` "never panics" suite over `mrz`
(`crates/mrz/tests/fuzz_props.rs`) — no extra setup needed. For deeper, coverage-guided fuzzing:

```bash
cargo install cargo-fuzz   # nightly toolchain required
cargo +nightly fuzz run mrz_find_and_parse -- -max_total_time=60
cargo +nightly fuzz run mrz_parse_td -- -max_total_time=60
```

`fuzz/` is its own detached Cargo workspace (see `fuzz/Cargo.toml`) so it never affects
`cargo build/test --workspace` at the repo root. If a run finds a crash, minimize it, add the
input to `fuzz/corpus/<target>/` as a permanent regression seed, add a matching `#[test]` in
`mrz` asserting no panic, then fix the bug.

## Guidelines

- **Tests are the contract.** The `mrz` crate ships ICAO specimen + real-OCR-noise vectors; keep
  them green and add one for any new format/repair path.
- **Match the surrounding style.** Run `cargo fmt` and `cargo clippy --workspace`; keep comment
  density and naming consistent with the file you're editing.
- **The `mrz` crate stays zero-dependency** (it compiles to `wasm32`); don't add runtime deps to it.
- **No PII in logs or fixtures.** Use the public-domain specimens in `samples/`.

  This is a review checklist item, not just an aspiration — check it on every PR that touches a
  `tracing` call or the metrics surface:

  - A log or span field may carry **shape**: byte counts, durations, booleans, tier names,
    `request_id`. It may never carry **content**: OCR markdown, an extracted field value, or an
    uploaded filename (routinely the holder's name).
  - Metric labels are a **closed set** (`method`, `stage`, `le`). Never label a series with
    anything derived from a document — that is a cardinality explosion and a data leak on an
    endpoint built to be scraped.
  - Error strings handed up from a backend are logged verbatim, so an error you *author* must
    describe the failure without quoting document content.
  - `crates/synthpass-pipeline/tests/pii_logging.rs` enforces the first point with sentinel
    values. If you add a stage, add its sentinel there. Note that it asserts the capture buffer is
    non-empty before asserting on its contents: `tracing` caches callsite interest globally, so a
    logging test that never captures anything will otherwise pass while proving nothing.
- **One logical change per PR**, with a clear description. Reference issues where relevant.

### Adding a corpus specimen

`samples/` and `crates/synthpass-ocr/examples/mrz_corpus.rs` grow one individually-vetted image at a
time (see `docs/CORPUS_COVERAGE.md` for the current backlog by country). Before adding a new
specimen, confirm it meets **both**:

1. **Provenance.** Either a genuine `SPECIMEN` watermark printed on the document itself, or an
   established synthetic-placeholder MRZ number already used in this corpus (e.g. `E00000000`,
   `000000000`, `007007007`) — not a coincidence, an intentional marker that the document is a
   template. When neither is present, treat the image as unverified and don't add it.
2. **No real personal data.** Reject anything that reads as an actual person's document: a real
   name + real photo + a non-placeholder document number with no specimen marking. Also reject
   images watermarked by novelty/fake-ID-document vendors (e.g. "mrpassports.com"-style sites) —
   different in kind from an official specimen even if the image itself looks clean.

**Instant first-pass checking.** `./scripts/watch-samples.ps1` watches `samples/` and, the moment
a new image appears, runs `crates/synthpass-ocr/examples/check_sample.rs` against it (via the
`synthpass-builder` Docker image) and opens the image for you. It reports whether the file OCRs to a
checksum-valid MRZ and whether the word "specimen" appears anywhere in the OCR text — a fast,
non-authoritative signal, not a verdict. It does not replace the provenance/PII judgment above; a
clean OCR hit is not proof of a genuine specimen, and a miss is not proof it isn't. To check a
single file by hand instead: `cargo run -p synthpass-ocr --release --example check_sample -- samples/<file>`
in the Docker image.

When in doubt, ask rather than include. Once accepted: rename to the
`<Country>_<DocType>_Specimen[_<Variant>].<ext>` convention (full country name, e.g.
`North_Macedonia` not `North_macedonia`), run `cargo run -p synthpass-ocr --release --example
mrz_corpus -- --dump` in the `synthpass-builder` Docker image to confirm a Tier-1 HIT and read the real
doc number off the MRZ, add it to `CORPUS` (or `NEGATIVE` if the document has no MRZ at all), and
update the corresponding row in `docs/CORPUS_COVERAGE.md`.

**PRADO (`consilium.europa.eu/prado`) is never a source for specimens or any other data in this
repo.** Its copyright notice explicitly prohibits harvesting, copying, or redistributing PRADO
section material outside official, non-commercial use. It may be consulted manually as a human
reference (e.g. to check whether a document format is genuinely MRZ-bearing before deciding
whether to source a specimen) but nothing from it is ever fetched programmatically or stored in
this repository.

## Git workflow

`main` is a protected branch: no direct pushes, no force-pushes, even for the repo owner. Every
change lands via a pull request:

```bash
git fetch origin && git merge --ff-only origin/main   # start from current main, every time
git checkout -b <topic>-branch
# make changes, commit
git push -u origin <topic>-branch
gh pr create
```

The PR must show 2 green required checks before the merge button unlocks: `Rust (Linux)`
and `Rust (macos-latest, default members)`. (Earlier releases also required a Python
gRPC smoke test and a cross-language bridge test; both were removed as required checks in v0.7.5
along with the gRPC backend and its Python sidecar — see CHANGELOG.md.) No review approval is
required (solo maintainer), but a PR and passing CI always are.

If you work from more than one machine, always `git fetch && git merge --ff-only origin/main`
(or `git pull --ff-only`) before branching off — this fails loudly instead of silently diverging
if another client already pushed work you don't have yet.

## Changelog fragments

**Do not edit `CHANGELOG.md` in a PR.** Add a file to [`changelog.d/`](changelog.d/) instead:

```bash
cat > changelog.d/<pr-number-or-branch-slug>.added.md <<'EOF'
- **Short bold lead.** What changed, and what to do about it if it breaks something.
EOF
```

Category suffix is one of `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`.
`CHANGELOG.md` has one append point per section, so two branches open at once conflict on it
every single time — a file per change has no shared append point, and merge day stops being a
rebase queue. See [`changelog.d/README.md`](changelog.d/README.md) for the full convention.

At release time, `scripts/assemble-changelog.sh --write` splices the fragments into the topmost
section of `CHANGELOG.md` and deletes them (run it with no arguments to preview). Purely internal
changes that a user would never notice need no fragment at all.

## Commit sign-off

Contributions are accepted under the project's [MIT license](LICENSE). By submitting a PR you certify
you have the right to contribute the code (DCO-style `Signed-off-by` is welcome but not required).
