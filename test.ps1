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

$oscan = ".\target\release\oscan.exe"
if (-not (Test-Path $oscan)) {
    Write-Host "Error: $oscan not found. Run without -SkipBuild first." -ForegroundColor Red
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
    Write-Host "`n━━━ Integration Tests (freestanding) ━━━" -ForegroundColor Cyan

    if (-not (Test-Path "tests\build")) {
        New-Item -ItemType Directory -Path "tests\build" | Out-Null
    }

    # ── Positive tests (freestanding via oscan compiler) ──
    Write-Host "`n── Positive tests (freestanding) ──" -ForegroundColor Yellow
    $pass = 0; $fail = 0
    foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
        $name = $bcFile.BaseName
        $expectedFile = "tests\expected\$name.expected"

        if (-not (Test-Path $expectedFile)) {
            Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
            $fail++; continue
        }

        # FFI tests need libc
        if ($name -match '^ffi') {
            & $oscan --libc $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        } else {
            & $oscan $bcFile.FullName -o "tests\build\$name.exe" 2>$null
        }
        if ($LASTEXITCODE -ne 0) {
            Write-Host "  FAIL: $name — compile error" -ForegroundColor Red
            $fail++; continue
        }

        # Run and compare
        $actual = & ".\tests\build\$name.exe" 2>&1 | Out-String
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
    foreach ($bcFile in Get-ChildItem "tests\negative\*.osc") {
        $name = $bcFile.BaseName

        & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
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

        Write-Host "`n── WSL Positive tests (freestanding) ──" -ForegroundColor Yellow
        $pass = 0; $fail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
                $fail++; continue
            }

            # FFI tests need libc
            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                    $fail++; continue
                }
                wsl bash -c "cd '$wslDir' && gcc -std=c99 tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                    $fail++; continue
                }
                wsl bash -c "cd '$wslDir' && gcc -std=gnu11 -ffreestanding -nostdlib -static tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_wsl" 2>&1 | Out-Null
            }
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

        Write-Host "`n── ARM64 Positive tests (freestanding) ──" -ForegroundColor Yellow
        $pass = 0; $fail = 0
        foreach ($bcFile in Get-ChildItem "tests\positive\*.osc") {
            $name = $bcFile.BaseName
            $expectedFile = "tests\expected\$name.expected"

            if (-not (Test-Path $expectedFile)) {
                Write-Host "  FAIL: $name — missing expected file" -ForegroundColor Red
                $fail++; continue
            }

            # FFI tests need libc
            if ($name -match '^ffi') {
                & $oscan --libc $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                    $fail++; continue
                }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=c99 -static tests/build/$name.c runtime/osc_runtime.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm -lm" 2>&1 | Out-Null
            } else {
                & $oscan $bcFile.FullName -o "tests\build\$name.c" 2>$null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "  FAIL: $name — transpile error" -ForegroundColor Red
                    $fail++; continue
                }
                wsl bash -c "cd '$wslDir' && aarch64-linux-gnu-gcc -std=gnu11 -ffreestanding -nostdlib -static tests/build/$name.c -Iruntime -Ideps/laststanding -o tests/build/${name}_arm" 2>&1 | Out-Null
            }
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
