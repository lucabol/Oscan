#!/usr/bin/env pwsh
# Oscan Test Runner — quiet by default, verbose on --VerboseOutput or failure
# Usage: .\test.ps1 [-SkipBuild] [-SkipUnit] [-SkipIntegration] [-SkipWSL] [-SkipARM] [-VerboseOutput]

param(
    [switch]$SkipBuild,
    [switch]$SkipUnit,
    [switch]$SkipIntegration,
    [switch]$SkipWSL,
    [switch]$SkipARM,
    [switch]$VerboseOutput
)

$ErrorActionPreference = "Continue"

# ── Result tracking ────────────────────────────────────
$script:Results = [System.Collections.ArrayList]::new()
$script:VerifyResults = [System.Collections.ArrayList]::new()
$script:Sizes = @{}
$script:BuildOk = $false
$script:UnitOk = $false
$script:UnitCount = 0
$script:WSLAvail = $false
$script:ARMAvail = $false

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

    $dumpbin = Find-Dumpbin
    if ($dumpbin) {
        $raw = & $dumpbin /nologo /dependents $exePath 2>$null
        $deps = $raw | Where-Object { $_ -match '^\s+\S+\.dll\s*$' } | ForEach-Object { $_.Trim() }
        $badDeps = $deps | Where-Object { $_ -notmatch '^(?i)KERNEL32\.dll$' }
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
    Write-Host "━━━ Oscan Test Suite ━━━" -ForegroundColor Cyan
    Write-Host ""

    # Build & Unit status
    $bSt = if ($SkipBuild) { "skipped" } elseif ($script:BuildOk) { "OK" } else { "FAIL" }
    $bCol = if ($bSt -eq "FAIL") { "Red" } else { "Green" }
    Write-Host ("  Build .................. {0}" -f $bSt) -ForegroundColor $bCol

    if ($SkipUnit) {
        Write-Host "  Unit tests ............. skipped" -ForegroundColor Yellow
    } elseif ($script:UnitOk) {
        Write-Host ("  Unit tests ({0,3}) ....... OK" -f $script:UnitCount) -ForegroundColor Green
    } else {
        Write-Host "  Unit tests ............. FAIL" -ForegroundColor Red
    }
    Write-Host ""

    # Integration table
    $archs = @(
        @{ Label = "Windows x64";   Arch = "win-x64";     Mode = "freestanding"; Skip = $SkipIntegration }
        @{ Label = "WSL Linux x64"; Arch = "wsl-x64";     Mode = "freestanding"; Skip = $SkipWSL -or -not $script:WSLAvail }
        @{ Label = "ARM64 (QEMU)";  Arch = "arm64";       Mode = "freestanding"; Skip = $SkipARM -or -not $script:ARMAvail }
        @{ Label = "Negative";      Arch = "win-x64-neg"; Mode = "—";            Skip = $SkipIntegration }
    )

    Write-Host "  Integration Tests:" -ForegroundColor White
    Write-Host ("  {0,-18} {1,6} {2,6} {3,6}  {4}" -f "Arch", "Pass", "Fail", "Skip", "Mode")
    Write-Host ("  {0,-18} {1,6} {2,6} {3,6}  {4}" -f ("─" * 18), ("─" * 6), ("─" * 6), ("─" * 6), ("─" * 14))

    foreach ($a in $archs) {
        $archResults = @($script:Results | Where-Object { $_.Arch -eq $a.Arch })
        if ($a.Skip -or $archResults.Count -eq 0) {
            Write-Host ("  {0,-18} {1,6} {2,6} {3,6}  {4}" -f $a.Label, "—", "—", "—", "skipped") -ForegroundColor Yellow
            continue
        }
        $p = @($archResults | Where-Object { $_.Status -eq 'PASS' }).Count
        $f = @($archResults | Where-Object { $_.Status -eq 'FAIL' }).Count
        $s = @($archResults | Where-Object { $_.Status -eq 'SKIP' }).Count
        $color = if ($f -gt 0) { "Red" } else { "Green" }
        Write-Host ("  {0,-18} {1,6} {2,6} {3,6}  {4}" -f $a.Label, $p, $f, $s, $a.Mode) -ForegroundColor $color
    }
    Write-Host ""

    # Freestanding verification tables (per-arch)
    $verifyArchs = @("win-x64", "wsl-x64", "arm64")
    $archLabels = @{ "win-x64" = "Windows x64"; "wsl-x64" = "WSL Linux x64"; "arm64" = "ARM64" }
    foreach ($va in $verifyArchs) {
        $vr = @($script:VerifyResults | Where-Object { $_.Arch -eq $va })
        if ($vr.Count -eq 0) { continue }
        Write-Host ("  Freestanding Verification ({0}):" -f $archLabels[$va]) -ForegroundColor White
        Write-Host ("  {0,-28} {1,10} {2,-18} {3,-8}" -f "Binary", "Size", "Dependencies", "Stdlib")
        Write-Host ("  {0,-28} {1,10} {2,-18} {3,-8}" -f ("─" * 28), ("─" * 10), ("─" * 18), ("─" * 8))
        foreach ($v in $vr) {
            $ok = ($v.DepCheck -ne 'FAIL') -and ($v.StdlibCheck -ne 'FAIL')
            $color = if ($ok) { "Green" } else { "Red" }
            $depStr = if ($v.DepDetail.Length -gt 18) { $v.DepDetail.Substring(0, 15) + "..." } else { $v.DepDetail }
            Write-Host ("  {0,-28} {1,10} {2,-18} {3,-8}" -f $v.Name, (Format-Size $v.Size), $depStr, $v.StdlibCheck) -ForegroundColor $color
        }
        Write-Host ""
    }

    # Cross-architecture size comparison (only if 2+ archs have data)
    $archsWithData = $verifyArchs | Where-Object { @($script:VerifyResults | Where-Object { $_.Arch -eq $_ }).Count -gt 0 }
    if ($archsWithData.Count -ge 2) {
        $testNames = $script:VerifyResults | Select-Object -ExpandProperty Name | Sort-Object -Unique
        Write-Host "  Binary Size Comparison (freestanding):" -ForegroundColor White
        $hdr = "  {0,-24}" -f "Test"
        foreach ($aa in $archsWithData) { $hdr += " {0,12}" -f $archLabels[$aa] }
        Write-Host $hdr
        $sep = "  {0,-24}" -f ("─" * 24)
        foreach ($aa in $archsWithData) { $sep += " {0,12}" -f ("─" * 12) }
        Write-Host $sep
        foreach ($tn in $testNames) {
            $line = "  {0,-24}" -f $tn
            foreach ($aa in $archsWithData) {
                $s = $script:Sizes["${tn}_${aa}"]
                $line += " {0,12}" -f (Format-Size $s)
            }
            Write-Host $line
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

if (-not $SkipBuild) {
    if ($VerboseOutput) { Write-Host "`n━━━ Building ━━━" -ForegroundColor Cyan }
    $buildOutput = cargo build --release 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "BUILD FAILED" -ForegroundColor Red
        $buildOutput | ForEach-Object { Write-Host "  $_" }
        exit 1
    }
    $script:BuildOk = $true
    if ($VerboseOutput) { Write-Host "Build OK" -ForegroundColor Green }
} else {
    $script:BuildOk = $true
}

$oscan = ".\target\release\oscan.exe"
if (-not (Test-Path $oscan)) {
    Write-Host "Error: $oscan not found. Run without -SkipBuild first." -ForegroundColor Red
    exit 1
}

# ══════════════════════════════════════════════════════
# ── Unit tests ────────────────────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipUnit) {
    if ($VerboseOutput) { Write-Host "`n━━━ Unit Tests ━━━" -ForegroundColor Cyan }
    $unitOutput = cargo test 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "UNIT TESTS FAILED" -ForegroundColor Red
        $unitOutput | ForEach-Object { Write-Host "  $_" }
        exit 1
    }
    $countLine = $unitOutput | Where-Object { $_ -match 'test result: ok\. (\d+) passed' } | Select-Object -Last 1
    if ($countLine -match '(\d+) passed') { $script:UnitCount = [int]$Matches[1] }
    $script:UnitOk = $true
    if ($VerboseOutput) { Write-Host "Unit tests OK ($($script:UnitCount) passed)" -ForegroundColor Green }
}

