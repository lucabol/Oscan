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

# Explicit hosted mode must match the C backend for existing FFI cases.
foreach ($name in @(
    "ffi",
    "ffi_advanced",
    "ffi_impure_wrapper"
)) {
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

# These fixtures intentionally no longer match the C backend's --libc stubs on
# Windows: native hosted archives should package the real implementations.
foreach ($name in @(
    "builtin_img_load",
    "builtin_svg_load",
    "builtin_tt_load",
    "gfx_text_width",
    "builtin_canvas_clipboard"
)) {
    $exe = Join-Path $buildRoot "$name-hosted$(Get-OracleExecutableSuffix)"
    $compile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @("--libc", "--backend", "native", (Join-Path $ScriptDir "positive\$name.osc"), "-o", $exe) `
        -WorkingDirectory $RepoRoot
    Assert-NativeHosted ($compile.ExitCode -eq 0) "$name native hosted compile failed: $($compile.Stderr)"
    $caseRunRoot = Join-Path $runRoot $name
    [void](New-Item -ItemType Directory -Path $caseRunRoot -Force)
    $run = Invoke-OracleProcess -FilePath $exe -WorkingDirectory $caseRunRoot
    Assert-NativeHosted ($run.ExitCode -eq 0) "$name native hosted run failed: stdout='$($run.Stdout)' stderr='$($run.Stderr)'"
    Assert-NativeHosted (Test-ExpectedOutputMatch `
        -ActualRaw $run.Stdout `
        -PrimaryExpectedFile (Join-Path $ScriptDir "expected\$name.expected")) `
        "$name native hosted output did not match the real-runtime expected output"
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

if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) {
    # Windows hosted native GUI programs must use the real canvas runtime, even
    # when linked as GUI-subsystem executables where stdout/stderr are hidden.
    $guiOsc = Join-Path $buildRoot "hosted-gui.osc"
    $guiConsoleExe = Join-Path $buildRoot "hosted-gui-console$(Get-OracleExecutableSuffix)"
    $guiWindowExe = Join-Path $buildRoot "hosted-gui-window$(Get-OracleExecutableSuffix)"
    $guiMarker = Join-Path $buildRoot "hosted-gui-marker.txt"
    Set-Content -LiteralPath $guiOsc -NoNewline -Value @'
fn! mark(text: str) {
    let opened: Result<i32, str> = file_open_write("hosted-gui-marker.txt");
    match opened {
        Result::Ok(fd) => {
            write_str(fd, text);
            file_close(fd);
        },
        Result::Err(_) => {},
    };
}

fn! run_window() -> i32 {
    let opened: Result<str, str> = canvas_open(160, 96, "native hosted gui regression");
    match opened {
        Result::Ok(_) => {
            canvas_clear(rgb(8, 12, 24));
            gfx_fill_rect(12, 12, 48, 28, rgb(0, 180, 80));
            gfx_rect(10, 10, 52, 32, rgba(255, 255, 255, 255));
            gfx_draw_text(16, 56, "ok", rgb(255, 255, 255), 0);
            canvas_flush();
            if canvas_alive() {
                print("opened-alive");
                mark("opened-alive");
            } else {
                print("opened-not-alive");
                mark("opened-not-alive");
            };
            canvas_close();
            0
        },
        Result::Err(e) => {
            println("open failed: {e}");
            mark("open-failed");
            1
        },
    }
}

fn! main() -> i32 {
    mark("entered");
    let code: i32 = run_window();
    if code == 0 {
        mark("closed");
    };
    code
}
'@

    Remove-Item -LiteralPath $guiConsoleExe,$guiWindowExe,$guiMarker -Force -ErrorAction SilentlyContinue
    $guiConsoleCompile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @("--libc", "--backend", "native", $guiOsc, "-o", $guiConsoleExe) `
        -WorkingDirectory $RepoRoot
    Assert-NativeHosted ($guiConsoleCompile.ExitCode -eq 0) "native hosted GUI console compile failed: $($guiConsoleCompile.Stderr)"
    $guiConsoleRun = Invoke-OracleProcess -FilePath $guiConsoleExe -WorkingDirectory $buildRoot
    Assert-NativeHosted ($guiConsoleRun.ExitCode -eq 0) "native hosted GUI console run failed: stdout='$($guiConsoleRun.Stdout)' stderr='$($guiConsoleRun.Stderr)'"
    Assert-NativeHosted ($guiConsoleRun.Stdout -eq "opened-alive") `
        "native hosted GUI console did not report a live canvas: stdout='$($guiConsoleRun.Stdout)' stderr='$($guiConsoleRun.Stderr)'"
    Assert-NativeHosted ((Get-Content -LiteralPath $guiMarker -Raw) -eq "closed") `
        "native hosted GUI console marker did not prove deterministic close"

    Remove-Item -LiteralPath $guiMarker -Force -ErrorAction SilentlyContinue
    $guiWindowCompile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @(
            "--libc", "--backend", "native",
            "--extra-cflags", "-Wl,--subsystem,windows",
            "--extra-cflags", "-Wl,--entry,mainCRTStartup",
            $guiOsc, "-o", $guiWindowExe
        ) `
        -WorkingDirectory $RepoRoot
    Assert-NativeHosted ($guiWindowCompile.ExitCode -eq 0) "native hosted GUI-subsystem compile failed: $($guiWindowCompile.Stderr)"
    $guiWindowRun = Invoke-OracleProcess -FilePath $guiWindowExe -WorkingDirectory $buildRoot
    Assert-NativeHosted ($guiWindowRun.ExitCode -eq 0) "native hosted GUI-subsystem run failed with exit $($guiWindowRun.ExitCode)"
    Assert-NativeHosted ((Get-Content -LiteralPath $guiMarker -Raw) -eq "closed") `
        "native hosted GUI-subsystem marker did not prove oscan_main opened and closed a live canvas"
}

Write-Host "native hosted-mode tests passed"
