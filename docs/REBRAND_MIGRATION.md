# REBRAND MIGRATION — `mlis-*` → `synthpass-*`

> **Status:** planning document. This specifies the mechanical migration from the `mlis-*`
> workspace to the `synthpass-*` namespace defined in [`BRANDING.md`](BRANDING.md). **Nothing
> here has been executed** — it is written to be handed off as a sequence of reviewable commits.
> Builds are verified through the Ubuntu Docker builder (`mlis-builder` / `mlis-dev`); the
> desktop/Windows environment has no local Rust linker.

## Scope at a glance

Measured against the current tree:

- **10 crates**, of which `mrz` + `mrz-wasm` **do not move** (see [`BRANDING.md`](BRANDING.md) §3).
- **~83** `mlis_<crate>::` / `use mlis_*` references across **18** `.rs` files.
- **~129** `MLIS_*` environment-variable occurrences across **18** `.rs` files (plus docs, CI,
  Dockerfiles).
- Binary rename `mlis` → `synthpass` touches the CLI `match` dispatcher, CI, Docker, the musl
  artifact name, README, and the demo.

## Crate mapping

| Current | Target | Notes |
|---|---|---|
| `mrz` | *unchanged* | Standalone MRZ library — keeps its identity and independence. |
| `mrz-wasm` | *unchanged* | Browser demo; travels with `mrz`. |
| `mlis-core` | `synthpass-core` | |
| `mlis-ocr` | `synthpass-ocr` | |
| `mlis-llm` | `synthpass-llm` | |
| `mlis-license` | `synthpass-license` | binary `mlis-license-issuer` → `synthpass-license-issuer` |
| `mlis-pipeline` | `synthpass-pipeline` | |
| `mlis-cli` | `synthpass-cli` | binary `mlis` → `synthpass` (keep `mlis` as an alias for one release) |
| `mlis-serve` | `synthpass-serve` | |
| `ocr-daemon` | `synthpass-tesseract-daemon` | Linux/WSL-only; stays **out** of `default-members`. |
| *(new — ROADMAP M2)* | `synthpass-gen` | The synthetic-document factory. |
| *(new — ROADMAP M4)* | `synthpass-bench` | The benchmark suite. |

> Rust crate names use underscores in code paths (`synthpass_core`) and hyphens in directory /
> package names (`synthpass-core`) — the same convention already in use for `mlis-*`.

## Sequenced execution (one reviewable commit per layer)

Keep `cargo build --workspace` (via the Docker builder) green at the end of **each** step.

1. **Directories + package names.** Rename `crates/mlis-*/` directories; update each crate's
   `[package].name`; update root [`Cargo.toml`](../Cargo.toml) `members` **and**
   `default-members` (keep `ocr-daemon` → `synthpass-tesseract-daemon` out of `default-members`, as
   today). Update every inter-crate path dependency key.
2. **Code references.** Rewrite `use mlis_*` / `mlis_*::` (~83 sites, 18 files) to
   `synthpass_*`. Grep-driven, per crate; the compiler is the checklist.
3. **Environment variables.** `MLIS_*` → `SYNTHPASS_*` (~129 sites). **Honour the old names for
   one release** with a deprecation warning — mirrors Atlas break **B4**
   ([`mlis_v2_0_0_preliminary_design.md`](mlis_v2_0_0_preliminary_design.md) §9). Centralise the
   old→new fallback in one helper rather than scattering `env::var` chains.
4. **Binaries + CLI.** Rename the `mlis` binary to `synthpass` and the license-issuer binary;
   update the hand-rolled `match` dispatcher in `crates/mlis-cli/src/main.rs` (add the
   `generate` arm here when ROADMAP M3 lands); keep a thin `mlis` alias for one release.
5. **Packaging + docs.** CI workflows, Dockerfiles, the static `musl` artifact name, README
   badges/links, `CHANGELOG.md`, and the WASM demo strings/URLs. Note the GitHub Pages demo
   URL and any custom domain (`checkin-demo.presentation.ba`) are deployment concerns — plan
   redirects rather than silent breaks.

## Non-goals for the rename

- **No GitHub org / remote migration.** Moving to `identra-org` + a `synthpass` repo is
  declared intent in [`BRANDING.md`](BRANDING.md), not part of this mechanical rename.
- **No behaviour change.** This is a rename only; feature work is the ROADMAP milestones.
- **`mrz` / `mrz-wasm` untouched.**

## Verification

- `cargo build --workspace` and `cargo test --workspace` green via the Docker builder after
  each step (and `cargo build -p synthpass-tesseract-daemon` + `-p synthpass-pipeline --features
  native-ocr` for the Linux-only path).
- Grep confirms zero remaining `mlis_*` code references (env-var fallbacks excepted, and only
  those, for the one-release deprecation window).
- The `synthpass` binary runs the same commands the `mlis` binary did; the `mlis` alias still
  works and emits the deprecation notice.
- No commits or pushes without an explicit go-ahead.
