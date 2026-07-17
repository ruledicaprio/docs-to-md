//! docling-client — CLI front-end for the air-gapped document pipeline.
//!
//! Run from the repository root so the Python sidecar finds
//! `extract_json.py`, the `.venv` and the GGUF model:
//!
//! ```powershell
//! cargo run -p docling-client -- samples/Croatian_passport_data_page.jpg
//! ```

use pipeline::Pipeline;
use std::env;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: cargo run -p docling-client -- <path_to_image_or_pdf>");
        return Ok(());
    }

    let input = Path::new(&args[1]);
    if !input.exists() {
        eprintln!("❌ Error: File not found at {}", input.display());
        return Ok(());
    }

    let pipeline = Pipeline::from_env();
    println!(
        "🔄 [Rust] Uploading and processing local file: {} (docling-serve: {})...",
        input.display(),
        pipeline.docling_url()
    );

    match pipeline.process_document(input).await {
        Ok(result) => {
            println!("✅ [Rust] Docling OCR successful!");
            println!("💾 [Rust] Saved Markdown to: {}", result.md_path.display());
            print!("{}", result.sidecar_stdout);
            match result.llm_error {
                None => println!(
                    "🎉 [Rust] Pipeline completed! JSON saved to: {}",
                    result.json_path.display()
                ),
                Some(e) => eprintln!("⚠️ [Rust] LLM extraction failed: {e}"),
            }
        }
        Err(e) => eprintln!("❌ [Rust] {e}"),
    }

    Ok(())
}
