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

Write-Host "`n━━━ Oscan Test Suite ━━━`n" -ForegroundColor Cyan

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

    # Positive tests
    Write-Phase "Windows x64 (positive)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (freestanding) ──" -ForegroundColor Yellow }
    $secPass = 0; $secFail = 0; $secTotal = @(Get-ChildItem "tests\positive\*.osc").Count
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName
        $expectedFile = "tests\expected\$name.expected"

        if (-not (Test-Path $expectedFile)) {
            Add-TestResult $name "win-x64" "positive" "FAIL" "missing expected file"
            $secFail++; continue
        }

        if ($name -match '^ffi') {
            & $oscan --libc $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        } else {
            & $oscan $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        }
        if ($LASTEXITCODE -ne 0) {
            Add-TestResult $name "win-x64" "positive" "FAIL" "compile error"
            $secFail++; continue
        }

        $actual = & ".\tests\build\$name.exe" 2>&1 | Out-String
        $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
        $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actual -eq $expected) {
            Add-TestResult $name "win-x64" "positive" "PASS" ""
            $secPass++
        } else {
            Add-TestResult $name "win-x64" "positive" "FAIL" "output mismatch"
            $secFail++
        }
    }
    if (-not $VerboseOutput) {
        $color = if ($secFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$secPass passed, $secFail failed" $color
    }

    # Negative tests
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
        $exe = "tests\build\$name.exe"
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
    # Windows libc (stdlib) tests
    Write-Phase "Windows x64 (libc)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (libc/stdlib) ──" -ForegroundColor Yellow }
    $libcPass = 0; $libcFail = 0
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName

        # Use libc-specific expected output if available, else standard
        $libcExpected = "tests\expected_libc\$name.expected"
        $stdExpected  = "tests\expected\$name.expected"
        $expectedFile = if (Test-Path $libcExpected) { $libcExpected } else { $stdExpected }

        if (-not (Test-Path $expectedFile)) {
            Add-TestResult $name "win-x64-libc" "positive-libc" "FAIL" "missing expected file"
            $libcFail++; continue
        }

        & $oscan --libc $bcFile.FullName -o "tests\build\${name}_libc.exe" 2>$null
        if ($LASTEXITCODE -ne 0) {
            Add-TestResult $name "win-x64-libc" "positive-libc" "FAIL" "compile error"
            $libcFail++; continue
        }

        $actual = & ".\tests\build\${name}_libc.exe" 2>&1 | Out-String
        $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
        $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actual -eq $expected) {
            Add-TestResult $name "win-x64-libc" "positive-libc" "PASS" ""
            $libcPass++
        } else {
            Add-TestResult $name "win-x64-libc" "positive-libc" "FAIL" "output mismatch"
            $libcFail++
        }
    }
    if (-not $VerboseOutput) {
        $color = if ($libcFail -gt 0) { "Red" } else { "Green" }
        Write-PhaseResult "$libcPass passed, $libcFail failed" $color
    }

    # Cleanup libc .obj files
    Get-ChildItem "*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    Get-ChildItem "tests\build\*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
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

        Write-Phase "WSL Linux x64 (positive)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── WSL Positive tests (freestanding) ──" -ForegroundColor Yellow }
        $wslPass = 0; $wslFail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "missing expected file"
                $wslFail++; continue
            }

            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "wsl-x64" "positive" "FAIL" "transpile error"; $wslFail++; continue }
                wsl bash -c "cd '$wslDir' && gcc -std=c99 tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "wsl-x64" "positive" "FAIL" "transpile error"; $wslFail++; continue }
                wsl bash -c "cd '$wslDir' && gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl" 2>&1 | Out-Null
            }
            if ($LASTEXITCODE -ne 0) {
                Add-TestResult $name "wsl-x64" "positive" "FAIL" "gcc compile error"
                $wslFail++; continue
            }

            $actual = wsl bash -c "cd '$wslDir' && ./tests/build/${name}_wsl" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
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

        # WSL freestanding verification
        Write-Phase "Verifying (WSL)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Freestanding verification (WSL) ──" -ForegroundColor Yellow }
        $wvPass = 0; $wvFail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            if ($name -match '^ffi') { continue }
            $wslExe = "tests/build/${name}_wsl"
            $exists = wsl bash -c "test -f '$wslDir/$wslExe' && echo yes || echo no" 2>&1 | Out-String
            if ($exists.Trim() -ne 'yes') { continue }

            $r = Test-LinuxFreestanding $wslExe $wslDir
            Add-VerifyResult $name "wsl-x64" $r.DepCheck $r.DepDetail $r.StdlibCheck $r.Size
            if (($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')) { $wvPass++ } else { $wvFail++ }

            if ($VerboseOutput) {
                $ok = ($r.DepCheck -ne 'FAIL') -and ($r.StdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $r.DepCheck, $r.StdlibCheck, (Format-Size $r.Size)) -ForegroundColor $color
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($wvFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$wvPass verified, $wvFail failed" $color
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

        Write-Phase "ARM64 (QEMU, positive)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── ARM64 Positive tests (freestanding) ──" -ForegroundColor Yellow }
        $armPass = 0; $armFail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Add-TestResult $name "arm64" "positive" "FAIL" "missing expected file"
                $armFail++; continue
            }

            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "arm64" "positive" "FAIL" "transpile error"; $armFail++; continue }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=c99 -static tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) { Add-TestResult $name "arm64" "positive" "FAIL" "transpile error"; $armFail++; continue }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm" 2>&1 | Out-Null
            }
            if ($LASTEXITCODE -ne 0) {
                Add-TestResult $name "arm64" "positive" "FAIL" "cross-compile error"
                $armFail++; continue
            }

            $actual = wsl bash -c "cd '$wslDir' && qemu-aarch64 ./tests/build/${name}_arm" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
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

        # ARM64 freestanding verification
        Write-Phase "Verifying (ARM64)"
        if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Freestanding verification (ARM64) ──" -ForegroundColor Yellow }
        $avPass = 0; $avFail = 0
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
            if (($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')) { $avPass++ } else { $avFail++ }

            if ($VerboseOutput) {
                $ok = ($depCheck -ne 'FAIL') -and ($stdlibCheck -ne 'FAIL')
                $color = if ($ok) { "Green" } else { "Red" }
                Write-Host ("  {0}: {1}  link={2} stdlib={3} size={4}" -f $(if ($ok) {'PASS'} else {'FAIL'}), $name, $depCheck, $stdlibCheck, (Format-Size $sizeNum)) -ForegroundColor $color
            }
        }
        if (-not $VerboseOutput) {
            $color = if ($avFail -gt 0) { "Red" } else { "Green" }
            Write-PhaseResult "$avPass verified, $avFail failed" $color
        }
    }
}

# ══════════════════════════════════════════════════════
# ── Summary ──────────────────────────────────────────
# ══════════════════════════════════════════════════════

$allPassed = Show-Summary

if ($allPassed) { exit 0 } else { exit 1 }
