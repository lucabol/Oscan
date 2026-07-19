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
    $env:OSCAN_TOOLCHAIN_DIR = (Resolve-Path -LiteralPath $ToolchainDir).Path

    & (Join-Path $ScriptDir "default_backend_native.tests.ps1") -Oscan $compiler

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
        Invoke-NativeValidation
        exit 0
    }
    if ($probeOutput -notmatch "running elevated \(Administrator\)") {
        throw "native-link preflight failed for a reason other than elevation: $probeOutput"
    }

    # GitHub-hosted Windows runners execute the runner service with an
    # elevated token. Product code must continue to reject that token, so run
    # the real final-link tests under a disposable standard-user token rather
    # than adding a CI-only bypass to Oscan.
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
