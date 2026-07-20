# Offline Licensing — CLI Walkthrough

Extraction (`synthpass <file>` and `synthpass-serve`'s `/api/extract`) requires an offline,
Ed25519-signed license — set once and checked with no network call. `synthpass doctor`, `synthpass
decrypt`, `synthpass fingerprint`, and `synthpass verify-license` all keep working without one (you need
`fingerprint` to get one in the first place). For local development, skip enforcement
entirely:

```powershell
$env:SYNTHPASS_LICENSE_SKIP = "1"
```

For the cryptographic design — signed-bytes format, `verify_strict`, the fingerprint scheme,
and the threat model stated plainly — see
[ARCHITECTURE.md §6](ARCHITECTURE.md#6-offline-cryptographic-licensing-v080). This document is
just the CLI steps.

## Customer flow

```powershell
cargo run -p synthpass-cli -- fingerprint                    # send this string to your vendor
# ...vendor emails back license.mlis...
cargo run -p synthpass-cli -- verify-license license.mlis     # confirm it before relying on it
```

## Vendor flow

The `synthpass-license-issuer` binary (`vendor` feature, never shipped to customers):

```powershell
cargo run -p synthpass-license --features vendor --bin synthpass-license-issuer -- keygen
# keep the private key offline; embed the printed public key in crates/synthpass-license/pubkey.b64

$env:SYNTHPASS_LICENSE_PRIVKEY = "<private key from keygen>"
cargo run -p synthpass-license --features vendor --bin synthpass-license-issuer -- `
  issue-license --customer "Acme Hospital" --tier enterprise --expires-in-days 365 `
  --hw <fingerprint from the customer> --out license.mlis
```

An empty `--hw` issues an unbound (site/trial) license instead of a machine-locked one.

## Configuration (environment)

| Variable | Default | Purpose |
| --- | --- | --- |
| `SYNTHPASS_LICENSE_PATH` | `license.mlis` | path to the signed license file |
| `SYNTHPASS_LICENSE_SKIP` | *(unset)* | `1` bypasses license enforcement entirely (local development/CI) |
| `SYNTHPASS_LICENSE_PUBKEY` | *(embedded)* | override the embedded verifying key (base64), for testing |
