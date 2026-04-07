param(
    [Parameter(Mandatory = $true)]
    [string]$ManifestPath,

    [Parameter(Mandatory = $true)]
    [string]$Destination,

    [string]$DownloadDir
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $DownloadDir) {
    $DownloadDir = Join-Path (Join-Path (Join-Path $RepoRoot "target") "release-artifacts") "downloads"
}

$tool = Join-Path $PSScriptRoot "release_tools.py"
$result = & python $tool fetch-toolchain --manifest $ManifestPath --download-dir $DownloadDir --destination $Destination
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
