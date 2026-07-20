<#
.SYNOPSIS
Watches samples/ for new specimen files and instantly runs the MRZ/specimen
checker (crates/synthpass-ocr/examples/check_sample.rs) against each one via the
synthpass-builder Docker image, then opens the image so you can eyeball it.

This assists the human vetting process in CONTRIBUTING.md's "Adding a
corpus specimen" checklist -- it does NOT replace it. A checker HIT plus a
"specimen" text match is not proof the document is a genuine template
rather than a real person's document; always look at the image yourself
before committing it.

.EXAMPLE
./scripts/watch-samples.ps1
#>

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path "$PSScriptRoot/..").Path
$samplesDir = Join-Path $repoRoot "samples"
$extensions = @(".jpg", ".jpeg", ".png", ".webp")

function Wait-FileReady([string]$Path) {
    # Newly-dropped files can still be mid-copy; wait until the size settles.
    $lastSize = -1
    for ($i = 0; $i -lt 30; $i++) {
        if (-not (Test-Path $Path)) { return $false }
        $size = (Get-Item $Path).Length
        if ($size -eq $lastSize -and $size -gt 0) { return $true }
        $lastSize = $size
        Start-Sleep -Milliseconds 300
    }
    return $true
}

Write-Host "Watching $samplesDir for new specimen files... (Ctrl+C to stop)" -ForegroundColor Cyan

$known = New-Object 'System.Collections.Generic.HashSet[string]'
foreach ($f in Get-ChildItem $samplesDir -File) { $known.Add($f.Name) | Out-Null }

while ($true) {
    Start-Sleep -Seconds 2
    foreach ($file in Get-ChildItem $samplesDir -File) {
        if ($known.Contains($file.Name)) { continue }
        $known.Add($file.Name) | Out-Null

        if ($extensions -notcontains $file.Extension.ToLowerInvariant()) { continue }
        if (-not (Wait-FileReady $file.FullName)) { continue }

        Write-Host "`n=== New sample: $($file.Name) ===" -ForegroundColor Yellow

        docker run --rm -v "${repoRoot}:/work" `
            -v synthpass_target:/work/target -v synthpass_cargo_registry:/usr/local/cargo/registry `
            -w /work synthpass-builder:latest `
            cargo run -p synthpass-ocr --release --example check_sample -- "samples/$($file.Name)"

        Start-Process $file.FullName
    }
}
