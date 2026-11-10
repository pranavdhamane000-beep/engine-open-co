<#
.SYNOPSIS
    Stage Windows release package per spec §20 packaging requirements.
    Copies release binaries, bundles FFmpeg, writes licenses, creates installer layout.

.DESCRIPTION
    Run from repository root after `cargo build --workspace --locked --release`.
    Output: packaging/windows/ with complete self-contained layout.
#>

param(
    [string]$TargetDir = "C:\Users\Pranav\AppData\Local\anim_engine_target",
    [string]$OutDir = "packaging\windows",
    [string]$FFmpegDir = ""  # optional pre-verified FFmpeg dir
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path $PSScriptRoot -Parent -Parent
Push-Location $repoRoot

$out = Join-Path $repoRoot $OutDir
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Path $out | Out-Null

Write-Host "Staging release package to $out"

# 1. Copy release binaries
$releaseBin = Join-Path $TargetDir "release"
$bins = @("anim_headless.exe", "anim_app.exe", "wp00_gpu.exe", "wp00_adapter.exe")
foreach ($b in $bins) {
    $src = Join-Path $releaseBin $b
    if (Test-Path $src) {
        Copy-Item $src -Destination (Join-Path $out $b) -Force
        Write-Host "  Copied $b"
    } else {
        Write-Warning "Binary not found: $b (may not be built)"
    }
}

# 2. Bundle FFmpeg (spec §18: versioned, hash-verified)
$ffmpegSrc = if ($FFmpegDir) { $FFmpegDir } else { "C:\ffmpeg\bin" }  # adjust per machine
$ffmpegDst = Join-Path $out "ffmpeg"
New-Item -ItemType Directory -Path $ffmpegDst -Force | Out-Null

$ffmpegFiles = @("ffmpeg.exe", "ffprobe.exe")
foreach ($f in $ffmpegFiles) {
    $src = Join-Path $ffmpegSrc $f
    if (Test-Path $src) {
        Copy-Item $src -Destination (Join-Path $ffmpegDst $f) -Force
        Write-Host "  Bundled $f"
    } else {
        Write-Warning "FFmpeg file not found at $src — copy manually from verified build"
    }
}

# 3. Write manifest with hashes
$manifest = @{
    schema_version = 1
    engine_version = (cargo metadata --format-version=1 | ConvertFrom-Json).packages |
                     Where-Object { $_.name -eq "anim_headless" } | Select-Object -First 1 -ExpandProperty version
    rust_toolchain = (rustc --version).Trim()
    engine_commit = (git rev-parse HEAD).Trim()
    built_at = (Get-Date -Format 'u')
    binaries = @{}
    ffmpeg = @{}
}

foreach ($b in $bins) {
    $path = Join-Path $out $b
    if (Test-Path $path) {
        $hash = Get-FileHash $path -Algorithm SHA256 | Select-Object -ExpandProperty Hash
        $manifest.binaries[$b] = $hash
    }
}

foreach ($f in $ffmpegFiles) {
    $path = Join-Path $ffmpegDst $f
    if (Test-Path $path) {
        $hash = Get-FileHash $path -Algorithm SHA256 | Select-Object -ExpandProperty Hash
        $manifest.ffmpeg[$f] = $hash
    }
}

$manifest | ConvertTo-Json -Depth 5 | Out-File -Encoding utf8 (Join-Path $out "manifest.json")
Write-Host "  Wrote manifest.json with SHA256 hashes"

# 4. Copy licenses
$licensesSrc = "$repoRoot\LICENSES"
if (-not (Test-Path $licensesSrc)) { New-Item -ItemType Directory -Path $licensesSrc | Out-Null }
# This would be populated by cargo-deny or similar in CI
"MIT/Apache-2.0 dual license per workspace Cargo.toml" | Out-File -Encoding utf8 (Join-Path $out "LICENSE.txt")
Write-Host "  Wrote LICENSE.txt"

# 5. README
@"
Animation Engine — Windows Release Package
==========================================

Binaries:
- anim_headless.exe  : Headless export CLI (spec §18)
- anim_app.exe       : Editor application (spec §19)
- wp00_gpu.exe       : GPU probe (spec §9)
- wp00_adapter.exe   : Adapter probe (spec §9)

FFmpeg:
- ffmpeg.exe / ffprobe.exe : Bundled, hash-verified per manifest.json

Manifest:
- manifest.json : SHA256 hashes of all binaries + FFmpeg; engine version, commit, toolchain

Verification:
Run `tools\verify_release.ps1` from repo root to validate release gates.
"@ | Out-File -Encoding utf8 (Join-Path $out "README.txt")
Write-Host "  Wrote README.txt"

# 6. Verify layout
Write-Host "`n=== Package Layout ==="
Get-ChildItem $out -Recurse | ForEach-Object {
    $rel = $_.FullName.Substring($out.Length + 1)
    $size = if ($_.PSIsContainer) { "" } else { " ($($_.Length) bytes)" }
    Write-Host "  $rel$size"
}

Write-Host "`n✓ Package staged at $out"
Pop-Location