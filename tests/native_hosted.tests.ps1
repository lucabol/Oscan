param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir
. (Join-Path $ScriptDir "backend_oracle.ps1")

function Assert-NativeHosted {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Get-ArtifactAscii {
    param([Parameter(Mandatory = $true)][string]$Path)
    return [System.Text.Encoding]::ASCII.GetString(
        [System.IO.File]::ReadAllBytes($Path)
    )
}

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$buildRoot = Join-Path $ScriptDir "build\native-hosted-mode"
$runRoot = Join-Path $buildRoot "runs"
[void](New-Item -ItemType Directory -Path $buildRoot -Force)

# Default native mode must remain runnable and free of CRT/libc markers.
$helloSource = Join-Path $ScriptDir "positive\hello_world.osc"
$helloExe = Join-Path $buildRoot "hello-freestanding$(Get-OracleExecutableSuffix)"
$freeCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @("--backend", "native", $helloSource, "-o", $helloExe) `
    -WorkingDirectory $RepoRoot
Assert-NativeHosted ($freeCompile.ExitCode -eq 0) "default native compile failed: $($freeCompile.Stderr)"

$freeRun = Invoke-OracleProcess -FilePath $helloExe -WorkingDirectory $buildRoot
$helloExpected = Normalize-OracleText (Get-Content (Join-Path $ScriptDir "expected\hello_world.expected") -Raw)
Assert-NativeHosted ($freeRun.ExitCode -eq 0) "default native executable failed"
Assert-NativeHosted ($freeRun.Stdout -eq $helloExpected) "default native output mismatch"

$freeAscii = Get-ArtifactAscii $helloExe
if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) {
    Assert-NativeHosted ($freeAscii -notmatch '(?i)msvcrt|ucrt|vcruntime|api-ms-win-crt') `
        "default native executable contains a Windows CRT dependency"
} else {
    Assert-NativeHosted ($freeAscii -notmatch 'libc\.so|libm\.so|__libc_start_main|GLIBC_') `
        "default native executable contains a libc/libm dependency"
}

# Explicit hosted mode must match the C backend for all existing FFI cases.
# ffi_advanced covers sqrt/pow/fabs/floor/ceil from libm/the platform CRT.
foreach ($name in @("ffi", "ffi_advanced", "ffi_impure_wrapper")) {
    $result = Invoke-DifferentialBackendTest `
        -Compiler $compiler `
        -Source (Join-Path $ScriptDir "positive\$name.osc") `
        -Name $name `
        -Backend "native" `
        -BuildRoot $buildRoot `
        -RunRoot (Join-Path $runRoot $name) `
        -ExpectedFile (Join-Path $ScriptDir "expected\$name.expected") `
        -ExpectedStderrFile (Join-Path $ScriptDir "expected_stderr\$name.expected") `
        -ExpectedExitFile (Join-Path $ScriptDir "expected_exit\$name.expected") `
        -FixtureRoot (Join-Path $ScriptDir "fixtures") `
        -ExpectedFixtureRoot (Join-Path $ScriptDir "expected_files") `
        -CompileArguments @("--libc")
    Assert-NativeHosted $result.Success "$name differential failed: $($result.Failures -join '; ')"

    $hostedAscii = Get-ArtifactAscii $result.Candidate.Artifact
    if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) {
        Assert-NativeHosted ($hostedAscii -match '(?i)msvcrt|ucrt|api-ms-win-crt') `
            "$name did not use the explicitly requested hosted CRT"
    }
}

# Object-only output must not discover/build/link either runtime archive.
$missingArchiveDir = Join-Path $buildRoot "deliberately-missing-archive"
$hostedObject = Join-Path $buildRoot "ffi-hosted-only.obj"
$savedArchiveDir = $env:OSCAN_RUNTIME_ARCHIVE_DIR
try {
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $missingArchiveDir
    $objectCompile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @(
            "--libc", "--backend", "native",
            (Join-Path $ScriptDir "positive\ffi.osc"),
            "-o", $hostedObject
        ) `
        -WorkingDirectory $RepoRoot
} finally {
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $savedArchiveDir
}
Assert-NativeHosted ($objectCompile.ExitCode -eq 0) "hosted object-only emission tried to link: $($objectCompile.Stderr)"
Assert-NativeHosted (Test-Path -LiteralPath $hostedObject -PathType Leaf) "hosted object was not emitted"

# An executable request with the same unavailable hosted archive must fail
# clearly rather than falling back to the available freestanding archive.
$noFallbackExe = Join-Path $buildRoot "must-not-fallback$(Get-OracleExecutableSuffix)"
Remove-Item -LiteralPath $noFallbackExe -Force -ErrorAction SilentlyContinue
try {
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $missingArchiveDir
    $noFallbackCompile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @(
            "--libc", "--backend", "native",
            (Join-Path $ScriptDir "positive\ffi.osc"),
            "-o", $noFallbackExe
        ) `
        -WorkingDirectory $RepoRoot
} finally {
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $savedArchiveDir
}
Assert-NativeHosted ($noFallbackCompile.ExitCode -ne 0) "hosted link silently fell back to another runtime"
Assert-NativeHosted ($noFallbackCompile.Stderr -match 'requested hosted runtime archive') `
    "missing hosted archive did not produce a mode-specific error"
Assert-NativeHosted (-not (Test-Path -LiteralPath $noFallbackExe)) `
    "hosted fallback unexpectedly produced an executable"

# Native hosted linking must pass both extra C sources and repeatable compiler
# flags through the selected GCC/Clang driver.
$extraOsc = Join-Path $buildRoot "extra-c.osc"
$extraC = Join-Path $buildRoot "extra-c.c"
$extraExe = Join-Path $buildRoot "extra-c$(Get-OracleExecutableSuffix)"
Set-Content -LiteralPath $extraOsc -NoNewline -Value @'
extern {
    fn! add_bias(value: i32) -> i32;
}

fn! main() {
    print_i32(add_bias(20));
    println("");
}
'@
Set-Content -LiteralPath $extraC -NoNewline -Value @'
#ifndef OSC_TEST_BIAS
#error OSC_TEST_BIAS must be provided through --extra-cflags
#endif
#ifndef OSC_TEST_SCALE
#error OSC_TEST_SCALE must be provided through --extra-cflags
#endif
int add_bias(int value) { return value * OSC_TEST_SCALE + OSC_TEST_BIAS; }
'@
$extraCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @(
        "--libc", "--backend", "native",
        "--extra-c", $extraC,
        "--extra-cflags", "-DOSC_TEST_BIAS=2",
        "--extra-cflags", "-DOSC_TEST_SCALE=2",
        $extraOsc, "-o", $extraExe
    ) `
    -WorkingDirectory $RepoRoot
Assert-NativeHosted ($extraCompile.ExitCode -eq 0) "native hosted extra-C compile failed: $($extraCompile.Stderr)"
$extraRun = Invoke-OracleProcess -FilePath $extraExe -WorkingDirectory $buildRoot
Assert-NativeHosted ($extraRun.ExitCode -eq 0 -and $extraRun.Stdout -eq "42") `
    "native hosted extra-C output mismatch"

Write-Host "native hosted-mode tests passed"
