#!/usr/bin/env pwsh
# Local test runner — mirrors CI behavior
# Usage: .\test.ps1 [-SkipBuild] [-SkipUnit] [-SkipIntegration] [-SkipWSL] [-SkipARM] [-Verbose]

param(
    [switch]$SkipBuild,
    [switch]$SkipUnit,
    [switch]$SkipIntegration,
    [switch]$SkipWSL,
    [switch]$SkipARM
)

$ErrorActionPreference = "Continue"
$script:TotalPass = 0
$script:TotalFail = 0
$script:WSLPass = 0
$script:WSLFail = 0
$script:WSLSkipped = $false
$script:ARMPass = 0
$script:ARMFail = 0
$script:ARMSkipped = $false

# ── Detect C compiler ──────────────────────────────────
function Find-CCompiler {
    # gcc
    if (Get-Command gcc -ErrorAction SilentlyContinue) {
        return @{ Cmd = "gcc"; Type = "gcc" }
    }
    # clang
    if (Get-Command clang -ErrorAction SilentlyContinue) {
        return @{ Cmd = "clang"; Type = "gcc" }
    }
    # cl.exe on PATH
    if (Get-Command cl.exe -ErrorAction SilentlyContinue) {
        return @{ Cmd = "cl.exe"; Type = "msvc" }
    }
    # cl.exe via vswhere
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsPath = & $vswhere -latest -property installationPath 2>$null
        if ($vsPath) {
            $vcvars = "$vsPath\VC\Auxiliary\Build\vcvarsall.bat"
            if (Test-Path $vcvars) {
                return @{ Cmd = "cl.exe"; Type = "msvc"; VcVars = $vcvars }
            }
        }
    }
    return $null
}

function Compile-C-GCC($cc, $srcC, $outExe) {
    & $cc -std=c99 $srcC runtime/bc_runtime.c -Iruntime -o $outExe -lm 2>&1
    return $LASTEXITCODE -eq 0
}

function Compile-C-MSVC($compiler, $srcC, $outExe) {
    if ($compiler.VcVars) {
        $bat = [System.IO.Path]::GetTempFileName() + ".bat"
        @"
@echo off
call "$($compiler.VcVars)" x64 >nul 2>&1
cl.exe /nologo /std:c11 /Iruntime "$srcC" runtime\bc_runtime.c /Fe:"$outExe" /link >nul 2>&1
exit /b %ERRORLEVEL%
"@ | Set-Content $bat
        cmd /c $bat 2>&1 | Out-Null
        $ok = $LASTEXITCODE -eq 0
        Remove-Item $bat -ErrorAction SilentlyContinue
        return $ok
    } else {
        & cl.exe /nologo /std:c11 /Iruntime $srcC runtime\bc_runtime.c /Fe:"$outExe" /link 2>&1 | Out-Null
        return $LASTEXITCODE -eq 0
    }
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
    } catch {
        return $false
    }
}

function Test-WSLCommand($cmd) {
    $result = wsl bash -c "command -v $cmd" 2>&1 | ForEach-Object { "$_" }
    return ($LASTEXITCODE -eq 0)
}

# ── Build ──────────────────────────────────────────────
if (-not $SkipBuild) {
    Write-Host "`n━━━ Building ━━━" -ForegroundColor Cyan
    cargo build --release 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "BUILD FAILED" -ForegroundColor Red
        exit 1
    }
    Write-Host "Build OK" -ForegroundColor Green
}

$BABELC = ".\target\release\babelc.exe"
if (-not (Test-Path $BABELC)) {
    Write-Host "Error: $BABELC not found. Run without -SkipBuild first." -ForegroundColor Red
    exit 1
}

# ── Unit tests ─────────────────────────────────────────
if (-not $SkipUnit) {
    Write-Host "`n━━━ Unit Tests (cargo test) ━━━" -ForegroundColor Cyan
    cargo test 2>&1 | ForEach-Object { "$_" }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "UNIT TESTS FAILED" -ForegroundColor Red
        exit 1
    }
    Write-Host "Unit tests OK" -ForegroundColor Green
}

