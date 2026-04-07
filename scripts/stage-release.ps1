param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [string]$OutputDir,

    [string]$ContractPath
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Get-DefaultOutputDir {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    if ($env:OS -eq "Windows_NT") {
        $baseDir = if ($env:RUNNER_TEMP) {
            $env:RUNNER_TEMP
        } elseif ($env:TEMP) {
            $env:TEMP
        } else {
            Join-Path $RepoRoot "target"
        }
        return Join-Path $baseDir "oscan-release-artifacts"
    }

    return Join-Path (Join-Path $RepoRoot "target") "release-artifacts"
}

if (-not $OutputDir) {
    $OutputDir = Get-DefaultOutputDir -RepoRoot $RepoRoot
}
if (-not $ContractPath) {
    $ContractPath = Join-Path $RepoRoot "packaging\toolchains\release-contract.json"
}

$tool = Join-Path $PSScriptRoot "release_tools.py"
$pythonArgs = @(
    $tool,
    "stage-release",
    "--target", $Target,
    "--version", $Version,
    "--binary", $BinaryPath,
    "--output-dir", $OutputDir,
    "--contract", $ContractPath
)

$result = & python @pythonArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
