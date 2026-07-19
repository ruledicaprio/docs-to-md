# Security Policy

`multi-level-id-strip` processes **personally identifiable information** (passports, ID cards).
Security is a first-class concern; this document describes the posture and how to report issues.

## Supported versions

Solo-maintained project, patch releases only — the latest `1.0.x` release is the only supported
one. Pre-1.0 versions (roadmap milestones `v0.4.0` through `v0.9.0`) are unmaintained; upgrade to
`1.0.x` for any security-relevant fix.

| Version | Supported |
| --- | --- |
| 1.0.x | ✅ |
| < 1.0 | ❌ |

## Reporting a vulnerability

Please report privately — **do not** open a public issue for a security bug.

- Email **rusmirskopljak@gmail.com** with a description, reproduction steps, and impact.
- Or use GitHub's [private vulnerability reporting](https://docs.github.com/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability) on the repository.

Expect an acknowledgment within a few days. Coordinated disclosure is appreciated; please allow
time for a fix before any public write-up.

## Security posture

- **Air-gapped by design.** No cloud calls in the processing path; all OCR and LLM inference run on
  the local host / loopback. No telemetry.
- **Loopback by default.** `mlis-serve` binds `127.0.0.1`. It **refuses a non-loopback bind unless
  `MLIS_TOKEN` is set**, and then enforces `Authorization: Bearer <token>` on every request.
- **Transport security.** Optional rustls TLS via `MLIS_TLS_CERT` / `MLIS_TLS_KEY`. When exposed
  beyond loopback, terminate TLS (directly or via a reverse proxy) and keep the bearer token secret.
- **PII hygiene.** Uploaded files and intermediate artifacts are deleted after each request
  (`KEEP_WORK=1` retains them only for local debugging).
- **At-rest options.** `MLIS_AUDIT_LOG` writes a **PII-free** SHA-256 audit trail (document
  fingerprint + method + timestamp, never names/numbers). `MLIS_KEY` (base64 32-byte AES-256-GCM)
  encrypts the output JSON to `<input>.json.enc`; read it back with `mlis decrypt`.
- **Deterministic core.** Tier 1 (ICAO 9303 MRZ) is checksum-verified math, not a model — no
  hallucinated identity fields when a valid MRZ is present.
- **Model & license integrity.** The GGUF, both OCR `.rten` weight files (or their compile-time
  embedded equivalents, see below), and every license file are SHA-256/Ed25519-verified before
  use — a tampered or substituted file fails closed rather than running silently.
- **PII memory hardening (v0.9.0, best-effort).** The highest-value in-memory PII carriers
  (extracted fields, the AES key, raw Tier-2 output) are wiped on drop via `zeroize`. This does
  not cover every intermediate copy (`serde_json::Value` internals) or on-disk plaintext
  artifacts (only the optional `MLIS_KEY`-encrypted output is protected at rest) — see
  [docs/ARCHITECTURE.md §7](docs/ARCHITECTURE.md#7-security--compliance-posture) for the exact
  scope, stated plainly rather than oversold.
- **Fuzz-tested ingest path (v0.9.0).** The untrusted-OCR-text repair logic in `mrz` (also the
  public WASM demo's parser) is covered by an always-on `proptest` suite plus opt-in
  coverage-guided `cargo-fuzz`.
- **Static, air-gapped binary (v1.0.0).** The `x86_64-unknown-linux-musl` release build embeds
  OCR models at compile time and makes zero runtime network calls — nothing to compromise over
  the wire because there is no wire. Licensing is not hardware-bound (root can read the machine
  fingerprint, a from-source rebuild bypasses the check) — a compliance/metering mechanism, not
  DRM; see [docs/ARCHITECTURE.md §6](docs/ARCHITECTURE.md#6-offline-cryptographic-licensing-v080)
  for the full threat model.

## Hardening checklist for production

- [ ] Set a strong, unique `MLIS_TOKEN`.
- [ ] Enable TLS (`MLIS_TLS_*`) or front with a TLS-terminating reverse proxy.
- [ ] Set `MLIS_KEY` to encrypt outputs at rest; store the key in a secrets manager.
- [ ] Enable `MLIS_AUDIT_LOG` and ship the log to your SIEM.
- [ ] Keep the GGUF model and containers/binaries on trusted, access-controlled hosts.
- [ ] Run `cargo deny check advisories` in CI (no Python dependencies remain as of v0.7.5).
