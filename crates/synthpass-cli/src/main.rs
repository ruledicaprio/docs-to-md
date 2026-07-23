//! synthpass (CLI) — command-line front-end for the SynthPass extraction pipeline.
//!
//! Run from the repository root so the in-process model files resolve:
//!
//! ```powershell
//! cargo run -p synthpass-cli -- samples/ocr_fixtures/Croatian_passport_data_page.jpg
//! ```

use serde_json::json;
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use synthpass_pipeline::Pipeline;

mod generate;

/// Interior width of the banner box (character count between the two `│`
/// border columns). Wide enough for the longest centered line (the tagline).
const BOX_WIDTH: usize = 76;

/// Centers `text` in a field of `width` chars, padding with spaces on both
/// sides. Truncates instead of panicking if `text` is already too long, so a
/// future edit that overruns `BOX_WIDTH` degrades gracefully rather than
/// crashing `--help`.
fn center(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return text.chars().take(width).collect();
    }
    let pad = width - len;
    let left = pad / 2;
    let right = pad - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

fn box_line(text: &str) -> String {
    format!("│{}│", center(text, BOX_WIDTH))
}

/// Static ASCII banner in the style of a boxed CLI splash screen — printed
/// once for `--help`/no-args, never on the hot extraction path (stdout there
/// stays script/jq-friendly JSON).
fn banner() -> String {
    const M: [&str; 5] = ["█   █", "██ ██", "█ █ █", "█   █", "█   █"];
    const L: [&str; 5] = ["█    ", "█    ", "█    ", "█    ", "█████"];
    const I: [&str; 5] = ["█████", "  █  ", "  █  ", "  █  ", "█████"];
    const S: [&str; 5] = [" ████", "█    ", " ███ ", "    █", "████ "];

    let mut out = String::new();
    out.push_str(&format!("┌{}┐\n", "─".repeat(BOX_WIDTH)));
    out.push_str(&format!("{}\n", box_line("")));
    out.push_str(&format!("{}\n", box_line("[ SYNTHPASS ]")));
    out.push_str(&format!("{}\n", box_line(&"-".repeat(50))));
    out.push_str(&format!("{}\n", box_line("")));
    for row in 0..5 {
        let line = format!("{}  {}  {}  {}", M[row], L[row], I[row], S[row]);
        out.push_str(&format!("{}\n", box_line(&line)));
    }
    out.push_str(&format!("{}\n", box_line("")));
    out.push_str(&format!(
        "{}\n",
        box_line("Offline ICAO 9303 ID extraction — zero cloud calls, air-gapped by design")
    ));
    out.push_str(&format!("{}\n", box_line("")));
    out.push_str(&format!(
        "{}\n",
        box_line("[ MRZ TIER-1 ]  [ LLM TIER-2 ]  [ ED25519 LICENSE ]  [ AIR-GAPPED ]")
    ));
    out.push_str(&format!("{}\n", box_line("")));
    out.push_str(&format!("└{}┘", "─".repeat(BOX_WIDTH)));
    out
}

