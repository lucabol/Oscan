# Oscan Test Runner (PowerShell) — quiet by default, -VerboseOutput for details
# Usage: .\run_tests.ps1 -Oscan <oscan-binary> [-VerboseOutput]

param(
    [Parameter(Mandatory=$true)]
    [string]$Oscan,

    [switch]$VerboseOutput
)

$ErrorActionPreference = "Continue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $ScriptDir

$Pass = 0; $Fail = 0; $NegPass = 0; $NegFail = 0
$Failures = [System.Collections.ArrayList]::new()
$LibcRegressionTests = @('builtin_typed_maps')

if (-not (Test-Path "build")) {
    New-Item -ItemType Directory -Path "build" | Out-Null
}

# --- Positive Tests ---
foreach ($oscFile in Get-ChildItem "positive\*.osc") {
    $name = $oscFile.BaseName
    $expectedFile = "expected\$name.expected"
    $compileArgs = @()

    if (-not (Test-Path $expectedFile)) {
        [void]$Failures.Add("$name — missing expected file")
        $Fail++; continue
    }

    if ($name -match '^ffi') {
        $compileArgs += '--libc'
    }
    & $Oscan @compileArgs $oscFile.FullName -o "build\$name.exe" 2>"build\$name.err"
    if ($LASTEXITCODE -ne 0) {
        [void]$Failures.Add("$name — compile error")
        $Fail++; continue
    }

    $actual = & ".\build\$name.exe" 2>&1 | Out-String
    $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
    $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

    if ($actual -ne $expected) {
        [void]$Failures.Add("$name — output mismatch")
        $Fail++
        continue
    }

    if (($LibcRegressionTests -contains $name) -and ($name -notmatch '^ffi')) {
        & $Oscan --libc $oscFile.FullName -o "build\$name.libc.exe" 2>"build\$name.libc.err"
        if ($LASTEXITCODE -ne 0) {
            [void]$Failures.Add("$name — libc compile error")
            $Fail++
            continue
        }

        $actualLibc = & ".\build\$name.libc.exe" 2>&1 | Out-String
        $actualLibc = $actualLibc.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actualLibc -ne $expected) {
            [void]$Failures.Add("$name — libc output mismatch")
            $Fail++
            continue
        }
    }

    if ($VerboseOutput) { Write-Host "  PASS: $name" -ForegroundColor Green }
    $Pass++
}

# --- Negative Tests ---
foreach ($oscFile in Get-ChildItem "negative\*.osc") {
    $name = $oscFile.BaseName
    & $Oscan $oscFile.FullName -o "build\$name.c" 2>"build\$name.err"
    if ($LASTEXITCODE -eq 0) {
        [void]$Failures.Add("$name — should have been rejected")
        $NegFail++
    } else {
        if ($VerboseOutput) { Write-Host "  PASS: $name — correctly rejected" -ForegroundColor Green }
        $NegPass++
    }
}

# --- Freestanding Verification ---
$VPass = 0; $VFail = 0
$dumpbin = Get-Command dumpbin -ErrorAction SilentlyContinue
foreach ($exe in Get-ChildItem "build\*.exe" -ErrorAction SilentlyContinue) {
    if ($exe.BaseName -match '^ffi' -or $exe.BaseName -like '*.libc') { continue }
    $stdlibPatterns = @('msvcrt', 'ucrt', 'vcruntime', 'api-ms-win-crt')
    $ok = $true

    if ($dumpbin) {
        $raw = & dumpbin /nologo /dependents $exe.FullName 2>$null
        $deps = $raw | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
        $badDeps = $deps | Where-Object { $_ -notmatch '^(?i)KERNEL32\.dll$' }
        if ($badDeps) { $ok = $false }
    }

    try {
        $bytes = [System.IO.File]::ReadAllBytes($exe.FullName)
        $text = [System.Text.Encoding]::ASCII.GetString($bytes)
        $found = $stdlibPatterns | Where-Object { $text -match $_ }
        if ($found) { $ok = $false }
    } catch {}

    if ($ok) { $VPass++ } else { $VFail++ }
}

# --- Summary ---
Write-Host ""
Write-Host "━━━ Oscan Test Suite ━━━" -ForegroundColor Cyan
Write-Host ("  Positive:     {0} passed, {1} failed" -f $Pass, $Fail)
Write-Host ("  Negative:     {0} passed, {1} failed" -f $NegPass, $NegFail)
if (($VPass + $VFail) -gt 0) {
    Write-Host ("  Freestanding: {0} verified, {1} failed" -f $VPass, $VFail)
}

if ($Failures.Count -gt 0) {
    Write-Host ""
    Write-Host "  Failures:" -ForegroundColor Red
    foreach ($f in $Failures) {
        Write-Host "    $f" -ForegroundColor Red
    }
}

Write-Host ""
$totalFail = $Fail + $NegFail + $VFail
Pop-Location

if ($totalFail -gt 0) {
    Write-Host "  SOME TESTS FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "  ALL TESTS PASSED ✓" -ForegroundColor Green
    exit 0
}
