//! `synthpass generate` — synthetic passport image + ground-truth label JSON
//! factory (M3). Wraps `synthpass_gen::generate_from_seed`.
//!
//! This command produces **no real PII** (every identity is fictional, drawn
//! deterministically from a seed — see `synthpass-gen`'s crate docs), so
//! unlike the default extraction path it is explicitly exempt from the
//! license gate: see the dispatch arm in `main.rs`, which returns before
//! `check_license()` is ever reached.

use serde::Serialize;
use std::path::Path;
use synthpass_gen::{generate_from_seed, GeneratorConfig, Labels};

/// Parsed `synthpass generate` arguments.
#[derive(Debug)]
struct GenerateArgs {
    count: u64,
    seed: u64,
    profile: String,
    out_dir: String,
}

impl Default for GenerateArgs {
    fn default() -> Self {
        Self {
            count: 1,
            seed: 0,
            profile: "clean".to_string(),
            out_dir: ".".to_string(),
        }
    }
}

const VALID_PROFILES: &[&str] = &["mobile", "scanner", "worn", "border-kiosk", "clean"];

fn usage() {
    eprintln!(
        "Usage: synthpass generate [--count N] [--seed N] [--profile NAME] [--out-dir DIR]"
    );
    eprintln!(
        "  --count N       number of documents to generate (default: 1)"
    );
    eprintln!(
        "  --seed N        base seed; document i uses seed N+i (default: 0)"
    );
    eprintln!(
        "  --profile NAME  clean|mobile|scanner|worn|border-kiosk (default: clean)"
    );
    eprintln!("  --out-dir DIR   output directory (default: .)");
}

/// Hand-rolled flag parser, consistent with the rest of this CLI's style
/// (no clap, no new arg-parsing dependency).
fn parse_args(args: &[String]) -> Result<GenerateArgs, String> {
    let mut parsed = GenerateArgs::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--count" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--count requires a value".to_string())?;
                parsed.count = v
                    .parse::<u64>()
                    .map_err(|_| format!("--count: not a valid number: {v}"))?;
                i += 2;
            }
            "--seed" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--seed requires a value".to_string())?;
                parsed.seed = v
                    .parse::<u64>()
                    .map_err(|_| format!("--seed: not a valid number: {v}"))?;
                i += 2;
            }
            "--profile" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--profile requires a value".to_string())?;
                let lower = v.to_lowercase();
                if !VALID_PROFILES.contains(&lower.as_str()) {
                    return Err(format!(
                        "--profile: unknown profile '{v}' (valid: {})",
                        VALID_PROFILES.join(", ")
                    ));
                }
                parsed.profile = lower;
                i += 2;
            }
            "--out-dir" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--out-dir requires a value".to_string())?;
                parsed.out_dir = v.clone();
                i += 2;
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
    }
    Ok(parsed)
}

/// Maps this CLI's `--profile` string to `synthpass_gen::degrade`'s
/// [`CaptureProfile`](synthpass_gen::degrade::CaptureProfile) and applies its
/// recipe; `clean` stays a no-op (the pristine render, no degradation).
fn degrade_placeholder(image: image::DynamicImage, profile: &str, seed: u64) -> image::DynamicImage {
    use synthpass_gen::degrade::{apply_profile, CaptureProfile};
    let capture_profile = match profile {
        "mobile" => CaptureProfile::Mobile,
        "scanner" => CaptureProfile::Scanner,
        "worn" => CaptureProfile::Worn,
        "border-kiosk" => CaptureProfile::BorderKiosk,
        _ => return image, // "clean" (validated in parse_args)
    };
    apply_profile(&image, capture_profile, seed)
}

