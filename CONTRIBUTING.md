# Contributing

Thanks for your interest in `multi-level-id-strip` (mlis). This is a pure-Rust workspace (no
Python, no Docker required for any functional path as of v0.7.5) plus a WASM browser demo.

## Layout

```
crates/mrz          zero-dep ICAO 9303 engine (TD1/TD2/TD3, checksum-verified OCR repair)
crates/mrz-wasm     wasm-bindgen wrapper for the GitHub Pages demo
crates/mlis-core    canonical Extraction schema + Tier-3 audit/crypto (feature `security`)
crates/mlis-llm     in-process Tier-2 inference: Qwen GGUF via `llama-cpp-2`
crates/mlis-ocr     in-process pure-Rust OCR: `ocrs`/`rten`
crates/mlis-pipeline  OcrEngine trait (rust | native) → Tier 1 MRZ → Tier 2 InferBackend → JSON
crates/mlis-cli     CLI (binary `mlis`, + `mlis decrypt`)
crates/mlis-serve   axum web app (auth, TLS)
crates/ocr-daemon   native Tesseract+Leptonica OCR engine (Linux/WSL only)
```

## Building & testing

The default toolchain target is `x86_64-pc-windows-msvc`; if you don't have the MSVC linker (or on
any OS), the simplest reproducible path is a Linux Rust container:

```bash
# needs: cmake, and (for ocr-daemon / native-ocr) clang + libtesseract/libleptonica
cargo test --workspace                 # cross-platform crates
cargo test -p ocr-daemon               # native OCR (Linux/WSL, tesseract installed)
cargo build -p mlis-pipeline --features native-ocr   # wire in the native engine
```

The native `ocr-daemon` is **Linux/WSL only** and excluded from `default-members`; CI must not build it
on Windows/macOS.

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
- **One logical change per PR**, with a clear description. Reference issues where relevant.

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

The PR must show 2 green required checks before the merge button unlocks: `Rust (Linux, incl.
native OCR)` and `Rust (macos-latest, default members)`. (Earlier releases also required a Python
gRPC smoke test and a cross-language bridge test; both were removed as required checks in v0.7.5
along with the gRPC backend and its Python sidecar — see CHANGELOG.md.) No review approval is
required (solo maintainer), but a PR and passing CI always are.

If you work from more than one machine, always `git fetch && git merge --ff-only origin/main`
(or `git pull --ff-only`) before branching off — this fails loudly instead of silently diverging
if another client already pushed work you don't have yet.

## Commit sign-off

Contributions are accepted under the project's [MIT license](LICENSE). By submitting a PR you certify
you have the right to contribute the code (DCO-style `Signed-off-by` is welcome but not required).
