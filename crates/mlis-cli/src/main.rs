//! mlis (CLI) — command-line front-end for the multi-level-id-strip pipeline.
//!
//! Run from the repository root so the inferer sidecar and OCR engine resolve:
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
        eprintln!("  cargo run -p mlis-cli -- <path_to_image_or_pdf>   extract");
        eprintln!("  cargo run -p mlis-cli -- decrypt <file.json.enc>  decrypt (needs MLIS_KEY)");
        eprintln!("  cargo run -p mlis-cli -- doctor                   preflight: OCR/inferer reachability, config sanity");
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

    let input = Path::new(&args[1]);
    if !input.exists() {
        eprintln!("❌ Error: File not found at {}", input.display());
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

    let ocr_engine = env::var("MLIS_OCR_ENGINE").unwrap_or_else(|_| "docling".into());
    if ocr_engine == "native" {
        println!("✅ OCR engine: native (in-process, no network check needed)");
    } else {
        let docling_url =
            env::var("DOCLING_URL").unwrap_or_else(|_| "http://localhost:5001".into());
        let addr = host_port(&docling_url);
        let reachable = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect(addr.as_str()),
        )
        .await;
        match reachable {
            Ok(Ok(_)) => println!("✅ OCR (docling-serve) reachable at {docling_url}"),
            _ => {
                println!("❌ OCR (docling-serve) NOT reachable at {docling_url}");
                ok = false;
            }
        }
    }

    let inferer_addr =
        env::var("MLIS_INFERER_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".into());
    match mlis_pipeline::inferer::inferer_client::InfererClient::connect(inferer_addr.clone()).await
    {
        Ok(mut client) => {
            match client
                .health(mlis_pipeline::inferer::HealthRequest {})
                .await
            {
                Ok(resp) => {
                    let loaded = resp.into_inner().model_loaded;
                    println!("✅ inferer reachable at {inferer_addr} (model_loaded: {loaded})");
                }
                Err(e) => {
                    println!("❌ inferer at {inferer_addr} connected but Health RPC failed: {e}");
                    ok = false;
                }
            }
        }
        Err(e) => {
            println!("❌ inferer NOT reachable at {inferer_addr}: {e}");
            ok = false;
        }
    }

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

/// Parses `scheme://host[:port][/path]` down to a `host:port` pair suitable
/// for `TcpStream::connect`, defaulting the port from the scheme when absent.
fn host_port(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let host_and_maybe_path = without_scheme.split('/').next().unwrap_or(without_scheme);
    if host_and_maybe_path.contains(':') {
        host_and_maybe_path.to_string()
    } else {
        let default_port = if url.starts_with("https://") { 443 } else { 80 };
        format!("{host_and_maybe_path}:{default_port}")
    }
}
