//! mlis (CLI) — command-line front-end for the multi-level-id-strip pipeline.
//!
//! Run from the repository root so the in-process model files resolve:
//!
//! ```powershell
//! cargo run -p mlis-cli -- samples/Croatian_passport_data_page.jpg
//! ```

use mlis_pipeline::Pipeline;
use std::env;
use std::io::Write;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  cargo run -p mlis-cli -- <path_to_image>          extract (needs a license — see below)");
        eprintln!("  cargo run -p mlis-cli -- decrypt <file.json.enc>  decrypt (needs MLIS_KEY)");
        eprintln!("  cargo run -p mlis-cli -- doctor                   preflight: OCR/inferer/license, config sanity");
        eprintln!("  cargo run -p mlis-cli -- fingerprint              print this machine's fingerprint (send to your vendor)");
        eprintln!("  cargo run -p mlis-cli -- verify-license [path]    verify a license file (default: MLIS_LICENSE_PATH or ./license.mlis)");
        return Ok(());
    }

    // `mlis decrypt <file>` — decrypt an AES-256-GCM payload to stdout.
    if args[1] == "decrypt" {
        return decrypt_command(args.get(2).map(String::as_str));
    }

    // `mlis doctor` — preflight checks before running the pipeline for real.
    if args[1] == "doctor" {
        return doctor_command().await;
    }

    // `mlis fingerprint` / `mlis verify-license` — diagnostic/recovery
    // commands that must work WITHOUT a valid license (you need
    // `fingerprint` to obtain one in the first place), so neither is gated
    // by `check_license` below.
    if args[1] == "fingerprint" {
        println!("{}", mlis_license::machine_fingerprint());
        return Ok(());
    }
    if args[1] == "verify-license" {
        return verify_license_command(args.get(2).map(String::as_str));
    }

    let input = Path::new(&args[1]);
    if !input.exists() {
        eprintln!("❌ Error: File not found at {}", input.display());
        return Ok(());
    }

    // Extraction is the one path that actually needs a valid license.
    if let Err(e) = check_license() {
        eprintln!("❌ {e}");
        eprintln!("   run `mlis fingerprint` and contact your vendor for a license, or set MLIS_LICENSE_SKIP=1 for local development");
        return Ok(());
    }

    let pipeline = Pipeline::from_env();
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
                mlis_pipeline::Method::MrzDeterministic => {
                    println!("🔐 [Rust] ICAO 9303 checksums valid — deterministic MRZ extraction (LLM skipped)");
                    if let Some(extracted) = &result.extracted {
                        println!("{}", serde_json::to_string_pretty(extracted)?);
                    }
                }
                mlis_pipeline::Method::Llm => {
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

/// Default path for the license file when `MLIS_LICENSE_PATH` is unset.
const DEFAULT_LICENSE_PATH: &str = "license.mlis";

/// Gate for the extraction path only (see call site in `main`) — `decrypt`,
/// `doctor`, `fingerprint`, and `verify-license` all stay usable without a
/// valid license. `MLIS_LICENSE_SKIP=1` bypasses this for local development,
/// mirroring `MLIS_MODEL_SKIP_VERIFY`.
fn check_license() -> Result<(), String> {
    if env::var("MLIS_LICENSE_SKIP").as_deref() == Ok("1") {
        return Ok(());
    }
    let path = env::var("MLIS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    mlis_license::load_and_check(Path::new(&path))
        .map(|_| ())
        .map_err(|e| format!("license check failed ({path}): {e}"))
}

/// `mlis doctor`'s license block: required unless `MLIS_LICENSE_SKIP=1`
/// (mirrors the `MLIS_KEY`/`MLIS_AUDIT_LOG` blocks' shape below, but this
/// one toggles `ok` since — unlike those two — a missing/invalid license
/// blocks the extraction path entirely, not just an optional feature).
fn check_license_doctor(ok: &mut bool) {
    if env::var("MLIS_LICENSE_SKIP").as_deref() == Ok("1") {
        println!("✅ License: skipped (MLIS_LICENSE_SKIP=1)");
        return;
    }
    let path = env::var("MLIS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into());
    match mlis_license::load_and_check(Path::new(&path)) {
        Ok(status) => {
            let days_left = status.days_until_expiry(mlis_license::current_unix());
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
            if matches!(e, mlis_license::LicenseError::Io(_)) {
                println!(
                    "   Tip: no license yet? Run `mlis fingerprint` to get one from your \
                     vendor, or set MLIS_LICENSE_SKIP=1 for local development."
                );
            }
            *ok = false;
        }
    }
}

/// `mlis verify-license [path]` — verify a license file and print its
/// status. `path` overrides `MLIS_LICENSE_PATH`/the default. Non-zero exit
/// on any failure, matching `doctor`'s convention — this command's whole job
/// is to report validity, so a meaningful exit code matters for scripting.
fn verify_license_command(path: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let path = path.map(String::from).unwrap_or_else(|| {
        env::var("MLIS_LICENSE_PATH").unwrap_or_else(|_| DEFAULT_LICENSE_PATH.into())
    });

    match mlis_license::load_and_check(Path::new(&path)) {
        Ok(status) => {
            let days_left = status.days_until_expiry(mlis_license::current_unix());
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

/// `mlis decrypt <file.json.enc>` — decrypt an AES-256-GCM payload (written when
/// `MLIS_KEY` is set) to stdout, using the same `MLIS_KEY`.
fn decrypt_command(file: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let Some(file) = file else {
        eprintln!("Usage: mlis decrypt <file.json.enc>   (reads key from MLIS_KEY)");
        return Ok(());
    };
    let key = match env::var("MLIS_KEY") {
        Ok(s) => mlis_core::crypt::key_from_base64(&s)?,
        Err(_) => {
            eprintln!("❌ set MLIS_KEY (base64-encoded 32-byte AES-256 key)");
            return Ok(());
        }
    };
    let data = std::fs::read(file)?;
    match mlis_core::crypt::decrypt(&key, &data) {
        Ok(plain) => std::io::stdout().write_all(&plain)?,
        Err(e) => eprintln!("❌ decrypt failed: {e}"),
    }
    Ok(())
}

/// `mlis doctor` — preflight checks: OCR/inferer reachability + config sanity.
/// OCR and inferer reachability are required for the pipeline to run at all
/// (a failure there is a non-zero exit); `MLIS_KEY`/`MLIS_AUDIT_LOG` checks are
/// advisory since those features are optional.
async fn doctor_command() -> Result<(), Box<dyn std::error::Error>> {
    let mut ok = true;

    let ocr_engine = env::var("MLIS_OCR_ENGINE").unwrap_or_else(|_| "rust".into());
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

    if let Ok(key) = env::var("MLIS_KEY") {
        match mlis_core::crypt::key_from_base64(&key) {
            Ok(_) => println!("✅ MLIS_KEY is a valid base64 32-byte key"),
            Err(e) => println!(
                "⚠️  MLIS_KEY is set but invalid ({e}) — encryption will be silently disabled"
            ),
        }
    }

    if let Ok(log_path) = env::var("MLIS_AUDIT_LOG") {
        let parent_ok = Path::new(&log_path)
            .parent()
            .map(|p| p.as_os_str().is_empty() || p.exists())
            .unwrap_or(true);
        if parent_ok {
            println!("✅ MLIS_AUDIT_LOG parent directory exists ({log_path})");
        } else {
            println!(
                "⚠️  MLIS_AUDIT_LOG parent directory does not exist ({log_path}) — audit records will silently fail to write"
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
/// under `MLIS_OCR_MODEL_DIR` (default `.`) and sha256-verified — unlike a
/// pure reachability check, this engine can fail at startup on missing or
/// corrupt weights.
#[cfg(all(feature = "ocr-native-rust", not(feature = "ocr-embedded")))]
type OcrModelVerifyFn = fn(&Path) -> Result<(), mlis_ocr::verify::VerifyError>;

#[cfg(all(feature = "ocr-native-rust", not(feature = "ocr-embedded")))]
fn check_rust_ocr_models(ok: &mut bool) {
    let model_dir = env::var("MLIS_OCR_MODEL_DIR").unwrap_or_else(|_| ".".into());
    let dir = Path::new(&model_dir);
    let skip = mlis_ocr::verify::skip_verify();
    let checks: [(&str, std::path::PathBuf, OcrModelVerifyFn); 2] = [
        (
            "detection",
            dir.join(mlis_ocr::download::DETECTION_FILENAME),
            mlis_ocr::verify::verify_detection_model,
        ),
        (
            "recognition",
            dir.join(mlis_ocr::download::RECOGNITION_FILENAME),
            mlis_ocr::verify::verify_recognition_model,
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
