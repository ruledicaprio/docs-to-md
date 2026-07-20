//! Vendor-only tool: generates keypairs and issues signed license files.
//! Requires the `vendor` feature — never shipped to customers (see
//! `crates/synthpass-license/src/sign.rs`'s module docs for why this is a
//! separate binary rather than a subcommand of the shipped `synthpass` CLI).
//!
//! ```text
//! synthpass-license-issuer keygen
//! synthpass-license-issuer issue-license --customer "Acme Hospital" --tier enterprise \
//!     --expires-in-days 365 [--hw <fingerprint>] [--features extract,decrypt] \
//!     [--license-id lic-001] [--out license.mlis]
//! ```

use std::collections::HashMap;
use std::env;
use synthpass_license::sign;
use synthpass_license::LicensePayload;

fn main() {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("keygen") => keygen_command(),
        Some("issue-license") => issue_license_command(&args[2..]),
        _ => {
            eprintln!("Usage:");
            eprintln!("  synthpass-license-issuer keygen");
            eprintln!("  synthpass-license-issuer issue-license --customer <name> --tier <tier> --expires-in-days <n> [--hw <fingerprint>] [--features a,b,c] [--license-id <id>] [--out <path>]");
            std::process::exit(1);
        }
    }
}

fn keygen_command() {
    let (priv_b64, pub_b64) = sign::generate_keypair();
    println!("--- VENDOR KEYPAIR GENERATED ---");
    println!();
    println!(
        "Private key (KEEP OFFLINE, NEVER COMMIT — set as SYNTHPASS_LICENSE_PRIVKEY to issue licenses):"
    );
    println!("{priv_b64}");
    println!();
    println!("Public key (safe to publish — embed in crates/synthpass-license/pubkey.b64):");
    println!("{pub_b64}");
}

/// Parses `--flag value` pairs into a map; flags without a following value
/// are ignored (this tool only has string-valued flags).
fn parse_flags(args: &[String]) -> HashMap<String, String> {
    let mut flags = HashMap::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if let Some(key) = args[i].strip_prefix("--") {
            flags.insert(key.to_string(), args[i + 1].clone());
            i += 2;
        } else {
            i += 1;
        }
    }
    flags
}

fn issue_license_command(args: &[String]) {
    let flags = parse_flags(args);

    let Ok(privkey_b64) = env::var("SYNTHPASS_LICENSE_PRIVKEY") else {
        eprintln!(
            "❌ set SYNTHPASS_LICENSE_PRIVKEY (base64 private key from `keygen`) to issue licenses"
        );
        std::process::exit(1);
    };
    let signing_key = match sign::signing_key_from_base64(&privkey_b64) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("❌ SYNTHPASS_LICENSE_PRIVKEY: {e}");
            std::process::exit(1);
        }
    };

    let Some(customer) = flags.get("customer") else {
        eprintln!("❌ --customer is required");
        std::process::exit(1);
    };
    let Some(tier) = flags.get("tier") else {
        eprintln!("❌ --tier is required");
        std::process::exit(1);
    };
    let Some(expires_in_days) = flags
        .get("expires-in-days")
        .and_then(|s| s.parse::<u64>().ok())
    else {
        eprintln!("❌ --expires-in-days <n> is required");
        std::process::exit(1);
    };

    let now = current_unix();
    let license_id = flags
        .get("license-id")
        .cloned()
        .unwrap_or_else(|| format!("lic-{now}"));
    let hw_fingerprint = flags.get("hw").cloned().unwrap_or_default();
    let features = flags
        .get("features")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let out_path = flags
        .get("out")
        .cloned()
        .unwrap_or_else(|| "license.mlis".to_string());

    let payload = LicensePayload {
        license_id,
        customer: customer.clone(),
        hw_fingerprint,
        issued_unix: now,
        expires_unix: now + expires_in_days * 86_400,
        tier: tier.clone(),
        features,
        mlis_min_version: flags.get("min-version").cloned(),
    };

    let signed = sign::issue(&signing_key, &payload);
    let json = serde_json::to_string_pretty(&signed).expect("SignedLicense always serializes");
    if let Err(e) = std::fs::write(&out_path, &json) {
        eprintln!("❌ could not write {out_path}: {e}");
        std::process::exit(1);
    }

    println!("✅ License issued: {out_path}");
    println!("   id: {}", payload.license_id);
    println!("   customer: {}", payload.customer);
    println!("   tier: {}", payload.tier);
    println!(
        "   bound to: {}",
        if payload.hw_fingerprint.is_empty() {
            "(unbound — any machine)".to_string()
        } else {
            payload.hw_fingerprint
        }
    );
    println!("   expires in: {expires_in_days} days");
}

fn current_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
