param(
    [Parameter(Mandatory = $true)]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$ToolchainDir,

    [string]$ToolchainManifest,
    [string]$OutputDir
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

$tool = Join-Path $PSScriptRoot "release_tools.py"
$pythonArgs = @($tool, "prepare-embed-assets", "--target", $Target, "--toolchain-dir", $ToolchainDir)
if ($ToolchainManifest) { $pythonArgs += @("--toolchain-manifest", $ToolchainManifest) }
if ($OutputDir) { $pythonArgs += @("--output-dir", $OutputDir) }

$result = & python @pythonArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
