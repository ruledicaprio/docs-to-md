# Third-Party Notices

`multi-level-id-strip` (MIT) bundles and builds on third-party open-source components.
All are permissively licensed (MIT / Apache-2.0 / BSD / ISC) — no copyleft.

> This file is hand-curated and, as of v1.0.0, still not independently verified against
> `cargo about`'s version-exact output for `llama-cpp-2`/`ocrs`/`rten` — treat the table below as
> a reliable *category* summary (all permissive, no copyleft), but regenerate the authoritative,
> version-exact list before a release:
>
> ```bash
> cargo install cargo-about cargo-deny
> cargo about generate about.hbs > THIRD_PARTY_NOTICES.md
> cargo deny check licenses           # fail the build on any surprise copyleft
> ```
>
> No Python dependencies remain in this project at all (removed in v0.7.5 along with the legacy
> gRPC sidecar and the `docling-serve` OCR engine).
>
> The `fuzz/` crate (coverage-guided fuzzing of `mrz`, v0.9.0) is its own detached Cargo
> workspace and is never built into a shipped binary — its `libfuzzer-sys` dependency is
> intentionally omitted from the table below, same reasoning as dev-only test dependencies.
>
> **Build-only tooling, never shipped in any binary** (v1.0.0's musl cross-compile path):
> `cargo-zigbuild` (MIT) and the [Zig](https://ziglang.org/) toolchain (MIT) are used at build
> time to cross-compile `mlis`/`mlis-serve` for `x86_64-unknown-linux-musl` — neither is a
> runtime or compile-time dependency of the crates themselves, so neither appears in the table
> below (same reasoning as the Rust toolchain itself not appearing there).

## Rust crates

| License | Crates (direct) |
| --- | --- |
| MIT OR Apache-2.0 | `tokio`, `axum`, `axum-server`, `tower`, `tower-http`, `hyper`, `hyper-util`, `serde`, `serde_json`, `rustls`, `image`, `thiserror`, `async-trait`, `uuid`, `getrandom` |
| MIT OR Apache-2.0 (RustCrypto / dalek-cryptography) | `sha2`, `aes-gcm`, `base64`, `ed25519-dalek` |
| MIT OR Apache-2.0 | `zeroize` (memory-wiping on drop, v0.9.0) |
| ISC AND (Apache-2.0 OR ISC) AND OpenSSL | `aws-lc-rs` / `aws-lc-sys` (default rustls crypto provider) |
| Apache-2.0 (wraps C libs) | `leptess` → **Tesseract** (Apache-2.0) + **Leptonica** (BSD-2-Clause) |
| *(unverified — see note above)* | `llama-cpp-2` → **llama.cpp**, `ocrs`, `rten` |
| MIT / Apache-2.0 | `wasm-bindgen` (WASM demo) |

The Rust toolchain and `std` are dual MIT/Apache-2.0 (© The Rust Project Developers).

## Frontend & models

| Component | License / Attribution |
| --- | --- |
| `tesseract.js` (browser OCR, loaded via CDN in the demo) | Apache-2.0 |
| Qwen 2.5 1.5B Instruct GGUF (downloaded at runtime, not vendored) | Apache-2.0 © Alibaba Cloud |
| **OCR-B model `web/tessdata/mrz.traineddata`** | **BSD-3-Clause © DoubangoTelecom** — see below |

### DoubangoTelecom OCR-B trained data (BSD-3-Clause)

The vendored MRZ OCR model (`web/tessdata/mrz.traineddata`) is redistributed from
[DoubangoTelecom/tesseractMRZ](https://github.com/DoubangoTelecom/tesseractMRZ) under the
BSD-3-Clause license. The full license text is preserved at
[`web/tessdata/LICENSE`](web/tessdata/LICENSE) and attributed in the live-demo footer.

---

Apache-2.0 components require their `NOTICE` files (where present) to be preserved; the
generated `cargo about` / `pip-licenses` output covers the complete, verbatim texts.
