<#
.SYNOPSIS
    Downloads and silently installs the latest Oscan release for Windows x86_64.

.DESCRIPTION
    Queries the GitHub Releases API for lucabol/Oscan, downloads the requested
    asset (MSI or full zip), verifies its SHA-256 checksum against the published
    SHA256SUMS file, and installs it without prompts.

    Defaults to the full zip + install.ps1 flow because it is per-user and does
    not require administrator privileges. Pass -Mode msi to use the MSI
    installer instead (this calls msiexec /quiet and may require elevation).

.PARAMETER Mode
    'zip' (default): download the full zip and run its bundled install.ps1.
    'msi': download the MSI and install it via msiexec /quiet.

.PARAMETER Version
    Optional explicit version tag (e.g. 'v0.5.0'). Defaults to the latest
    published release.

.PARAMETER InstallDir
    Forwarded to install.ps1 when -Mode zip. Ignored for MSI.

.PARAMETER NoPathUpdate
    Forwarded to install.ps1 when -Mode zip. Ignored for MSI.

.PARAMETER SkipChecksum
    Skip SHA-256 verification of the downloaded asset. Not recommended.

.EXAMPLE
    iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex

.EXAMPLE
    .\install-latest.ps1 -Mode msi
#>
[CmdletBinding()]
param(
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

# Force TLS 1.2 for older PowerShell hosts
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {}

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

$pattern = if ($Mode -eq 'msi') {
    '*windows-x86_64.msi'
} else {
    '*windows-x86_64-full.zip'
}

$asset = $release.assets | Where-Object { $_.name -like $pattern } | Select-Object -First 1
if (-not $asset) {
    throw "No asset matching '$pattern' found in release $tagName."
}

$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("oscan-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot -Force | Out-Null

try {
    $assetPath = Join-Path $tempRoot $asset.name
    Write-Host "Downloading $($asset.name) ($([math]::Round($asset.size / 1MB, 2)) MB)..."
    Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $assetPath -UseBasicParsing

    if (-not $SkipChecksum) {
        $sumsAsset = $release.assets | Where-Object { $_.name -ieq 'SHA256SUMS' } | Select-Object -First 1
        if ($sumsAsset) {
            $sumsPath = Join-Path $tempRoot 'SHA256SUMS'
            Invoke-WebRequest -Uri $sumsAsset.browser_download_url -OutFile $sumsPath -UseBasicParsing
            $expected = $null
            foreach ($line in Get-Content $sumsPath) {
                $parts = $line -split '\s+', 2
                if ($parts.Length -eq 2 -and ($parts[1].Trim().TrimStart('*')) -eq $asset.name) {
                    $expected = $parts[0].ToLowerInvariant()
                    break
                }
            }
            if (-not $expected) {
                throw "Checksum for $($asset.name) not found in SHA256SUMS. Use -SkipChecksum to bypass."
            }
            $actual = (Get-FileHash -Path $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($actual -ne $expected) {
                throw "Checksum mismatch for $($asset.name): expected $expected, got $actual."
            }
            Write-Host "Checksum verified."
        } else {
            Write-Warning "SHA256SUMS not published for this release; skipping verification."
        }
    }

    if ($Mode -eq 'msi') {
        Write-Host "Installing $($asset.name) silently via msiexec..."
        $logPath = Join-Path $tempRoot 'oscan-msi.log'
        $proc = Start-Process -FilePath 'msiexec.exe' `
            -ArgumentList @('/i', "`"$assetPath`"", '/quiet', '/norestart', '/l*v', "`"$logPath`"") `
            -Wait -PassThru
        if ($proc.ExitCode -ne 0) {
            throw "msiexec exited with code $($proc.ExitCode). See log: $logPath"
        }
        Write-Host "Installed Oscan $tagName via MSI."
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

Write-Host "Done. Open a new terminal and run: oscan --help"
