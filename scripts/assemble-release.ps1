param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    # Which package profile to assemble.
    [Parameter(Mandatory = $true)]
    [Alias("Backend")]
    [ValidateSet("full", "llvm", "cranelift", "c")]
    [string]$Profile,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    # The feature-gated binary for this variant: built with
    # the exact feature set declared by the release contract and
    # OSCAN_DISTRIBUTION_PROFILE=<Profile>.
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [string]$OutputDir,

    [string]$ContractPath,

    # Prepared inputs. CI fetches/caches the pinned source archives, prepares
    # the native-link asset set, and builds the freestanding runtime archives
    # once per target, then reuses them for every package profile of that
    # target. Staging never downloads.
    [string]$PrebuiltRuntimeArchiveDir,

    [string]$NativeLinkDir,

    # License notices for code and link inputs embedded into a strict
    # single-executable compiler.
    [string]$EmbeddedNoticesDir,

    # Digest-pinned source archives (see 'release_tools.py resolve-archive').
    [string]$ToolchainArchive,

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

$contract = Get-Content $ContractPath -Raw | ConvertFrom-Json -AsHashtable
if (-not $contract["variants"].ContainsKey($Target)) {
    throw "Release contract does not define target '$Target'."
}
$targetSpec = $contract["variants"][$Target]
if (-not $targetSpec["profiles"].ContainsKey($Profile)) {
    $known = ($targetSpec["profiles"].Keys | Sort-Object) -join ", "
    throw "Release contract does not define profile '$Profile' for target '$Target' (known: $known)."
}
$variant = $targetSpec["profiles"][$Profile]
$components = @($variant["components"])
$runtimeProfiles = @($variant["runtime_profiles"])

# Sidecar object variants need a prepared native-link asset set. Strict
# in-process variants embed their linker and runtime inputs in the binary.
if ($components -contains "direct_link_sidecar" -and -not $NativeLinkDir) {
    throw "Profile '$Profile' for '$Target' needs -NativeLinkDir (run prepare-embed-assets for this target first)."
}
if ($variant["link_mode"] -eq "inprocess" -and -not $EmbeddedNoticesDir) {
    throw "Profile '$Profile' for '$Target' needs -EmbeddedNoticesDir containing the notices for its statically linked and embedded inputs."
}
if ($components -contains "c_toolchain" -and -not $ToolchainArchive) {
    throw "Profile '$Profile' for '$Target' needs -ToolchainArchive (the pinned C toolchain source archive; resolve it with 'release_tools.py resolve-archive --component toolchain')."
}
if ($components -contains "llvm_provider" -and
    $variant["llvm_provider_source"] -eq "toolchain-manifest" -and
    -not $LlvmProviderArchive) {
    throw "Profile '$Profile' for '$Target' needs -LlvmProviderArchive (the pinned LLVM provider source archive; resolve it with 'release_tools.py resolve-archive --component llvm-provider')."
}

$runtimeArchiveDir = $null
if ($components -contains "runtime_archives") {
    if (-not $PrebuiltRuntimeArchiveDir) {
        throw "Profile '$Profile' for '$Target' needs -PrebuiltRuntimeArchiveDir containing its freestanding runtime archives ($($runtimeProfiles -join ', '))."
    }
    if (-not (Test-Path -LiteralPath $PrebuiltRuntimeArchiveDir)) {
        throw "PrebuiltRuntimeArchiveDir '$PrebuiltRuntimeArchiveDir' does not exist."
    }
    $runtimeArchiveDir = $PrebuiltRuntimeArchiveDir
}

$stageArgs = @{
    Target = $Target
    Profile = $Profile
    Version = $Version
    BinaryPath = $BinaryPath
    OutputDir = $OutputDir
    ContractPath = $ContractPath
}
if ($runtimeArchiveDir) {
    $stageArgs["RuntimeArchiveDir"] = $runtimeArchiveDir
}
if ($NativeLinkDir) {
    $stageArgs["NativeLinkDir"] = $NativeLinkDir
}
if ($EmbeddedNoticesDir) {
    $stageArgs["EmbeddedNoticesDir"] = $EmbeddedNoticesDir
}
if ($ToolchainArchive) {
    $stageArgs["ToolchainArchive"] = $ToolchainArchive
}
if ($LlvmProviderArchive) {
    $stageArgs["LlvmProviderArchive"] = $LlvmProviderArchive
}

$result = & (Join-Path $PSScriptRoot "stage-release.ps1") @stageArgs
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
Write-Output $result
