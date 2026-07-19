//! Machine fingerprint for optionally binding a license to one installation.
//!
//! **Threat model, stated plainly (not oversold):** this binds to an OS
//! *installation*, not physically to hardware. A root user can read/copy the
//! underlying identifier, it survives a full disk clone, and it says nothing
//! about the CPU/motherboard. This deters casual license-sharing and
//! produces a compliance artifact — it is not DRM and isn't tamper-proof.
//! True hardware attestation would need a TPM/HSM, out of scope here.
//!
//! Deliberately does NOT use `sysinfo`(OS name + hostname + CPU brand): the
//! hostname is trivially changed and the CPU brand string is identical
//! across thousands of same-SKU machines, so two identical servers would
//! hash the same and one license would unlock both. `/etc/machine-id` is a
//! real, stable, per-install 128-bit identifier — far more unique.

use mlis_core::audit::sha256_hex;

/// A stable per-machine fingerprint, hex-encoded SHA-256 of the best
/// identifier this platform exposes. Binding to a license is optional (an
/// empty `hw_fingerprint` in the payload skips the check entirely), so this
/// never needs to be perfect — only good enough to deter casual sharing.
pub fn machine_fingerprint() -> String {
    sha256_hex(raw_identifier().as_bytes())
}

#[cfg(target_os = "linux")]
fn raw_identifier() -> String {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(id) = std::fs::read_to_string(path) {
            let id = id.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }
    // No OS-level machine-id — common on minimal/musl deployments (e.g. a
    // stock `alpine:3.20` image ships neither file, no systemd/dbus;
    // verified empirically as part of the v1.0.0 musl milestone spike).
    // Without this tier, every such install would silently collapse onto
    // the same fixed placeholder — exactly the "two machines, one
    // fingerprint" failure this module's docs already reject `sysinfo` for.
    persisted_instance_id(&instance_id_path())
}

/// Default location for the persisted-on-first-run fallback identifier used
/// when no OS-level machine-id exists. Override with `MLIS_INSTANCE_ID_PATH`
/// (used by tests, and by deployments where `/var/lib/mlis` isn't writable
/// or desired).
#[cfg(target_os = "linux")]
fn instance_id_path() -> std::path::PathBuf {
    std::env::var("MLIS_INSTANCE_ID_PATH")
        .unwrap_or_else(|_| "/var/lib/mlis/instance-id".to_string())
        .into()
}

/// A random id generated once and persisted to `path`, read back on every
/// subsequent call — stable across restarts as long as the path stays
/// writable/readable. If the filesystem is read-only, every call regenerates
/// a fresh id instead of a stable one: worse than a real machine-id, but
/// strictly better than every unwritable-fs install colliding on one shared
/// placeholder value.
///
/// Concurrent first-run callers (e.g. two processes starting at once) race
/// to create the file — `hard_link` only succeeds for the first one (it
/// fails with `AlreadyExists` rather than silently overwriting, unlike a
/// plain `write`), so every racer converges on the same winning value
/// instead of each returning a different one it can never see invalidated.
///
/// Takes `path` as a parameter (rather than reading `MLIS_INSTANCE_ID_PATH`
/// internally) so tests can exercise this without mutating a process-global
/// env var — a prior lesson from v0.8.0's test-isolation bug, where a
/// `#[test]`-mutated env var leaked across threads under `cargo test`'s
/// default parallelism.
#[cfg(target_os = "linux")]
fn persisted_instance_id(path: &std::path::Path) -> String {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let existing = existing.trim();
        if !existing.is_empty() {
            return existing.to_string();
        }
    }
    let fresh = generate_random_id();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp_path = path.with_extension(format!("part-{}", std::process::id()));
    if std::fs::write(&tmp_path, &fresh).is_ok() {
        let outcome = std::fs::hard_link(&tmp_path, path);
        let _ = std::fs::remove_file(&tmp_path);
        match outcome {
            Ok(()) => return fresh,
            Err(_) => {
                // Another process won the race and created `path` first —
                // defer to its value so every caller ends up agreeing on
                // one canonical id, not "whichever one I generated".
                if let Ok(winner) = std::fs::read_to_string(path) {
                    let winner = winner.trim();
                    if !winner.is_empty() {
                        return winner.to_string();
                    }
                }
            }
        }
    }
    fresh
}

/// 16 bytes from `/dev/urandom`, hashed into an opaque id string. Not used
/// for anything cryptographic — only needs to differ across installs, which
/// kernel-backed randomness trivially satisfies.
#[cfg(target_os = "linux")]
fn generate_random_id() -> String {
    use std::io::Read;
    let mut buf = [0u8; 16];
    match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)) {
        Ok(()) => sha256_hex(&buf),
        Err(_) => {
            // /dev/urandom is unavailable on essentially no real Linux
            // system; last-resort so this never panics.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            format!("urandom-unavailable-{nanos}-{}", std::process::id())
        }
    }
}

#[cfg(target_os = "windows")]
fn raw_identifier() -> String {
    // Best-effort, dev-only: the musl target this scheme is actually
    // designed for is Linux (see module docs); Windows/macOS builds only
    // need something stable enough for local testing.
    std::env::var("COMPUTERNAME")
        .map(|c| format!("windows-dev-{c}"))
        .unwrap_or_else(|_| "unbound-dev-windows".to_string())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn raw_identifier() -> String {
    "unbound-dev-unknown-platform".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stable_across_calls() {
        assert_eq!(machine_fingerprint(), machine_fingerprint());
    }

    #[test]
    fn is_a_sha256_hex_string() {
        let fp = machine_fingerprint();
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // Simulates the "no /etc/machine-id" case (stock Alpine, verified
    // empirically during the v1.0.0 musl spike) by pointing the fallback at
    // a scratch path instead of the real /var/lib/mlis — this test never
    // touches actual system state.
    #[cfg(target_os = "linux")]
    #[test]
    fn persisted_instance_id_is_stable_and_unique_per_path() {
        let path_a = std::env::temp_dir().join(format!(
            "mlis-fingerprint-test-a-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_b = path_a.with_file_name(format!(
            "{}-b",
            path_a.file_name().unwrap().to_str().unwrap()
        ));

        // No env var mutation needed — `persisted_instance_id` takes the
        // path directly, so this test can run safely alongside every other
        // test in this file under `cargo test`'s default parallelism.
        let first = persisted_instance_id(&path_a);
        let second = persisted_instance_id(&path_a);
        assert_eq!(first, second, "same path must yield a stable id");

        let third = persisted_instance_id(&path_b);
        assert_ne!(
            first, third,
            "a fresh path must generate a distinct id, not share one placeholder"
        );

        std::fs::remove_file(&path_a).ok();
        std::fs::remove_file(&path_b).ok();
    }
}
