#!/usr/bin/env pwsh
# Oscan Test Runner — quiet by default, verbose on --VerboseOutput or failure
# Usage: .\test.ps1 [-Backend <name> | --backend <name>] [-SkipBuild] [-SkipUnit] [-SkipIntegration] [-SkipWSL] [-SkipARM] [-SkipLibc] [-VerboseOutput]

param(
    [switch]$SkipBuild,
    [switch]$SkipUnit,
    [switch]$SkipIntegration,
    [switch]$SkipWSL,
    [switch]$SkipARM,
    [switch]$SkipLibc,
    [string]$Backend = "c",
    [string]$BackendOption = "--backend",
    [string[]]$UnstableStderrTests = @(),
    [switch]$VerboseOutput,
    [switch]$SourceOnly  # When set, only define functions (for dot-sourcing by CI)
)

$ErrorActionPreference = "Continue"
. (Join-Path $PSScriptRoot "tests\backend_oracle.ps1")

try {
    $backendSelection = Resolve-OracleBackendSelection -Backend $Backend -BackendOption $BackendOption
    $Backend = $backendSelection.Backend
    $BackendOption = $backendSelection.BackendOption
} catch {
    Write-Host "Backend selection failed: $_" -ForegroundColor Red
    exit 2
}

# ── Result tracking ────────────────────────────────────
$script:Results = [System.Collections.ArrayList]::new()
$script:VerifyResults = [System.Collections.ArrayList]::new()
$script:Sizes = @{}
$script:BuildOk = $false
$script:UnitOk = $false
$script:UnitCount = 0
$script:WSLAvail = $false
$script:ARMAvail = $false

# ── Progress display ───────────────────────────────────
function Write-Phase($Label) {
    Write-Host "  $Label ... " -NoNewline -ForegroundColor White
}

function Write-PhaseResult($Text, $Color) {
    Write-Host $Text -ForegroundColor $Color
}

function Write-Counter($Current, $Total) {
    Write-Host "`r  $(' ' * 40)`r" -NoNewline  # clear line
}

function Add-TestResult($Name, $Arch, $Category, $Status, $Detail) {
    [void]$script:Results.Add([PSCustomObject]@{
        Name = $Name; Arch = $Arch; Category = $Category
        Status = $Status; Detail = $Detail
    })
    if ($VerboseOutput) {
        $color = if ($Status -eq 'PASS') { 'Green' } else { 'Red' }
        $msg = "  ${Status}: $Name"
        if ($Detail -and $Status -ne 'PASS') { $msg += " — $Detail" }
        Write-Host $msg -ForegroundColor $color
    }
}

function Write-PhaseFailures($Arch) {
    if ($VerboseOutput) { return }
    foreach ($failure in @($script:Results | Where-Object { $_.Arch -eq $Arch -and $_.Status -eq 'FAIL' })) {
        $message = "    FAIL: $($failure.Name)"
        if ($failure.Detail) { $message += " - $($failure.Detail)" }
        Write-Host $message -ForegroundColor Red
    }
}

function Add-VerifyResult($Name, $Arch, $DepCheck, $DepDetail, $StdlibCheck, $Size) {
    [void]$script:VerifyResults.Add([PSCustomObject]@{
        Name = $Name; Arch = $Arch
        DepCheck = $DepCheck; DepDetail = $DepDetail
        StdlibCheck = $StdlibCheck; Size = $Size
    })
    $script:Sizes["${Name}_${Arch}"] = $Size
}

# ── WSL helpers ────────────────────────────────────────
function Convert-ToWSLPath($windowsPath) {
    $full = (Resolve-Path $windowsPath).Path
    $drive = $full.Substring(0, 1).ToLower()
    $rest = $full.Substring(2).Replace('\', '/')
    return "/mnt/$drive$rest"
}

function Test-WSLAvailable {
    try {
        $out = wsl -e echo ok 2>&1 | ForEach-Object { "$_" }
        return ($out -match "ok")
    } catch { return $false }
}

function Test-WSLCommand($cmd) {
    wsl bash -c "command -v $cmd" 2>&1 | Out-Null
    return ($LASTEXITCODE -eq 0)
}

# Validates and parses the "OK|name|exitcode" / "FAIL|name|detail" records
# produced by the batched WSL native cross-link-and-run script (a single WSL
# call compiles/links/runs every cross-emitted object, so this is the only
# place that can catch problems specific to that batch). It is a pure
# function (no WSL/filesystem access) so it can be exercised directly by
# focused tests. Guarantees exactly one record per name in $AttemptedNames:
#   - a program's exit code must equal $ExpectedExitCodes[name] (default 0
#     when the name has no entry, matching Invoke-OracleBackendCase's own
#     expected_exit convention) or it is FAIL even if its stdout later turns
#     out to match (the caller compares stdout separately) — this is what
#     lets a test like result_main_exit_code, whose tests/expected_exit file
#     declares "1", pass here instead of being forced to exit 0
#   - if the wsl invocation itself failed ($BatchExitCode -ne 0), that is
#     reported as an error and any attempted name without its own record
#     becomes a synthetic FAIL
#   - duplicate records for the same name, records for names outside
#     $AttemptedNames, and malformed records (missing fields or a
#     non-numeric OK exit code) are rejected rather than silently accepted
#   - any attempted name with no matching record at all becomes a synthetic
#     FAIL ("fewer records than attempted")
function Resolve-WslNativeBatchRecords($BatchOutput, $BatchExitCode, [string[]]$AttemptedNames, $ExpectedExitCodes = @{}) {
    $records = [ordered]@{}
    $errors = [System.Collections.Generic.List[string]]::new()
    $attemptedSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$AttemptedNames)

    foreach ($rawLine in (($BatchOutput -replace "`r", "") -split "`n")) {
        $line = $rawLine.Trim()
        if ($line -eq '' -or $line -notmatch '^(OK|FAIL)\|') { continue }

        $parts = $line -split '\|', 3
        $status = $parts[0]
        $name = if ($parts.Count -ge 2) { $parts[1].Trim() } else { '' }

        if ([string]::IsNullOrEmpty($name)) {
            $errors.Add("malformed batch record (no name): '$line'")
            continue
        }
        if (-not $attemptedSet.Contains($name)) {
            $errors.Add("batch record for unattempted name '$name'")
            continue
        }
        if ($records.Contains($name)) {
            $errors.Add("duplicate batch record for '$name'")
            $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = 'duplicate batch record' }
            continue
        }
        if ($parts.Count -lt 3) {
            $errors.Add("malformed batch record for '$name' (missing detail field): '$line'")
            $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = 'malformed batch record' }
            continue
        }

        $detail = $parts[2].Trim()
        if ($status -eq 'OK') {
            $exitCode = 0
            if (-not [int]::TryParse($detail, [ref]$exitCode)) {
                $errors.Add("malformed exit code for '$name': '$detail'")
                $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = "malformed exit code '$detail'" }
            } else {
                $expectedExit = 0
                if ($ExpectedExitCodes -and $ExpectedExitCodes.Contains($name)) {
                    $expectedExit = $ExpectedExitCodes[$name]
                }
                if ($exitCode -ne $expectedExit) {
                    $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = "program exited $exitCode (expected $expectedExit)" }
                } else {
                    $records[$name] = @{ Name = $name; Status = 'OK'; Detail = '' }
                }
            }
        } else {
            $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = $(if ($detail) { $detail } else { 'link error' }) }
        }
    }

    if ($BatchExitCode -ne 0) {
        $errors.Add("wsl native batch command failed (exit $BatchExitCode)")
    }

    foreach ($name in $AttemptedNames) {
        if (-not $records.Contains($name)) {
            $errors.Add("missing batch result record for '$name'")
            $records[$name] = @{ Name = $name; Status = 'FAIL'; Detail = 'missing batch result record' }
        }
    }

    return @{ Records = @($records.Values); Errors = @($errors) }
}

