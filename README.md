# 🪪 multi-level-id-strip (mlis)

<!-- Language / backend -->
![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
![Tokio](https://img.shields.io/badge/Tokio-0B7261?style=flat&logo=rust&logoColor=white)
![Axum](https://img.shields.io/badge/Axum-000000?style=flat&logo=rust&logoColor=white)
<!-- ML / inference -->
![llama.cpp](https://img.shields.io/badge/llama.cpp-in--process-555555?style=flat)
![Qwen 2.5](https://img.shields.io/badge/Qwen%202.5-1.5B%20q4__k__m-6236FF?style=flat)
![Python](https://img.shields.io/badge/Python-legacy%20fallback-3776AB?style=flat&logo=python&logoColor=white)
<!-- OCR / runtime -->
![ocrs](https://img.shields.io/badge/ocrs%2Frten-OCR%20in--process-FF6600?style=flat)
![Docker](https://img.shields.io/badge/Docker-optional-2496ED?style=flat&logo=docker&logoColor=white)
<!-- MRZ / demo -->
![WebAssembly](https://img.shields.io/badge/WebAssembly-654FF0?style=flat&logo=webassembly&logoColor=white)
![ICAO 9303](https://img.shields.io/badge/ICAO%209303-MRZ%20checksums-0B7261?style=flat)
[![Live demo](https://img.shields.io/badge/live%20demo-GitHub%20Pages-222222?style=flat&logo=github&logoColor=white)](https://ruledicaprio.github.io/multi-level-id-strip/)
<!-- Posture -->
![Air-gapped](https://img.shields.io/badge/deploy-air--gapped-10B981?style=flat)
![Cloud calls](https://img.shields.io/badge/cloud%20calls-0-brightgreen?style=flat)
![License](https://img.shields.io/badge/license-MIT-blue?style=flat)

Air-gapped document extraction: passports and ID cards in — structured JSON out, with **zero cloud calls**. A shared Rust pipeline OCRs the input, validates identity documents **deterministically via ICAO 9303 MRZ check digits** (Tier 1), and only falls back to a quantized Qwen 2.5 GGUF model (Tier 2) when no valid MRZ exists — which also catches other unstructured scans, though there's no dedicated extraction schema for them yet. As of **v0.7.0** both stages run **in-process** by default: OCR via [`mlis-ocr`](crates/mlis-ocr/) (`ocrs`/`rten`, pure Rust) and Tier 2 via [`mlis-llm`](crates/mlis-llm/) (`llama-cpp-2`) — no Python, no gRPC sidecar, and no Docker container required for either stage, on Windows/macOS/Linux alike. `docling-serve` (Docker) is still available as an opt-in engine for PDF input, which `ocrs` can't parse — see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the full design and the road to a single static binary. Use it from the CLI, a self-hostable axum web app, or the [**browser-only MRZ demo**](https://ruledicaprio.github.io/multi-level-id-strip/) — no PII ever leaves your machine.

## 🔀 Pipeline

```mermaid
flowchart LR
    subgraph entry["Entry points"]
        A["📄 Image / PDF"]
        A2["🌐 Browser demo"]
    end

    A --> B{"mlis-cli<br/>or mlis-serve"}
    B --> P["⚙️ mlis-pipeline<br/>process_document()"]

    P -->|"OcrEngine trait"| OCR{"OCR engine"}
    OCR -->|"rust (default)"| RUSTOCR["mlis-ocr crate<br/>in-process ocrs/rten<br/>pure Rust · image-only"]
    OCR -->|"docling (PDF input)"| DOCLING["🐳 docling-serve<br/>Docker, PDF-capable"]
    OCR -->|"native (Linux/WSL,<br/>opt-in)"| TESS["native ocr-daemon<br/>(Tesseract)"]
    RUSTOCR -->|Markdown| P
    DOCLING -->|Markdown| P
    TESS -->|Markdown| P

    P --> T1{"🔐 Tier 1<br/>ICAO 9303 checksum<br/>valid?"}
    T1 -->|"yes — deterministic,<br/>Tier 2 skipped"| OUT["📦 .md + .json"]

    T1 -->|"no — fallback"| T2{"🧠 Tier 2<br/>InferBackend trait"}
    T2 -->|"native (default)"| NAT["mlis-llm crate<br/>in-process llama.cpp<br/>Qwen 2.5 GGUF · CPU"]
    T2 -->|"grpc (legacy,<br/>one-release fallback)"| GRPC["🐍 Python sidecar<br/>llama-cpp-python · gRPC :50051"]
    NAT --> J2["strict JSON"]
    GRPC --> J2
    J2 --> OUT

    B -.->|"live SSE progress<br/>(mlis-serve only)"| T2

    A2 -->|"tesseract.js OCR +<br/>mrz crate as WASM"| PAGES["GitHub Pages<br/>no server at all"]
```

Both stages are deliberately pluggable. Tier 2 (see [`InferBackend`](crates/mlis-pipeline/src/infer.rs)): the **native** backend keeps everything in one process and one binary, which is the whole point of the road to v1.0.0's static musl build; the **gRPC** backend is what shipped through v0.5.x and stays available for one release as a safety net, with GPU acceleration for anyone who's already set up `llama-cpp-python` + CUDA. OCR (see [`OcrEngine`](crates/mlis-pipeline/src/ocr.rs)): the **rust** engine (`ocrs`/`rten`) is the new default as of v0.7.0 and needs no Docker/Python/C libraries at all; **docling** stays available for PDF input, which `ocrs` can't parse; **native** (Tesseract) is a Linux/WSL-only accuracy fallback. Full rationale in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## 🖼️ Example

A public-domain Croatian passport specimen (from [`samples/`](samples/)) → deterministic **Tier 1** extraction, with every ICAO 9303 check digit verified. The LLM never runs:

<img src="samples/Croatian_passport_data_page.jpg" alt="Croatian passport data page — public-domain specimen" width="460">

```json
{
  "document_type": "P",
  "issuing_country": "HRV",
  "issuing_country_name": "Croatia",
  "document_number": "007007007",
  "surname": "SPECIMEN",
  "given_names": "SPECIMEN",
  "nationality": "HRV",
  "nationality_name": "Croatia",
  "date_of_birth": "1982-12-25",
  "sex": "F",
  "date_of_expiry": "2014-07-01",
  "mrz_line": "P<HRVSPECIMEN<<SPECIMEN<<<<<<<<<<<<<<<<<<<<<\n0070070071HRV8212258F1407019<<<<<<<<<<<<<<06",
  "mrz_checksums_valid": true,
  "validity": { "dates_well_formed": true, "in_date": false, "dob_before_expiry": true },
  "extraction_method": "mrz-deterministic"
}
```

> `in_date: false` — the specimen expired in 2014 (the live output also carries an exact `days_until_expiry`). A valid composite check digit proves a faithful **read** of the printed zone; whether the document is *in date* is a separate, non-cryptographic judgement.

## 🔐 Deterministic MRZ validation (Tier 1)

The [`mrz`](crates/mrz/) crate is a zero-dependency ICAO 9303 parser: TD1/TD2/TD3, every check digit verified (7-3-1 weighting), with **checksum-verified OCR repair** — common misreads (`B`↔`8`, `O`↔`0`, filler runs read as `K`/`L`, dropped or hallucinated characters) are corrected by generating candidate readings and letting the composite check digit prove which one matches the printed zone. A valid composite is mathematical proof of a faithful read; a failed one flags a bad scan or a tampered document. When Tier 1 validates, the LLM never runs: extraction is instant, deterministic and hallucination-free.

> **Date validity ≠ authenticity.** A valid MRZ checksum and self-consistent dates prove the extraction faithfully matches what's printed on the document — not that the document itself is genuine or unaltered. This is an OCR/data-integrity tool, not a forgery-detection tool.

**[Try the live demo →](https://ruledicaprio.github.io/multi-level-id-strip/)** The same Rust code compiled to WebAssembly, with tesseract.js OCR, on a static GitHub Pages site. **No data is persistent on any server — there is no server.** Images are downscaled and processed entirely inside your browser tab; the extracted JSON is shown for 10 seconds with a copy button, then wiped.

## 🧠 In-process LLM fallback (Tier 2, v0.6.0)

When no valid MRZ exists, Tier 2 asks a quantized Qwen 2.5 1.5B model to read the OCR Markdown and fill the same [`Extraction`](crates/mlis-core/src/lib.rs) schema Tier 1 produces. As of v0.6.0 that model runs **in the same process** as the CLI or web server — [`mlis-llm`](crates/mlis-llm/) loads the GGUF once via [`llama-cpp-2`](https://crates.io/crates/llama-cpp-2), verifies its SHA-256 against a known-good hash before first use, and keeps it warm for the process lifetime. There's no sidecar to start, no gRPC port to open, and (once the pure-Rust OCR milestone lands) no Python anywhere in the default path.

This is explicitly a probabilistic fallback, not a second source of truth: a 1.5B model reading garbled OCR of a document it's never seen the layout for will not match a deterministic checksum for accuracy, and the field-level parity harness (`crates/mlis-llm/tests/parity.rs`) exists to catch regressions in that fallback quality, not to promise perfection. That asymmetry is *why* Tier 1 exists and is tried first on every document.

## 🚀 Quickstart

No Docker required for the default path — both OCR and Tier 2 run in-process. Image input only
(no PDF); see the [PDF / `docling`](#pdf-input--docling-serve) note below if you need that.

**1. Download the model** (~1 GB, not tracked in git):
```powershell
curl -L -o qwen2.5-1.5b-instruct-q4_k_m.gguf `
  https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf
```
The two OCR `.rten` weight files (~12 MB total) are fetched and SHA-256-verified automatically on
first run into `MLIS_OCR_MODEL_DIR` (repo root by default) — no manual step needed.

**2. Run** (from the repo root):
```powershell
# Preflight: checks OCR/inferer reachability and config before a real run
cargo run -p mlis-cli -- doctor

# CLI — one-shot extraction (binary `mlis`):
cargo run -p mlis-cli -- samples/Croatian_passport_data_page.jpg

# Web app — upload page + JSON API on http://127.0.0.1:8080
cargo run -p mlis-serve
```

```powershell
# API example:
curl -F "file=@samples/Passport_of_Serbia_ID_2009_version.jpg" http://127.0.0.1:8080/api/extract
```

### PDF input / `docling-serve`

`ocrs` (the default OCR engine) only accepts raster images. For PDF input, run the legacy
`docling-serve` Docker container and set `MLIS_OCR_ENGINE=docling`:
```powershell
docker run -d --name docling-serve -p 5001:5001 `
  -e OMP_NUM_THREADS=4 -e MKL_NUM_THREADS=4 `
  ghcr.io/docling-project/docling-serve
```

**Or bring the whole legacy stack up with Docker:** `docker compose -f docker/docker-compose.yml up`
(docling-serve + the gRPC `inferer` sidecar + web app, all pinned explicitly — see the compose
file's comments. The `inferer` container auto-downloads and checksum-verifies the GGUF into
`models/` on first start.)

<details>
<summary><b>Legacy: the Python gRPC sidecar</b> (feature <code>inferer-grpc</code>, kept as a fallback for one release)</summary>

```powershell
python -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install ./python                             # CPU-only (grpcio, pydantic, llama-cpp-python)
# or, with NVIDIA GPU acceleration (prebuilt CUDA 12.4 wheel):
pip install llama-cpp-python --extra-index-url https://abetlen.github.io/llama-cpp-python/whl/cu124

cd python; python generate_grpc.py; python -m inferer; cd ..   # serves gRPC on :50051
```
Then set `MLIS_INFERER=grpc` (and `MLIS_INFERER_ADDR` if not on `127.0.0.1:50051`) before running the
CLI or `mlis-serve`.
</details>

### Configuration (environment)

| Variable | Default | Purpose |
| --- | --- | --- |
| `MLIS_OCR_ENGINE` | `rust` | `rust` (in-process ocrs/rten, default), `docling` (PDF input), or `native` (Linux/WSL Tesseract, `--features native-ocr`) |
| `MLIS_OCR_MODEL_DIR` | `.` (repo root) | directory holding `text-detection.rten` / `text-recognition.rten`, `rust` engine only |
| `MLIS_OCR_DETECTION_SHA256` / `MLIS_OCR_RECOGNITION_SHA256` | *(built-in hash)* | override the expected checksums, `rust` engine only |
| `MLIS_OCR_MODEL_SKIP_VERIFY` | *(unset)* | skip the `rust` engine's model checksum verification |
| `MLIS_OCR_AUTO_DOWNLOAD` | `1` | `rust` engine: fetch missing `.rten` files automatically; `0` requires pre-staged files |
| `DOCLING_URL` | `http://localhost:5001` | docling-serve OCR endpoint, `docling` engine only |
| `MLIS_INFERER` | `native` | Tier-2 backend: `native` (in-process llama.cpp, default) or `grpc` (legacy Python sidecar) |
| `MLIS_MODEL_PATH` | `./qwen2.5-1.5b-instruct-q4_k_m.gguf` | GGUF path, `native` backend only |
| `MLIS_MODEL_N_CTX` | `2048` | context window (tokens), `native` backend only |
| `MLIS_MODEL_SHA256` / `MLIS_MODEL_SKIP_VERIFY` | *(built-in hash)* / *(unset)* | override the expected model checksum, or skip verification (`native` backend) |
| `MLIS_INFERER_ADDR` | `http://127.0.0.1:50051` | gRPC inferer endpoint, `grpc` backend only |
| `MLIS_MAX_QUEUE_DEPTH` | `4` | `mlis-serve`: reject uploads with 503 once this many Tier-2 requests are queued/in-flight |
| `MLIS_TOKEN` | *(unset)* | require `Authorization: Bearer <token>`; **mandatory for non-loopback `BIND_ADDR`** |
| `MLIS_TLS_CERT` / `MLIS_TLS_KEY` | *(unset)* | enable rustls TLS on `mlis-serve` |
| `MLIS_AUDIT_LOG` | *(unset)* | append PII-free SHA-256 audit records (JSONL) |
| `MLIS_KEY` | *(unset)* | base64 32-byte AES-256 key → encrypt output to `<input>.json.enc` (`mlis decrypt` to read) |

> **Windows note:** the native Tier-2 backend needs CMake + LLVM/libclang + MSVC to build `llama-cpp-2`'s bundled `llama.cpp` (see `crates/mlis-llm`). The default `rust` OCR engine (`mlis-ocr`, `ocrs`/`rten`) needs no native toolchain at all and works unchanged on Windows. The Tesseract-based `native` OCR engine (`ocr-daemon`) is Linux/WSL-only.
>
> **OCR accuracy note:** `ocrs` is brand-new to this project and has not yet been benchmarked against the field-accuracy parity harness that exists for Tier 2. On this workspace's own low/medium-resolution specimen samples, its out-of-the-box text recognition is not always clean enough to reconstruct a checksum-valid MRZ line (filler runs get mis-recognized or truncated) — see `docs/ARCHITECTURE.md`'s honest limitations section. When Tier 1 misses, Tier 2 still runs as usual.

## 📁 Repository Layout

```
├── crates/
│   ├── mrz/           Zero-dep ICAO 9303 engine: TD1/TD2/TD3, checksum-verified OCR
│   │                  repair, date-plausibility, ISO/ICAO country names
│   ├── mrz-wasm/      wasm-bindgen wrapper for the browser demo
│   ├── mlis-core/     Canonical Extraction schema + Tier-3 audit/crypto helpers
│   ├── mlis-llm/      In-process Tier-2 inference: Qwen GGUF via `llama-cpp-2`, ChatML
│   │                  prompting, JSON repair, model integrity check
│   ├── mlis-ocr/      In-process pure-Rust OCR: ocrs/rten, model download + integrity check
│   ├── mlis-pipeline/ OcrEngine trait (rust | docling | native) → Tier 1 MRZ →
│   │                  Tier 2 InferBackend (native | gRPC) → JSON
│   ├── mlis-cli/      CLI front-end (binary `mlis`; also `mlis decrypt`)
│   ├── mlis-serve/    axum web app: upload page + POST /api/extract (SSE progress on Tier 2),
│   │                  bearer auth + TLS
│   └── ocr-daemon/    Native Tesseract+Leptonica OCR engine (Linux/WSL only, legacy accuracy fallback)
├── proto/            inferer.proto — the gRPC contract for the legacy Python sidecar backend
├── python/inferer/   Legacy Tier-2 sidecar (grpcio, Qwen GGUF via llama.cpp) — one-release fallback
├── docker/           Legacy stack: Dockerfiles + docker-compose.yml (docling OCR + gRPC inferer + web)
├── web/              GitHub Pages demo site (static, client-side only)
├── samples/          Public-domain specimen documents + example outputs
└── docs/             Architectural manifest & roadmap
```

## 🔒 Security (Tier 3)

Everything runs on loopback by default. `mlis-serve` **refuses a non-loopback bind unless `MLIS_TOKEN` is set**, then enforces `Authorization: Bearer <token>` on every request; set `MLIS_TLS_CERT`/`MLIS_TLS_KEY` for rustls TLS. Uploaded files and intermediate artifacts are deleted after each request. Two optional at-rest controls: `MLIS_AUDIT_LOG` appends a **PII-free** SHA-256 audit trail (fingerprint + method + timestamp, no names/numbers), and `MLIS_KEY` (base64 32-byte AES-256) encrypts the output JSON to `<input>.json.enc` — decrypt with `mlis decrypt`. The native Tier-2 model itself is SHA-256-verified before use, so a tampered or substituted GGUF fails closed rather than running silently. Full rationale in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## 🙏 Acknowledgments

A solo-authored project where the ideas, architecture and direction are the human's; the execution was AI-accelerated.

- **Rusmir Skopljak** ([@ruledicaprio](https://github.com/ruledicaprio)) — creator, author, architecture & direction
- **Claude Opus 4.8 / Sonnet 5** (Anthropic) — orchestration & implementation
- **DeepSeek v4 Pro** — advisory / architectural review

Copyright and authorship rest with the human author; the AI tools are credited as assistants, not legal authors.

## 📜 License

[MIT](LICENSE) © Rusmir Skopljak. Bundled third-party licenses are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md); the security policy is in [SECURITY.md](SECURITY.md). The MRZ demo's OCR-B model (`web/tessdata/mrz.traineddata`) is © [DoubangoTelecom](https://github.com/DoubangoTelecom/tesseractMRZ), BSD-3-Clause.
