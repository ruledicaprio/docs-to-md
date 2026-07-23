//! `synthpass-bench` — dev/CI corpus runner for M4 (Regression &
//! Benchmarking). Not a subcommand of the shipped `synthpass` CLI: this is a
//! vendor-side measurement tool, same separation `synthpass-license-issuer`
//! already demonstrates for a non-user-facing binary living in its own
//! crate.
//!
//! Generates a fixed, deterministic corpus from seeds `seed..seed+count`,
//! runs each through [`synthpass_bench::check_document`], and writes a
//! generated JSON report — the report is never hand-edited, only produced by
//! this binary, so it stays an honest reflection of the last real run.
//!
//! ```text
//! synthpass-bench [--count N] [--seed N] [--profile NAME] [--out PATH] [--min-hit-rate F]
//!   --count N          number of documents to check (default: 100)
//!   --seed N           base seed; document i uses seed N+i (default: 0)
//!   --profile NAME     clean|mobile|scanner|worn|border-kiosk|all (default: clean)
//!                      "all" round-robins the five profiles across the corpus
//!   --out PATH         report JSON path (default: bench-report.json)
//!   --min-hit-rate F   exit non-zero if the measured hit rate is below F
//!                      (e.g. 0.35); unset means "measure and report only"
//! ```

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use synthpass_bench::{check_document, MissReason};
use synthpass_gen::degrade::{apply_profile, CaptureProfile};
use synthpass_gen::{generate_from_seed, GeneratorConfig};
use synthpass_ocr::NativeOcr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileChoice {
    Clean,
    Mobile,
    Scanner,
    Worn,
    BorderKiosk,
    Damaged,
    All,
}

// Deliberately excludes `Damaged`: the other profiles degrade legibility,
// occlusion destroys data outright, and mixing it into `all` would move the
// hit-rate number every existing gate is calibrated against. Ask for it by
// name (`--profile damaged`) to measure recovery on its own.
const ROUND_ROBIN: [ProfileChoice; 5] = [
    ProfileChoice::Clean,
    ProfileChoice::Mobile,
    ProfileChoice::Scanner,
    ProfileChoice::Worn,
    ProfileChoice::BorderKiosk,
];

impl ProfileChoice {
    fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "clean" => Ok(Self::Clean),
            "mobile" => Ok(Self::Mobile),
            "scanner" => Ok(Self::Scanner),
            "worn" => Ok(Self::Worn),
            "border-kiosk" => Ok(Self::BorderKiosk),
            "damaged" => Ok(Self::Damaged),
            "all" => Ok(Self::All),
            other => Err(format!(
                "unknown profile '{other}' (valid: clean, mobile, scanner, worn, border-kiosk, damaged, all)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::Mobile => "mobile",
            Self::Scanner => "scanner",
            Self::Worn => "worn",
            Self::BorderKiosk => "border-kiosk",
            Self::Damaged => "damaged",
            Self::All => "all",
        }
    }

    /// `None` for `Clean` (no degradation applied); `All` resolves to a
    /// concrete per-seed choice before this is ever called.
    fn capture_profile(self) -> Option<CaptureProfile> {
        match self {
            Self::Clean | Self::All => None,
            Self::Mobile => Some(CaptureProfile::Mobile),
            Self::Scanner => Some(CaptureProfile::Scanner),
            Self::Worn => Some(CaptureProfile::Worn),
            Self::BorderKiosk => Some(CaptureProfile::BorderKiosk),
            Self::Damaged => Some(CaptureProfile::Damaged),
        }
    }
}

struct Args {
    count: u64,
    seed: u64,
    profile: ProfileChoice,
    out: String,
    min_hit_rate: Option<f64>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            count: 100,
            seed: 0,
            profile: ProfileChoice::Clean,
            out: "bench-report.json".to_string(),
            min_hit_rate: None,
        }
    }
}

fn usage() {
    eprintln!(
        "Usage: synthpass-bench [--count N] [--seed N] [--profile NAME] [--out PATH] [--min-hit-rate F]"
    );
    eprintln!("  --count N          number of documents to check (default: 100)");
    eprintln!("  --seed N           base seed; document i uses seed N+i (default: 0)");
    eprintln!("  --profile NAME     clean|mobile|scanner|worn|border-kiosk|all (default: clean)");
    eprintln!("  --out PATH         report JSON path (default: bench-report.json)");
    eprintln!("  --min-hit-rate F   exit non-zero if the measured hit rate is below F");
}

