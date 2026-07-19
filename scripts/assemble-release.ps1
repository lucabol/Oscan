param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,

    [string]$OutputDir,

    [string]$ContractPath,

    # Reuse runtime archives already built earlier in the pipeline (e.g. the
    # release workflow's "build runtime archives with the shim baked in"
    # step, run before `cargo build --release` per
    # docs/design/native-link-embedding.md §5.5) instead of re-fetching the
    # toolchain and rebuilding them here.
    [string]$PrebuiltRuntimeArchiveDir,

    # Directory of per-target cross-linker sidecar subdirs (e.g.
    # build/cross-linker-sidecars, produced by the release workflow's
    # "Prepare cross-linker sidecars for packaging" step per
    # docs/design/native-link-embedding.md §11.1/§13.5) to bundle as
    # cross-linkers/<target>/ inside this archive.
    [string]$CrossLinkerSidecarDir
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
    $ContractPath = Join-Path $RepoRoot "packaging/toolchains/release-contract.json"
}

$contract = Get-Content $ContractPath -Raw | ConvertFrom-Json -AsHashtable
$targetSpec = if ($contract["bundled_targets"].ContainsKey($Target)) {
    $contract["bundled_targets"][$Target]
} elseif ($contract["binary_only_targets"].ContainsKey($Target)) {
    $contract["binary_only_targets"][$Target]
} else {
    throw "Release contract does not define target '$Target'."
}
$runtimeBuildToolchain = $null
$runtimeArchiveDir = $null
$nativeModes = @($targetSpec["native_runtime_modes"])
if ($PrebuiltRuntimeArchiveDir) {
    # Reuse archives an earlier pipeline step already built (with the native
    # shim baked in) instead of re-fetching the toolchain and rebuilding here.
    if (-not (Test-Path -LiteralPath $PrebuiltRuntimeArchiveDir)) {
        throw "PrebuiltRuntimeArchiveDir '$PrebuiltRuntimeArchiveDir' does not exist."
    }
    $runtimeArchiveDir = $PrebuiltRuntimeArchiveDir
} elseif ($nativeModes.Count -gt 0) {
    # "all" builds every mode the runtime-archive contract knows about in one
    # pass (see release_tools.py's build_runtime_archive); only fall back to
    # naming a single mode when there is exactly one to build, so this keeps
    # working correctly as native_runtime_modes grows (e.g. the
    # "freestanding_core" sibling archive) without needing a fixed count.
    $archiveMode = if ($nativeModes.Count -gt 1) { "all" } else { [string]$nativeModes[0] }
    $runtimeArchiveDir = Join-Path $OutputDir "runtime-archives/$Target"
    $archiveArgs = @{
        Target = $Target
        Mode = $archiveMode
        OutDir = $runtimeArchiveDir
    }

    if ($targetSpec.ContainsKey("toolchain_manifest")) {
        $manifestPath = Join-Path (Split-Path -Parent $ContractPath) ([string]$targetSpec["toolchain_manifest"])
        $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json -AsHashtable
        $runtimeToolchain = $manifest["toolchain"]["runtime"]
        if ($runtimeToolchain) {
            $buildToolchain = Join-Path $OutputDir "runtime-toolchains/$Target"
            $runtimeBuildToolchain = $buildToolchain
            $downloadDir = Join-Path $OutputDir "downloads"
            $null = & (Join-Path $PSScriptRoot "fetch-toolchain.ps1") `
                -ManifestPath $manifestPath `
                -Destination $buildToolchain `
                -DownloadDir $downloadDir
            if ($LASTEXITCODE -ne 0) {
                exit $LASTEXITCODE
            }
            $archiveArgs["CC"] = Join-Path $buildToolchain ([string]$runtimeToolchain["compiler"]["path"])
            $archiveArgs["AR"] = Join-Path $buildToolchain ([string]$runtimeToolchain["archiver"]["path"])
            $archiveArgs["ToolchainManifest"] = $manifestPath
        }
    }

    $null = & (Join-Path $PSScriptRoot "build-runtime-archive.ps1") @archiveArgs
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}

$stageArgs = @{
    Target = $Target
    Version = $Version
    BinaryPath = $BinaryPath
    OutputDir = $OutputDir
    ContractPath = $ContractPath
}
if ($runtimeArchiveDir) {
    $stageArgs["RuntimeArchiveDir"] = $runtimeArchiveDir
}
if ($CrossLinkerSidecarDir) {
    $stageArgs["CrossLinkerSidecarDir"] = $CrossLinkerSidecarDir
}
$result = & (Join-Path $PSScriptRoot "stage-release.ps1") @stageArgs
$stageExitCode = $LASTEXITCODE
if ($runtimeBuildToolchain -and (Test-Path -LiteralPath $runtimeBuildToolchain)) {
    Remove-Item -LiteralPath $runtimeBuildToolchain -Recurse -Force
    $runtimeToolchainRoot = Split-Path -Parent $runtimeBuildToolchain
    if ((Test-Path -LiteralPath $runtimeToolchainRoot) -and
        -not (Get-ChildItem -LiteralPath $runtimeToolchainRoot -Force)) {
        Remove-Item -LiteralPath $runtimeToolchainRoot -Force
    }
}
if ($stageExitCode -ne 0) {
    exit $stageExitCode
}
Write-Output $result
