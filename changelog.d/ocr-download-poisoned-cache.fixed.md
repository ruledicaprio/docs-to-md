- **A corrupted OCR model download no longer poisons the model directory permanently.**
  `synthpass_ocr::download::ensure_models` verified nothing before renaming a fetched `.rten` file
  into its final name, so a truncated or tampered response was committed to disk. The load path in
  `synthpass-pipeline` then rejected it (correctly, and it remains the security boundary) — but
  `download_if_missing` skips any path that already exists, so every later run reused the same bad
  file and failed again, with nothing in the error pointing at the fix. Recovery meant deleting
  `text-detection.rten` / `text-recognition.rten` by hand. The bytes are now checked against the
  same known-good hash *before* the rename; on a mismatch the `.part` scratch file is discarded and
  the error says so, so simply re-running retries the download. `SYNTHPASS_OCR_MODEL_SKIP_VERIFY=1`
  and the `SYNTHPASS_OCR_*_SHA256` overrides apply here exactly as they do to the existing
  file-based checks.
