param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [Alias("Backend")]
    [ValidateSet("full", "llvm", "cranelift", "c")]
    [string]$Profile,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [string]$OutputDir,

    [string]$ContractPath,

    [string]$RuntimeArchiveDir,

    # Prepared native-link asset directory (native-link-assets.json plus its
    # assets at their install_subpaths) for the llvm/cranelift variants. This
    # stays a directory because every file in it carries a repo-derived
    # manifest digest that staging re-verifies.
    [string]$NativeLinkDir,

    # License notices for a strict compiler's statically linked and embedded
    # LLVM/LLD and llvm-mingw inputs.
    [string]$EmbeddedNoticesDir,

    # The pinned C toolchain *source archive* for the c variant. Staging
    # verifies it against the digest in packaging/toolchains/<target>.json
    # before extracting a single member, so an arbitrary or foreign
    # toolchain can never be packaged. Resolve it with
    # 'release_tools.py resolve-archive'.
    [string]$ToolchainArchive,

    # The pinned LLVM provider *source archive* for targets whose provider
    # comes from the toolchain manifest instead of the native-link sidecar.
    # Verified against toolchain.llvm_code_generator.archive.digest; only the
    # manifest-declared members are ever staged.
    [string]$LlvmProviderArchive,

    # Removed: a prepared directory cannot be authenticated.
    [string]$ToolchainDir,

    [string]$LlvmProviderDir
)

$ErrorActionPreference = "Stop"
if ($ToolchainDir) {
    throw "-ToolchainDir has been removed from release staging: a prepared toolchain directory cannot be checked against the digest the toolchain manifest pins. Pass -ToolchainArchive with the pinned source archive instead."
}
if ($LlvmProviderDir) {
    throw "-LlvmProviderDir has been removed from release staging: its provenance record was self-asserted. Pass -LlvmProviderArchive with the pinned source archive instead."
}
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
    $ContractPath = Join-Path $RepoRoot "packaging/toolchains/release-contract.json"
}

$tool = Join-Path $PSScriptRoot "release_tools.py"
$pythonArgs = @(
    $tool,
    "stage-release",
    "--target", $Target,
    "--profile", $Profile,
    "--version", $Version,
    "--binary", $BinaryPath,
    "--output-dir", $OutputDir,
    "--contract", $ContractPath
)
if ($RuntimeArchiveDir) {
    $pythonArgs += @("--runtime-archive-dir", $RuntimeArchiveDir)
}
if ($NativeLinkDir) {
    $pythonArgs += @("--native-link-dir", $NativeLinkDir)
}
if ($EmbeddedNoticesDir) {
    $pythonArgs += @("--embedded-notices-dir", $EmbeddedNoticesDir)
}
if ($ToolchainArchive) {
    $pythonArgs += @("--toolchain-archive", $ToolchainArchive)
}
if ($LlvmProviderArchive) {
    $pythonArgs += @("--llvm-provider-archive", $LlvmProviderArchive)
}

$result = & python @pythonArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