fn print_usage() {
    println!("{}", banner());
    println!();
    println!(
        "synthpass v{}  |  github.com/ruledicaprio/multi-level-id-strip",
        env!("CARGO_PKG_VERSION")
    );
    println!("{}", "-".repeat(BOX_WIDTH + 2));
    println!();
    println!("Commands");
    println!("  synthpass <path_to_image>          extract (needs a license — see below)");
    println!(
        "  synthpass batch <dir|glob>         extract every image in a directory or matching a glob"
    );
    println!("                                     (needs the license 'batch' feature; emits one JSON per input + a summary)");
    println!("  synthpass decrypt <file.json.enc>  decrypt (needs SYNTHPASS_KEY)");
    println!("  synthpass doctor                   preflight: OCR/inferer/license, config sanity");
    println!(
        "  synthpass fingerprint              print this machine's fingerprint (send to your vendor)"
    );
    println!("  synthpass verify-license [path]    verify a license file (default: SYNTHPASS_LICENSE_PATH or ./license.mlis)");
    println!("  synthpass generate [--count N] [--seed N] [--profile NAME] [--out-dir DIR]");
    println!("                                     generate synthetic passport images + label JSON (no license required)");
    println!("  synthpass --help, -h               show this message");
    println!("  synthpass --version, -V            show the version");
    println!();
    println!("No license yet? Run `synthpass fingerprint` and contact your vendor, or set");
    println!("SYNTHPASS_LICENSE_SKIP=1 to bypass the gate for local development.");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "--help" | "-h" => {
            print_usage();
            return Ok(());
        }
        "--version" | "-V" => {
            println!("synthpass {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        // `synthpass batch <dir|glob>` — extract every matching image, one JSON
        // object per input plus a summary (M5 job queue). Stays synchronous
        // end to end (submit + wait) — only synthpass-serve exposes async job
        // endpoints; see synthpass_pipeline::jobs's module doc.
        "batch" => return batch_command(args.get(2).map(String::as_str)).await,
        // `synthpass decrypt <file>` — decrypt an AES-256-GCM payload to stdout.
        "decrypt" => return decrypt_command(args.get(2).map(String::as_str)),
        // `synthpass generate` — synthetic passport image + label-JSON factory (M3).
        // No real PII is ever produced, so this bypasses `check_license` entirely,
        // same as `fingerprint`/`verify-license` below.
        "generate" => return generate::generate_command(&args[2..]),
        // `synthpass doctor` — preflight checks before running the pipeline for real.
        "doctor" => return doctor_command().await,
        // `synthpass fingerprint` / `synthpass verify-license` — diagnostic/recovery
        // commands that must work WITHOUT a valid license (you need
        // `fingerprint` to obtain one in the first place), so neither is gated
        // by `check_license` below.
        "fingerprint" => {
            println!("{}", synthpass_license::machine_fingerprint());
            return Ok(());
        }
        "verify-license" => return verify_license_command(args.get(2).map(String::as_str)),
        _ => {}
    }

    // Anything else is either a file path to extract or a typo'd flag/command
    // — an unknown `-`-prefixed arg is almost never a real filename, so give
    // a targeted error instead of a confusing "File not found: --hlep".
    if args[1].starts_with('-') {
        eprintln!("❌ Unknown option: {}", args[1]);
        eprintln!("   Run `synthpass --help` for usage.");
        return Ok(());
    }

    let input = Path::new(&args[1]);
    if !input.exists() {
        eprintln!("❌ Error: File not found at {}", input.display());
        return Ok(());
    }

    // Extraction is the one path that actually needs a valid license.
    if let Err(e) = check_license() {
        eprintln!("❌ {e}");
        eprintln!("   run `synthpass fingerprint` and contact your vendor for a license, or set SYNTHPASS_LICENSE_SKIP=1 for local development");
        return Ok(());
    }

    let pipeline = Pipeline::from_env();
    println!(
        "⚙️  [Rust] config: ocr={}, inferer={}, license={}",
        pipeline.ocr_engine(),
        pipeline.infer_describe(),
        if env::var("SYNTHPASS_LICENSE_SKIP").as_deref() == Ok("1") {
            "skipped".to_string()
        } else {
            env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into())
        }
    );
    println!(
        "🔄 [Rust] Uploading and processing local file: {} (ocr: {})...",
        input.display(),
        pipeline.ocr_engine()
    );

    match pipeline.process_document(input).await {
        Ok(result) => {
            println!("✅ [Rust] OCR successful!");
            println!("💾 [Rust] Saved Markdown to: {}", result.md_path.display());
            match result.method {
                synthpass_pipeline::Method::MrzDeterministic => {
                    println!("🔐 [Rust] ICAO 9303 checksums valid — deterministic MRZ extraction (LLM skipped)");
                    // The check digits cover `document_number`, the two dates
                    // and `personal_number` — not `document_type`,
                    // `issuing_country`, `nationality` or the names. Printing
                    // only "checksums valid" over a record whose own integrity
                    // verdict flags one of those reads as an all-clear, so the
                    // verdict goes to the terminal too, not just to the JSON.
                    if let Some(synthpass_core::fusion::Verdict::NeedsReview { reasons }) = result
                        .extracted_v2
                        .as_ref()
                        .and_then(|v2| v2.line1_integrity.as_ref())
                    {
                        println!(
                            "⚠️ [Rust] ...but {} line-1 field(s) carry no check digit and look wrong — review before trusting:",
                            reasons.len()
                        );
                        for reason in reasons {
                            println!("[Rust]   • {reason:?}");
                        }
                    }
                    if let Some(extracted) = &result.extracted {
                        println!("{}", serde_json::to_string_pretty(extracted)?);
                    }
                }
                synthpass_pipeline::Method::Llm => {
                    match &result.mrz {
                        Some(m) => println!(
                            "⚠️ [Rust] MRZ found but checksums failed ({:?}) — falling back to LLM",
                            m.checks
                        ),
                        None => println!("ℹ️ [Rust] No MRZ found — using LLM extraction"),
                    }
                    print!("{}", result.sidecar_stdout);
                }
            }
            match result.llm_error {
                None => println!(
                    "🎉 [Rust] Pipeline completed via {}! JSON saved to: {}",
                    result.method.as_str(),
                    result.json_path.display()
                ),
                Some(e) => eprintln!("⚠️ [Rust] LLM extraction failed: {e}"),
            }
        }
        Err(e) => eprintln!("❌ [Rust] {e}"),
    }

    Ok(())
}

