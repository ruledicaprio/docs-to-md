# 🚀 mlis v2.0.0 — Complete Design Change Plan

**Status:** accepted, in execution (branch `v2-dev`)
**Baseline:** v1.1.0 (`main` @ `9a3b193`)
**Codename:** *Atlas* — because v2 carries more weight: more documents, more concurrency, more certainty.

> This document is the single source of truth for the v2.0.0 redesign. It follows the
> house style of `docs/ARCHITECTURE.md`: say what's true, state limitations plainly,
> and don't oversell. Each pillar lists its execution milestone (M1–M6) and its
> acceptance tests. When v2.0.0 ships, this file's deltas get folded back into
> `ARCHITECTURE.md` and this file remains as the design record.

---

## 1. Vision

v1 proved the thesis: **air-gapped, deterministic-first identity extraction as a single
static binary** — ICAO 9303 checksums as mathematical ground truth, a local LLM only as
a fallback, zero cloud calls, zero PII egress, offline Ed25519 licensing.

v1's ceiling is that it is a **single-document appliance**: one global LLM semaphore,
an MRZ-shaped output schema, OCR that returns a bare string, `println!` observability,
and a licensing layer that parses capacity fields but never enforces them.

**v2.0.0 turns the appliance into a platform:**

1. **Richer truth** — a versioned `Extraction` v2 schema with per-field confidence and
   provenance, address/portrait/barcode slots, and multi-document results.
2. **Eyes, not guesses** — OCR returns positioned, confidence-scored text lines, so the
   pipeline *detects* the MRZ region instead of assuming the bottom 45% of the page.
3. **Real capacity** — a bounded job queue, parallel OCR, configurable concurrent LLM
   contexts, batch extraction, and license-enforced throughput tiers.
4. **Production-grade operations** — `tracing` structured logs, `/health` + `/metrics`,
   request IDs, e2e HTTP tests, and a promoted accuracy gate.
5. **Smarter fallback** — grammar-constrained (GBNF) JSON decoding for Tier 2, making
   malformed-output repair the exception rather than the rule.

Breaking changes are allowed and deliberate — that is what the major version is for.
Every break is listed in §9 with its migration path.

## 2. Design principles (unchanged from v1, restated)

- **Deterministic before probabilistic.** Tier 1 first, always. The LLM never runs when
  a checksum can prove the answer.
- **Air-gapped or it doesn't ship.** Zero network calls in the processing path. Model
  fetches remain an explicit, checksum-verified bootstrap step, never runtime behavior.
- **Honesty over marketing.** Accuracy numbers come from the corpus harness, not vibes.
- **One binary.** Everything in-process; the musl static build remains the reference
  artifact. New dependencies must be pure Rust or justify themselves in writing.
- **PII paranoia.** Zeroize discipline extends to every new field; audit stays PII-free.

## 3. Pillar 1 — `Extraction` schema v2 (the keystone)  ·  Milestone M1

The v1 schema is MRZ-shaped: everything the MRZ can't say (address, portrait, document
expiry *status* as opposed to dates, per-field certainty) has nowhere to live, and Tier 2's
output is indistinguishable in confidence from Tier 1's proof.

**`mlis-core` gains `ExtractionV2`:**

```rust
pub struct ExtractionV2 {
    pub schema_version: u32,                    // = 2
    pub document: DocumentClass,                // passport | id_card | other + td format
    pub fields: ExtractionFields,               // v1 fields + address, personal_number_2
    pub confidence: FieldConfidence,            // per-field: proven (T1) | model (T2) score
    pub provenance: Provenance,                 // mrz-checksum | llm:<model-id> | wasm-client
    pub portrait: Option<ImageRef>,             // cropped face region (bbox + optional bytes)
    pub barcodes: Vec<BarcodeHit>,              // PDF417 etc. — slot now, decoder later
    pub mrz: Option<MrzBlock>,                  // raw lines + per-check-digit results
    pub validity: Option<Validity>,             // unchanged semantics from v1
    pub documents: Vec<ExtractionV2>,           // empty unless multi-doc input (M4)
    pub extraction_method: String,              // unchanged vocabulary
}
```

