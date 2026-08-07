<#
.SYNOPSIS
    Downloads and installs a coexistence-safe Oscan package profile for
    Windows x86_64.

.DESCRIPTION
    Windows publishes a full package containing all backends and three slim
    packages (LLVM, Cranelift, and C). Archive installs use profile-specific
    roots and qualified commands, so installing one profile does not remove
    another. The unqualified oscan command is created when no selector exists
    and changes only when -SetDefault is supplied.

    This script queries the GitHub Releases API for lucabol/Oscan, picks the
    asset whose name is exactly the one the release contract derives from the
    resolved tag and the requested profile, verifies its SHA-256 against the
    release's SHA256SUMS file, and installs it without prompts. No fuzzy,
    suffix or version-drift matching is performed: if the exact name is not
    published, the script fails with the list of assets that are.

    The slim LLVM package remains the transition default. Use -Profile full
    for one compiler with --backend llvm|cranelift|c. The zip flow is the
    default because it supports profile coexistence without administrator
    privileges. The legacy LLVM MSI remains the only MSI and is intentionally
    a separate, single-product install surface.

.PARAMETER Profile
    'slim' (default) selects the package named by -Backend. 'full' installs
    one compiler containing LLVM, Cranelift, and C.

.PARAMETER Backend
    For -Profile slim: 'llvm' (default, recommended), 'cranelift', or 'c'.
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
    Root under which profile-specific payloads and the stable bin directory
    are installed. Forwarded to install.ps1 for zip installs. Ignored for MSI.

.PARAMETER SetDefault
    Select the installed archive profile for the unqualified oscan command.
    Without this switch, an existing selection is preserved.

.PARAMETER NoPathUpdate
    Forwarded to install.ps1 when installing a zip. Ignored for MSI.

.PARAMETER SkipChecksum
    Skip SHA-256 verification of the downloaded asset. Not recommended.
    Without it, a release that publishes no SHA256SUMS is a hard error rather
    than a silently unverified install.

.EXAMPLE
    iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex

.EXAMPLE
    .\install-latest.ps1 -Profile full -SetDefault

.EXAMPLE
    .\install-latest.ps1 -Profile slim -Backend cranelift