/// Default path for the license file when `SYNTHPASS_LICENSE_PATH` is unset.
const DEFAULT_LICENSE_PATH: &str = "license.mlis";

/// Gate for the extraction path only (see call site in `main`) — `decrypt`,
/// `doctor`, `fingerprint`, and `verify-license` all stay usable without a
/// valid license. `SYNTHPASS_LICENSE_SKIP=1` bypasses this for local development,
/// mirroring `SYNTHPASS_MODEL_SKIP_VERIFY`.
fn check_license() -> Result<(), String> {
    if env::var("SYNTHPASS_LICENSE_SKIP").as_deref() == Ok("1") {
        return Ok(());
    }
    let path = env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    synthpass_license::load_and_check(Path::new(&path))
        .map(|_| ())
        .map_err(|e| format!("license check failed ({path}): {e}"))
}

/// Like [`check_license`], but for one specific license feature — used by
/// `batch` (gated on `FEATURE_BATCH`, the same commercial boundary
/// `synthpass-serve`'s `/api/extract/batch` enforces; BRANDING §5: capacity
/// is a legitimate paid gate, core single-document extraction is not).
fn check_license_feature(feature: &str) -> Result<(), String> {
    if env::var("SYNTHPASS_LICENSE_SKIP").as_deref() == Ok("1") {
        return Ok(());
    }
    let path = env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    let status = synthpass_license::load_and_check(Path::new(&path))
        .map_err(|e| format!("license check failed ({path}): {e}"))?;
    synthpass_license::check_feature(&status.payload, feature).map_err(|e| e.to_string())
}

/// File extensions the pure-Rust OCR engine can read — mirrors
/// `synthpass_pipeline::ocr`'s own allowlist (not exported from that crate,
/// so duplicated here rather than pulling in a new dependency just to share
/// eight string literals).
fn looks_like_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "tif" | "tiff" | "bmp" | "gif"
            )
        })
}

/// Minimal shell-style glob matcher: `*` matches any run of characters
/// (including none), `?` matches exactly one. No bracket/brace/double-star
/// support — this exists only so `synthpass batch` can accept a pattern like
/// `samples/passports/*.jpg` without pulling in a `glob` crate dependency for it.
fn glob_match(pattern: &str, name: &str) -> bool {
    fn helper(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => helper(&p[1..], n) || (!n.is_empty() && helper(p, &n[1..])),
            (Some(b'?'), Some(_)) => helper(&p[1..], &n[1..]),
            (Some(pc), Some(nc)) if pc == nc => helper(&p[1..], &n[1..]),
            _ => false,
        }
    }
    helper(pattern.as_bytes(), name.as_bytes())
}

/// Recursively walks `dir`, collecting every file for which `keep` returns
/// `true`. Used so `collect_batch_inputs` can find images nested in
/// subdirectories (e.g. `samples/passports/`, `samples/id_cards/`) rather
/// than only those directly inside the given directory.
fn walk_dir_files(dir: &Path, keep: &impl Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            walk_dir_files(&path, keep, out);
        } else if path.is_file() && keep(&path) {
            out.push(path);
        }
    }
}

