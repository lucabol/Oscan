#!/usr/bin/env pwsh
# update-deps.ps1 — Pull latest deps/laststanding, rebuild, and test.
# Idempotent: safe to run anytime. Exits non-zero on failure.

param()

$ErrorActionPreference = "Stop"
$root = $PSScriptRoot

Write-Host "`n=== Updating deps/laststanding ===" -ForegroundColor Cyan

Push-Location (Join-Path $root "deps" "laststanding")
try {
    $before = git rev-parse HEAD
    git pull origin main
    $after = git rev-parse HEAD

    if ($before -eq $after) {
        Write-Host "  Already up to date ($($before.Substring(0,7)))" -ForegroundColor Yellow
    } else {
        Write-Host "  Updated: $($before.Substring(0,7)) -> $($after.Substring(0,7))" -ForegroundColor Green
    }
} finally {
    Pop-Location
}

Write-Host "`n=== Building Oscan (cargo build --release) ===" -ForegroundColor Cyan

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  BUILD FAILED" -ForegroundColor Red
        exit 1
    }
    Write-Host "  Build succeeded" -ForegroundColor Green
} finally {
    Pop-Location
}

Write-Host "`n=== Running test suite ===" -ForegroundColor Cyan

Push-Location $root
try {
    & (Join-Path $root "test.ps1")
    if ($LASTEXITCODE -ne 0) {
        Write-Host "`n  TESTS FAILED (exit code $LASTEXITCODE)" -ForegroundColor Red
        exit 1
    }
    Write-Host "`n  All tests passed" -ForegroundColor Green
} finally {
    Pop-Location
}

Write-Host "`n=== update-deps: SUCCESS ===" -ForegroundColor Green
exit 0
