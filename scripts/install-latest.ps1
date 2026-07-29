<#
.SYNOPSIS
    Downloads and silently installs the latest backend-specific Oscan release
    for Windows x86_64.

.DESCRIPTION
    Every published Oscan artifact is one (target, backend) package. For
    Windows x86_64 the release publishes exactly three archives —
    oscan-vX.Y.Z-windows-x86_64-llvm.zip, ...-cranelift.zip and ...-c.zip —
    plus one installer, the recommended LLVM MSI
    (oscan-vX.Y.Z-windows-x86_64-llvm.msi). No combined all-backends package
    is published.

    This script queries the GitHub Releases API for lucabol/Oscan, picks the
    asset whose name is exactly the one the release contract derives from the
    resolved tag and the requested backend, verifies its SHA-256 against the
    release's SHA256SUMS file, and installs it without prompts. No fuzzy,
    suffix or version-drift matching is performed: if the exact name is not
    published, the script fails with the list of assets that are.

    LLVM is the recommended backend and the default here. The zip flow is the
    default install mode because it is per-user and does not require
    administrator privileges; -Mode msi installs the recommended LLVM MSI
    through msiexec /quiet instead (which may require elevation).

.PARAMETER Backend
    'llvm' (default, recommended), 'cranelift', or 'c'. The backend selects
    the package: an llvm/cranelift package emits object code directly and
    carries no C compiler; the c package carries the pinned C toolchain.
    'native' is accepted as the compiler's deprecated CLI alias spelling and
    resolves to the cranelift package; it is never an artifact name.

.PARAMETER Mode
    'zip' (default): download the backend's exact zip and run its bundled
    install.ps1.
    'msi': download the recommended LLVM MSI and install it via msiexec
    /quiet. Only the llvm backend publishes an MSI; if that exact MSI is not
    published, the script falls back to the exact llvm zip and nothing else.

.PARAMETER Version
    Optional explicit version tag (e.g. 'v0.5.0'). Defaults to the latest
    published release.

.PARAMETER InstallDir
    Forwarded to install.ps1 when installing a zip. Ignored for MSI.

.PARAMETER NoPathUpdate
    Forwarded to install.ps1 when installing a zip. Ignored for MSI.

.PARAMETER SkipChecksum
    Skip SHA-256 verification of the downloaded asset. Not recommended.
    Without it, a release that publishes no SHA256SUMS is a hard error rather
    than a silently unverified install.

.EXAMPLE
    iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex

.EXAMPLE
    .\install-latest.ps1 -Backend cranelift

.EXAMPLE
    .\install-latest.ps1 -Backend llvm -Mode msi