# ── Freestanding verification helpers ──────────────────

function Find-Dumpbin {
    try {
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path $vswhere) {
            $vsPath = & $vswhere -latest -property installationPath 2>$null
            if ($vsPath) {
                $db = Get-ChildItem "$vsPath\VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($db) { return $db.FullName }
            }
        }
    } catch {}
    $inPath = Get-Command dumpbin -ErrorAction SilentlyContinue
    if ($inPath) { return $inPath.Source }
    return $null
}

function Test-WindowsFreestanding($exePath) {
    $size = (Get-Item $exePath).Length
    $depCheck = "SKIP"; $depDetail = "no tool"; $stdlibCheck = "SKIP"

    # Socket/UDP tests legitimately need WS2_32.dll; TLS tests also need Secur32 + Crypt32;
    # canvas/clipboard tests open a real Win32 window / touch the desktop clipboard, so they
    # legitimately need User32 + Gdi32 (the non-interactive gfx_*/img_*/svg_*/tt_* builtins are
    # pure in-memory pixel-buffer code and do not, so they are intentionally not matched here).
    $testName = [System.IO.Path]::GetFileNameWithoutExtension($exePath)
    $allowPattern = if ($testName -match '^tls') {
        '^(?i)(KERNEL32|WS2_32|SECUR32|CRYPT32)\.dll$'
    } elseif ($testName -match 'socket|udp') {
        '^(?i)(KERNEL32|WS2_32)\.dll$'
    } elseif ($testName -match 'canvas|clipboard') {
        '^(?i)(KERNEL32|USER32|GDI32)\.dll$'
    } else {
        '^(?i)KERNEL32\.dll$'
    }

    $dumpbin = Find-Dumpbin
    if ($dumpbin) {
        $raw = & $dumpbin /nologo /dependents $exePath 2>$null
        $deps = $raw | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
        $badDeps = $deps | Where-Object { $_ -notmatch $allowPattern }
        if ($badDeps) {
            $depCheck = "FAIL"; $depDetail = ($badDeps -join ", ")
        } else {
            $depCheck = "PASS"; $depDetail = ($deps -join ", ")
            if (-not $depDetail) { $depDetail = "none" }
        }
    }

    # Scan binary for stdlib strings
    $stdlibPatterns = @('msvcrt', 'ucrt', 'vcruntime', 'api-ms-win-crt')
    try {
        $bytes = [System.IO.File]::ReadAllBytes($exePath)
        $text = [System.Text.Encoding]::ASCII.GetString($bytes)
        $found = $stdlibPatterns | Where-Object { $text -match $_ }
        $stdlibCheck = if ($found) { "FAIL" } else { "PASS" }
    } catch {
        $stdlibCheck = "SKIP"
    }

    return @{ DepCheck = $depCheck; DepDetail = $depDetail; StdlibCheck = $stdlibCheck; Size = $size }
}

function Test-LinuxFreestanding($wslExePath, $wslDir) {
    $fullPath = "$wslDir/$wslExePath"
    $fileInfo = wsl bash -c "file '$fullPath'" 2>&1 | Out-String
    $sizeStr = wsl bash -c "stat -c%s '$fullPath'" 2>&1 | Out-String
    $size = [int64]($sizeStr.Trim())

    $depCheck = "PASS"; $depDetail = "static"; $stdlibCheck = "PASS"

    if ($fileInfo -notmatch 'statically linked') {
        $depCheck = "FAIL"
        $lddOut = wsl bash -c "ldd '$fullPath' 2>&1" | Out-String
        if ($lddOut -match 'not a dynamic executable') {
            $depDetail = "static (no dynamic section)"
            $depCheck = "PASS"
        } else {
            $depDetail = "dynamic"
        }
    }

    $cnt = wsl bash -c "strings '$fullPath' | grep -ciE 'libc\.so|libm\.so|glibc|__libc_start_main' || true" 2>&1 | Out-String
    if ([int]($cnt.Trim()) -gt 0) { $stdlibCheck = "FAIL" }

    return @{ DepCheck = $depCheck; DepDetail = $depDetail; StdlibCheck = $stdlibCheck; Size = $size }
}

# ── Display helpers ────────────────────────────────────

function Format-Size($bytes) {
    if ($null -eq $bytes -or $bytes -eq 0) { return "-" }
    if ($bytes -lt 1024) { return "{0:N0} B" -f $bytes }
    elseif ($bytes -lt 1048576) { return "{0:N1} KB" -f ($bytes / 1024) }
    else { return "{0:N1} MB" -f ($bytes / 1048576) }
}