/// Resolves `synthpass batch <arg>`'s argument into a sorted list of image
/// files: every image found recursively under `arg` (including
/// subdirectories) if it's a directory, or every file in `arg`'s parent
/// directory matching `arg`'s filename as a glob pattern otherwise (e.g.
/// `samples/passports/*.jpg`). Sorted so batch output order is deterministic
/// across runs (directory iteration order is not guaranteed by any
/// platform).
fn collect_batch_inputs(arg: &str) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(arg);
    if path.is_dir() {
        let mut files = Vec::new();
        walk_dir_files(path, &|p| looks_like_image(p), &mut files);
        files.sort();
        return Ok(files);
    }

    let (dir, pattern) = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => (
            parent.to_path_buf(),
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string(),
        ),
        None => (PathBuf::from("."), arg.to_string()),
    };
    if !dir.is_dir() {
        return Err(format!("{arg}: not a file, directory, or resolvable glob"));
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("could not read directory {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| glob_match(&pattern, name))
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(format!("no files matched {arg}"));
    }
    Ok(files)
}

/// `synthpass batch <dir|glob>` — extract every matching image. Submits the
/// whole batch as one job via `Pipeline::submit` (exercising the same job
/// abstraction `synthpass-serve`'s async endpoints use) and immediately
/// `.wait()`s on it, so from the operator's point of view this behaves like
/// a simple loop over `synthpass <path>` — one JSON object printed per
/// input, in the same order they were collected, plus a summary line.
async fn batch_command(arg: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let Some(arg) = arg else {
        eprintln!("Usage: synthpass batch <dir|glob>");
        return Ok(());
    };

    if let Err(e) = check_license_feature(synthpass_license::FEATURE_BATCH) {
        eprintln!("❌ {e}");
        eprintln!(
            "   run `synthpass fingerprint` and contact your vendor for a license with the \
             'batch' feature, or set SYNTHPASS_LICENSE_SKIP=1 for local development"
        );
        return Ok(());
    }

    let inputs = match collect_batch_inputs(arg) {
        Ok(inputs) if inputs.is_empty() => {
            eprintln!("❌ no image files found at {arg}");
            return Ok(());
        }
        Ok(inputs) => inputs,
        Err(e) => {
            eprintln!("❌ {e}");
            return Ok(());
        }
    };

    println!(
        "🔄 [Rust] submitting {} document(s) from {arg} for batch extraction...",
        inputs.len()
    );

    let pipeline = Pipeline::from_env();
    let handle = pipeline.submit(inputs.clone());
    let status = handle.wait().await;

    let mut tier1 = 0usize;
    let mut tier2 = 0usize;
    let mut failed = 0usize;

    for (input, entry) in inputs.iter().zip(handle.documents().iter()) {
        let record = match &entry.status {
            synthpass_pipeline::DocumentStatus::Done(result) => {
                match result.method {
                    synthpass_pipeline::Method::MrzDeterministic => tier1 += 1,
                    synthpass_pipeline::Method::Llm => tier2 += 1,
                }
                json!({
                    "input": input.display().to_string(),
                    "method": result.method.as_str(),
                    "extracted": result.extracted,
                    "error": result.llm_error,
                })
            }
            synthpass_pipeline::DocumentStatus::Failed(e) => {
                failed += 1;
                json!({ "input": input.display().to_string(), "error": e })
            }
            // Unreachable once `wait()` has returned (every document is
            // populated by then — see `JobHandle::wait`'s doc), but degrade
            // to a clearly-labeled record rather than panicking if that
            // invariant is ever violated.
            synthpass_pipeline::DocumentStatus::Pending => {
                failed += 1;
                json!({
                    "input": input.display().to_string(),
                    "error": "job ended without a result for this document",
                })
            }
        };
        println!("{}", serde_json::to_string_pretty(&record)?);
    }

    println!(
        "🎉 [Rust] batch complete ({}): {tier1} via Tier 1, {tier2} via Tier 2, {failed} failed",
        status.as_str()
    );

    Ok(())
}