# ══════════════════════════════════════════════════════
# ── Integration tests (Windows) ──────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipIntegration) {
    if ($VerboseOutput) { Write-Host "`n━━━ Integration Tests ━━━" -ForegroundColor Cyan }

    if (-not (Test-Path "tests\build")) {
        New-Item -ItemType Directory -Path "tests\build" | Out-Null
    }

    # Positive tests
    if ($VerboseOutput) { Write-Host "`n── Positive tests (freestanding) ──" -ForegroundColor Yellow }
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName
        $expectedFile = "tests\expected\$name.expected"

        if (-not (Test-Path $expectedFile)) {
            Add-TestResult $name "win-x64" "positive" "FAIL" "missing expected file"
            continue
        }

        if ($name -match '^ffi') {
            & $oscan --libc $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        } else {
            & $oscan $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        }
        if ($LASTEXITCODE -ne 0) {
            Add-TestResult $name "win-x64" "positive" "FAIL" "compile error"
            continue
        }

        $actual = & ".\tests\build\$name.exe" 2>&1 | Out-String
        $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
        $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actual -eq $expected) {
            Add-TestResult $name "win-x64" "positive" "PASS" ""
        } else {
            Add-TestResult $name "win-x64" "positive" "FAIL" "output mismatch"
        }
    }

    # Negative tests
    if ($VerboseOutput) { Write-Host "`n── Negative tests ──" -ForegroundColor Yellow }
    foreach ($bcFile in Get-ChildItem "tests\negative\*.osc") {
        $name = $bcFile.BaseName
        & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
        if ($LASTEXITCODE -eq 0) {
            Add-TestResult $name "win-x64-neg" "negative" "FAIL" "should have been rejected"
        } else {
            Add-TestResult $name "win-x64-neg" "negative" "PASS" "correctly rejected"
        }
    }

    # Cleanup .obj files
    Get-ChildItem "*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    Get-ChildItem "tests\build\*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue

    # Windows freestanding verification
    if ($VerboseOutput) { Write-Host "`n── Freestanding verification (Windows) ──" -ForegroundColor Yellow }
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName
        if ($name -match '^ffi') { continue }
        $exe = "tests\build\$name.exe"
        if (-not (Test-Path $exe)) { continue }

        $r = Test-WindowsFreestanding $exe
        Add-VerifyResult $name "win-x64" $r.DepCheck $r.DepDetail $r.StdlibCheck $r.Size

        if ($VerboseOutput) {
            $ok = ($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')
            $color = if ($ok) { "Green" } else { "Red" }
            Write-Host ("  {0}: {1}  deps={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $r.DepCheck, $r.StdlibCheck, (Format-Size $r.Size)) -ForegroundColor $color
        }
    }
}

