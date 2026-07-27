param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [string]$RuntimeArchiveDir = "build\runtime-archives\windows-x86_64",

    [string]$ToolchainDir = "build\toolchain-windows-x86_64",

    [switch]$StandardUserChild
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir

function Invoke-NativeValidation {
    $compiler = (Resolve-Path -LiteralPath $Oscan).Path
    $runtimeArchives = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $runtimeArchives
    $toolchain = (Resolve-Path -LiteralPath $ToolchainDir).Path
    $env:OSCAN_TOOLCHAIN_DIR = $toolchain

    & (Join-Path $ScriptDir "default_backend.tests.ps1") -Oscan $compiler -ExpectedBackend llvm

    & (Join-Path $ScriptDir "llvm_toolchain_isolation.tests.ps1") `
        -Oscan $compiler `
        -RuntimeArchiveDir $runtimeArchives `
        -LlvmLibrary (Join-Path $toolchain "bin\libLLVM-22.dll")

    & (Join-Path $RepoRoot "scripts\compare-backend-size.ps1") `
        -Oscan $compiler `
        -OutputDirectory (Join-Path $ScriptDir "build\backend-size-ci")

    & (Join-Path $ScriptDir "native_link_isolation.tests.ps1") `
        -Oscan $compiler `
        -ToolchainDir $ToolchainDir
    if ($LASTEXITCODE -ne 0) {
        throw "native-link isolation suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "run_tests.ps1") -Oscan $compiler -Backend native
    if ($LASTEXITCODE -ne 0) {
        throw "C-vs-native differential suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "run_tests.ps1") -Oscan $compiler -Backend llvm
    if ($LASTEXITCODE -ne 0) {
        throw "C-vs-LLVM differential suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "native_extern_str_abi.tests.ps1") -Oscan $compiler -Backend native
    if ($LASTEXITCODE -ne 0) {
        throw "native extern str ABI suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "native_extern_str_abi.tests.ps1") -Oscan $compiler -Backend llvm
    if ($LASTEXITCODE -ne 0) {
        throw "LLVM extern str ABI suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "panic_message.tests.ps1") `
        -Oscan $compiler `
        -Backends @("c", "native", "llvm") `
        -SkipWSL
    if ($LASTEXITCODE -ne 0) {
        throw "cross-backend panic-message suite failed with exit code $LASTEXITCODE"
    }

    $savedNoToolchain = $env:OSCAN_NO_TOOLCHAIN
    try {
        $env:OSCAN_NO_TOOLCHAIN = "1"
        & (Join-Path $ScriptDir "backend_parity.ps1") `
            -Oscan $compiler `
            -Backend llvm `
            -FreestandingOnly
        if ($LASTEXITCODE -ne 0) {
            throw "strict no-toolchain LLVM parity suite failed with exit code $LASTEXITCODE"
        }
        & (Join-Path $ScriptDir "backend_parity.ps1") `
            -Oscan $compiler `
            -Backend native `
            -FreestandingOnly
        if ($LASTEXITCODE -ne 0) {
            throw "strict no-toolchain Cranelift parity suite failed with exit code $LASTEXITCODE"
        }
    } finally {
        $env:OSCAN_NO_TOOLCHAIN = $savedNoToolchain
    }
}

function Invoke-TrustedElevatedNativeLinkShape {
    param([Parameter(Mandatory = $true)][string]$Compiler)

    $buildRoot = Join-Path $ScriptDir "build\trusted-elevated-native-link"
    [void](New-Item -ItemType Directory -Path $buildRoot -Force)
    $oscSource = Join-Path $buildRoot "oscanweb-command-shape.osc"
    $bridgeSource = Join-Path $buildRoot "oscanweb-command-shape-bridge.c"
    $exePath = Join-Path $buildRoot "oscanweb-command-shape.exe"

    Set-Content -LiteralPath $oscSource -NoNewline -Value @'
extern {
    fn! host_label(value: str) -> str;
}

fn! main() {
    println(host_label("trusted"));
}
'@

    Set-Content -LiteralPath $bridgeSource -NoNewline -Value @'
#include "osc_runtime.h"

osc_str host_label(osc_str value) {
    (void)value;
    static const char text[] = "trusted elevated native link";
    osc_str out;
    out.data = text;
    out.len = 28;
    return out;
}
'@

    $savedRuntimeArchiveDir = $env:OSCAN_RUNTIME_ARCHIVE_DIR
    $savedToolchainDir = $env:OSCAN_TOOLCHAIN_DIR
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
    $env:OSCAN_TOOLCHAIN_DIR = (Resolve-Path -LiteralPath $ToolchainDir).Path
    $compileArgs = @(
        "--backend", "native",
        "--native-target", "host",
        "--libc",
        "--allow-elevated-native-link",
        "--extra-c", $bridgeSource,
        $oscSource,
        "-o", $exePath
    )
    try {
        $compileOutput = & $Compiler @compileArgs 2>&1 | Out-String
        if ($LASTEXITCODE -ne 0) {
            throw "trusted elevated native link command shape failed to compile with args '$($compileArgs -join ' ')': $compileOutput"
        }
    } finally {
        $env:OSCAN_RUNTIME_ARCHIVE_DIR = $savedRuntimeArchiveDir
        $env:OSCAN_TOOLCHAIN_DIR = $savedToolchainDir
    }

    $runOutput = & $exePath 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "trusted elevated native link command shape executable failed: $runOutput"
    }
    if ($runOutput.Trim() -ne "trusted elevated native link") {
        throw "trusted elevated native link command shape output mismatch: '$runOutput'"
    }
}

Push-Location $RepoRoot
try {
    if ($StandardUserChild) {
        Invoke-NativeValidation
        exit 0
    }

    $compiler = (Resolve-Path -LiteralPath $Oscan).Path
    $probeOutput = & $compiler examples\hello.osc --backend native `
        -o tests\build\native_ci_elevation_probe.exe 2>&1 | Out-String
    $probeExit = $LASTEXITCODE

    if ($probeExit -eq 0) {
        Invoke-TrustedElevatedNativeLinkShape -Compiler $compiler
        Invoke-NativeValidation
        exit 0
    }
    if ($probeOutput -notmatch "running elevated \(Administrator\)") {
        throw "native-link preflight failed for a reason other than elevation: $probeOutput"
    }

    Invoke-TrustedElevatedNativeLinkShape -Compiler $compiler

    # GitHub-hosted Windows runners execute the runner service with an
    # elevated token. Product code must continue to reject that token by
    # default; the targeted command-shape test above proves the explicit
    # trusted-input opt-in reaches linking, while the broad differential suite
    # still runs under a disposable standard-user token.
    $logDir = Join-Path $ScriptDir "build\windows-native-standard-user"
    [void](New-Item -ItemType Directory -Path $logDir -Force)
    . (Join-Path $RepoRoot "scripts\windows-standard-user.ps1")
    Invoke-WindowsStandardUserPowerShell `
        -ScriptPath $PSCommandPath `
        -Parameters @{
            Oscan = $compiler
            RuntimeArchiveDir = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
            ToolchainDir = (Resolve-Path -LiteralPath $ToolchainDir).Path
            StandardUserChild = $true
        } `
        -WorkingDirectory $RepoRoot `
        -StateBaseDir $logDir `
        -ReadOnlyPaths @($RepoRoot) `
        -WritablePaths @(
            (Join-Path $RepoRoot "build"),
            (Join-Path $ScriptDir "build")
        ) `
        -LogDirectory $logDir `
        -LogPrefix "native-validation" `
        -TimeoutSeconds 1800
} finally {
    Pop-Location
}