- **Per-field confidence**: Tier 1 fields are `1.0` (checksum-proven); Tier 2 fields get a
  heuristic score from GBNF parse cleanliness + field-level validators (date well-formed,
  country code in ISO table, check-digit-consistent document number when present).
- **Provenance** makes every downstream consumer able to distinguish *proven* from
  *inferred* — the single most commercially valuable property the product has, now explicit.
- **Backward compatibility**: `ExtractionV2: From<Extraction>` (confidence = method-derived
  default); `mlis-serve` serves v2 by default, `?v=1` (or `Accept`) returns the legacy
  shape for one major release. The `Extraction` v1 type stays, deprecated.
- **Acceptance**: round-trip tests v1→v2→JSON; schema snapshot test; WASM demo updated to
  render confidence/provenance.

## 4. Pillar 2 — OCR with eyes: structured `OcrResult`  ·  Milestone M2

Today `OcrEngine::to_markdown` returns `String`. The pipeline then *guesses* where the
MRZ lives (bottom-45% band, upscaled, retried up to 3×). `ocrs` already produces
`TextLine`s with bounding rects — we throw the geometry away.

**v2 widens the trait (breaking, allowed):**

```rust
pub struct OcrResult {
    pub markdown: String,                 // still produced — Tier 2 prompt input
    pub lines: Vec<OcrLine>,              // text + bbox + recognition confidence
}
#[async_trait]
pub trait OcrEngine: Send + Sync {
    async fn recognize(&self, path: &Path) -> Result<OcrResult, String>;
    fn describe(&self) -> String;
}
```

- **MRZ region detection**: find MRZ candidate lines by *content + geometry* (charset
  density of `[A-Z0-9<]`, line length ≈ 30/36/44, monospaced aspect) instead of page
  position. Retry passes crop the *detected* region, not a fixed band.
