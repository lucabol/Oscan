# Oscan Test Runner (PowerShell)
# Usage: .\run_tests.ps1 -Oscan <oscan-binary> [-CC <cc-compiler>]
# Example: .\run_tests.ps1 -Oscan ..\target\release\oscan.exe -CC gcc

param(
    [Parameter(Mandatory=$true)]
    [string]$Oscan,

    [string]$CC = "gcc"
)

$ErrorActionPreference = "Continue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $ScriptDir

$Pass = 0
$Fail = 0
$Total = 0

# Ensure build directory exists
if (-not (Test-Path "build")) {
    New-Item -ItemType Directory -Path "build" | Out-Null
}

Write-Host "=== Oscan Test Suite ==="
Write-Host "Compiler: $Oscan"
Write-Host "C Compiler: $CC"
Write-Host ""

# --- Positive Tests ---
Write-Host "--- Positive Tests (must compile and produce correct output) ---"
Write-Host ""

foreach ($oscFile in Get-ChildItem "positive\*.osc") {
    $name = $oscFile.BaseName
    $expectedFile = "expected\$name.expected"
    $Total++
    Write-Host -NoNewline "  Testing $name... "

    # Check expected file exists
    if (-not (Test-Path $expectedFile)) {
        Write-Host "FAIL (missing expected file: $expectedFile)"
        $Fail++
        continue
    }

    # Step 1: Transpile .osc -> .c
    $transpileErr = $null
    & $Oscan $oscFile.FullName -o "build\$name.c" 2>"build\$name.err"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAIL (transpile error)"
        if (Test-Path "build\$name.err") {
            Get-Content "build\$name.err" | ForEach-Object { Write-Host "    $_" }
        }
        $Fail++
        continue
    }

    # Step 2: Compile .c -> binary
    & $CC "build\$name.c" "..\runtime\osc_runtime.c" "-I..\runtime" -o "build\$name.exe" -std=c99 -lm 2>"build\$name.err"
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAIL (C compile error)"
        if (Test-Path "build\$name.err") {
            Get-Content "build\$name.err" | ForEach-Object { Write-Host "    $_" }
        }
        $Fail++
        continue
    }

    # Step 3: Run and compare output
    $actual = & ".\build\$name.exe" 2>&1 | Out-String
    $actual = $actual.TrimEnd("`r`n").TrimEnd("`n")
    $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n")

    if ($actual -eq $expected) {
        Write-Host "PASS"
        $Pass++
    } else {
        Write-Host "FAIL (output mismatch)"
        Write-Host "    Expected:"
        Write-Host "      $expected"
        Write-Host "    Actual:"
        Write-Host "      $actual"
        $Fail++
    }
}

Write-Host ""

# --- Negative Tests ---
Write-Host "--- Negative Tests (must be rejected by the compiler) ---"
Write-Host ""

foreach ($oscFile in Get-ChildItem "negative\*.osc") {
    $name = $oscFile.BaseName
    $Total++
    Write-Host -NoNewline "  Testing reject $name... "

    & $Oscan $oscFile.FullName -o "build\$name.c" 2>"build\$name.err"
    if ($LASTEXITCODE -eq 0) {
        Write-Host "FAIL (should have been rejected)"
        $Fail++
    } else {
        Write-Host "PASS (correctly rejected)"
        $Pass++
    }
}

Write-Host ""
Write-Host "========================================="
Write-Host "Results: $Pass passed, $Fail failed out of $Total tests"
Write-Host "========================================="

Pop-Location

if ($Fail -gt 0) {
    exit 1
} else {
    exit 0
}