#>
[CmdletBinding()]
param(
    [ValidateSet('llvm', 'cranelift', 'c', 'native')]
    [string]$Backend = 'llvm',
    [ValidateSet('zip', 'msi')]
    [string]$Mode = 'zip',
    [string]$Version,
    [string]$InstallDir,
    [string]$BinDir,
    [switch]$NoPathUpdate,
    [switch]$SkipChecksum
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'lucabol/Oscan'
$ApiBase = "https://api.github.com/repos/$Repo/releases"

# The one Windows target this installer serves, and the only backend that
# publishes an installer.
$InstallerTarget = 'windows-x86_64'
$MsiBackend = 'llvm'

# Force TLS 1.2 for older PowerShell hosts
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

function Resolve-OscanBackend {
    <#
        The canonical package labels are llvm, cranelift and c. 'native' is
        only the compiler's deprecated CLI alias for cranelift, so it is
        accepted here and immediately normalised; it is never part of an
        asset name.
    #>
    param([Parameter(Mandatory)][string]$Requested)

    $normalized = $Requested.Trim().ToLowerInvariant()
    if ($normalized -eq 'native') {
        Write-Warning "'-Backend native' is deprecated; use '-Backend cranelift' ('native' remains a compatibility alias)."
        return 'cranelift'
    }
    if ($normalized -notin @('llvm', 'cranelift', 'c')) {
        throw "Unknown backend '$Requested'. Valid backends are: llvm (recommended), cranelift, c."
    }
    return $normalized
}

function Get-OscanAssetName {
    <#
        The exact canonical asset name for a (tag, backend, kind) triple, as
        rendered by packaging/toolchains/release-contract.json. Nothing here
        globs: an installer must never resolve to another backend's package.
    #>
    param(
        [Parameter(Mandatory)][string]$Tag,
        [Parameter(Mandatory)][ValidateSet('llvm', 'cranelift', 'c')][string]$Backend,
        [Parameter(Mandatory)][ValidateSet('zip', 'msi')][string]$Kind
    )

    if ($Kind -eq 'msi' -and $Backend -ne $MsiBackend) {
        throw "Only the recommended $MsiBackend package is published as an MSI; the $Backend package ships as a zip archive."
    }
    return "oscan-$Tag-$InstallerTarget-$Backend.$Kind"
}

function Select-OscanReleaseAsset {
    <#
        Pick the exact canonical asset for this backend out of a release's
        asset list.

        * Matching is by exact, contract/tag-derived name only. A release
          whose asset names disagree with its own tag is a broken release,
          not something to guess around: no suffix, glob or version-drift
          matching happens here, so another backend's package, a source
          archive, or a legacy combined archive can never be selected.
        * llvm + -Mode msi prefers the exact LLVM MSI and falls back only to
          the exact LLVM zip when a release published no installer.
        * cranelift and c always resolve to their own exact zip; they have
          no MSI, and -Mode msi is refused rather than silently installing
          the LLVM package.
    #>
    param(
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Assets,
        [Parameter(Mandatory)][string]$Tag,
        [Parameter(Mandatory)][ValidateSet('llvm', 'cranelift', 'c')][string]$Backend,
        [Parameter(Mandatory)][ValidateSet('zip', 'msi')][string]$Mode
    )

    if ($Mode -eq 'msi' -and $Backend -ne $MsiBackend) {
        throw "-Mode msi is only available for the recommended $MsiBackend package. Re-run with '-Backend $MsiBackend -Mode msi', or install the $Backend package with '-Backend $Backend' (zip)."
    }

    $wanted = @()
    if ($Mode -eq 'msi') {
        $wanted += [PSCustomObject]@{ Kind = 'msi'; Name = (Get-OscanAssetName -Tag $Tag -Backend $Backend -Kind 'msi') }
    }
    $wanted += [PSCustomObject]@{ Kind = 'zip'; Name = (Get-OscanAssetName -Tag $Tag -Backend $Backend -Kind 'zip') }

    foreach ($candidate in $wanted) {
        $match = @($Assets | Where-Object { $_.name -ieq $candidate.Name })
        if ($match.Count -gt 1) {
            throw "Release $Tag publishes more than one asset named '$($candidate.Name)'; refusing to guess which one to install."
        }
        if ($match.Count -eq 1) {
            if ($candidate.Kind -ne $Mode) {
                Write-Warning "Release $Tag publishes no $Backend MSI; installing $($match[0].name) instead."
            }
            return [PSCustomObject]@{ Asset = $match[0]; Kind = $candidate.Kind }
        }
    }

    $published = @($Assets | Where-Object { $_.name } | ForEach-Object { $_.name } | Sort-Object)
    $expected = ($wanted | ForEach-Object { $_.Name }) -join ' or '
    throw "Release $Tag publishes no $expected. Asset names are derived from the release tag, and only that exact name is accepted. Windows releases ship one archive per backend (llvm, cranelift, c) and one recommended llvm MSI; there is no combined package. Published assets: $(if ($published) { $published -join ', ' } else { '(none)' })."
}

function Get-OscanExpectedChecksum {
    param(
        [Parameter(Mandatory)][string[]]$SumsLines,
        [Parameter(Mandatory)][string]$AssetName
    )

    foreach ($line in $SumsLines) {
        $parts = $line -split '\s+', 2
        if ($parts.Length -eq 2 -and ($parts[1].Trim().TrimStart('*')) -ieq $AssetName) {
            return $parts[0].Trim().ToLowerInvariant()
        }
    }
    return $null
}

function Invoke-GitHubApi {
    param([Parameter(Mandatory)][string]$Url)
    $headers = @{
        'User-Agent' = 'oscan-install-latest'
        'Accept'     = 'application/vnd.github+json'
    }
    if ($env:GITHUB_TOKEN) {
        $headers['Authorization'] = "******"
    }
    Invoke-RestMethod -Uri $Url -Headers $headers
}

function Invoke-OscanInstallLatest {
    param(
        [string]$Backend = 'llvm',
        [string]$Mode = 'zip',
        [string]$Version,
        [string]$InstallDir,
        [string]$BinDir,
        [switch]$NoPathUpdate,
        [switch]$SkipChecksum
    )

    $resolvedBackend = Resolve-OscanBackend $Backend

    if ($Version) {
        $tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
        Write-Host "Querying release $tag..."
        $release = Invoke-GitHubApi "$ApiBase/tags/$tag"
    } else {
        Write-Host "Querying latest release..."
        $release = Invoke-GitHubApi "$ApiBase/latest"
    }

    $tagName = $release.tag_name
    Write-Host "Latest release: $tagName"

    $selection = Select-OscanReleaseAsset `
        -Assets @($release.assets) `
        -Tag $tagName `
        -Backend $resolvedBackend `
        -Mode $Mode
    $asset = $selection.Asset
    Write-Host "Selected the $resolvedBackend package: $($asset.name)"

    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("oscan-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

    try {
        $assetPath = Join-Path $tempRoot $asset.name
        Write-Host "Downloading $($asset.name) ($([math]::Round($asset.size / 1MB, 2)) MB)..."
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $assetPath -UseBasicParsing

        if (-not $SkipChecksum) {
            # Fail closed: a release with no SHA256SUMS cannot be verified,
            # and silently installing an unverified asset is exactly what
            # this check exists to prevent.
            $sumsAsset = $release.assets | Where-Object { $_.name -ieq 'SHA256SUMS' } | Select-Object -First 1
            if (-not $sumsAsset) {
                throw "Release $tagName publishes no SHA256SUMS, so $($asset.name) cannot be verified. Re-run with -SkipChecksum to install it anyway (not recommended)."
            }
            $sumsPath = Join-Path $tempRoot 'SHA256SUMS'
            Invoke-WebRequest -Uri $sumsAsset.browser_download_url -OutFile $sumsPath -UseBasicParsing
            $expected = Get-OscanExpectedChecksum -SumsLines @(Get-Content $sumsPath) -AssetName $asset.name
            if (-not $expected) {
                throw "Checksum for $($asset.name) not found in SHA256SUMS. Use -SkipChecksum to bypass."
            }
            $actual = (Get-FileHash -Path $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $expected) {
                throw "Checksum mismatch for $($asset.name): expected $expected, got $actual."
            }
            Write-Host "Checksum verified."
        } else {
            Write-Warning "-SkipChecksum was supplied: $($asset.name) is being installed without SHA-256 verification."
        }

        if ($selection.Kind -eq 'msi') {
            Write-Host "Installing $($asset.name) silently via msiexec..."
            $logPath = Join-Path $tempRoot 'oscan-msi.log'
            $proc = Start-Process -FilePath 'msiexec.exe' `
                -ArgumentList @('/i', "`"$assetPath`"", '/quiet', '/norestart', '/l*v', "`"$logPath`"") `
                -Wait -PassThru
            if ($proc.ExitCode -ne 0) {
                throw "msiexec exited with code $($proc.ExitCode). See log: $logPath"
            }
            Write-Host "Installed Oscan $tagName ($resolvedBackend) via MSI."
        } else {
            $extractDir = Join-Path $tempRoot 'extract'
            Write-Host "Extracting $($asset.name)..."
            Expand-Archive -Path $assetPath -DestinationPath $extractDir -Force
            $bundle = Get-ChildItem -Path $extractDir -Directory | Select-Object -First 1
            if (-not $bundle) {
                throw "Extracted archive does not contain a bundle directory."
            }
            $installScript = Join-Path $bundle.FullName 'install.ps1'
            if (-not (Test-Path $installScript)) {
                throw "install.ps1 not found in extracted bundle: $($bundle.FullName)"
            }
            $installArgs = @{}
            if ($InstallDir)   { $installArgs['InstallDir']   = $InstallDir }
            if ($BinDir)       { $installArgs['BinDir']       = $BinDir }
            if ($NoPathUpdate) { $installArgs['NoPathUpdate'] = $true }
            Write-Host "Running bundled installer..."
            & $installScript @installArgs
        }
    }
    finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tempRoot
    }

    Write-Host "Installed the $resolvedBackend package. Open a new terminal and run: oscan --help"
    if ($resolvedBackend -ne 'c') {
        Write-Host "This package builds with --backend $resolvedBackend only; --backend c, --emit-c, -o *.c, --libc and --extra-c are refused by design."
    }
}

# Dot-sourcing the script (". ./install-latest.ps1") defines the functions
# above without installing anything, so they can be unit-tested offline.
if ($MyInvocation.InvocationName -ne '.') {
    Invoke-OscanInstallLatest `
        -Backend $Backend `
        -Mode $Mode `
        -Version $Version `
        -InstallDir $InstallDir `
        -BinDir $BinDir `
        -NoPathUpdate:$NoPathUpdate `
        -SkipChecksum:$SkipChecksum
}