# ── Integration tests ──────────────────────────────────
if (-not $SkipIntegration) {
    $cc = Find-CCompiler
    if (-not $cc) {
        Write-Host "No C compiler found (gcc, clang, or cl.exe). Skipping integration tests." -ForegroundColor Yellow
        exit 0
    }
    Write-Host "`n━━━ Integration Tests (using $($cc.Cmd)) ━━━" -ForegroundColor Cyan

    if (-not (Test-Path "tests\build")) {
        New-Item -ItemType Directory -Path "tests\build" | Out-Null
    }

    # ── Positive tests ──
    Write-Host "`n── Positive tests ──" -ForegroundColor Yellow
    $pass = 0; $fail = 0
    foreach ($bcFile in Get-ChildItem "tests\positive\*.bc") {
        $name = $bcFile.BaseName
        $expectedFile = "tests\expected\$name.expected"

        if (-not (Test-Path $expectedFile)) {
            Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
            $fail++; continue
        }

        # Transpile
        & $BABELC $bcFile.FullName -o "tests\build\$name.c" 2>$null
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
            $fail++; continue
        }

        # Compile C
        $outExe = "tests\build\$name.exe"
        if ($cc.Type -eq "msvc") {
            $ok = Compile-C-MSVC $cc "tests\build\$name.c" $outExe
        } else {
            $ok = Compile-C-GCC $cc.Cmd "tests\build\$name.c" $outExe
        }
        if (-not $ok) {
            Write-Host "  FAIL: $name — C compile error" -ForegroundColor Red
            $fail++; continue
        }

        # Run and compare
        $actual = & $outExe 2>&1 | Out-String
        $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
        $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

        if ($actual -eq $expected) {
            Write-Host "  PASS: $name" -ForegroundColor Green
            $pass++
        } else {
            Write-Host "  FAIL: $name — output mismatch" -ForegroundColor Red
            $fail++
        }
    }
    Write-Host "Positive: $pass passed, $fail failed" -ForegroundColor $(if ($fail -gt 0) {"Red"} else {"Green"})
    $script:TotalPass += $pass; $script:TotalFail += $fail

    # ── Negative tests ──
    Write-Host "`n── Negative tests ──" -ForegroundColor Yellow
    $pass = 0; $fail = 0
    foreach ($bcFile in Get-ChildItem "tests\negative\*.bc") {
        $name = $bcFile.BaseName

        & $BABELC $bcFile.FullName -o "tests\build\$name.c" 2>$null
        if ($LASTEXITCODE -eq 0) {
            Write-Host "  FAIL: $name — should have been rejected" -ForegroundColor Red
            $fail++
        } else {
            Write-Host "  PASS: $name — correctly rejected" -ForegroundColor Green
            $pass++
        }
    }
    Write-Host "Negative: $pass passed, $fail failed" -ForegroundColor $(if ($fail -gt 0) {"Red"} else {"Green"})
    $script:TotalPass += $pass; $script:TotalFail += $fail

    # Cleanup .obj files left by MSVC
    Get-ChildItem "*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
    Get-ChildItem "tests\build\*.obj" -ErrorAction SilentlyContinue | Remove-Item -ErrorAction SilentlyContinue
}

# ── WSL Integration tests (Linux/GCC via WSL) ─────────
if (-not $SkipWSL) {
    Write-Host "`n━━━ WSL Integration Tests (Linux/GCC) ━━━" -ForegroundColor Cyan

    if (-not (Test-WSLAvailable)) {
        Write-Host "WSL not available — skipping. Install WSL: wsl --install" -ForegroundColor Yellow
        $script:WSLSkipped = $true
    } elseif (-not (Test-WSLCommand "gcc")) {
        Write-Host "gcc not found in WSL — skipping. Install: wsl sudo apt install gcc" -ForegroundColor Yellow
        $script:WSLSkipped = $true
    } else {
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        Write-Host "`n── WSL Positive tests ──" -ForegroundColor Yellow
        $pass = 0; $fail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.bc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
                $fail++; continue
            }

            # Transpile (Windows-native babelc.exe)
            & $BABELC $bcFile.FullName -o "tests\build\$name.c" 2>$null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                $fail++; continue
            }

            # Compile inside WSL with gcc
            wsl bash -c "cd '$wslDir' && gcc -std=c99 tests/build/$name.c runtime/bc_runtime.c -Iruntime -o tests/build/${name}_wsl -lm" 2>&1 | ForEach-Object { "$_" } | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  FAIL: $name — gcc compile error (WSL)" -ForegroundColor Red
                $fail++; continue
            }

            # Run inside WSL and compare
            $actual = wsl bash -c "cd '$wslDir' && ./tests/build/${name}_wsl" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
                Write-Host "  PASS: $name" -ForegroundColor Green
                $pass++
            } else {
                Write-Host "  FAIL: $name — output mismatch" -ForegroundColor Red
                $fail++
            }
        }
        Write-Host "WSL Positive: $pass passed, $fail failed" -ForegroundColor $(if ($fail -gt 0) {"Red"} else {"Green"})
        $script:WSLPass = $pass; $script:WSLFail = $fail
    }
}