/// Hand-rolled flag parser, consistent with `synthpass-cli`'s style (no
/// clap, no new arg-parsing dependency).
fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
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
                parsed.profile = ProfileChoice::parse(v)?;
                i += 2;
            }
            "--out" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--out requires a value".to_string())?;
                parsed.out = v.clone();
                i += 2;
            }
            "--min-hit-rate" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| "--min-hit-rate requires a value".to_string())?;
                parsed.min_hit_rate = Some(
                    v.parse::<f64>()
                        .map_err(|_| format!("--min-hit-rate: not a valid number: {v}"))?,
                );
                i += 2;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(parsed)
}

#[derive(Serialize)]
struct SeedResult {
    seed: u64,
    profile: &'static str,
    hit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    /// Coarse miss class, separate from `reason`'s human-readable text so
    /// misses can be counted by kind without parsing prose.
    #[serde(skip_serializing_if = "Option::is_none")]
    miss_kind: Option<&'static str>,
    elapsed_ms: u128,
    /// Per-field character error rates, keyed by field name. Reported for
    /// every document that produced a parseable MRZ *and* for those that did
    /// not (as a total loss), so a mean over this is not biased by dropping
    /// the worst documents.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    fields: Vec<FieldReport>,
    /// `true` iff `synthpass_core::fusion::check_line1_integrity` flagged
    /// this read — reported independently of `hit`, since the checksum a
    /// hit proves never covered `document_type`/`issuing_country`/`surname`/
    /// `given_names` in the first place (see `docs/ROADMAP.md`'s per-field
    /// CER note). `false` when no MRZ was read at all.
    line1_flagged: bool,
}

#[derive(Serialize)]
struct FieldReport {
    field: &'static str,
    cer: f64,
    /// Only carried on an imperfect read — a report full of identical
    /// expected/got pairs is noise, and these are synthetic values so there
    /// is no PII concern in recording the ones that differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    got: Option<String>,
}

/// Stable, machine-readable name for a miss class.
fn miss_kind(reason: &MissReason) -> &'static str {
    match reason {
        MissReason::OcrError(_) => "ocr_error",
        MissReason::NoMrzFound(_) => "no_mrz_found",
        MissReason::ChecksumFailed => "checksum_failed",
        MissReason::DocumentNumberMismatch { .. } => "document_number_mismatch",
    }
}

#[derive(Serialize)]
struct Report {
    timestamp_unix: u64,
    profile: &'static str,
    count: u64,
    seed_start: u64,
    hits: u64,
    hit_rate: f64,
    results: Vec<SeedResult>,
}