/// A JSON-serializable mirror of `synthpass_gen::labels::FieldLabel` — the
/// upstream type has no `Serialize` derive, so this local copy exists purely
/// for the sidecar JSON.
#[derive(Serialize)]
struct FieldLabelJson {
    value: String,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl From<&synthpass_gen::FieldLabel> for FieldLabelJson {
    fn from(f: &synthpass_gen::FieldLabel) -> Self {
        Self {
            value: f.value.clone(),
            x: f.rect.x,
            y: f.rect.y,
            width: f.rect.width,
            height: f.rect.height,
        }
    }
}

/// A JSON-serializable mirror of `synthpass_gen::Labels`, plus generation
/// metadata (seed/profile/image dimensions) that isn't part of the upstream
/// type at all.
#[derive(Serialize)]
struct LabelsJson {
    document_type: FieldLabelJson,
    issuing_country: FieldLabelJson,
    surname: FieldLabelJson,
    given_names: FieldLabelJson,
    document_number: FieldLabelJson,
    nationality: FieldLabelJson,
    date_of_birth: FieldLabelJson,
    sex: FieldLabelJson,
    date_of_expiry: FieldLabelJson,
    #[serde(skip_serializing_if = "Option::is_none")]
    personal_number: Option<FieldLabelJson>,
    mrz_line1: String,
    mrz_line2: String,
    mrz_rect: FieldLabelRect,
    seed: u64,
    profile: String,
    image_width: u32,
    image_height: u32,
}

#[derive(Serialize)]
struct FieldLabelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn labels_to_json(labels: &Labels, seed: u64, profile: &str, width: u32, height: u32) -> LabelsJson {
    LabelsJson {
        document_type: (&labels.document_type).into(),
        issuing_country: (&labels.issuing_country).into(),
        surname: (&labels.surname).into(),
        given_names: (&labels.given_names).into(),
        document_number: (&labels.document_number).into(),
        nationality: (&labels.nationality).into(),
        date_of_birth: (&labels.date_of_birth).into(),
        sex: (&labels.sex).into(),
        date_of_expiry: (&labels.date_of_expiry).into(),
        personal_number: labels.personal_number.as_ref().map(Into::into),
        mrz_line1: labels.mrz_line1.clone(),
        mrz_line2: labels.mrz_line2.clone(),
        mrz_rect: FieldLabelRect {
            x: labels.mrz_rect.x,
            y: labels.mrz_rect.y,
            width: labels.mrz_rect.width,
            height: labels.mrz_rect.height,
        },
        seed,
        profile: profile.to_string(),
        image_width: width,
        image_height: height,
    }
}

/// `synthpass generate [--count N] [--seed N] [--profile NAME] [--out-dir DIR]` —
/// generates `count` synthetic passport images (PNG) + ground-truth label
/// JSON sidecars into `out_dir`. Document `i` in the batch uses seed
/// `seed + i`, so a batch is fully reproducible and each document differs.
///
/// No license required: see the module doc comment.
pub fn generate_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = match parse_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("❌ {e}");
            usage();
            return Ok(());
        }
    };

    std::fs::create_dir_all(&parsed.out_dir)?;

    for i in 0..parsed.count {
        let seed = parsed.seed + i;
        let config = GeneratorConfig::new(seed);
        let (image, labels, passport) = generate_from_seed(&config);
        let image = degrade_placeholder(image, &parsed.profile, seed);
        let (width, height) = (image.width(), image.height());

        let png_path = Path::new(&parsed.out_dir).join(format!("synthpass_{seed}.png"));
        let json_path = Path::new(&parsed.out_dir).join(format!("synthpass_{seed}.json"));

        image.save(&png_path)?;

        let labels_json = labels_to_json(&labels, seed, &parsed.profile, width, height);
        let json_str = serde_json::to_string_pretty(&labels_json)?;
        std::fs::write(&json_path, json_str)?;

        println!(
            "✅ [seed {seed}] {} {} — doc# {} (profile: {}) -> {} / {}",
            passport.given_names,
            passport.surname,
            passport.document_number,
            parsed.profile,
            png_path.display(),
            json_path.display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the full generate command against a temp directory and checks
    /// the expected PNG/JSON pairs exist and the sidecar's ground truth is
    /// sane (non-empty document number, two 44-char MRZ lines).
    #[test]
    fn generate_batch_produces_valid_outputs() {
        let out_dir = std::env::temp_dir().join(format!(
            "synthpass_generate_smoke_{}",
            std::process::id()
        ));
        let out_dir_str = out_dir.to_string_lossy().to_string();

        let args = vec![
            "--count".to_string(),
            "3".to_string(),
            "--seed".to_string(),
            "42".to_string(),
            "--profile".to_string(),
            "mobile".to_string(),
            "--out-dir".to_string(),
            out_dir_str.clone(),
        ];

        generate_command(&args).expect("generate_command should succeed");

        for i in 0..3u64 {
            let seed = 42 + i;
            let png_path = out_dir.join(format!("synthpass_{seed}.png"));
            let json_path = out_dir.join(format!("synthpass_{seed}.json"));
            assert!(png_path.exists(), "missing PNG for seed {seed}");
            assert!(json_path.exists(), "missing JSON sidecar for seed {seed}");

            let json_str = std::fs::read_to_string(&json_path).expect("read sidecar");
            let value: serde_json::Value =
                serde_json::from_str(&json_str).expect("sidecar should be valid JSON");

            let doc_number = value["document_number"]["value"]
                .as_str()
                .expect("document_number.value should be a string");
            assert!(!doc_number.is_empty(), "document_number should not be empty");

            let mrz1 = value["mrz_line1"].as_str().expect("mrz_line1");
            let mrz2 = value["mrz_line2"].as_str().expect("mrz_line2");
            assert_eq!(mrz1.len(), 44, "mrz_line1 should be 44 chars");
            assert_eq!(mrz2.len(), 44, "mrz_line2 should be 44 chars");

            assert_eq!(value["seed"].as_u64(), Some(seed));
            assert_eq!(value["profile"].as_str(), Some("mobile"));
        }

        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[test]
    fn rejects_unknown_profile() {
        let args = vec!["--profile".to_string(), "bogus".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("unknown profile"));
    }

    #[test]
    fn rejects_unknown_flag() {
        let args = vec!["--nope".to_string()];
        let err = parse_args(&args).unwrap_err();
        assert!(err.contains("unknown argument"));
    }
}
