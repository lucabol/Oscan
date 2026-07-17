# Oscan Test Runner (PowerShell) — quiet by default, -VerboseOutput for details
# Usage: .\run_tests.ps1 -Oscan <oscan-binary> [-Backend <name>] [-VerboseOutput]

param(
    [Parameter(Mandatory=$true)]
    [string]$Oscan,

    [string]$Backend = "c",

    [string]$BackendOption = "--backend",

    [string[]]$UnstableStderrTests = @(),

    [switch]$VerboseOutput
)

$ErrorActionPreference = "Continue"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $ScriptDir
. (Join-Path $ScriptDir "backend_oracle.ps1")

$Pass = 0; $Fail = 0; $NegPass = 0; $NegFail = 0
$Failures = [System.Collections.ArrayList]::new()
$LibcRegressionTests = @('builtin_typed_maps')
$OracleOnlyStdoutTests = @('tls_fetch')

if (-not (Test-Path "build")) {
    New-Item -ItemType Directory -Path "build" | Out-Null
}

if ($Backend -ne "c") {
    try {
        Assert-OracleBackendAvailable -Compiler $Oscan -Backend $Backend -BackendOption $BackendOption
    } catch {
        Pop-Location
        Write-Error $_
        exit 2
    }
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

    if ($name -match '^ffi') { $compileArgs += '--libc' }

    if ($Backend -eq "c") {
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
    } else {
        $oracleResult = Invoke-DifferentialBackendTest `
            -Compiler $Oscan `
            -Source $oscFile.FullName `
            -Name $name `
            -Backend $Backend `
            -BackendOption $BackendOption `
            -BuildRoot (Join-Path $ScriptDir "build") `
            -RunRoot (Join-Path $ScriptDir "build\oracle-runs\$name") `
            -ExpectedFile (Join-Path $ScriptDir $expectedFile) `
            -ExpectedStderrFile (Join-Path $ScriptDir "expected_stderr\$name.expected") `
            -ExpectedExitFile (Join-Path $ScriptDir "expected_exit\$name.expected") `
            -FixtureRoot (Join-Path $ScriptDir "fixtures") `
            -ExpectedFixtureRoot (Join-Path $ScriptDir "expected_files") `
            -CompileArguments $compileArgs `
            -CompareStderr ($UnstableStderrTests -notcontains $name) `
            -CompareExpectedStdout ($OracleOnlyStdoutTests -notcontains $name)
        if (-not $oracleResult.Success) {
            [void]$Failures.Add("$name — $($oracleResult.Failures -join '; ')")
            $Fail++
            continue
        }
    }

    if (($Backend -eq "c") -and ($LibcRegressionTests -contains $name) -and ($name -notmatch '^ffi')) {
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
    $negativeBackends = @("c")
    if ($Backend -ne "c") { $negativeBackends += $Backend }
    $rejectedByAllBackends = $true
    foreach ($negativeBackend in $negativeBackends) {
        $backendTag = ConvertTo-OracleBackendTag $negativeBackend
        $backendArgs = @()
        if ($negativeBackend -ne "c") {
            $backendArgs += @($BackendOption, $negativeBackend)
        }
        & $Oscan @backendArgs $oscFile.FullName `
            -o "build\$name.$backendTag.negative" `
            2>"build\$name.$backendTag.err"
        if ($LASTEXITCODE -eq 0) {
            [void]$Failures.Add("$name — should have been rejected by $negativeBackend")
            $rejectedByAllBackends = $false
        }
    }
    if (-not $rejectedByAllBackends) {
        $NegFail++
    } else {
        if ($VerboseOutput) {
            Write-Host "  PASS: $name — correctly rejected by $($negativeBackends -join ', ')" -ForegroundColor Green
        }
        $NegPass++
    }
}

# --- Freestanding Verification ---
$VPass = 0; $VFail = 0
$dumpbin = Get-Command dumpbin -ErrorAction SilentlyContinue
$readelf = Get-Command readelf -ErrorAction SilentlyContinue
$verificationBackendTag = ConvertTo-OracleBackendTag $Backend
$executablePattern = if ($Backend -eq "c") {
    "build\*.exe"
} else {
    "build\*.backend-$verificationBackendTag$(Get-OracleExecutableSuffix)"
}
foreach ($exe in Get-ChildItem $executablePattern -File -ErrorAction SilentlyContinue) {
    if ($exe.Name -match '^ffi' -or $exe.Name -like '*.libc*') { continue }
    $stdlibPatterns = @('msvcrt', 'ucrt', 'vcruntime', 'api-ms-win-crt')
    $ok = $true

    if ($dumpbin) {
        $raw = & dumpbin /nologo /dependents $exe.FullName 2>$null
        $deps = $raw | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
        $badDeps = $deps | Where-Object { $_ -notmatch '^(?i)KERNEL32\.dll$' }
        if ($badDeps) { $ok = $false }
    }

    if ($readelf -and [System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        $programHeaders = & $readelf.Source -l $exe.FullName 2>$null | Out-String
        $dynamicSection = & $readelf.Source -d $exe.FullName 2>&1 | Out-String
        if ($programHeaders -match '\bINTERP\b' -or $dynamicSection -match '\bNEEDED\b') {
            $ok = $false
        }
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
if ($Backend -ne "c") {
    Write-Host ("  Oracle:       C vs {0}" -f $Backend)
}
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
