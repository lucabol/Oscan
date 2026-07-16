param(
    [string]$Target,

    [ValidateSet("hosted", "freestanding", "all")]
    [string]$Mode = "all",

    [string]$CC,
    [string]$AR,
    [string]$TargetTriple,
    [string]$Sysroot,
    [string]$ToolchainManifest,
    [string]$OutDir,
    [string]$ContractPath,
    [switch]$KeepObjects
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if (-not $ContractPath) {
    $ContractPath = Join-Path $RepoRoot "packaging\toolchains\runtime-archive-contract.json"
}

$tool = Join-Path $PSScriptRoot "release_tools.py"
$pythonArgs = @($tool, "build-runtime-archive", "--mode", $Mode, "--contract", $ContractPath)
if ($Target) { $pythonArgs += @("--target", $Target) }
if ($CC) { $pythonArgs += @("--cc", $CC) }
if ($AR) { $pythonArgs += @("--ar", $AR) }
if ($TargetTriple) { $pythonArgs += @("--target-triple", $TargetTriple) }
if ($Sysroot) { $pythonArgs += @("--sysroot", $Sysroot) }
if ($ToolchainManifest) { $pythonArgs += @("--toolchain-manifest", $ToolchainManifest) }
if ($OutDir) { $pythonArgs += @("--out-dir", $OutDir) }
if ($KeepObjects) { $pythonArgs += @("--keep-objects") }

$result = & python @pythonArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
