<#
.SYNOPSIS
    Unjess 4.0 Workspace Build & Verification Pipeline
.DESCRIPTION
    Builds the complete Unjess 4.0 Rust workspace from source:
    1. Runs `cargo test --workspace` to verify all unit tests in `unjess-core` and workspace crates
    2. Compiles all 5 release binaries via `cargo build --release --workspace`:
       - Unjess.exe (Desktop App - eframe/egui GPU + embedded in-process Tokio/Axum engine)
       - unjessd.exe (Standalone Headless Axum/Tokio Daemon on :9758)
       - unjess-cli.exe (Terminal CLI & Interactive REPL)
       - Unjess-Setup.exe (Native Windows Installer)
       - Unjess-Uninstall.exe (Native Windows Uninstaller)
    3. Stages binaries into `dist/` and `bin/`
    4. Generates `version.json` with SHA-256 checksums
#>

param(
    [Parameter(Mandatory = $false)]
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"

Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "  UNJESS 4.0 -- NATIVE RUST WORKSPACE BUILD PIPELINE      " -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Cyan

$RepoRoot = $PSScriptRoot
$distDir = Join-Path $RepoRoot "dist"
$binDir = Join-Path $RepoRoot "bin"
$targetRelease = Join-Path $RepoRoot "target\release"

if (-not (Test-Path $distDir)) {
    New-Item -ItemType Directory -Path $distDir -Force | Out-Null
}
if (-not (Test-Path $binDir)) {
    New-Item -ItemType Directory -Path $binDir -Force | Out-Null
}

Push-Location $RepoRoot
try {
    if (-not $SkipTests) {
        Write-Host "`n[1/4] Running Rust workspace unit tests (cargo test --workspace)..." -ForegroundColor Yellow
        & cargo test --workspace
        if ($LASTEXITCODE -ne 0) {
            throw "cargo test --workspace failed with exit code $LASTEXITCODE"
        }
        Write-Host "  -> All Rust unit tests passed!" -ForegroundColor Green
    } else {
        Write-Host "`n[1/4] Skipping unit tests (-SkipTests specified)" -ForegroundColor Gray
    }

    Write-Host "`n[2/4] Building release binaries (cargo build --release --workspace)..." -ForegroundColor Yellow
    & cargo build --release --workspace
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release --workspace failed with exit code $LASTEXITCODE"
    }

    Write-Host "`n[3/4] Staging binaries into dist/ and bin/..." -ForegroundColor Yellow
    $binaries = @(
        "Unjess.exe",
        "unjessd.exe",
        "unjess-cli.exe",
        "Unjess-Setup.exe",
        "Unjess-Uninstall.exe"
    )

    foreach ($bin in $binaries) {
        $src = Join-Path $targetRelease $bin
        if (-not (Test-Path $src)) {
            throw "Expected release binary not found: $src"
        }
        $dst = Join-Path $distDir $bin
        Copy-Item $src $dst -Force
        $sizeMB = [math]::Round((Get-Item $dst).Length / 1MB, 2)
        Write-Host "  -> Staged dist\$bin ($sizeMB MB)" -ForegroundColor Green
    }

    Copy-Item (Join-Path $targetRelease "unjess-cli.exe") (Join-Path $binDir "unjess.exe") -Force

    Write-Host "`n[4/4] Computing SHA-256 hashes & writing version.json..." -ForegroundColor Yellow
    $unjessHash = (Get-FileHash -Path (Join-Path $distDir "Unjess.exe") -Algorithm SHA256).Hash.ToLower()
    $daemonHash = (Get-FileHash -Path (Join-Path $distDir "unjessd.exe") -Algorithm SHA256).Hash.ToLower()
    $cliHash = (Get-FileHash -Path (Join-Path $distDir "unjess-cli.exe") -Algorithm SHA256).Hash.ToLower()
    $setupHash = (Get-FileHash -Path (Join-Path $distDir "Unjess-Setup.exe") -Algorithm SHA256).Hash.ToLower()

    $manifest = [ordered]@{
        version      = "4.0.1"
        edition      = "native-rust"
        architecture = "windows-x86_64"
        port         = 9758
        sha256       = $unjessHash
        binaries     = [ordered]@{
            "Unjess.exe"       = $unjessHash
            "unjessd.exe"      = $daemonHash
            "unjess-cli.exe"   = $cliHash
            "Unjess-Setup.exe" = $setupHash
        }
        built_at     = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    }

    $jsonOut = $manifest | ConvertTo-Json -Depth 5
    [System.IO.File]::WriteAllText((Join-Path $RepoRoot "version.json"), $jsonOut, (New-Object System.Text.UTF8Encoding($false)))
    [System.IO.File]::WriteAllText((Join-Path $distDir "version.json"), $jsonOut, (New-Object System.Text.UTF8Encoding($false)))
    Write-Host "  -> Unjess.exe SHA-256: $unjessHash" -ForegroundColor Cyan
} finally {
    Pop-Location
}

Write-Host "`n==========================================================" -ForegroundColor Green
Write-Host "  UNJESS 4.0 NATIVE RUST BUILD COMPLETE!                  " -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Cyan
