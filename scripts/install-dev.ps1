<#
.SYNOPSIS
    Build and install the current repo checkout as the system's default oscan
    (per-user, no admin required).

.DESCRIPTION
    1. Runs `cargo build --release` (skip with -SkipBuild).
    2. Stages target\release\oscan.exe (and libbearssl.a if present under
       deps\laststanding\bearssl\build\) into a temporary directory.
    3. Delegates to scripts\install-oscan.ps1, which copies the bundle to
       $env:LOCALAPPDATA\Programs\oscan (override with -InstallDir) and adds
       it to the User PATH.

    Unlike install-oscan.ps1, this script works directly against the repo:
    no release bundle is required.

    Note: this installs oscan WITHOUT the bundled LLVM-MinGW toolchain. oscan
    will fall back to clang / gcc / VS clang on PATH when compiling .osc files.

.PARAMETER InstallDir
    Install location. Defaults to $env:LOCALAPPDATA\Programs\oscan.

.PARAMETER BinDir
    Optional directory where a thin `oscan.cmd` shim is created. When set,
    only BinDir is added to PATH (not InstallDir).

.PARAMETER NoPathUpdate
    Skip modifying the User PATH.

.PARAMETER SkipBuild
    Don't run `cargo build --release`; just package whatever is already in
    target\release.

.PARAMETER Configuration
    Cargo profile directory to install from. Defaults to 'release'.

.EXAMPLE
    .\scripts\install-dev.ps1

.EXAMPLE
    .\scripts\install-dev.ps1 -SkipBuild -InstallDir C:\tools\oscan
#>
[CmdletBinding()]
param(
    [string]$InstallDir,
    [string]$BinDir,
    [switch]$NoPathUpdate,
    [switch]$SkipBuild,
    [ValidateSet('release', 'debug')]
    [string]$Configuration = 'release'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Installer = Join-Path $PSScriptRoot 'install-oscan.ps1'
if (-not (Test-Path $Installer)) {
    throw "Cannot find companion installer: $Installer"
}

if (-not $SkipBuild) {
    # Ensure deps/laststanding submodule is populated (required for include_str! macros).
    $LastStanding = Join-Path $RepoRoot 'deps\laststanding\l_os.h'
    if (-not (Test-Path $LastStanding)) {
        Write-Host "Initializing deps/laststanding submodule..."
        Push-Location $RepoRoot
        try {
            & git submodule update --init --recursive deps/laststanding
            if ($LASTEXITCODE -ne 0) {
                throw "git submodule update failed (exit code $LASTEXITCODE)."
            }
        } finally {
            Pop-Location
        }
    }

    Write-Host "Building oscan ($Configuration)..."
    Push-Location $RepoRoot
    try {
        $cargoArgs = @('build')
        if ($Configuration -eq 'release') { $cargoArgs += '--release' }
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed (exit code $LASTEXITCODE)."
        }
    } finally {
        Pop-Location
    }
}

$BuiltExe = Join-Path $RepoRoot "target\$Configuration\oscan.exe"
if (-not (Test-Path $BuiltExe)) {
    throw "Built binary not found at $BuiltExe. Run without -SkipBuild."
}

# Stage files to a temp dir so install-oscan.ps1 can MIRror it cleanly.
$Stage = Join-Path ([System.IO.Path]::GetTempPath()) ("oscan-dev-stage-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $Stage -Force | Out-Null

try {
    Copy-Item -LiteralPath $BuiltExe -Destination (Join-Path $Stage 'oscan.exe') -Force

    # Optionally bundle libbearssl.a so TLS-using programs keep working after install.
    $BearSsl = Join-Path $RepoRoot 'deps\laststanding\bearssl\build\libbearssl.a'
    if (Test-Path $BearSsl) {
        $LibDir = Join-Path $Stage 'lib'
        New-Item -ItemType Directory -Path $LibDir -Force | Out-Null
        Copy-Item -LiteralPath $BearSsl -Destination (Join-Path $LibDir 'libbearssl.a') -Force
        Write-Host "Bundling libbearssl.a from deps\laststanding\bearssl\build\"
    }

    $installerArgs = @{
        SourceDir = $Stage
    }
    if ($PSBoundParameters.ContainsKey('InstallDir')) { $installerArgs.InstallDir = $InstallDir }
    if ($PSBoundParameters.ContainsKey('BinDir'))     { $installerArgs.BinDir     = $BinDir }
    if ($NoPathUpdate)                                 { $installerArgs.NoPathUpdate = $true }

    & $Installer @installerArgs
    if ($LASTEXITCODE -ne 0) {
        throw "install-oscan.ps1 failed (exit code $LASTEXITCODE)."
    }
} finally {
    if (Test-Path $Stage) {
        Remove-Item -LiteralPath $Stage -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ""
Write-Host "Dev install complete. Open a new shell (or refresh PATH) and run: oscan --help"