- **Rotation/orientation**: score OCR confidence at 0/90/180/270° using the line
  geometry (port of ocr-daemon's approach, minus the Linux-only Tesseract dependency),
  auto-rotate before the main pass. Removes the biggest known gap between `rust` and
  `native` engines.
- **Portrait crop**: face-region bbox heuristic (top-left quadrant of the VIZ for TD3/TD1)
  feeds `ExtractionV2.portrait`. Cropping only — no face recognition, no biometric
  matching, explicitly out of scope forever (see §11).
- **Acceptance**: corpus harness hit-rate ≥ 6/6 maintained *without* the fixed-band
  fallback; a rotated (90°) specimen validates end-to-end; `native` engine reimplemented
  behind the same trait or retired (decision recorded in CHANGELOG).

## 5. Pillar 3 — Capacity: queue, concurrency, batch  ·  Milestone M3

v1's model: one global semaphore, queue depth 4, then 503. Fine for a desk; useless for
a back office with a scanner feed.

- **Bounded job queue in `mlis-pipeline`**: `Pipeline::submit(job) -> JobHandle` with
  `JobStatus {queued, running, done, failed}` kept in a bounded ring buffer. Both
  binaries become thin submitters/pollers of the same queue — CLI stays synchronous
  (submit + wait), serve gains async job endpoints.
- **Parallel OCR**: OCR is stateless per document and pure CPU — run up to
  `MLIS_OCR_THREADS` (default: available cores − 1) OCR jobs concurrently. MRZ-valid
  documents never touch the LLM, so most of a batch parallelizes fully.
- **Configurable LLM concurrency**: `MLIS_LLM_CONTEXTS` (default 1) spins N independent
  `LlamaContext`s behind a semaphore of N permits. Single-context stays the default
  (CPU contention is real); the point is that capacity becomes a *knob*, and licensing
  (Pillar 5) can meter it.
- **Batch API**: `POST /api/extract/batch` (multipart, N files or a zip) → `202` +
  job id; `GET /api/jobs/{id}` → status + per-document results; SSE per-job progress
  retained. CLI: `mlis batch <dir|glob>` writing one JSON per input + a summary.
- **New endpoints**: `GET /health` (liveness + model warmth + license expiry, no auth),
  `GET /metrics` (Prometheus text: counters for docs by method/tier, histograms for
  OCR/Tier-2 latency, queue depth gauge).
- **Acceptance**: load test — 20 mixed documents (10 MRZ-valid, 10 Tier-2) through the
  batch API with `MLIS_LLM_CONTEXTS=2` completes with correct results and no semaphore
  starvation; queue-full returns 503 with `Retry-After`; e2e HTTP tests in CI.

## 6. Pillar 4 — Observability  ·  Milestone M3 (ships with capacity)

- **`tracing` everywhere**: replace `println!`/`eprintln!` with structured spans
  (`extract`, `ocr`, `tier1`, `tier2`) carrying a per-request `request_id` (UUID,
  surfaced in responses and audit records). `tracing-subscriber` fmt layer, `MLIS_LOG`
  filter (default `info`), optional JSON logs for log pipelines (`MLIS_LOG_FORMAT=json`).
- **PII rule, codified**: log macros may never receive document content or extracted
  fields — enforced by convention + a clippy-passing review checklist in CONTRIBUTING;
  audit log remains SHA-256-only.
- **Acceptance**: a failing extraction produces one greppable request-id trace from
  upload to error; no field value appears in any log line (test asserts on fixtures).

## 7. Pillar 5 — Licensing v2: enforce what's parsed  ·  Milestone M4

`mlis-license` already parses `features` and `mlis_min_version` and enforces neither.
v2 makes the license the capacity contract:

- **Feature gating**: `features: ["batch", "multi-context", "metrics"]` checked at
  startup; `mlis-serve` refuses to enable a gated surface the license doesn't name
  (fail closed, message says which feature is missing).
- **`mlis_min_version` enforced** at verify time against `CARGO_PKG_VERSION`.
- **Capacity metering**: optional `max_llm_contexts` in the payload caps
  `MLIS_LLM_CONTEXTS` (env asks, license permits; effective = min).
- **Issuer UX**: `mlis-license-issuer` gains `--features` presets (`trial`, `pro`,
  `enterprise`) so issuing a real license is one command, and the **placeholder
  `pubkey.b64` is replaced by a documented keygen step in the release runbook**.
- **Threat model unchanged and restated**: this meters the official binary, it is not
  DRM. v2 does not pretend otherwise.
- **Acceptance**: serve boots with a `trial` license → batch endpoint 403s with a
  named feature; `max_llm_contexts: 1` + `MLIS_LLM_CONTEXTS=4` → effective 1, logged.

## 8. Pillar 6 — Tier 2 gets a grammar, and the truth gets measured  ·  Milestone M5

Tier 2 is the weakest link (~45% per-field exact match, 25% test floor) and its output
path still runs regex-based JSON repair (`mlis-llm::repair`). Two moves:

1. **GBNF-constrained decoding**: llama.cpp grammars can force the model to emit
   schema-valid JSON, token by token. A grammar derived from `ExtractionFields` makes
   malformed output *unrepresentable* — `repair.rs` shrinks from "the thing that makes
   Tier 2 work" to a last-ditch fallback. Expected: elimination of the parse-failure
   class of errors and a measurable field-accuracy bump (the model spends its tokens on
   values, not syntax).
2. **Corpus, ground-truthed and gated**: the four untracked specimen photos
   (Germany/Austria/Türkiye/Spain) get ground-truth JSON + MD and join the corpus
   harness; the harness is promoted from `examples/` to a **required CI accuracy gate**
   (Tier-1 hit-rate must not regress; Tier-2 floor raised from 25% → measured-baseline
   −5%). Accuracy claims in docs are regenerated from harness output, not hand-edited.
- **Model agnosticism**: `MLIS_MODEL_PATH` + grammar approach works for any GGUF;
  docs gain a "bring a bigger model" section (Qwen 3B/7B) with measured accuracy/latency
  trade-offs. Shipping weights stays out of scope.
- **Acceptance**: parity harness shows statistically real improvement (reported honestly
  either way); zero repair-fallback invocations on the corpus; CI blocks a Tier-1
  regression.

## 9. Breaking changes & migration (the "2.0" in 2.0.0)

| # | Break | Migration |
|---|---|---|
| B1 | `OcrEngine::to_markdown -> recognize() -> OcrResult` | Trait has ≤2 in-tree impls; out-of-tree impls add bbox/confidence or use `OcrResult::from_text` shim |
| B2 | Default API response becomes `ExtractionV2` | `?v=1` / `Accept: application/vnd.mlis.v1+json` for one major release |
| B3 | `PipelineResult` carries `ExtractionV2` | `From<Extraction>` lift provided; field-by-field mapping documented |
| B4 | `MLIS_MAX_QUEUE_DEPTH` semantics move to the job queue (`MLIS_QUEUE_CAPACITY`) | Old var honored with deprecation warning for one release |
| B5 | `ocr-daemon`/`native-ocr` engine retired or reimplemented behind `OcrResult` | Decision recorded in CHANGELOG; Linux accuracy fallback preserved via orientation port if retired |
| B6 | License `features`/`mlis_min_version` now enforced | Existing licenses without `features` = all features (grandfathered, logged once) |
| B7 | `sidecar_stdout`, `MLIS_INFERER` vestiges deleted | Already dead; removal only |

Non-breaking by design: CLI command vocabulary, env-var names that still mean something,
WASM demo URL, audit-log format (additive fields only).

## 10. Milestones & execution order

| MS | Scope | Crates touched | Gate |
|---|---|---|---|
| **M1** | Schema v2 + confidence + provenance + compat | mlis-core, mlis-pipeline, mlis-serve, mrz-wasm, web | workspace green; snapshot + round-trip tests |
| **M2** | `OcrResult` + MRZ region detection + orientation | mlis-ocr, mlis-pipeline, (ocr-daemon decision) | corpus ≥6/6 without fixed band; rotated specimen passes |
| **M3** | Job queue + parallel OCR + LLM contexts + batch API + `/health` `/metrics` + tracing | mlis-pipeline, mlis-serve, mlis-cli | load test; e2e HTTP in CI; no PII in logs test |
| **M4** | Licensing v2 enforcement + issuer presets + real pubkey runbook | mlis-license, mlis-serve, mlis-cli | feature-gate + capacity-cap tests |
| **M5** | GBNF grammar decoding + corpus expansion + accuracy gate | mlis-llm, mlis-ocr, samples, CI | parity delta reported; CI accuracy gate required |
| **M6** | v2 release: version bump, ARCHITECTURE.md rewrite, README, CHANGELOG, musl artifact, demo redeploy | all | full matrix green; tag v2.0.0 |

Execution notes: milestones land on `v2-dev` as reviewable commits; M1→M2→M3 is the
dependency spine, M4/M5 can interleave. `main` keeps receiving nothing until M6.

## 11. Explicit non-goals for v2.0.0

- **Face recognition / biometric matching / liveness** — cropping a portrait region is
  in; *identifying a person* is out, permanently. This is an OCR/data-integrity tool.
- **Forgery / tamper detection** — unchanged from v1: checksums prove a faithful read,
  not authenticity. v2's confidence scores describe extraction certainty, not document
  genuineness.
- **GPU builds** — CPU-only remains the reference; `llama.cpp` upstream flags work but
  are unsupported and untested.
- **PDF/HEIC input** — still blocked on licensing (AGPL) and scope; the named-error
  behavior stays. (A commercial-licensed decoder remains an open follow-up.)
- **Cloud anything** — no telemetry, no model CDNs at runtime, no exception.
- **TPM/HSM attestation** — licensing threat model stays as documented.

## 12. Risks, stated plainly

- **GBNF on a 1.5B model** may improve syntax more than semantics; the parity harness is
  the judge and its verdict will be published whichever way it lands.
- **Multi-context llama.cpp on CPU** may contend badly beyond 2 contexts; the knob ships
  with measured guidance, not promises.
- **Schema v2 scope creep** is the schedule risk; `barcodes` is a slot, not a decoder,
  and multi-document input is M4-nice-to-have, not a gate.
- **Corpus is still small** (6+3, +4 in M5). v2 makes the corpus *growable and gated*;
  it does not claim production accuracy proof.