/// `synthpass doctor`'s license block: required unless `SYNTHPASS_LICENSE_SKIP=1`
/// (mirrors the `SYNTHPASS_KEY`/`SYNTHPASS_AUDIT_LOG` blocks' shape below, but this
/// one toggles `ok` since — unlike those two — a missing/invalid license
/// blocks the extraction path entirely, not just an optional feature).
fn check_license_doctor(ok: &mut bool) {
    if env::var("SYNTHPASS_LICENSE_SKIP").as_deref() == Ok("1") {
        println!("✅ License: skipped (SYNTHPASS_LICENSE_SKIP=1)");
        return;
    }
    let path = env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    match synthpass_license::load_and_check(Path::new(&path)) {
        Ok(status) => {
            let days_left = status.days_until_expiry(synthpass_license::current_unix());
            if days_left < 30 {
                println!(
                    "⚠️  License ({path}): {} — expires in {days_left} days",
                    status.payload.tier
                );
            } else {
                println!(
                    "✅ License ({path}): {} — expires in {days_left} days",
                    status.payload.tier
                );
            }
        }
        Err(e) => {
            println!("❌ License ({path}): {e}");
            if matches!(e, synthpass_license::LicenseError::Io(_)) {
                println!(
                    "   Tip: no license yet? Run `synthpass fingerprint` to get one from your \
                     vendor, or set SYNTHPASS_LICENSE_SKIP=1 for local development."
                );
            }
            *ok = false;
        }
    }
}

