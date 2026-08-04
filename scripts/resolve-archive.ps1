param(
    [Parameter(Mandatory = $true)]
    [string]$ManifestPath,

    # Which pinned archive to resolve: the base C toolchain, the separately
    # pinned LLVM provider, or one of the strict in-process LLVM build inputs.
    [ValidateSet(
        "toolchain",
        "llvm-provider",
        "inprocess-llvm-sdk",
        "inprocess-llvm-source"
    )]
    [string]$Component = "toolchain",

    [string]$DownloadDir,

    # Download when the archive is not already cached. Without this the
    # command is strictly offline, which is what release staging needs.
    [switch]$Download
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $DownloadDir) {
    $DownloadDir = Join-Path (Join-Path (Join-Path $RepoRoot "target") "release-artifacts") "downloads"
}

$tool = Join-Path $PSScriptRoot "release_tools.py"
$pythonArgs = @(
    $tool,
    "resolve-archive",
    "--manifest", $ManifestPath,
    "--download-dir", $DownloadDir,
    "--component", $Component
)
if ($Download) {
    $pythonArgs += "--download"
}

$result = & python @pythonArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