#>
[CmdletBinding()]
param(
    [ValidateSet('slim', 'full')]
    [string]$Profile = 'slim',
    [ValidateSet('llvm', 'cranelift', 'c', 'native')]
    [string]$Backend = 'llvm',
    [ValidateSet('zip', 'msi')]
    [string]$Mode = 'zip',
    [string]$Version,
    [string]$InstallDir,
    [string]$BinDir,
    [switch]$SetDefault,
    [switch]$NoPathUpdate,
    [switch]$SkipChecksum,
    [switch]$AllowMsiCommandConflict
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'lucabol/Oscan'
$ApiBase = "https://api.github.com/repos/$Repo/releases"

# The one Windows target this installer serves, and the only profile that
# publishes an installer.
$InstallerTarget = 'windows-x86_64'
$MsiProfile = 'llvm'

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

function Resolve-OscanPackageProfile {
    param(
        [Parameter(Mandatory)][ValidateSet('slim', 'full')][string]$Profile,
        [Parameter(Mandatory)][string]$Backend
    )

    if ($Profile -eq 'full') {
        return 'full'
    }
    return Resolve-OscanBackend $Backend
}

function Get-OscanAssetName {
    <#
        The exact canonical asset name for a (tag, profile, kind) triple, as
        rendered by packaging/toolchains/release-contract.json. Nothing here
        globs: an installer must never resolve to another backend's package.
    #>
    param(
        [Parameter(Mandatory)][string]$Tag,
        [Parameter(Mandatory)][ValidateSet('full', 'llvm', 'cranelift', 'c')][string]$Profile,
        [Parameter(Mandatory)][ValidateSet('zip', 'msi')][string]$Kind
    )

    if ($Kind -eq 'msi' -and $Profile -ne $MsiProfile) {
        throw "Only the recommended $MsiProfile profile is published as an MSI; the $Profile profile ships as a zip archive."
    }
    return "oscan-$Tag-$InstallerTarget-$Profile.$Kind"
}

function Select-OscanReleaseAsset {
    <#
        Pick the exact canonical asset for this profile out of a release's
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
        [Parameter(Mandatory)][ValidateSet('full', 'llvm', 'cranelift', 'c')][string]$Profile,
        [Parameter(Mandatory)][ValidateSet('zip', 'msi')][string]$Mode
    )

    if ($Mode -eq 'msi' -and $Profile -ne $MsiProfile) {
        throw "-Mode msi is only available for the recommended $MsiProfile profile. Install '$Profile' with -Mode zip."
    }

    $wanted = @()
    if ($Mode -eq 'msi') {
        $wanted += [PSCustomObject]@{ Kind = 'msi'; Name = (Get-OscanAssetName -Tag $Tag -Profile $Profile -Kind 'msi') }
    }
    $wanted += [PSCustomObject]@{ Kind = 'zip'; Name = (Get-OscanAssetName -Tag $Tag -Profile $Profile -Kind 'zip') }

    foreach ($candidate in $wanted) {
        $match = @($Assets | Where-Object { $_.name -ieq $candidate.Name })
        if ($match.Count -gt 1) {
            throw "Release $Tag publishes more than one asset named '$($candidate.Name)'; refusing to guess which one to install."
        }
        if ($match.Count -eq 1) {
            if ($candidate.Kind -ne $Mode) {
                Write-Warning "Release $Tag publishes no $Profile MSI; installing $($match[0].name) instead."
            }
            return [PSCustomObject]@{ Asset = $match[0]; Kind = $candidate.Kind }
        }
    }

    $published = @($Assets | Where-Object { $_.name } | ForEach-Object { $_.name } | Sort-Object)
    $expected = ($wanted | ForEach-Object { $_.Name }) -join ' or '
    throw "Release $Tag publishes no $expected. Asset names are derived from the release tag, and only that exact name is accepted. Windows releases ship full, llvm, cranelift, and c archives plus one recommended llvm MSI. Published assets: $(if ($published) { $published -join ', ' } else { '(none)' })."
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

function Assert-OscanArchiveBundle {
    param(
        [Parameter(Mandatory)][string]$BundleDir,
        [Parameter(Mandatory)][ValidateSet('full', 'llvm', 'cranelift', 'c')][string]$ExpectedProfile
    )

    $metadataPath = Join-Path $BundleDir 'oscan-package.json'
    if (-not (Test-Path -LiteralPath $metadataPath -PathType Leaf)) {
        throw "The downloaded archive has no profile-aware oscan-package.json. Legacy archives contain a destructive flat installer and cannot be installed over coexistence-safe profiles; extract it as a portable bundle or choose a schema-3 release."
    }
    try {
        $metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
    } catch {
        throw "The downloaded archive has unreadable oscan-package.json: $($_.Exception.Message)"
    }
    if ($metadata.schema_version -ne 2 -or
        $metadata.target -ne $InstallerTarget -or
        $metadata.profile -ne $ExpectedProfile -or
        $metadata.package_id -ne "oscan-$ExpectedProfile" -or
        $metadata.is_distribution -ne $true) {
        throw "The downloaded archive metadata does not identify schema-2 package 'oscan-$ExpectedProfile' for '$InstallerTarget'; refusing to run its installer."
    }
    $expectedBackends = if ($ExpectedProfile -eq 'full') {
        @('llvm', 'cranelift', 'c')
    } else {
        @($ExpectedProfile)
    }
    if ((@($metadata.available_backends) -join ',') -ne ($expectedBackends -join ',')) {
        throw "The downloaded archive's backend inventory does not match profile '$ExpectedProfile'."
    }
    $expectedDefault = if ($ExpectedProfile -eq 'full') { 'llvm' } else { $ExpectedProfile }
    if ($metadata.default_backend -ne $expectedDefault) {
        throw "The downloaded archive's default backend does not match profile '$ExpectedProfile'."
    }
    return $metadata
}

function Invoke-GitHubApi {
    param([Parameter(Mandatory)][string]$Url)
    $headers = @{
        'User-Agent' = 'oscan-install-latest'
        'Accept'     = 'application/vnd.github+json'
    }
    if ($env:GITHUB_TOKEN) {
        $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN"
    }
    Invoke-RestMethod -Uri $Url -Headers $headers
}

function Invoke-OscanInstallLatest {
    param(
        [string]$Profile = 'slim',
        [string]$Backend = 'llvm',
        [string]$Mode = 'zip',
        [string]$Version,
        [string]$InstallDir,
        [string]$BinDir,
        [switch]$SetDefault,
        [switch]$NoPathUpdate,
        [switch]$SkipChecksum,
        [switch]$AllowMsiCommandConflict
    )

    $packageProfile = Resolve-OscanPackageProfile -Profile $Profile -Backend $Backend
    if ($Mode -eq 'msi' -and $SetDefault) {
        throw "-SetDefault applies to coexistence-safe zip profiles. The legacy LLVM MSI owns its own unqualified PATH command."
    }

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
        -Profile $packageProfile `
        -Mode $Mode
    $asset = $selection.Asset
    Write-Host "Selected the $packageProfile profile: $($asset.name)"

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
            $archiveRoot = if ($InstallDir) {
                [System.IO.Path]::GetFullPath($InstallDir)
            } else {
                Join-Path $env:LOCALAPPDATA 'Programs\Oscan'
            }
            $archiveSelector = Join-Path $archiveRoot 'default-profile'
            $archiveCommand = Join-Path (Join-Path $archiveRoot 'bin') 'oscan.cmd'
            $legacyArchiveCommand = Join-Path $archiveRoot 'oscan.exe'
            $archiveMarker = Join-Path $env:LOCALAPPDATA 'Programs\Oscan\archive-default'
            if (-not $AllowMsiCommandConflict -and
                ((Test-Path -LiteralPath $archiveSelector) -or
                 (Test-Path -LiteralPath $archiveCommand) -or
                 (Test-Path -LiteralPath $legacyArchiveCommand) -or
                 (Test-Path -LiteralPath $archiveMarker))) {
                throw "A per-user archive command is installed under '$archiveRoot'. The legacy LLVM MSI also exports an unqualified oscan command, so PATH order would be ambiguous. Use the zip profile, remove the archive selector/legacy install first, or pass -AllowMsiCommandConflict to acknowledge the collision."
            }
            Write-Host "Installing $($asset.name) silently via msiexec..."
            $logPath = Join-Path $tempRoot 'oscan-msi.log'
            $msiArgs = @('/i', "`"$assetPath`"", '/quiet', '/norestart', '/l*v', "`"$logPath`"")
            if ($AllowMsiCommandConflict) {
                $msiArgs += 'OSCAN_ALLOW_ARCHIVE_CONFLICT=1'
            }
            $proc = Start-Process -FilePath 'msiexec.exe' `
                -ArgumentList $msiArgs `
                -Wait -PassThru
            if ($proc.ExitCode -ne 0) {
                throw "msiexec exited with code $($proc.ExitCode). See log: $logPath"
            }
            Write-Host "Installed Oscan $tagName ($packageProfile) via MSI."
        } else {
            $extractDir = Join-Path $tempRoot 'extract'
            Write-Host "Extracting $($asset.name)..."
            Expand-Archive -Path $assetPath -DestinationPath $extractDir -Force
            $bundles = @(Get-ChildItem -Path $extractDir -Directory)
            if ($bundles.Count -ne 1) {
                throw "Extracted archive must contain exactly one bundle directory; found $($bundles.Count)."
            }
            $bundle = $bundles[0]
            Assert-OscanArchiveBundle -BundleDir $bundle.FullName -ExpectedProfile $packageProfile | Out-Null
            $installScript = Join-Path $bundle.FullName 'install.ps1'
            if (-not (Test-Path $installScript)) {
                throw "install.ps1 not found in extracted bundle: $($bundle.FullName)"
            }
            $installArgs = @{}
            if ($InstallDir)   { $installArgs['InstallRoot']  = $InstallDir }
            if ($BinDir)       { $installArgs['BinDir']       = $BinDir }
            if ($SetDefault)   { $installArgs['SetDefault']   = $true }
            if ($NoPathUpdate) { $installArgs['NoPathUpdate'] = $true }
            if ($AllowMsiCommandConflict) {
                $installArgs['AllowMsiCommandConflict'] = $true
            }
            Write-Host "Running bundled installer..."
            & $installScript @installArgs
        }
    }
    finally {
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tempRoot
    }

    Write-Host "Installed the $packageProfile profile. Qualified command: oscan-$packageProfile --help"
    if ($packageProfile -eq 'full') {
        Write-Host "Select LLVM, Cranelift, or C with --backend."
    } elseif ($packageProfile -ne 'c') {
        Write-Host "This slim profile builds with --backend $packageProfile only; C-specific modes are refused by design."
    }
}

# Dot-sourcing the script (". ./install-latest.ps1") defines the functions
# above without installing anything, so they can be unit-tested offline.
if ($MyInvocation.InvocationName -ne '.') {
    Invoke-OscanInstallLatest `
        -Profile $Profile `
        -Backend $Backend `
        -Mode $Mode `
        -Version $Version `
        -InstallDir $InstallDir `
        -BinDir $BinDir `
        -SetDefault:$SetDefault `
        -NoPathUpdate:$NoPathUpdate `
        -SkipChecksum:$SkipChecksum `
        -AllowMsiCommandConflict:$AllowMsiCommandConflict
}