/// `synthpass verify-license [path]` — verify a license file and print its
/// status. `path` overrides `SYNTHPASS_LICENSE_PATH`/the default. Non-zero exit
/// on any failure, matching `doctor`'s convention — this command's whole job
/// is to report validity, so a meaningful exit code matters for scripting.
fn verify_license_command(path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let path = path.map(String::from).unwrap_or_else(|| {
        env::var("SYNTHPASS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into())
    });

    match synthpass_license::load_and_check(Path::new(&path)) {
        Ok(status) => {
            let days_left = status.days_until_expiry(synthpass_license::current_unix());
            println!("✅ License valid ({path})");
            println!("   id: {}", status.payload.license_id);
            println!("   customer: {}", status.payload.customer);
            println!("   tier: {}", status.payload.tier);
            println!(
                "   bound to: {}",
                if status.payload.hw_fingerprint.is_empty() {
                    "(unbound — any machine)".to_string()
                } else {
                    status.payload.hw_fingerprint
                }
            );
            println!("   days until expiry: {days_left}");
            Ok(())
        }
        Err(e) => {
            eprintln!("❌ License invalid ({path}): {e}");
            Err(e.into())
        }
    }
}

/// `synthpass decrypt <file.json.enc>` — decrypt an AES-256-GCM payload (written when
/// `SYNTHPASS_KEY` is set) to stdout, using the same `SYNTHPASS_KEY`.
fn decrypt_command(file: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let Some(file) = file else {
        eprintln!("Usage: synthpass decrypt <file.json.enc>   (reads key from SYNTHPASS_KEY)");
        return Ok(());
    };
    let key = match env::var("SYNTHPASS_KEY") {
        Ok(s) => synthpass_core::crypt::key_from_base64(&s)?,
        Err(_) => {
            eprintln!("❌ set SYNTHPASS_KEY (base64-encoded 32-byte AES-256 key)");
            return Ok(());
        }
    };
    let data = std::fs::read(file)?;
    match synthpass_core::crypt::decrypt(&key, &data) {
        Ok(plain) => std::io::stdout().write_all(&plain)?,
        Err(e) => eprintln!("❌ decrypt failed: {e}"),
    }
    Ok(())
}

/// `synthpass doctor` — preflight checks: OCR/inferer reachability + config sanity.
/// OCR and inferer reachability are required for the pipeline to run at all
/// (a failure there is a non-zero exit); `SYNTHPASS_KEY`/`SYNTHPASS_AUDIT_LOG` checks are
/// advisory since those features are optional.
async fn doctor_command() -> Result<(), Box<dyn std::error::Error>> {
    let mut ok = true;

    let ocr_engine = env::var("SYNTHPASS_OCR_ENGINE").unwrap_or_else(|_| "rust".into());
    match ocr_engine.as_str() {
        "native" => println!("✅ OCR engine: native (in-process, no network check needed)"),
        _ => check_rust_ocr_models(&mut ok),
    }

    let pipeline = Pipeline::from_env();
    let infer_desc = pipeline.infer_describe();
    match pipeline.infer_health().await {
        Ok(status) => println!("✅ Tier-2 inferer ({infer_desc}): {status}"),
        Err(e) => {
            println!("❌ Tier-2 inferer ({infer_desc}) NOT healthy: {e}");
            ok = false;
        }
    }

    check_license_doctor(&mut ok);

    if let Ok(key) = env::var("SYNTHPASS_KEY") {
        match synthpass_core::crypt::key_from_base64(&key) {
            Ok(_) => println!("✅ SYNTHPASS_KEY is a valid base64 32-byte key"),
            Err(e) => println!(
                "⚠️  SYNTHPASS_KEY is set but invalid ({e}) — encryption will be silently disabled"
            ),
        }
    }

    if let Ok(log_path) = env::var("SYNTHPASS_AUDIT_LOG") {
        let parent_ok = Path::new(&log_path)
            .parent()
            .map(|p| p.as_os_str().is_empty() || p.exists())
            .unwrap_or(true);
        if parent_ok {
            println!("✅ SYNTHPASS_AUDIT_LOG parent directory exists ({log_path})");
        } else {
            println!(
                "⚠️  SYNTHPASS_AUDIT_LOG parent directory does not exist ({log_path}) — audit records will silently fail to write"
            );
        }
    }

    if ok {
        Ok(())
    } else {
        Err("doctor: one or more required checks failed".into())
    }
}

/// Models are baked into the binary at compile time (`ocr-embedded` feature,
/// musl release builds) — nothing on disk to check, and no filesystem/network
/// access at all is exactly the point.
#[cfg(all(feature = "ocr-native-rust", feature = "ocr-embedded"))]
fn check_rust_ocr_models(_ok: &mut bool) {
    println!("✅ OCR (rust) detection+recognition models embedded in binary");
}

/// Checks the default `rust` OCR engine's two `.rten` weight files: present
/// under `SYNTHPASS_OCR_MODEL_DIR` (default `.`) and sha256-verified — unlike a
/// pure reachability check, this engine can fail at startup on missing or
/// corrupt weights.
#[cfg(all(feature = "ocr-native-rust", not(feature = "ocr-embedded")))]
type OcrModelVerifyFn = fn(&Path) -> Result<(), synthpass_ocr::verify::VerifyError>;

#[cfg(all(feature = "ocr-native-rust", not(feature = "ocr-embedded")))]
fn check_rust_ocr_models(ok: &mut bool) {
    let model_dir = env::var("SYNTHPASS_OCR_MODEL_DIR").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&model_dir);
    let skip = synthpass_ocr::verify::skip_verify();
    let checks: [(&str, std::path::PathBuf, OcrModelVerifyFn); 2] = [
        (
            "detection",
            dir.join(synthpass_ocr::download::DETECTION_FILENAME),
            synthpass_ocr::verify::verify_detection_model,
        ),
        (
            "recognition",
            dir.join(synthpass_ocr::download::RECOGNITION_FILENAME),
            synthpass_ocr::verify::verify_recognition_model,
        ),
    ];
    for (label, path, verify_fn) in checks {
        if !path.exists() {
            println!("❌ OCR (rust) {label} model missing at {}", path.display());
            *ok = false;
        } else if skip {
            println!(
                "✅ OCR (rust) {label} model present at {} (sha256 verification skipped)",
                path.display()
            );
        } else {
            match verify_fn(&path) {
                Ok(()) => println!(
                    "✅ OCR (rust) {label} model present and sha256-verified at {}",
                    path.display()
                ),
                Err(e) => {
                    println!("❌ OCR (rust) {label} model: {e}");
                    *ok = false;
                }
            }
        }
    }
}

#[cfg(not(feature = "ocr-native-rust"))]
fn check_rust_ocr_models(ok: &mut bool) {
    println!("❌ OCR engine 'rust' selected but this build lacks the `ocr-native-rust` feature");
    *ok = false;
}