fn resolve_profile(choice: ProfileChoice, seed_index: u64) -> ProfileChoice {
    if choice == ProfileChoice::All {
        ROUND_ROBIN[(seed_index as usize) % ROUND_ROBIN.len()]
    } else {
        choice
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("❌ {e}");
            usage();
            std::process::exit(1);
        }
    };

    let root = repo_root();
    let ocr = NativeOcr::load(
        &root.join("text-detection.rten"),
        &root.join("text-recognition.rten"),
    )
    .expect("failed to load OCR models — run from the repo root");

    let seeds: Vec<u64> = (0..parsed.count).map(|i| parsed.seed + i).collect();

    // Deliberately sequential: `NativeOcr::recognize` budgets its MRZ-retry
    // passes against wall-clock time (see synthpass-ocr's `max_duration`).
    // Running checks concurrently oversubscribes the CPU against rten's own
    // internal inference threads, inflating each pass's wall-clock time and
    // causing the retry budget to cut passes short — this was observed to
    // drop the measured hit rate by ~20 points versus running one at a time,
    // which is a resource-contention artifact, not a real accuracy signal.
    let results: Vec<SeedResult> = seeds
        .iter()
        .map(|&seed| {
            let resolved = resolve_profile(parsed.profile, seed - parsed.seed);
            let config = GeneratorConfig::new(seed);
            let (image, labels, _passport) = generate_from_seed(&config);
            let image = match resolved.capture_profile() {
                Some(cp) => apply_profile(&image, cp, seed),
                None => image,
            };
            let result = check_document(&ocr, &image, &labels);
            let line1_flagged = matches!(
                result.line1_integrity,
                Some(synthpass_core::fusion::Verdict::NeedsReview { .. })
            );
            SeedResult {
                seed,
                profile: resolved.as_str(),
                hit: result.hit,
                miss_kind: result.reason.as_ref().map(miss_kind),
                reason: result.reason.map(|r| r.to_string()),
                elapsed_ms: result.elapsed.as_millis(),
                line1_flagged,
                fields: result
                    .fields
                    .into_iter()
                    .map(|f| {
                        let imperfect = f.cer > 0.0;
                        FieldReport {
                            field: f.field,
                            cer: f.cer,
                            expected: imperfect.then_some(f.expected),
                            got: if imperfect { f.got } else { None },
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    let hits = results.iter().filter(|r| r.hit).count() as u64;
    let hit_rate = hits as f64 / parsed.count.max(1) as f64;

    for r in &results {
        if r.hit {
            println!("seed {} [{}]: HIT ({} ms)", r.seed, r.profile, r.elapsed_ms);
        } else {
            println!(
                "seed {} [{}]: MISS ({} ms) - {}",
                r.seed,
                r.profile,
                r.elapsed_ms,
                r.reason.as_deref().unwrap_or("unknown")
            );
        }
    }
    println!(
        "\n{hits}/{} = {:.1}% (profile: {})",
        parsed.count,
        hit_rate * 100.0,
        parsed.profile.as_str()
    );

    // Miss classes. A hit rate alone cannot distinguish "OCR found no MRZ"
    // from "OCR read the MRZ and one character was wrong" — those are
    // different problems with different fixes.
    let mut kinds: BTreeMap<&'static str, usize> = BTreeMap::new();
    for r in results.iter().filter_map(|r| r.miss_kind) {
        *kinds.entry(r).or_default() += 1;
    }
    if !kinds.is_empty() {
        println!("\nmisses by kind:");
        for (kind, n) in &kinds {
            println!("  {n:>4}  {kind}");
        }
    }

    // The headline this measurement exists for: the checksum a `hit` proves
    // never covered document_type/issuing_country/surname/given_names, so a
    // passing Tier-1 gate and a structurally wrong line 1 can both be true of
    // the same document at once. This reports how often that actually
    // happens, rather than leaving it as the one hand-counted number in
    // docs/ROADMAP.md's per-field CER note.
    if hits > 0 {
        let flagged_hits = results.iter().filter(|r| r.hit && r.line1_flagged).count();
        println!(
            "\nof {hits} Tier-1 hits, {flagged_hits} ({:.1}%) still have a line-1 integrity finding",
            flagged_hits as f64 / hits as f64 * 100.0
        );
    }

    // Mean CER per field, over every document — including those that never
    // produced an MRZ, which count as a total loss. This is the number that
    // says *where* the accuracy goes, rather than only how much of it.
    let mut totals: BTreeMap<&'static str, (f64, usize)> = BTreeMap::new();
    for f in results.iter().flat_map(|r| &r.fields) {
        let entry = totals.entry(f.field).or_insert((0.0, 0));
        entry.0 += f.cer;
        entry.1 += 1;
    }
    if !totals.is_empty() {
        let mut rows: Vec<(&str, f64)> = totals
            .iter()
            .map(|(field, (sum, n))| (*field, sum / *n as f64))
            .collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        println!("\nmean character error rate by field (worst first):");
        for (field, mean) in &rows {
            println!("  {:>7.2}%  {field}", mean * 100.0);
        }
    }

    let report = Report {
        timestamp_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        profile: parsed.profile.as_str(),
        count: parsed.count,
        seed_start: parsed.seed,
        hits,
        hit_rate,
        results,
    };
    let json = serde_json::to_string_pretty(&report).expect("serialize report");
    std::fs::write(&parsed.out, json).expect("write report");
    println!("report written to {}", parsed.out);

    if let Some(min) = parsed.min_hit_rate {
        if hit_rate < min {
            eprintln!(
                "❌ hit rate {:.1}% is below the required minimum {:.1}%",
                hit_rate * 100.0,
                min * 100.0
            );
            std::process::exit(1);
        }
    }
}

fn repo_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/synthpass-bench is two levels below the repo root")
        .to_path_buf()
}
