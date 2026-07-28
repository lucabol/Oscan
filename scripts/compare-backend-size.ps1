#!/usr/bin/env pwsh

param(
    [string]$Source = "examples\hello.osc",
    [string]$Oscan = "target\release\oscan.exe",
    [string]$OutputDirectory = "tests\build\backend-size"
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
    return "{0:N0} bytes ({1:N2} KiB)" -f $Bytes, ($Bytes / 1KB)
}

$sourcePath = Resolve-FromRoot $Source
$oscanPath = Resolve-FromRoot $Oscan
$outputDir = Resolve-FromRoot $OutputDirectory

if (-not (Test-Path $sourcePath -PathType Leaf)) {
    throw "Oscan source not found: $sourcePath"
}

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
$cOutput = Join-Path $outputDir "hello-c$extension"
$nativeOutput = Join-Path $outputDir "hello-native$extension"
$llvmOutput = Join-Path $outputDir "hello-llvm$extension"

foreach ($output in @($cOutput, $nativeOutput, $llvmOutput)) {
    Remove-Item $output -Force -ErrorAction SilentlyContinue
}

Write-Host "Compiling C backend..."
& $oscanPath --backend c $sourcePath -o $cOutput
if ($LASTEXITCODE -ne 0) {
    throw "C backend compilation failed with exit code $LASTEXITCODE"
}

Write-Host "Compiling native backend..."
& $oscanPath --backend native $sourcePath -o $nativeOutput
if ($LASTEXITCODE -ne 0) {
    throw "Native backend compilation failed with exit code $LASTEXITCODE"
}

Write-Host "Compiling LLVM backend..."
& $oscanPath --backend llvm $sourcePath -o $llvmOutput
if ($LASTEXITCODE -ne 0) {
    throw "LLVM backend compilation failed with exit code $LASTEXITCODE"
}

$cSize = (Get-Item $cOutput).Length
$nativeSize = (Get-Item $nativeOutput).Length
$llvmSize = (Get-Item $llvmOutput).Length
$difference = $nativeSize - $cSize
$percent = if ($cSize -eq 0) { 0.0 } else { 100.0 * $difference / $cSize }
$ratio = if ($cSize -eq 0) { 0.0 } else { $nativeSize / $cSize }
$llvmDifference = $llvmSize - $cSize
$llvmPercent = if ($cSize -eq 0) { 0.0 } else { 100.0 * $llvmDifference / $cSize }
$llvmRatio = if ($cSize -eq 0) { 0.0 } else { $llvmSize / $cSize }
$llvmNoLargerThanC = $llvmSize -le $cSize

$cRun = (& $cOutput 2>&1 | Out-String).TrimEnd()
$cExit = $LASTEXITCODE
$nativeRun = (& $nativeOutput 2>&1 | Out-String).TrimEnd()
$nativeExit = $LASTEXITCODE
$llvmRun = (& $llvmOutput 2>&1 | Out-String).TrimEnd()
$llvmExit = $LASTEXITCODE
if ($cExit -ne $nativeExit -or $cRun -ne $nativeRun -or
    $cExit -ne $llvmExit -or $cRun -ne $llvmRun) {
    throw "Backend outputs differ; refusing to compare non-equivalent executables"
}

Write-Host ""
Write-Host "Backend executable size comparison"
Write-Host "  Source:      $sourcePath"
Write-Host "  C backend:   $(Format-Size $cSize)"
Write-Host "  Cranelift:   $(Format-Size $nativeSize)"
Write-Host "  LLVM:        $(Format-Size $llvmSize)"
Write-Host ("  Cranelift-C: {0:+#,##0;-#,##0;0} bytes ({1:+0.00;-0.00;0.00}%), {2:N3}x" -f $difference, $percent, $ratio)
Write-Host ("  LLVM-C:      {0:+#,##0;-#,##0;0} bytes ({1:+0.00;-0.00;0.00}%), {2:N3}x" -f $llvmDifference, $llvmPercent, $llvmRatio)
Write-Host "  LLVM <= C:   $(if ($llvmNoLargerThanC) { "PASS" } else { "FAIL" })"

if (-not $llvmNoLargerThanC) {
    throw "LLVM executable size regression: $llvmSize bytes exceeds the equivalent C backend executable at $cSize bytes"
}

[PSCustomObject]@{
    Source = $sourcePath
    CBackendBytes = $cSize
    NativeBackendBytes = $nativeSize
    LlvmBackendBytes = $llvmSize
    DifferenceBytes = $difference
    DifferencePercent = $percent
    NativeToCRatio = $ratio
    LlvmDifferenceBytes = $llvmDifference
    LlvmDifferencePercent = $llvmPercent
    LlvmToCRatio = $llvmRatio
    LlvmNoLargerThanC = $llvmNoLargerThanC
}
