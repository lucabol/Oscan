# Backend parity check: compile and run every positive test with one
# backend and compare stdout against tests/expected/<name>.expected.
#
# This is the local, toolchain-light complement to `run_tests.ps1`'s
# differential C oracle: it does not need a fully-provisioned C toolchain
# (system import libraries and all), only a working final linker, so it
# can validate the LLVM and Cranelift object backends on a developer
# machine and in CI alike.
#
# Usage:
#   pwsh -File tests/backend_parity.ps1 -Oscan ../target/debug/oscan.exe -Backend llvm

param(
    [Parameter(Mandatory = $true)][string]$Oscan,
    [Parameter(Mandatory = $true)][ValidateSet('llvm', 'cranelift', 'c')][string]$Backend,
    [string[]]$Only = @(),
    [switch]$FreestandingOnly,
    [switch]$VerboseOutput
)

$ErrorActionPreference = 'Continue'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $ScriptDir

$buildRoot = Join-Path $ScriptDir "build\parity-$Backend"
if (Test-Path $buildRoot) { Remove-Item -Recurse -Force $buildRoot }
New-Item -ItemType Directory -Path $buildRoot | Out-Null

# Tests whose stdout is environment dependent (network, clock, graphics)
# and therefore only meaningful under the differential oracle.
$SkipStdout = @('tls_fetch')

$pass = 0
$fail = 0
$skipped = 0
$failures = [System.Collections.ArrayList]::new()

foreach ($oscFile in Get-ChildItem "positive\*.osc" | Sort-Object Name) {
    $name = $oscFile.BaseName
    if ($Only.Count -gt 0 -and ($Only -notcontains $name)) { continue }
    if ($FreestandingOnly -and $name -match '^ffi') {
        $skipped++
        continue
    }

    $expectedFile = "expected\$name.expected"
    if (-not (Test-Path $expectedFile)) {
        [void]$failures.Add("$name - missing expected file")
        $fail++
        continue
    }

    $compileArgs = @()
    if ($name -match '^ffi') { $compileArgs += '--libc' }

    $exe = Join-Path $buildRoot "$name.exe"
    $errFile = Join-Path $buildRoot "$name.err"
    & $Oscan '--backend' $Backend @compileArgs $oscFile.FullName -o $exe 2>$errFile
    if ($LASTEXITCODE -ne 0) {
        $detail = (Get-Content $errFile -Raw -ErrorAction SilentlyContinue)
        if ($detail) { $detail = ($detail -split "`n" | Select-Object -Last 3) -join ' | ' }
        [void]$failures.Add("$name - compile error: $detail")
        $fail++
        continue
    }

    if ($SkipStdout -contains $name) {
        $skipped++
        continue
    }

    Push-Location $buildRoot
    $actual = & $exe 2>&1 | Out-String
    Pop-Location
    $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
    $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

    if ($actual -ne $expected) {
        [void]$failures.Add("$name - output mismatch")
        if ($VerboseOutput) {
            Write-Host "--- $name expected ---" -ForegroundColor Yellow
            Write-Host $expected
            Write-Host "--- $name actual ---" -ForegroundColor Yellow
            Write-Host $actual
        }
        $fail++
        continue
    }

    if ($VerboseOutput) { Write-Host "  PASS: $name" -ForegroundColor Green }
    $pass++
}

Write-Host ""
Write-Host "backend=$Backend  pass=$pass  fail=$fail  skipped=$skipped"
if ($failures.Count -gt 0) {
    Write-Host "failures:" -ForegroundColor Red
    foreach ($f in $failures) { Write-Host "  $f" -ForegroundColor Red }
}
Pop-Location
if ($fail -gt 0) { exit 1 }
exit 0
