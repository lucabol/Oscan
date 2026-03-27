#!/usr/bin/env pwsh
# Local test runner — mirrors CI behavior
# Usage: .\test.ps1 [-SkipBuild] [-SkipUnit] [-SkipIntegration] [-Verbose]

param(
    [switch]$SkipBuild,
    [switch]$SkipUnit,
    [switch]$SkipIntegration
)

$ErrorActionPreference = "Continue"
$script:TotalPass = 0
$script:TotalFail = 0

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

# ── Build ──────────────────────────────────────────────
if (-not $SkipBuild) {
    Write-Host "`n━━━ Building ━━━" -ForegroundColor Cyan
    cargo build --release 2>&1
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
    cargo test 2>&1
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

# ── Summary ────────────────────────────────────────────
Write-Host "`n━━━ Summary ━━━" -ForegroundColor Cyan
Write-Host "Integration: $($script:TotalPass) passed, $($script:TotalFail) failed"
if ($script:TotalFail -gt 0) {
    Write-Host "SOME TESTS FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "ALL TESTS PASSED" -ForegroundColor Green
    exit 0
}
