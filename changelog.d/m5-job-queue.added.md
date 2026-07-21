- **Batch job queue (M5): `POST /api/extract/batch` + `GET /api/jobs/{id}`, and `synthpass batch
  <dir|glob>`.** A new `synthpass_pipeline::jobs` module adds `Pipeline::submit`/`Pipeline::job`:
  submit N documents, get a `JobHandle` back immediately (`Queued` → `Running` → `Done`/`Failed`),
  and poll per-document results as they land. Each document is dispatched as an independent
  `process_document` call — a batch parallelizes exactly as much as the two concurrency
  semaphores below allow, nothing more. Completed jobs are retained in a bounded ring buffer
  (`SYNTHPASS_QUEUE_CAPACITY`, default 100) so a long-running server's job map doesn't grow
  unbounded; jobs are in-memory only and do not survive a restart.

  `synthpass-serve` gains `POST /api/extract/batch` (multipart upload, `202 Accepted` + job id)
  and `GET /api/jobs/{id}` (status + per-document results, `404` once a job ages out of
  retention), both gated on the `batch` license feature and reusing the exact same
  `queue_full_error`/`api_error` shapes as `/api/extract` — capacity is a legitimate paid
  boundary (BRANDING §5), single-document extraction stays ungated. `synthpass batch <dir|glob>`
  adds the same capability to the CLI, staying synchronous end-to-end (submit + wait, one JSON
  object printed per input plus a summary) — only `synthpass-serve` exposes async job polling.

  **`SYNTHPASS_MAX_QUEUE_DEPTH` note:** this variable already controls `synthpass-serve`'s
  `/api/extract` queue-full threshold and keeps that exact meaning. The new job queue reads
  `SYNTHPASS_QUEUE_CAPACITY` for its own (unrelated) completed-job retention size, but — for one
  release — falls back to `SYNTHPASS_MAX_QUEUE_DEPTH` if only that's set, with a deprecation
  warning, so an operator who already tuned it doesn't get silently ignored on upgrade. Set
  `SYNTHPASS_QUEUE_CAPACITY` explicitly to silence the warning; the two settings will fully
  separate in a future release.
- **Independent OCR-stage concurrency (`SYNTHPASS_OCR_THREADS`).** OCR is stateless, pure-CPU
  work with no shared model context to serialize (unlike Tier-2's single loaded `llama.cpp`
  context), so `synthpass-pipeline` now bounds it with its own semaphore, separate from the
  existing Tier-2 `llm_semaphore`. Default: available cores − 1, floored at 1. A single
  document's processing only ever holds one of the two semaphores at a time — the OCR permit is
  fully released before a Tier-2 permit is ever requested — so the two can't deadlock each other,
  and most of a batch (every MRZ-valid document) never touches the LLM semaphore at all.