# ══════════════════════════════════════════════════════
# ── WSL Integration tests ────────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipWSL) {
    if ($VerboseOutput) { Write-Host "`n━━━ WSL Integration Tests ━━━" -ForegroundColor Cyan }

    if (-not (Test-WSLAvailable)) {
        if ($VerboseOutput) { Write-Host "WSL not available — skipping." -ForegroundColor Yellow }
    } elseif (-not (Test-WSLCommand "gcc")) {
        if ($VerboseOutput) { Write-Host "gcc not found in WSL — skipping." -ForegroundColor Yellow }
    } else {
        $script:WSLAvail = $true
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        if ($VerboseOutput) { Write-Host "`n── WSL Positive tests (freestanding) ──" -ForegroundColor Yellow }
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "missing expected file"
                continue
            }

            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "wsl-x64" "positive" "FAIL" "transpile error"; continue }
                wsl bash -c "cd '$wslDir' && gcc -std=c99 tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "wsl-x64" "positive" "FAIL" "transpile error"; continue }
                wsl bash -c "cd '$wslDir' && gcc -std=gnu11 -ffreestanding -nostdlib -static tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl" 2>&1 | Out-Null
            }
            if ($LASTEXITCODE -ne 0) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "gcc compile error"
                continue
            }

            $actual = wsl bash -c "cd '$wslDir' && ./tests/build/${name}_wsl" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
                Add-TestResult $name "wsl-x64" "positive" "PASS" ""
            } else {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "output mismatch"
            }
        }

        # WSL freestanding verification
        if ($VerboseOutput) { Write-Host "`n── Freestanding verification (WSL) ──" -ForegroundColor Yellow }
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            if ($name -match '^ffi') { continue }
            $wslExe = "tests/build/${name}_wsl"
            $exists = wsl bash -c "test -f '$wslDir/$wslExe' && echo yes || echo no" 2>&1 | Out-String
            if ($exists.Trim() -ne 'yes') { continue }

            $r = Test-LinuxFreestanding $wslExe $wslDir
            Add-VerifyResult $name "wsl-x64" $r.DepCheck $r.DepDetail $r.StdlibCheck $r.Size

            if ($VerboseOutput) {
                $ok = ($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $r.DepCheck, $r.StdlibCheck, (Format-Size $r.Size)) -ForegroundColor $color
            }
        }
    }
}