function Show-Summary {
    Write-Host ""

    # Compact freestanding verification — one representative binary (hello_world) per arch
    $verifyArchs = @("win-x64", "wsl-x64", "arm64")
    $archLabels = @{ "win-x64" = "Windows x64"; "wsl-x64" = "WSL Linux x64"; "arm64" = "ARM64" }
    $probeRows = @()
    foreach ($va in $verifyArchs) {
        $probe = $script:VerifyResults | Where-Object { $_.Arch -eq $va -and $_.Name -eq "hello_world" } | Select-Object -First 1
        if ($probe) { $probeRows += $probe }
    }
    if ($probeRows.Count -gt 0) {
        # Get stdlib (libc) exe sizes for hello_world per arch
        $stdlibSizes = @{}
        $hwLibc = "tests\build\hello_world_libc.exe"
        if (Test-Path $hwLibc) { $stdlibSizes["win-x64"] = (Get-Item $hwLibc).Length }
        # WSL: compile hello_world with libc and get size
        if ($script:WSLAvail) {
            try {
                $wslDir = Convert-ToWSLPath (Get-Location).Path
                $wslLibcExe = "tests/build/hello_world_libc_wsl"
                # Compile with libc (std=c99, link -lm)
                & $oscan --libc "tests\positive\hello_world.osc" -o "tests\build\hello_world_libc.c" 2>$null
                wsl bash -c "cd '$wslDir' && gcc -std=c99 tests/build/hello_world_libc.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o $wslLibcExe -lm" 2>&1 | Out-Null
                $sz = wsl bash -c "stat -c%s '$wslDir/$wslLibcExe' 2>/dev/null || echo 0" 2>&1 | Out-String
                $szVal = [int64]($sz.Trim())
                if ($szVal -gt 0) { $stdlibSizes["wsl-x64"] = $szVal }
            } catch {}
        }
        # ARM64: compile hello_world with libc and get size
        if ($script:ARMAvail) {
            try {
                $wslDir = Convert-ToWSLPath (Get-Location).Path
                $armLibcExe = "tests/build/hello_world_libc_arm"
                & $oscan --libc "tests\positive\hello_world.osc" -o "tests\build\hello_world_libc_arm.c" 2>$null
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=c99 tests/build/hello_world_libc_arm.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o $armLibcExe -lm" 2>&1 | Out-Null
                $sz = wsl bash -c "stat -c%s '$wslDir/$armLibcExe' 2>/dev/null || echo 0" 2>&1 | Out-String
                $szVal = [int64]($sz.Trim())
                if ($szVal -gt 0) { $stdlibSizes["arm64"] = $szVal }
            } catch {}
        }

        Write-Host "  Freestanding Verification (hello_world):" -ForegroundColor White
        Write-Host ("  {0,-16} {1,10} {2,-18} {3,10}" -f "Architecture", "Size", "Dependencies", "Stdlib")
        Write-Host ("  {0,-16} {1,10} {2,-18} {3,10}" -f ("─" * 16), ("─" * 10), ("─" * 18), ("─" * 10))
        foreach ($v in $probeRows) {
            $ok = ($v.DepCheck -ne 'FAIL') -and ($v.StdlibCheck -ne 'FAIL')
            $color = if ($ok) { "Green" } else { "Red" }
            $depStr = if ($v.DepDetail.Length -gt 18) { $v.DepDetail.Substring(0, 15) + "..." } else { $v.DepDetail }
            $stdlibStr = if ($stdlibSizes.ContainsKey($v.Arch)) { Format-Size $stdlibSizes[$v.Arch] } else { "-" }
            if ($v.StdlibCheck -eq 'FAIL') { $stdlibStr = "FAIL" }
            Write-Host ("  {0,-16} {1,10} {2,-18} {3,10}" -f $archLabels[$v.Arch], (Format-Size $v.Size), $depStr, $stdlibStr) -ForegroundColor $color
        }
        Write-Host ""
    }

    # Show failures (always, regardless of verbose)
    $failures = @($script:Results | Where-Object { $_.Status -eq 'FAIL' })
    if ($failures.Count -gt 0 -and -not $VerboseOutput) {
        Write-Host "  Failed tests:" -ForegroundColor Red
        foreach ($f in $failures) {
            $msg = "    $($f.Arch)/$($f.Name)"
            if ($f.Detail) { $msg += " — $($f.Detail)" }
            Write-Host $msg -ForegroundColor Red
        }
        Write-Host ""
    }

    $vfailures = @($script:VerifyResults | Where-Object { $_.DepCheck -eq 'FAIL' -or $_.StdlibCheck -eq 'FAIL' })
    if ($vfailures.Count -gt 0) {
        Write-Host "  Freestanding verification failures:" -ForegroundColor Red
        foreach ($v in $vfailures) {
            $msg = "    $($v.Arch)/$($v.Name)"
            if ($v.DepCheck -eq 'FAIL') { $msg += " deps:[$($v.DepDetail)]" }
            if ($v.StdlibCheck -eq 'FAIL') { $msg += " stdlib:FOUND" }
            Write-Host $msg -ForegroundColor Red
        }
        Write-Host ""
    }

    # Final
    $totalFailed = $failures.Count + $vfailures.Count
    if ($totalFailed -gt 0) {
        Write-Host "  SOME TESTS FAILED" -ForegroundColor Red
        return $false
    } else {
        Write-Host "  ALL TESTS PASSED ✓" -ForegroundColor Green
        return $true
    }
}

# ══════════════════════════════════════════════════════
# ── Build ─────────────────────────────────────────────
# ══════════════════════════════════════════════════════

# When SourceOnly is set, stop here — only function definitions were needed
if ($SourceOnly) { return }

Write-Host "`n━━━ Oscan Test Suite ━━━`n" -ForegroundColor Cyan

# Clean build directory
if (Test-Path "tests\build") { Remove-Item "tests\build\*" -Recurse -Force -ErrorAction SilentlyContinue }
else { New-Item -ItemType Directory -Path "tests\build" -Force | Out-Null }
# Keep compiled examples in examples\ for manual testing

if (-not $SkipBuild) {
    Write-Phase "Building (release)"
    $buildOutput = cargo build --release 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-PhaseResult "FAIL" Red
        $buildOutput | ForEach-Object { Write-Host "    $_" }
        exit 1
    }
    $script:BuildOk = $true
    Write-PhaseResult "OK" Green
} else {
    $script:BuildOk = $true
    Write-Phase "Build"; Write-PhaseResult "skipped" Yellow
}

$oscan = ".\target\release\oscan.exe"
if (-not (Test-Path $oscan)) {
    Write-Host "Error: $oscan not found. Run without -SkipBuild first." -ForegroundColor Red
    exit 1
}
$projectRoot = (Get-Location).Path
$throttle = [Math]::Max(1, [Environment]::ProcessorCount)

if ($Backend -ne "c") {
    try {
        Assert-OracleBackendAvailable -Compiler $oscan -Backend $Backend -BackendOption $BackendOption
    } catch {
        Write-Host "Backend selection failed: $_" -ForegroundColor Red
        exit 2
    }
}

# ══════════════════════════════════════════════════════
# ── Unit tests ────────────────────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipUnit) {
    Write-Phase "Unit tests"
    $unitOutput = cargo test 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-PhaseResult "FAIL" Red
        $unitOutput | ForEach-Object { Write-Host "    $_" }
        exit 1
    }
    $countLine = $unitOutput | Where-Object { $_ -match 'test result: ok\. (\d+) passed' } | Select-Object -Last 1
    if ($countLine -match '(\d+) passed') { $script:UnitCount = [int]$Matches[1] }
    $script:UnitOk = $true
    Write-PhaseResult "OK ($($script:UnitCount) passed)" Green
} else {
    Write-Phase "Unit tests"; Write-PhaseResult "skipped" Yellow
}