# ── ARM64 Integration tests (QEMU via WSL) ────────────
if (-not $SkipARM) {
    Write-Host "`n━━━ ARM64 Integration Tests (QEMU via WSL) ━━━" -ForegroundColor Cyan

    if (-not (Test-WSLAvailable)) {
        Write-Host "WSL not available — skipping ARM64 tests." -ForegroundColor Yellow
        $script:ARMSkipped = $true
    } elseif (-not (Test-WSLCommand "aarch64-linux-gnu-gcc") -or -not (Test-WSLCommand "qemu-aarch64")) {
        Write-Host "ARM64 tools not found in WSL — skipping." -ForegroundColor Yellow
        Write-Host "  Install: wsl sudo apt install gcc-aarch64-linux-gnu qemu-user" -ForegroundColor Yellow
        $script:ARMSkipped = $true
    } else {
        $wslDir = Convert-ToWSLPath (Get-Location).Path

        if (-not (Test-Path "tests\build")) {
            New-Item -ItemType Directory -Path "tests\build" | Out-Null
        }

        Write-Host "`n── ARM64 Positive tests ──" -ForegroundColor Yellow
        $pass = 0; $fail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.bc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
                $fail++; continue
            }

            # Transpile (Windows-native babelc.exe)
            & $BABELC $bcFile.FullName -o "tests\build\$name.c" 2>$null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                $fail++; continue
            }

            # Cross-compile with aarch64-linux-gnu-gcc -static inside WSL
            wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=c99 -static tests/build/$name.c runtime/bc_runtime.c -Iruntime -o tests/build/${name}_arm -lm" 2>&1 | ForEach-Object { "$_" } | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Host "  FAIL: $name — cross-compile error (ARM64)" -ForegroundColor Red
                $fail++; continue
            }

            # Run via qemu-aarch64 inside WSL
            $actual = wsl bash -c "cd '$wslDir' && qemu-aarch64 ./tests/build/${name}_arm" 2>&1 | Out-String
            $actual = $actual.TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")
            $expected = (Get-Content $expectedFile -Raw).TrimEnd("`r`n").TrimEnd("`n").Replace("`r`n", "`n")

            if ($actual -eq $expected) {
                Write-Host "  PASS: $name (ARM64)" -ForegroundColor Green
                $pass++
            } else {
                Write-Host "  FAIL: $name — output mismatch (ARM64)" -ForegroundColor Red
                $fail++
            }
        }
        Write-Host "ARM64 Positive: $pass passed, $fail failed" -ForegroundColor $(if ($fail -gt 0) {"Red"} else {"Green"})
        $script:ARMPass = $pass; $script:ARMFail = $fail
    }
}

# ── Summary ────────────────────────────────────────────
Write-Host "`n━━━ Summary ━━━" -ForegroundColor Cyan
Write-Host "Native integration: $($script:TotalPass) passed, $($script:TotalFail) failed"

if (-not $SkipWSL) {
    if ($script:WSLSkipped) {
        Write-Host "WSL (Linux/GCC):    skipped (not available)" -ForegroundColor Yellow
    } else {
        Write-Host "WSL (Linux/GCC):    $($script:WSLPass) passed, $($script:WSLFail) failed"
    }
}
if (-not $SkipARM) {
    if ($script:ARMSkipped) {
        Write-Host "ARM64 (QEMU):       skipped (tools not installed)" -ForegroundColor Yellow
    } else {
        Write-Host "ARM64 (QEMU):       $($script:ARMPass) passed, $($script:ARMFail) failed"
    }
}

$totalFailed = $script:TotalFail + $script:WSLFail + $script:ARMFail
if ($totalFailed -gt 0) {
    Write-Host "SOME TESTS FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "ALL TESTS PASSED" -ForegroundColor Green
    exit 0
}