# ══════════════════════════════════════════════════════
# ── ARM64 Integration tests ──────────────────────────
# ══════════════════════════════════════════════════════

if (-not $SkipARM) {
    if ($VerboseOutput) { Write-Host "`n━━━ ARM64 Integration Tests ━━━" -ForegroundColor Cyan }

    if (-not (Test-WSLAvailable)) {
        if ($VerboseOutput) { Write-Host "WSL not available — skipping ARM64." -ForegroundColor Yellow }
    } elseif (-not (Test-WSLCommand "aarch64-linux-gnu-gcc") -or -not (Test-WSLCommand "qemu-aarch64")) {
        if ($VerboseOutput) { Write-Host "ARM64 tools not found — skipping." -ForegroundColor Yellow }
    } else {
        $script:ARMAvail = $true
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        if ($VerboseOutput) { Write-Host "`n── ARM64 Positive tests (freestanding) ──" -ForegroundColor Yellow }
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "arm64" "positive" "FAIL" "missing expected file"
                continue
            }

            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "arm64" "positive" "FAIL" "transpile error"; continue }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=c99 -static tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "arm64" "positive" "FAIL" "transpile error"; continue }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=gnu11 -ffreestanding -nostdlib -static tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm" 2>&1 | Out-Null
            }
            if ($LASTEXITCODE -ne 0) {
                Add-TestResult $name "arm64" "positive" "FAIL" "cross-compile error"
                continue
            }

            $actual = wsl bash -c "cd '$wslDir' && qemu-aarch64 ./tests/build/${name}_arm" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
                Add-TestResult $name "arm64" "positive" "PASS" ""
            } else {
                Add-TestResult $name "arm64" "positive" "FAIL" "output mismatch"
            }
        }

        # ARM64 freestanding verification
        if ($VerboseOutput) { Write-Host "`n── Freestanding verification (ARM64) ──" -ForegroundColor Yellow }
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            if ($name -match '^ffi') { continue }
            $armExe = "tests/build/${name}_arm"
            $exists = wsl bash -c "test -f '$wslDir/$armExe' && echo yes || echo no" 2>&1 | Out-String
            if ($exists.Trim() -ne 'yes') { continue }

            $fileInfo = wsl bash -c "file '$wslDir/$armExe'" 2>&1 | Out-String
            $sizeStr = wsl bash -c "stat -c%s '$wslDir/$armExe'" 2>&1 | Out-String
            $sizeNum = [int64]($sizeStr.Trim())

            $depCheck = if ($fileInfo -match 'statically linked') { "PASS" } else { "FAIL" }
            $depDetail = if ($depCheck -eq "PASS") { "static" } else { "not static" }

            $cnt = wsl bash -c "strings '$wslDir/$armExe' | grep -ciE 'libc\.so|libm\.so|glibc|__libc_start_main' || true" 2>&1 | Out-String
            $stdlibCheck = if ([int]($cnt.Trim()) -gt 0) { "FAIL" } else { "PASS" }

            Add-VerifyResult $name "arm64" $depCheck $depDetail $stdlibCheck $sizeNum

            if ($VerboseOutput) {
                $ok = ($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $depCheck, $stdlibCheck, (Format-Size $sizeNum)) -ForegroundColor $color
            }
        }
    }
}

# ══════════════════════════════════════════════════════
# ── Summary ──────────────────────────────────────────
# ══════════════════════════════════════════════════════

$allPassed = Show-Summary

if ($allPassed) { exit 0 } else { exit 1 }
