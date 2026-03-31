#!/usr/bin/env pwsh
# Oscan Test Runner — quiet by default, verbose on --VerboseOutput or failure
# Usage: .\test.ps1 [-SkipBuild] [-SkipUnit] [-SkipIntegration] [-SkipWSL] [-SkipARM] [-SkipLibc] [-VerboseOutput]

param(
    [switch]$SkipBuild,
    [switch]$SkipUnit,
    [switch]$SkipIntegration,
    [switch]$SkipWSL,
    [switch]$SkipARM,
    [switch]$SkipLibc,
    [switch]$VerboseOutput,
    [switch]$SourceOnly  # When set, only define functions (for dot-sourcing by CI)
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

    # Socket/UDP tests legitimately need WS2_32.dll
    $testName = [System.IO.Path]::GetFileNameWithoutExtension($exePath)
    $allowPattern = if ($testName -match 'socket|udp') { '^(?i)(KERNEL32|WS2_32)\.dll$' } else { '^(?i)KERNEL32\.dll$' }

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

    $throttle = [Math]::Max(1, [Environment]::ProcessorCount)

    # Positive tests (parallel compile + run)
    Write-Phase "Windows x64 (positive)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (freestanding) ──" -ForegroundColor Yellow }
    $secPass = 0; $secFail = 0
    $projectRoot = (Get-Location).Path
    $parallelResults = Get-ChildItem "tests\positive\*.osc" | ForEach-Object -Parallel {
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

    foreach ($r in $parallelResults) {
        Add-TestResult $r.Name "win-x64" "positive" $r.Status $r.Detail
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
    # Windows libc (stdlib) tests — parallel
    if (-not $SkipLibc) {
    Write-Phase "Windows x64 (libc)"
    if ($VerboseOutput) { Write-Host ""; Write-Host "  ── Positive tests (libc/stdlib) ──" -ForegroundColor Yellow }
    $libcPass = 0; $libcFail = 0
    $libcResults = Get-ChildItem "tests\positive\*.osc" | ForEach-Object -Parallel {
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
        foreach ($t in $transpileResults) {
            if (-not $t.Success) {
                $bashLines += "echo 'FAIL|$($t.Name)|transpile error'"
                continue
            }
            $n = $t.Name
            if ($t.IsFfi) {
                $bashLines += "gcc -std=c99 tests/build/$n.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${n}_wsl -lm 2>/dev/null"
            } else {
                $bashLines += "gcc -std=gnu11 -ffreestanding -nostdlib -static -Oz -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Wl,--gc-sections,--build-id=none -s tests/build/$n.c -Iruntime -Ideps/laststanding -o tests/build/${n}_wsl 2>/dev/null"
            }
            $bashLines += "if [ `$? -ne 0 ]; then echo 'FAIL|$n|gcc compile error'; else"
            $bashLines += "  ./tests/build/${n}_wsl > tests/build/${n}_wsl.out 2>&1; echo `"OK|$n|`$?`"; fi"
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
            $actual = (Get-Content $outFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
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
            $actual = (Get-Content $outFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
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
