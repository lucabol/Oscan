# Focused regression test for osc_panic's hand-rolled freestanding message
# formatting (see runtime/osc_runtime.c's panic handler, which builds
# "panic at FILE:LINE: MESSAGE\n" without snprintf/l_snprintf).
#
# This intentionally does NOT use the tests/positive + tests/expected
# convention: __FILE__ inside osc_runtime.c's own OSC_PANIC call sites
# resolves to wherever that translation unit was compiled from, which is a
# per-invocation temp directory for the C backend (it rewrites
# runtime/osc_runtime.c into a fresh temp dir every compile) and the
# repository's checked-out path for the native backend's precompiled
# archive. Comparing that path exactly would be comparing incidental build
# environment/location, not runtime behavior, so this checks the stable
# parts instead: the process exit code, the stdout printed before the
# panic, and the "osc_runtime.c:<line>: <message>" suffix of the panic
# line (which must be byte-identical between backends and platforms, since
# it is the part osc_panic actually formats).
#
# Usage: .\panic_message.tests.ps1 -Oscan <oscan-binary> [-SkipWSL]

param(
    [Parameter(Mandatory = $true)][string]$Oscan,
    [switch]$SkipWSL
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $ScriptDir "backend_oracle.ps1")

$oscanPath = (Resolve-Path -LiteralPath $Oscan).Path
$repoRoot = Split-Path -Parent $ScriptDir
$workDir = Join-Path $ScriptDir "build\panic-message"
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

$source = @'
fn get_zero() -> i32 {
    0
}

fn! main() {
    println("before panic");
    let z: i32 = get_zero();
    let r: i32 = 10 / z;
    println("after: {r}");
}
'@
$sourcePath = Join-Path $workDir "panic_probe.osc"
Set-Content -LiteralPath $sourcePath -Value $source -NoNewline

# Matches the fixed part of osc_panic's output: "osc_runtime.c:<line>: <message>".
# The path prefix before "osc_runtime.c" is deliberately excluded -- see the
# file-level comment above.
$stablePattern = 'osc_runtime\.c:(\d+): (.*)$'

function Get-PanicStableSuffix {
    param([Parameter(Mandatory = $true)][string]$StderrText)
    $normalized = Normalize-OracleText $StderrText
    if ($normalized -notmatch $stablePattern) {
        throw "panic stderr did not contain the expected 'osc_runtime.c:<line>: <message>' shape. Actual: $normalized"
    }
    return [PSCustomObject]@{ Line = $Matches[1]; Message = $Matches[2] }
}

function Assert-PanicTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw "panic_message.tests.ps1: $Message" }
}

$failures = [System.Collections.Generic.List[string]]::new()

# ── Windows: C backend ──────────────────────────────────────────────
$cExe = Join-Path $workDir "panic_probe.c.exe"
Remove-Item -LiteralPath $cExe -Force -ErrorAction SilentlyContinue
& $oscanPath --backend c $sourcePath -o $cExe 2>$null
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $cExe)) {
    $failures.Add("C backend failed to compile the panic probe")
} else {
    $cRun = Invoke-OracleProcess -FilePath $cExe -WorkingDirectory $workDir
    if ($cRun.ExitCode -ne 1) { $failures.Add("C backend: expected exit code 1, got $($cRun.ExitCode)") }
    if ($cRun.Stdout -ne "before panic") { $failures.Add("C backend: expected stdout 'before panic', got '$($cRun.Stdout)'") }

    # ── Windows: native backend ──────────────────────────────────────
    $nativeExe = Join-Path $workDir "panic_probe.native.exe"
    Remove-Item -LiteralPath $nativeExe -Force -ErrorAction SilentlyContinue
    & $oscanPath --backend native $sourcePath -o $nativeExe 2>$null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $nativeExe)) {
        $failures.Add("native backend failed to compile the panic probe")
    } else {
        $nativeRun = Invoke-OracleProcess -FilePath $nativeExe -WorkingDirectory $workDir
        if ($nativeRun.ExitCode -ne 1) { $failures.Add("native backend: expected exit code 1, got $($nativeRun.ExitCode)") }
        if ($nativeRun.Stdout -ne "before panic") { $failures.Add("native backend: expected stdout 'before panic', got '$($nativeRun.Stdout)'") }

        try {
            $cSuffix = Get-PanicStableSuffix $cRun.Stderr
            $nativeSuffix = Get-PanicStableSuffix $nativeRun.Stderr
            if ($cSuffix.Line -ne $nativeSuffix.Line) {
                $failures.Add("panic line number differs: c='$($cSuffix.Line)' native='$($nativeSuffix.Line)'")
            }
            if ($cSuffix.Message -ne "i32 division by zero" -or $nativeSuffix.Message -ne "i32 division by zero") {
                $failures.Add("panic message differs from 'i32 division by zero': c='$($cSuffix.Message)' native='$($nativeSuffix.Message)'")
            }
        } catch {
            $failures.Add("$_")
        }
    }
}

Write-Host "Windows: C backend stderr:   $($cRun.Stderr)"
if ($nativeRun) { Write-Host "Windows: native backend stderr: $($nativeRun.Stderr)" }

