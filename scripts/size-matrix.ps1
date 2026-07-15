#!/usr/bin/env pwsh
#
# Size-matrix regression tool for the native backend's freestanding runtime
# profiles (see src/backend/link.rs's "Freestanding runtime profiles" docs
# and runtime/osc_runtime_freestanding_core.c).
#
# Measures C-backend vs native-backend executable size for a handful of
# representative programs — core (no feature-specific builtins), sockets,
# TLS, and graphics — and asserts native/C stays within a per-family ratio
# threshold, rather than pinning an exact byte count (executable size shifts
# by a few bytes with any compiler/runtime change; a brittle exact-byte
# assertion would need constant updating for unrelated reasons). "core" is
# the family this repo's native-size-* work measures/enforces most tightly
# (see compare-backend-size.ps1 and the native-size-profiles todo); sockets/
# TLS/graphics are report-and-guard: they establish a regression ceiling at
# today's measured ratio (rounded up for headroom) without requiring exact
# native/C parity, since their remaining gap is ordinary Cranelift-vs-Clang
# code-generation density (tracked separately by native-size-codegen), not
# unreachable dead weight.
#
# The ratio thresholds below assume the release-pinned llvm-mingw Clang/LLD
# toolchain (or a comparably size-tuned modern Clang+LLD) is what actually
# builds the runtime archives/objects — see packaging/toolchains/*.json and
# the native-size-toolchain todo. Without it (e.g. a plain GCC found on
# PATH, or an on-demand archive build falling back to whatever compiler
# scripts/release_tools.py's default_cc_for_target discovers), native
# executables are legitimately much larger and this script will correctly
# report a threshold failure — that reflects the active toolchain, not a
# regression in the runtime-profile logic this script exists to guard. Set
# OSCAN_RUNTIME_ARCHIVE_DIR to a directory containing archives built with
# the pinned toolchain to reproduce the release-quality numbers locally
# (see docs/releasing.md).
#
# Usage: scripts/size-matrix.ps1 [-Oscan <path>] [-SkipNetwork]

param(
    [string]$Oscan = "target\release\oscan.exe",
    [string]$OutputDirectory = "tests\build\size-matrix",
    [switch]$SkipNetwork
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot

function Resolve-FromRoot([string]$Path) {
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return $Path
    }
    return Join-Path $Root $Path
}

function Format-Size([long]$Bytes) {
    return "{0:N0} B" -f $Bytes
}

$oscanPath = Resolve-FromRoot $Oscan
$outputDir = Resolve-FromRoot $OutputDirectory

if (-not (Test-Path $oscanPath -PathType Leaf)) {
    Write-Host "Building release compiler..."
    Push-Location $Root
    try {
        cargo build --release --quiet
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
$extension = if ($IsWindows -or $env:OS -eq "Windows_NT") { ".exe" } else { "" }

# Name, source .osc, whether a network-dependent run-equivalence check
# should be attempted (skipped entirely under -SkipNetwork), and the
# max acceptable native/C size ratio.
$matrix = @(
    [PSCustomObject]@{ Name = "core";   Source = "examples\hello.osc";                Network = $false; MaxRatio = 1.10 },
    [PSCustomObject]@{ Name = "socket"; Source = "tests\positive\builtin_socket.osc";  Network = $false; MaxRatio = 1.20 },
    [PSCustomObject]@{ Name = "tls";    Source = "tests\positive\tls_fetch.osc";       Network = $true;  MaxRatio = 1.25 },
    [PSCustomObject]@{ Name = "gfx";    Source = "tests\positive\gfx_text_width.osc"; Network = $false; MaxRatio = 1.20 }
)

$results = @()
$failed = $false

foreach ($case in $matrix) {
    $sourcePath = Resolve-FromRoot $case.Source
    if (-not (Test-Path $sourcePath -PathType Leaf)) {
        Write-Host "SKIP $($case.Name): source not found: $sourcePath"
        continue
    }

    $cOutput = Join-Path $outputDir "$($case.Name)-c$extension"
    $nativeOutput = Join-Path $outputDir "$($case.Name)-native$extension"
    foreach ($output in @($cOutput, $nativeOutput)) {
        Remove-Item $output -Force -ErrorAction SilentlyContinue
    }

    Write-Host "Building '$($case.Name)' ($sourcePath)..."
    $cCompileOutput = & $oscanPath --backend c $sourcePath -o $cOutput 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $cOutput)) {
        Write-Host "FAIL $($case.Name): C backend compilation failed`n$cCompileOutput"
        $failed = $true
        continue
    }
    $nativeCompileOutput = & $oscanPath --backend native $sourcePath -o $nativeOutput 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $nativeOutput)) {
        Write-Host "FAIL $($case.Name): native backend compilation failed`n$nativeCompileOutput"
        $failed = $true
        continue
    }

    $cSize = (Get-Item $cOutput).Length
    $nativeSize = (Get-Item $nativeOutput).Length
    $ratio = if ($cSize -eq 0) { 0.0 } else { $nativeSize / $cSize }

    $runOk = $true
    if ($case.Network -and $SkipNetwork) {
        Write-Host "  (skipping run-equivalence check for '$($case.Name)': -SkipNetwork)"
    } else {
        $cRun = (& $cOutput 2>&1 | Out-String).TrimEnd()
        $cExit = $LASTEXITCODE
        $nativeRun = (& $nativeOutput 2>&1 | Out-String).TrimEnd()
        $nativeExit = $LASTEXITCODE
        if ($cExit -ne $nativeExit -or $cRun -ne $nativeRun) {
            if ($case.Network) {
                # Network-dependent cases (e.g. tls_fetch.osc's real HTTPS
                # requests) can legitimately fail/differ in a sandboxed or
                # offline CI runner; that's an environment fact, not a size
                # regression, so this only warns rather than failing the
                # size-matrix gate. Re-run with connectivity (or pass
                # -SkipNetwork) to get a real equivalence check.
                Write-Host "  WARN $($case.Name): outputs differ/network-dependent (not failing size check) — C: '$cRun' (exit $cExit) vs native: '$nativeRun' (exit $nativeExit)"
            } else {
                Write-Host "FAIL $($case.Name): outputs differ — C: '$cRun' (exit $cExit) vs native: '$nativeRun' (exit $nativeExit)"
                $failed = $true
                $runOk = $false
            }
        }
    }

    $withinThreshold = $ratio -le $case.MaxRatio
    if (-not $withinThreshold -and $runOk) {
        $failed = $true
    }

    $results += [PSCustomObject]@{
        Family      = $case.Name
        CBytes      = $cSize
        NativeBytes = $nativeSize
        Ratio       = $ratio
        MaxRatio    = $case.MaxRatio
        Pass        = ($withinThreshold -and $runOk)
    }
}

Write-Host ""
Write-Host "Native/C executable size matrix"
Write-Host ("{0,-8} {1,12} {2,12} {3,8} {4,8} {5,5}" -f "Family", "C bytes", "Native", "Ratio", "MaxOK", "Pass")
foreach ($r in $results) {
    Write-Host ("{0,-8} {1,12} {2,12} {3,8:N3} {4,8:N2} {5,5}" -f `
        $r.Family, (Format-Size $r.CBytes), (Format-Size $r.NativeBytes), $r.Ratio, $r.MaxRatio, $(if ($r.Pass) { "OK" } else { "FAIL" }))
}
Write-Host ""

if ($failed) {
    Write-Host "size-matrix: FAILED (native backend exceeded its native/C size ratio threshold, or a non-network case's output diverged, for at least one family)"
    exit 1
}

Write-Host "size-matrix: PASSED"
exit 0