# ══════════════════════════════════════════════════════
# ── Integration tests (Windows) ──────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipIntegration) {
    if ($VerboseOutput) { Write-Host "`n━━━ Integration Tests ━━━" -ForegroundColor Cyan }

    if (-not (Test-Path "tests\build")) {
        New-Item -ItemType Directory -Path "tests\build" | Out-Null
    }

    # Positive tests (parallel compile + run)
    $positivePhase = if ($Backend -eq "c") { "Windows x64 (positive)" } else { "Windows x64 (C vs $Backend)" }
    Write-Phase $positivePhase
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (freestanding) ──" -ForegroundColor Yellow }
    $secPass = 0; $secFail = 0
    # Skip socket_hostnames on Windows: binding to a hostname triggers the
    # Windows Firewall prompt (interactive popup) which hangs CI. Linux/WSL
    # and ARM still exercise the hostname path.
    $positiveFiles = Get-ChildItem "tests\positive\*.osc" | Where-Object { $_.BaseName -ne 'socket_hostnames' }

    # --backend native defaults to the freestanding runtime archive, matching
    # the C backend's freestanding-by-default output. FFI fixtures explicitly
    # pass --libc below, selecting the hosted archive and normal CRT/libm link;
    # neither native mode silently falls back to the other.
    if ($Backend -eq "c") {
        $parallelResults = $positiveFiles | ForEach-Object -Parallel {
            $name = $_.BaseName
            $root = $using:projectRoot
            $oscanExe = "$root\target\release\oscan.exe"
            $expectedFile = "$root\tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                return [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'missing expected file' }
            }

            if ($name -match '^ffi') {
                & $oscanExe --libc $_.FullName -o "$root\tests\build\$name.exe" 2>$null
            } else {
                & $oscanExe $_.FullName -o "$root\tests\build\$name.exe" 2>$null
            }
            if ($LASTEXITCODE -ne 0) {
                return [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'compile error' }
            }

            $actual = & "$root\tests\build\$name.exe" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
                [PSCustomObject]@{ Name = $name; Status = 'PASS'; Detail = '' }
            } else {
                [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'output mismatch' }
            }
        } -ThrottleLimit $throttle
    } else {
        $parallelResults = foreach ($oscFile in $positiveFiles) {
            $name = $oscFile.BaseName
            # NOTE: the leading "," (unary comma operator) on each branch is
            # required — without it, PowerShell's pipeline output capture
            # unwraps an empty-array branch result (@()) to $null (a
            # long-standing, version-dependent PowerShell quirk), which then
            # makes Invoke-DifferentialBackendTest's -CompileArguments $null
            # turn into a one-element array containing an empty string once
            # it crosses @($CompileArguments), corrupting the compiler's
            # argument list with a spurious blank argument.
            $compileArgs = if ($name -match '^ffi') { , @('--libc') } else { , @() }
            $oracleResult = Invoke-DifferentialBackendTest `
                -Compiler $oscan `
                -Source $oscFile.FullName `
                -Name $name `
                -Backend $Backend `
                -BackendOption $BackendOption `
                -BuildRoot (Join-Path $projectRoot "tests\build") `
                -RunRoot (Join-Path $projectRoot "tests\build\oracle-runs\$name") `
                -ExpectedFile (Join-Path $projectRoot "tests\expected\$name.expected") `
                -ExpectedStderrFile (Join-Path $projectRoot "tests\expected_stderr\$name.expected") `
                -ExpectedExitFile (Join-Path $projectRoot "tests\expected_exit\$name.expected") `
                -FixtureRoot (Join-Path $projectRoot "tests\fixtures") `
                -ExpectedFixtureRoot (Join-Path $projectRoot "tests\expected_files") `
                -CompileArguments $compileArgs `
                -CompareStderr ($UnstableStderrTests -notcontains $name)
            if ($oracleResult.Success) {
                [PSCustomObject]@{ Name = $name; Status = 'PASS'; Detail = '' }
            } else {
                [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = ($oracleResult.Failures -join '; ') }
            }
        }
    }

    foreach ($r in $parallelResults) {
        $resultArch = if ($Backend -eq "c") { "win-x64" } else { "win-x64-$Backend" }
        Add-TestResult $r.Name $resultArch "positive" $r.Status $r.Detail
        if ($r.Status -eq 'PASS') { $secPass++ } else { $secFail++ }
    }
    if (-not $VerboseOutput) {
        $color = if ($secFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$secPass passed, $secFail failed" $color
    }

    # Negative tests (sequential — fast, no compilation needed)
    Write-Phase "Negative tests"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Negative tests ──" -ForegroundColor Yellow }
    $negPass = 0; $negFail = 0
    foreach ($bcFile in Get-ChildItem "tests\negative\*.osc") {
        $name = $bcFile.BaseName
        & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
        if ($LASTEXITCODE -eq 0) {
            Add-TestResult $name "win-x64-neg" "negative" "FAIL" "should have been rejected"
            $negFail++
        } else {
            Add-TestResult $name "win-x64-neg" "negative" "PASS" "correctly rejected"
            $negPass++
        }
    }
    if (-not $VerboseOutput) {
        $color = if ($negFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$negPass passed, $negFail failed" $color
    }

    # Cleanup .obj files
    Get-ChildItem "*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    Get-ChildItem "tests\build\*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue

    # Windows freestanding verification
    Write-Phase "Verifying (Win x64)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Freestanding verification (Windows) ──" -ForegroundColor Yellow }
    $vPass = 0; $vFail = 0
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName
        if ($name -match '^ffi') { continue }
        if ($name -eq 'socket_hostnames') { continue }
        $exe = if ($Backend -eq "c") {
            "tests\build\$name.exe"
        } else {
            "tests\build\$name.$(ConvertTo-OracleBackendTag $Backend).exe"
        }
        if (-not (Test-Path $exe)) { continue }

        $r = Test-WindowsFreestanding $exe
        Add-VerifyResult $name "win-x64" $r.DepCheck $r.DepDetail $r.StdlibCheck $r.Size
        if (($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')) { $vPass++ } else { $vFail++ }

        if ($VerboseOutput) {
            $ok = ($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')
            $color = if ($ok) { "Green" } else { "Red" }
            Write-Host ("  {0}: {1}  deps={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $r.DepCheck, $r.StdlibCheck, (Format-Size $r.Size)) -ForegroundColor $color
        }
    }
    if (-not $VerboseOutput) {
        $color = if ($vFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$vPass verified, $vFail failed" $color
    }
    # Windows libc (stdlib) tests — parallel
    if (-not $SkipLibc) {
    Write-Phase "Windows x64 (libc)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (libc/stdlib) ──" -ForegroundColor Yellow }
    $libcPass = 0; $libcFail = 0
    # See comment above: skip on Windows to avoid firewall prompt.
    $libcResults = Get-ChildItem "tests\positive\*.osc" | Where-Object { $_.BaseName -ne 'socket_hostnames' } | ForEach-Object -Parallel {
        $name = $_.BaseName
        $root = $using:projectRoot
        $oscanExe = "$root\target\release\oscan.exe"

        # Use libc-specific expected output if available, else standard
        $libcExpected = "$root\tests\expected_libc\$name.expected"
        $stdExpected  = "$root\tests\expected\$name.expected"
        $expectedFile = if (Test-Path $libcExpected) { $libcExpected } else { $stdExpected }

        if (-not (Test-Path $expectedFile)) {
            return [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'missing expected file' }
        }

        & $oscanExe --libc $_.FullName -o "$root\tests\build\${name}_libc.exe" 2>$null
        if ($LASTEXITCODE -ne 0) {
            return [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'compile error' }
        }

        $actual = & "$root\tests\build\${name}_libc.exe" 2>&1 | Out-String
        $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
        $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actual -eq $expected) {
            [PSCustomObject]@{ Name = $name; Status = 'PASS'; Detail = '' }
        } else {
            [PSCustomObject]@{ Name = $name; Status = 'FAIL'; Detail = 'output mismatch' }
        }
    } -ThrottleLimit $throttle

    foreach ($r in $libcResults) {
        Add-TestResult $r.Name "win-x64-libc" "positive-libc" $r.Status $r.Detail
        if ($r.Status -eq 'PASS') { $libcPass++ } else { $libcFail++ }
    }
    if (-not $VerboseOutput) {
        $color = if ($libcFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$libcPass passed, $libcFail failed" $color
    }

    # Cleanup libc .obj files
    Get-ChildItem "*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    Get-ChildItem "tests\build\*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    } else {
        Write-Phase "Windows x64 (libc)"; Write-PhaseResult "skipped" Yellow
    }
}

# ══════════════════════════════════════════════════════
# ── WSL Integration tests ────────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipWSL) {
    if (-not (Test-WSLAvailable)) {
        Write-Phase "WSL Linux x64"; Write-PhaseResult "skipped (WSL not available)" Yellow
    } elseif (-not (Test-WSLCommand "gcc")) {
        Write-Phase "WSL Linux x64"; Write-PhaseResult "skipped (gcc not found)" Yellow
    } else {
        $script:WSLAvail = $true
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        # Phase 1: Transpile all .osc → .c in parallel on Windows
        Write-Phase "WSL Linux x64 (positive)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── WSL Positive tests (freestanding) ──" -ForegroundColor Yellow }

        $oscFiles = @(Get-ChildItem "tests\positive\*.osc")
        $transpileResults = $oscFiles | ForEach-Object -Parallel {
            $name = $_.BaseName
            $root = $using:projectRoot
            $oscanExe = "$root\target\release\oscan.exe"
            if ($name -match '^ffi') {
                & $oscanExe --libc $_.FullName -o "$root\tests\build\${name}.c" 2>$null
            } else {
                & $oscanExe $_.FullName -o "$root\tests\build\${name}.c" 2>$null
            }
            [PSCustomObject]@{ Name = $name; IsFfi = ($name -match '^ffi'); Success = ($LASTEXITCODE -eq 0) }
        } -ThrottleLimit $throttle

        # Phase 2: Generate batch bash script for compile + run
        $bashLines = @("#!/bin/bash", "cd '$wslDir'")
        $wslTestTimeoutSeconds = 30
        # l_img.h's vendored stb_image integration requires laststanding's
        # freestanding stdlib/string shims on Linux (see its README).
        $freestandingCompat = "-Ideps/laststanding/compat"
        foreach ($t in $transpileResults) {
            if (-not $t.Success) {
                $bashLines += "echo 'FAIL|$($t.Name)|transpile error'"
                continue
            }
            $n = $t.Name
            $isTls = $n -match '^tls_'
            if ($t.IsFfi) {
                $bashLines += "gcc -std=c99 tests/build/$n.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${n}_wsl -lm 2>/dev/null"
            } elseif ($isTls) {
                # TLS tests need BearSSL (libbearssl.a) linked after the source.
                # Use the pre-built static library from packaging/prebuilt.
                $bashLines += "gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$n.c -Iruntime -Ideps/laststanding $freestandingCompat packaging/prebuilt/linux-x86_64/libbearssl.a -o tests/build/${n}_wsl 2>/dev/null"
            } else {
                $bashLines += "gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$n.c -Iruntime -Ideps/laststanding $freestandingCompat -o tests/build/${n}_wsl 2>/dev/null"
            }
            $bashLines += "if [ `$? -ne 0 ]; then echo 'FAIL|$n|gcc compile error'; else"
            $bashLines += "  timeout --kill-after=5s ${wslTestTimeoutSeconds}s ./tests/build/${n}_wsl > tests/build/${n}_wsl.out 2>&1"
            $bashLines += "  rc=`$?"
            $bashLines += "  if [ `$rc -eq 124 ] || [ `$rc -eq 137 ]; then echo 'FAIL|$n|timed out after ${wslTestTimeoutSeconds}s'; else echo `"OK|$n|`$rc`"; fi"
            $bashLines += "fi"
        }

        $bashScript = $bashLines -join "`n"
        Set-Content "tests\build\_wsl_batch.sh" $bashScript -NoNewline

        # Phase 3: Execute single WSL call
        $batchOutput = wsl bash -c "cd '$wslDir' && bash tests/build/_wsl_batch.sh" 2>&1 | Out-String

        # Phase 4: Parse results and compare outputs
        $wslPass = 0; $wslFail = 0
        foreach ($line in ($batchOutput -split "`n" | Where-Object { $_ -match '^(OK|FAIL)\|' })) {
            $parts = $line.Trim() -split '\|', 3
            $status = $parts[0]; $name = $parts[1]; $detail = $parts[2]

            if ($status -eq 'FAIL') {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" $detail
                $wslFail++; continue
            }

            # Compare output from .out file
            $expectedFile = "tests\expected\$name.expected"
            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "missing expected file"
                $wslFail++; continue
            }
            $outFile = "tests\build\${name}_wsl.out"
            if (-not (Test-Path $outFile)) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "no output file"
                $wslFail++; continue
            }
            # A handful of builtins (canvas/clipboard defaults, tls_fetch's
            # network dependency) are legitimately reduced-capability outside
            # Windows — see Test-ExpectedOutputMatch's comment in
            # backend_oracle.ps1 — so also accept the established
            # expected_libc/<name>.expected text when the primary (Windows
            # freestanding) expected output doesn't match.
            $libcExpectedFile = "tests\expected_libc\$name.expected"
            if (Test-ExpectedOutputMatch -ActualRaw (Get-Content $outFile -Raw) -PrimaryExpectedFile $expectedFile -FallbackExpectedFile $libcExpectedFile) {
                Add-TestResult $name "wsl-x64" "positive" "PASS" ""
                $wslPass++
            } else {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "output mismatch"
                $wslFail++
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($wslFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$wslPass passed, $wslFail failed" $color
        }
        Write-PhaseFailures "wsl-x64"

        # WSL freestanding verification (batch: single WSL call)
        Write-Phase "Verifying (WSL)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Freestanding verification (WSL) ──" -ForegroundColor Yellow }
        $wvPass = 0; $wvFail = 0
        # Build a list of binaries to verify
        $verifyNames = @()
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            if ($name -match '^ffi') { continue }
            $verifyNames += $name
        }
        # Generate batch verification script
        $vBash = @("#!/bin/bash", "cd '$wslDir'")
        foreach ($name in $verifyNames) {
            $exe = "tests/build/${name}_wsl"
            $vBash += "if [ -f '$exe' ]; then"
            $vBash += "  sz=`$(stat -c%s '$exe' 2>/dev/null || echo 0)"
            $vBash += "  fi=`$(file '$exe')"
            $vBash += "  sc=`$(strings '$exe' | grep -ciE 'libc\.so|libm\.so|glibc|__libc_start_main' || true)"
            $vBash += "  echo `"VERIFY|$name|`$sz|`$fi|`$sc`""
            $vBash += "fi"
        }
        Set-Content "tests\build\_wsl_verify.sh" ($vBash -join "`n") -NoNewline
        $verifyOutput = wsl bash -c "cd '$wslDir' && bash tests/build/_wsl_verify.sh" 2>&1 | Out-String

        foreach ($line in ($verifyOutput -split "`n" | Where-Object { $_ -match '^VERIFY\|' })) {
            $parts = $line.Trim() -split '\|', 5
            $name = $parts[1]; $size = [int64]$parts[2]; $fileInfo = $parts[3]; $stdlibCount = [int]$parts[4]

            $depCheck = "PASS"; $depDetail = "static"; $stdlibCheck = "PASS"
            if ($fileInfo -notmatch 'statically linked' -and $fileInfo -notmatch 'not a dynamic executable') {
                $depCheck = "FAIL"; $depDetail = "dynamic"
            }
            if ($stdlibCount -gt 0) { $stdlibCheck = "FAIL" }

            Add-VerifyResult $name "wsl-x64" $depCheck $depDetail $stdlibCheck $size
            if (($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')) { $wvPass++ } else { $wvFail++ }

            if ($VerboseOutput) {
                $ok = ($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $depCheck, $stdlibCheck, (Format-Size $size)) -ForegroundColor $color
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($wvFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$wvPass verified, $wvFail failed" $color
        }

        # ── WSL native-backend cross-emit + link + run ────────────────
        # Only meaningful when testing --backend native: cross-emits a
        # linux-x86_64 relocatable object from the *Windows* oscan.exe
        # (Cranelift's cross-codegen support — see src/backend/target.rs),
        # then links it under WSL against the linux-x86_64 freestanding
        # runtime by default, or the hosted runtime for the FFI fixtures'
        # explicit --libc objects. Both archives and mode-matched native
        # shims are built on demand via scripts/release_tools.py. This is
        # real linking/running (not just object emission), which
        # src/backend/link.rs itself cannot do yet for a non-host target
        # (no cross linker wired in). The freestanding path prefers musl
        # (matching scripts/release_tools.py's own default_cc_for_target
        # preference for linux-x86_64): glibc's system headers are not
        # freestanding-safe with l_os.h's macro-redirect approach (e.g.
        # <limits.h>'s transitive <features.h> conflicts with l_os.h's
        # strcpy/memcpy/etc. redefinitions), so a plain distro `gcc` can
        # fail here even where it happily builds the hosted archive.
        if ($Backend -ne "c") {
            Write-Phase "WSL Linux x64 ($Backend cross-link)"
            if ($VerboseOutput) { Write-Host ""; Write-Host "  ── WSL native-backend positive tests (linux-x86_64, explicit runtime modes) ──" -ForegroundColor Yellow }

            $archiveDirWin = "build\runtime-archives\linux-x86_64"
            $archiveDirWSL = "$wslDir/build/runtime-archives/linux-x86_64"
            $archivePath = Join-Path $archiveDirWin "libosc_runtime_freestanding.a"
            $archiveManifestPath = Join-Path $archiveDirWin "libosc_runtime_freestanding.json"
            $hostedArchivePath = Join-Path $archiveDirWin "libosc_runtime_hosted.a"
            $hostedArchiveManifestPath = Join-Path $archiveDirWin "libosc_runtime_hosted.json"
            $hostedCC = "gcc"

            if (-not (Test-Path $archiveDirWin)) { New-Item -ItemType Directory -Path $archiveDirWin -Force | Out-Null }
            $freestandingSourceTime = (Get-ChildItem "runtime\osc_runtime*.c" | Sort-Object LastWriteTimeUtc -Descending | Select-Object -First 1).LastWriteTimeUtc
            if ((Test-Path $archivePath) -and ((Get-Item $archivePath).LastWriteTimeUtc -lt $freestandingSourceTime)) {
                Remove-Item -LiteralPath $archivePath, $archiveManifestPath -Force -ErrorAction SilentlyContinue
            }
            $hostedSourceTime = (Get-Item "runtime\osc_runtime.c").LastWriteTimeUtc
            if ((Test-Path $hostedArchivePath) -and ((Get-Item $hostedArchivePath).LastWriteTimeUtc -lt $hostedSourceTime)) {
                Remove-Item -LiteralPath $hostedArchivePath, $hostedArchiveManifestPath -Force -ErrorAction SilentlyContinue
            }
            $freestandingCC = $null
            if ((Test-Path $archivePath) -and (Test-Path $archiveManifestPath)) {
                try {
                    $recordedCC = (Get-Content -LiteralPath $archiveManifestPath -Raw | ConvertFrom-Json).cc
                    if ($recordedCC -and (Test-WSLCommand $recordedCC)) {
                        $freestandingCC = $recordedCC
                    }
                } catch {
                    Remove-Item -LiteralPath $archivePath, $archiveManifestPath -Force -ErrorAction SilentlyContinue
                }
            }
            if (-not $freestandingCC) {
                $freestandingCandidates = @()
                if (Test-WSLCommand "x86_64-linux-musl-gcc") {
                    $freestandingCandidates += "x86_64-linux-musl-gcc"
                }
                $freestandingCandidates += "gcc"
                foreach ($candidateCC in $freestandingCandidates) {
                    Remove-Item -LiteralPath $archivePath, $archiveManifestPath -Force -ErrorAction SilentlyContinue
                    wsl bash -c "cd '$wslDir' && python3 scripts/release_tools.py build-runtime-archive --mode freestanding --target linux-x86_64 --cc $candidateCC --ar ar --out-dir '$archiveDirWSL'" 2>&1 | Out-Null
                    if ((Test-Path $archivePath) -and (Test-Path $archiveManifestPath)) {
                        $freestandingCC = $candidateCC
                        break
                    }
                }
            }
            if (-not (Test-Path $hostedArchivePath)) {
                wsl bash -c "cd '$wslDir' && python3 scripts/release_tools.py build-runtime-archive --mode hosted --target linux-x86_64 --cc $hostedCC --ar ar --out-dir '$archiveDirWSL'" 2>&1 | Out-Null
            }
            $shimPath = if ($freestandingCC) {
                Join-Path $archiveDirWin "osc_native_shim.freestanding.$freestandingCC.o"
            } else {
                $null
            }
            $hostedShimPath = Join-Path $archiveDirWin "osc_native_shim.hosted.$hostedCC.o"
            $shimSrcTime = (Get-Item "runtime\osc_native_shim.c").LastWriteTimeUtc
            $needShim = $shimPath -and ((-not (Test-Path $shimPath)) -or ((Get-Item $shimPath).LastWriteTimeUtc -lt $shimSrcTime))
            if ($needShim) {
                wsl bash -c "cd '$wslDir' && $freestandingCC -std=gnu11 -ffreestanding -w -Os -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Iruntime -c runtime/osc_native_shim.c -o '$archiveDirWSL/osc_native_shim.freestanding.$freestandingCC.o'" 2>&1 | Out-Null
            }
            $needHostedShim = (-not (Test-Path $hostedShimPath)) -or ((Get-Item $hostedShimPath).LastWriteTimeUtc -lt $shimSrcTime)
            if ($needHostedShim) {
                wsl bash -c "cd '$wslDir' && $hostedCC -std=c99 -O2 -w -ffunction-sections -fdata-sections -Iruntime -c runtime/osc_native_shim.c -o '$archiveDirWSL/osc_native_shim.hosted.$hostedCC.o'" 2>&1 | Out-Null
            }

            if (-not $freestandingCC -or -not (Test-Path $archivePath) -or -not (Test-Path $shimPath) -or -not (Test-Path $hostedArchivePath) -or -not (Test-Path $hostedShimPath)) {
                Write-PhaseResult "skipped (could not build mode-matched linux-x86_64 runtime archives/shims under WSL)" Yellow
            } else {
                $nDir = "tests\build\wsl-native"
                if (-not (Test-Path $nDir)) { New-Item -ItemType Directory -Path $nDir -Force | Out-Null }

                # Cross-emit every positive test's object on the Windows side
                # (fast — pure Cranelift codegen, no linker involved yet).
                $objResults = foreach ($oscFile in (Get-ChildItem "tests\positive\*.osc")) {
                    $name = $oscFile.BaseName
                    $objArgs = @($oscFile.FullName)
                    if ($name -match '^ffi') { $objArgs += '--libc' }
                    $objArgs += @('--backend', 'native', '--native-target', 'linux-x86_64', '-o', "$nDir\$name.o")
                    & $oscan @objArgs 2>"$nDir\$name.objerr"
                    [PSCustomObject]@{ Name = $name; Success = ($LASTEXITCODE -eq 0) }
                }

                # One batched WSL call links + runs every object that was
                # successfully cross-emitted (mirrors the C-backend WSL
                # phase's single-call batching above). Each object is linked
                # with the same explicit runtime mode used to emit it.
                $nBash = @("#!/bin/bash", "cd '$wslDir'")
                foreach ($r in $objResults) {
                    $n = $r.Name
                    if (-not $r.Success) {
                        $nBash += "echo `"FAIL|$n|native object generation error`""
                        continue
                    }
                    $obj = "tests/build/wsl-native/$n.o"
                    $exe = "tests/build/wsl-native/${n}_native"
                    if ($n -match '^ffi') {
                        $linkCommand = "$hostedCC '$obj' '$archiveDirWSL/osc_native_shim.hosted.$hostedCC.o' '$archiveDirWSL/libosc_runtime_hosted.a' -Wl,--gc-sections -no-pie -lm -o '$exe'"
                    } else {
                        $linkCommand = "$freestandingCC '$obj' '$archiveDirWSL/osc_native_shim.freestanding.$freestandingCC.o' '$archiveDirWSL/libosc_runtime_freestanding.a' -nostdlib -static -Wl,--gc-sections,--build-id=none -o '$exe'"
                    }
                    $nBash += "if $linkCommand 2>tests/build/wsl-native/$n.linkerr; then"
                    $nBash += "  timeout --kill-after=5s ${wslTestTimeoutSeconds}s ./'$exe' > tests/build/wsl-native/$n.out 2>&1"
                    $nBash += "  rc=`$?"
                    $nBash += "  if [ `$rc -eq 124 ] || [ `$rc -eq 137 ]; then echo `"FAIL|$n|timed out after ${wslTestTimeoutSeconds}s`"; else echo `"OK|$n|`$rc`"; fi"
                    $nBash += "else"
                    $nBash += "  echo `"FAIL|$n|link error`""
                    $nBash += "fi"
                }
                Set-Content "tests\build\wsl-native\_wsl_native_batch.sh" ($nBash -join "`n") -NoNewline
                $nBatchOutput = wsl bash -c "cd '$wslDir' && bash tests/build/wsl-native/_wsl_native_batch.sh" 2>&1 | Out-String
                $nBatchExitCode = $LASTEXITCODE

                # Read the same tests/expected_exit/<name>.expected files the
                # Windows oracle harness (Invoke-OracleBackendCase) uses, so a
                # deliberately-nonzero-exit test like result_main_exit_code
                # isn't forced through a hardcoded "must exit 0" assumption.
                # Default is 0, matching Invoke-OracleBackendCase.
                $expectedExitCodes = @{}
                foreach ($n in @($objResults | ForEach-Object { $_.Name })) {
                    $exitFile = "tests\expected_exit\$n.expected"
                    if (Test-Path $exitFile) {
                        $expectedExitCodes[$n] = [int](Get-Content -LiteralPath $exitFile -Raw).Trim()
                    }
                }

                # Resolve-WslNativeBatchRecords is what actually enforces
                # correctness of this single batched WSL call: a program
                # exit code that doesn't match its declared expected exit
                # (0 by default) fails even when stdout later matches, a
                # nonzero exit from the wsl invocation itself is surfaced
                # (not silently swallowed as "0 passed, 0 failed"), and
                # every attempted name is guaranteed exactly one record —
                # missing, duplicate, out-of-set, or malformed records are
                # all rejected rather than accepted.
                $attemptedNames = @($objResults | ForEach-Object { $_.Name })
                $batchResult = Resolve-WslNativeBatchRecords $nBatchOutput $nBatchExitCode $attemptedNames $expectedExitCodes
                foreach ($err in $batchResult.Errors) {
                    Write-Host "    wsl native batch: $err" -ForegroundColor Red
                }

                $nPass = 0; $nFail = 0
                foreach ($record in $batchResult.Records) {
                    $name = $record.Name

                    if ($record.Status -eq 'FAIL') {
                        Add-TestResult $name "wsl-x64-$Backend" "positive" "FAIL" $record.Detail
                        $nFail++; continue
                    }

                    # The freestanding archive has real TLS support (BearSSL
                    # on Linux), matching the freestanding C oracle, so no
                    # unconditional hosted-mode fallback is needed — but a
                    # handful of builtins (canvas/clipboard defaults,
                    # tls_fetch's network dependency) are legitimately
                    # reduced-capability outside Windows even here; see
                    # Test-ExpectedOutputMatch's comment in backend_oracle.ps1.
                    $expectedFile = "tests\expected\$name.expected"
                    if (-not (Test-Path $expectedFile)) {
                        Add-TestResult $name "wsl-x64-$Backend" "positive" "FAIL" "missing expected file"
                        $nFail++; continue
                    }
                    $outFile = "tests\build\wsl-native\$name.out"
                    if (-not (Test-Path $outFile)) {
                        Add-TestResult $name "wsl-x64-$Backend" "positive" "FAIL" "no output file"
                        $nFail++; continue
                    }
                    $libcExpectedFile = "tests\expected_libc\$name.expected"
                    if (Test-ExpectedOutputMatch -ActualRaw (Get-Content $outFile -Raw) -PrimaryExpectedFile $expectedFile -FallbackExpectedFile $libcExpectedFile) {
                        Add-TestResult $name "wsl-x64-$Backend" "positive" "PASS" ""
                        $nPass++
                    } else {
                        Add-TestResult $name "wsl-x64-$Backend" "positive" "FAIL" "output mismatch"
                        $nFail++
                    }
                }
                $color = if ($nFail -gt 0) { "Red" } else { "Green" }
                Write-PhaseResult "$nPass passed, $nFail failed" $color
                Write-PhaseFailures "wsl-x64-$Backend"

                # Freestanding verification (Test-LinuxFreestanding): every
                # produced wsl-native binary must be statically linked with
                # no libc/glibc symbols, same bar as the C-backend WSL exes.
                Write-Phase "Verifying (WSL $Backend)"
                $nvPass = 0; $nvFail = 0
                foreach ($r in $objResults) {
                    if (-not $r.Success) { continue }
                    $n = $r.Name
                    if ($n -match '^ffi') { continue }
                    $wslExeRel = "tests/build/wsl-native/${n}_native"
                    if (-not (Test-Path "tests\build\wsl-native\${n}_native")) { continue }
                    $lf = Test-LinuxFreestanding $wslExeRel $wslDir
                    Add-VerifyResult $n "wsl-x64-$Backend" $lf.DepCheck $lf.DepDetail $lf.StdlibCheck $lf.Size
                    if (($lf.DepCheck -ne 'FAIL') -and ($lf.StdlibCheck -ne 'FAIL')) { $nvPass++ } else { $nvFail++ }
                }
                $vcolor = if ($nvFail -gt 0) { "Red" } else { "Green" }
                Write-PhaseResult "$nvPass verified, $nvFail failed" $vcolor
            }
        }
    }
}

# ══════════════════════════════════════════════════════
# ── ARM64 Integration tests ──────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipARM) {
    if (-not (Test-WSLAvailable)) {
        Write-Phase "ARM64 (QEMU)"; Write-PhaseResult "skipped (WSL not available)" Yellow
    } elseif (-not (Test-WSLCommand "aarch64-linux-gnu-gcc") -or -not (Test-WSLCommand "qemu-aarch64")) {
        Write-Phase "ARM64 (QEMU)"; Write-PhaseResult "skipped (tools not found)" Yellow
    } else {
        $script:ARMAvail = $true
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        # Reuse .c files from WSL phase if available, otherwise transpile
        Write-Phase "ARM64 (QEMU, positive)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── ARM64 Positive tests (freestanding) ──" -ForegroundColor Yellow }

        $oscFiles = @(Get-ChildItem "tests\positive\*.osc")

        # Transpile any missing .c files (skip if WSL phase already generated them)
        $needTranspile = @($oscFiles | Where-Object { -not (Test-Path "tests\build\$($_.BaseName).c") })
        if ($needTranspile.Count -gt 0) {
            $needTranspile | ForEach-Object -Parallel {
                $name = $_.BaseName
                $root = $using:projectRoot
                $oscanExe = "$root\target\release\oscan.exe"
                if ($name -match '^ffi') {
                    & $oscanExe --libc $_.FullName -o "$root\tests\build\${name}.c" 2>$null
                } else {
                    & $oscanExe $_.FullName -o "$root\tests\build\${name}.c" 2>$null
                }
            } -ThrottleLimit $throttle
        }

        # Generate batch bash script for ARM64 compile + run
        $bashLines = @("#!/bin/bash", "cd '$wslDir'")
        foreach ($bcFile in $oscFiles) {
            $n = $bcFile.BaseName
            if (-not (Test-Path "tests\build\$n.c")) {
                $bashLines += "echo 'FAIL|$n|transpile error'"
                continue
            }
            if ($n -match '^ffi') {
                $bashLines += "aarch64-linux-gnu-gcc -std=c99 -static tests/build/$n.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${n}_arm -lm 2>/dev/null"
            } else {
                $bashLines += "aarch64-linux-gnu-gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$n.c -Iruntime -Ideps/laststanding -o tests/build/${n}_arm 2>/dev/null"
            }
            $bashLines += "if [ `$? -ne 0 ]; then echo 'FAIL|$n|cross-compile error'; else"
            $bashLines += "  qemu-aarch64 ./tests/build/${n}_arm > tests/build/${n}_arm.out 2>&1; echo `"OK|$n|`$?`"; fi"
        }

        Set-Content "tests\build\_arm_batch.sh" ($bashLines -join "`n") -NoNewline
        $batchOutput = wsl bash -c "cd '$wslDir' && bash tests/build/_arm_batch.sh" 2>&1 | Out-String

        # Parse results
        $armPass = 0; $armFail = 0
        foreach ($line in ($batchOutput -split "`n" | Where-Object { $_ -match '^(OK|FAIL)\|' })) {
            $parts = $line.Trim() -split '\|', 3
            $status = $parts[0]; $name = $parts[1]; $detail = $parts[2]

            if ($status -eq 'FAIL') {
                Add-TestResult $name "arm64" "positive" "FAIL" $detail
                $armFail++; continue
            }

            $expectedFile = "tests\expected\$name.expected"
            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "arm64" "positive" "FAIL" "missing expected file"
                $armFail++; continue
            }
            $outFile = "tests\build\${name}_arm.out"
            if (-not (Test-Path $outFile)) {
                Add-TestResult $name "arm64" "positive" "FAIL" "no output file"
                $armFail++; continue
            }
            # img_load/svg_load/tt_load decoding and the l_tls.h-backed
            # tls_connect are only wired up for __x86_64__/_WIN32 targets
            # (see src/codegen.rs's emit_includes), so on aarch64 they
            # legitimately fall back to the same "not supported"/reduced
            # output already established for hosted/libc mode; canvas
            # alive-before-open defaults are also POSIX-vs-Windows-specific.
            # See Test-ExpectedOutputMatch's comment in backend_oracle.ps1.
            $libcExpectedFile = "tests\expected_libc\$name.expected"
            if (Test-ExpectedOutputMatch -ActualRaw (Get-Content $outFile -Raw) -PrimaryExpectedFile $expectedFile -FallbackExpectedFile $libcExpectedFile) {
                Add-TestResult $name "arm64" "positive" "PASS" ""
                $armPass++
            } else {
                Add-TestResult $name "arm64" "positive" "FAIL" "output mismatch"
                $armFail++
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($armFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$armPass passed, $armFail failed" $color
        }

        # ARM64 freestanding verification (batch: single WSL call)
        Write-Phase "Verifying (ARM64)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Freestanding verification (ARM64) ──" -ForegroundColor Yellow }
        $avPass = 0; $avFail = 0
        $vBash = @("#!/bin/bash", "cd '$wslDir'")
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            if ($name -match '^ffi') { continue }
            $exe = "tests/build/${name}_arm"
            $vBash += "if [ -f '$exe' ]; then"
            $vBash += "  sz=`$(stat -c%s '$exe' 2>/dev/null || echo 0)"
            $vBash += "  fi=`$(file '$exe')"
            $vBash += "  sc=`$(strings '$exe' | grep -ciE 'libc\.so|libm\.so|glibc|__libc_start_main' || true)"
            $vBash += "  echo `"VERIFY|$name|`$sz|`$fi|`$sc`""
            $vBash += "fi"
        }
        Set-Content "tests\build\_arm_verify.sh" ($vBash -join "`n") -NoNewline
        $verifyOutput = wsl bash -c "cd '$wslDir' && bash tests/build/_arm_verify.sh" 2>&1 | Out-String

        foreach ($line in ($verifyOutput -split "`n" | Where-Object { $_ -match '^VERIFY\|' })) {
            $parts = $line.Trim() -split '\|', 5
            $name = $parts[1]; $size = [int64]$parts[2]; $fileInfo = $parts[3]; $stdlibCount = [int]$parts[4]

            $depCheck = if ($fileInfo -match 'statically linked') { "PASS" } else { "FAIL" }
            $depDetail = if ($depCheck -eq "PASS") { "static" } else { "not static" }
            $stdlibCheck = if ($stdlibCount -gt 0) { "FAIL" } else { "PASS" }

            Add-VerifyResult $name "arm64" $depCheck $depDetail $stdlibCheck $size
            if (($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')) { $avPass++ } else { $avFail++ }

            if ($VerboseOutput) {
                $ok = ($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $depCheck, $stdlibCheck, (Format-Size $size)) -ForegroundColor $color
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($avFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$avPass verified, $avFail failed" $color
        }
    }
}

# ══════════════════════════════════════════════════════
# ── Examples compilation check ───────────────────────
# ══════════════════════════════════════════════════════

Write-Phase "Examples"
if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Examples compilation ──" -ForegroundColor Yellow }
$exPass = 0; $exFail = 0
foreach ($exFile in Get-ChildItem "examples\*.osc") {
    $name = $exFile.BaseName
    & $oscan $exFile.FullName -o "examples\${name}.exe" 2>$null
    if ($LASTEXITCODE -eq 0) {
        Add-TestResult $name "win-x64" "examples" "PASS" ""
        $exPass++
    } else {
        Add-TestResult $name "win-x64" "examples" "FAIL" "compile error"
        $exFail++
    }
}
if (-not $VerboseOutput) {
    $color = if ($exFail -gt 0) { "Red" } else { "Green" }
    Write-PhaseResult "$exPass compiled, $exFail failed" $color
}

# Graphics examples (freestanding only — may fail on platforms without display)
if (Test-Path "examples\gfx\*.osc") {
    foreach ($exFile in Get-ChildItem "examples\gfx\*.osc") {
        $name = $exFile.BaseName
        & $oscan $exFile.FullName -o "examples\gfx\${name}.exe" 2>$null
        if ($LASTEXITCODE -eq 0) {
            $exPass++
        }
        # Don't count gfx failures — they require a display
    }
}

# ══════════════════════════════════════════════════════
# ── Summary ──────────────────────────────────────────
# ══════════════════════════════════════════════════════

$allPassed = Show-Summary

if ($allPassed) { exit 0 } else { exit 1 }