# ── WSL: freestanding native cross-link ──────────────────────────────
if (-not $SkipWSL) {
    $wslAvailable = $false
    try {
        $null = & wsl -d Ubuntu -- bash -lc "command -v gcc" 2>$null
        $wslAvailable = ($LASTEXITCODE -eq 0)
    } catch { $wslAvailable = $false }

    if (-not $wslAvailable) {
        Write-Host "WSL: skipped (WSL/gcc not available)"
    } else {
        $linuxObj = Join-Path $workDir "panic_probe.linux.o"
        Remove-Item -LiteralPath $linuxObj -Force -ErrorAction SilentlyContinue
        & $oscanPath --backend native --native-target linux-x86_64 $sourcePath -o $linuxObj 2>$null
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $linuxObj)) {
            $failures.Add("failed to emit the linux-x86_64 native object for the panic probe")
        } else {
            $wslRoot = (& wsl -d Ubuntu -- wslpath -a ($repoRoot -replace '\\', '/')).Trim()
            $archiveDir = "$wslRoot/build/runtime-archives/linux-x86_64"
            $relObj = [System.IO.Path]::GetRelativePath($repoRoot, $linuxObj) -replace '\\', '/'
            $relExe = "tests/build/panic-message/panic_probe.linux.exe"
            $shimObj = "$archiveDir/osc_native_shim.freestanding.gcc.o"
            $runtimeArchive = "$archiveDir/libosc_runtime_freestanding.a"
            $buildShimLine = "if [ ! -f '$shimObj' ]; then gcc -std=gnu11 -ffreestanding -w -Os -fno-builtin -fno-asynchronous-unwind-tables -fomit-frame-pointer -ffunction-sections -fdata-sections -Iruntime -c runtime/osc_native_shim.c -o '$shimObj'; fi"
            $linkLine = "gcc '$relObj' '$shimObj' '$runtimeArchive' -nostdlib -static -Wl,--gc-sections,--build-id=none -o '$relExe'"
            $script = @(
                "set -e",
                "cd '$wslRoot'",
                $buildShimLine,
                $linkLine,
                "set +e",
                "'./$relExe' >'/tmp/panic_probe_stdout.$PID' 2>'/tmp/panic_probe_stderr.$PID'",
                "echo EXITCODE=`$?",
                "echo STDOUT_BEGIN",
                "cat '/tmp/panic_probe_stdout.$PID'",
                "echo STDOUT_END",
                "echo STDERR_BEGIN",
                "cat '/tmp/panic_probe_stderr.$PID'",
                "echo STDERR_END",
                "rm -f '/tmp/panic_probe_stdout.$PID' '/tmp/panic_probe_stderr.$PID'"
            ) -join "`n"
            $scriptPath = Join-Path $workDir "wsl_panic_probe.sh"
            [System.IO.File]::WriteAllText($scriptPath, $script, [System.Text.Encoding]::ASCII)
            $wslScriptPath = (& wsl -d Ubuntu -- wslpath -a ($scriptPath -replace '\\', '/')).Trim()
            $wslOut = (& wsl -d Ubuntu -- bash $wslScriptPath 2>&1 | Out-String)

            if ($wslOut -notmatch 'EXITCODE=(\d+)') {
                $failures.Add("WSL: could not determine exit code from batch output: $wslOut")
            } else {
                $wslExit = [int]$Matches[1]
                $wslStdout = ""
                $wslStderr = ""
                if ($wslOut -match '(?s)STDOUT_BEGIN\r?\n(.*?)\r?\nSTDOUT_END') { $wslStdout = Normalize-OracleText $Matches[1] }
                if ($wslOut -match '(?s)STDERR_BEGIN\r?\n(.*?)\r?\nSTDERR_END') { $wslStderr = Normalize-OracleText $Matches[1] }

                if ($wslExit -ne 1) { $failures.Add("WSL: expected exit code 1, got $wslExit") }
                if ($wslStdout -ne "before panic") { $failures.Add("WSL: expected stdout 'before panic', got '$wslStdout'") }
                try {
                    $wslSuffix = Get-PanicStableSuffix $wslStderr
                    if ($wslSuffix.Message -ne "i32 division by zero") {
                        $failures.Add("WSL: panic message differs from 'i32 division by zero': '$($wslSuffix.Message)'")
                    }
                    if ($cRun -and $wslSuffix.Line -ne (Get-PanicStableSuffix $cRun.Stderr).Line) {
                        $failures.Add("WSL panic line number differs from Windows C backend")
                    }
                } catch {
                    $failures.Add("WSL: $_")
                }
                Write-Host "WSL:     native backend stderr: $wslStderr"
            }
        }
    }
}

Write-Host ""
if ($failures.Count -gt 0) {
    Write-Host "panic_message tests FAILED:" -ForegroundColor Red
    foreach ($f in $failures) { Write-Host "  - $f" -ForegroundColor Red }
    exit 1
} else {
    Write-Host "panic_message tests PASSED ✓" -ForegroundColor Green
    exit 0
}
