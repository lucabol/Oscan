[CmdletBinding()]
param(
    [string]$SourceDir = $PSScriptRoot,
    [Alias("InstallDir")]
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "Programs\Oscan"),
    [string]$BinDir,
    [ValidateSet("full", "llvm", "cranelift", "c", "dev")]
    [string]$Profile,
    [switch]$SetDefault,
    [switch]$Uninstall,
    [switch]$NoPathUpdate,
    [switch]$AllowMsiCommandConflict
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Add-UserPathEntry {
    param(
        [Parameter(Mandatory = $true)][string]$Entry,
        [string]$LegacyEntry
    )

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = if ($current) {
        @($current.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries))
    } else {
        @()
    }
    $filtered = @($parts | Where-Object {
        $_ -ne $Entry -and (-not $LegacyEntry -or $_ -ne $LegacyEntry)
    })
    [Environment]::SetEnvironmentVariable("Path", (@($Entry) + $filtered -join ';'), "User")

    $processParts = @($env:Path.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries) |
        Where-Object { $_ -ne $Entry -and (-not $LegacyEntry -or $_ -ne $LegacyEntry) })
    $env:Path = @($Entry) + $processParts -join ';'
}

function Write-AtomicText {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )

    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    $temporary = Join-Path $parent (".tmp-" + [guid]::NewGuid().ToString("N"))
    try {
        Set-Content -LiteralPath $temporary -Value $Content -Encoding ASCII -NoNewline
        Move-Item -LiteralPath $temporary -Destination $Path -Force
    } finally {
        Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue
    }
}

function Write-ProfileShim {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$RelativeExecutable
    )

    Write-AtomicText -Path $Path -Content "@echo off`r`n`"%~dp0$RelativeExecutable`" %*`r`n"
}

function Write-DefaultShim {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$SelectedProfile
    )

    Write-AtomicText -Path $Path `
        -Content "@echo off`r`ncall `"%~dp0oscan-$SelectedProfile.cmd`" %*`r`n"
}

function Restore-AtomicFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][bool]$Existed,
        [AllowEmptyString()][string]$Content
    )

    if ($Existed) {
        Write-AtomicText -Path $Path -Content $Content
    } else {
        Remove-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
    }
}

function Ensure-ProfileJunction {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Target
    )

    if (Test-Path -LiteralPath $Path) {
        $item = Get-Item -LiteralPath $Path -Force
        if (-not $item.PSIsContainer -or
            -not ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
            throw "Profile-link path '$Path' already exists and is not a managed directory junction."
        }
        $existingTarget = [string]@($item.Target)[0]
        if (-not [System.IO.Path]::IsPathRooted($existingTarget)) {
            $existingTarget = Join-Path (Split-Path -Parent $Path) $existingTarget
        }
        if (-not [string]::Equals(
                [System.IO.Path]::GetFullPath($existingTarget).TrimEnd('\'),
                [System.IO.Path]::GetFullPath($Target).TrimEnd('\'),
                [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Profile-link junction '$Path' points somewhere other than '$Target'."
        }
        return $false
    }

    New-Item -ItemType Junction -Path $Path -Target $Target -ErrorAction Stop | Out-Null
    return $true
}

function Test-JunctionPointsTo {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Target
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $false
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (-not $item.PSIsContainer -or
        -not ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        return $false
    }
    $existingTarget = [string]@($item.Target)[0]
    if (-not [System.IO.Path]::IsPathRooted($existingTarget)) {
        $existingTarget = Join-Path (Split-Path -Parent $Path) $existingTarget
    }
    return [string]::Equals(
        [System.IO.Path]::GetFullPath($existingTarget).TrimEnd('\'),
        [System.IO.Path]::GetFullPath($Target).TrimEnd('\'),
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Test-LegacyProfileShimOwnedByRoot {
    param(
        [Parameter(Mandatory = $true)][string]$BinDir,
        [Parameter(Mandatory = $true)][string]$InstallRoot,
        [Parameter(Mandatory = $true)][string]$ProfilesRoot,
        [Parameter(Mandatory = $true)][string]$Profile
    )

    $shim = Join-Path $BinDir "oscan-$Profile.cmd"
    if (-not (Test-Path -LiteralPath $shim -PathType Leaf)) {
        return $false
    }
    $content = Get-Content -LiteralPath $shim -Raw
    $defaultBinDir = [System.IO.Path]::GetFullPath(
        (Join-Path $InstallRoot "bin")
    ).TrimEnd('\')
    if ([string]::Equals(
            $BinDir.TrimEnd('\'),
            $defaultBinDir,
            [System.StringComparison]::OrdinalIgnoreCase)) {
        return $content.Contains("%~dp0..\profiles\$Profile\")
    }

    $profileJunction = Join-Path $BinDir ".oscan-profiles"
    return (Test-JunctionPointsTo -Path $profileJunction -Target $ProfilesRoot) -and
        $content.Contains("%~dp0.oscan-profiles\$Profile\")
}

function Get-SelectorOwnership {
    param(
        [Parameter(Mandatory = $true)][string]$CommandPath,
        [Parameter(Mandatory = $true)][string]$OwnerPath,
        [Parameter(Mandatory = $true)][string]$OwnerId,
        [Parameter(Mandatory = $true)][bool]$LegacyOwned
    )

    if (Test-Path -LiteralPath $OwnerPath) {
        $recordedOwner = (Get-Content -LiteralPath $OwnerPath -Raw).Trim()
        if ($recordedOwner -eq $OwnerId) {
            return "Current"
        }
        return "Foreign"
    }
    if (Test-Path -LiteralPath $CommandPath) {
        if ($LegacyOwned) {
            return "Current"
        }
        return "Unmanaged"
    }
    return "Absent"
}

function Copy-InstallTree {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    New-Item -ItemType Directory -Path $Destination -Force | Out-Null
    $robocopy = Get-Command robocopy.exe -ErrorAction SilentlyContinue
    if ($robocopy) {
        & $robocopy.Source $Source $Destination /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
        if ($LASTEXITCODE -gt 7) {
            throw "robocopy failed while staging the install (exit code $LASTEXITCODE)."
        }
    } else {
        Copy-Item (Join-Path $Source "*") -Destination $Destination -Recurse -Force
    }
}

function Test-OscanMsiInstalled {
    $installer = New-Object -ComObject WindowsInstaller.Installer
    try {
        $products = @(
            $installer.RelatedProducts("{F7A3B2C1-4D5E-6F78-9A0B-1C2D3E4F5A6B}")
        )
        return $products.Count -gt 0
    } finally {
        [void][System.Runtime.InteropServices.Marshal]::FinalReleaseComObject($installer)
    }
}

function Enter-OscanInstallLock {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Name
    )

    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    $path = Join-Path $Root $Name
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($true) {
        try {
            return [System.IO.File]::Open(
                $path,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            if ([DateTime]::UtcNow -ge $deadline) {
                throw "Timed out waiting for another Oscan install or uninstall to finish at '$Root'."
            }
            Start-Sleep -Milliseconds 100
        }
    }
}

$SourceDir = [System.IO.Path]::GetFullPath($SourceDir)
$resolvedInstallRoot = [System.IO.Path]::GetFullPath($InstallRoot)
if ([System.IO.Path]::GetPathRoot($resolvedInstallRoot) -eq $resolvedInstallRoot) {
    throw "Refusing to use a drive root as the Oscan install root: $InstallRoot"
}
$InstallRoot = $resolvedInstallRoot.TrimEnd([char[]]"\/")
if (-not $BinDir) {
    $BinDir = Join-Path $InstallRoot "bin"
}
$resolvedBinDir = [System.IO.Path]::GetFullPath($BinDir)
$BinDir = if ([System.IO.Path]::GetPathRoot($resolvedBinDir) -eq $resolvedBinDir) {
    $resolvedBinDir
} else {
    $resolvedBinDir.TrimEnd([char[]]"\/")
}

$installLockPath = Join-Path $InstallRoot ".install.lock"
$selectorLockPath = Join-Path $BinDir ".oscan-selectors.lock"
$installLock = Enter-OscanInstallLock -Root $InstallRoot -Name ".install.lock"
$selectorLock = $null
try {
$selectorLock = Enter-OscanInstallLock -Root $BinDir -Name ".oscan-selectors.lock"
$metadataPath = Join-Path $SourceDir "oscan-package.json"
$metadata = $null
if (Test-Path -LiteralPath $metadataPath) {
    $metadata = Get-Content -LiteralPath $metadataPath -Raw | ConvertFrom-Json
    if ($metadata.schema_version -ne 2) {
        throw "Unsupported oscan-package.json schema '$($metadata.schema_version)'; expected 2."
    }
    if ($metadata.profile -notin @("full", "llvm", "cranelift", "c")) {
        throw "Package metadata contains unknown profile '$($metadata.profile)'."
    }
    if ($Profile -and $Profile -ne $metadata.profile) {
        throw "Requested profile '$Profile' does not match package profile '$($metadata.profile)'."
    }
    $Profile = [string]$metadata.profile
    $version = [string]$metadata.version
    if (-not $version) {
        throw "Package metadata does not declare a version."
    }
    if ($metadata.package_id -ne "oscan-$Profile") {
        throw "Package metadata package_id '$($metadata.package_id)' does not match profile '$Profile'."
    }
    if ($metadata.target -ne "windows-x86_64") {
        throw "Package metadata target '$($metadata.target)' cannot be installed by the Windows x86_64 installer."
    }
    if ($metadata.is_distribution -ne $true) {
        throw "Package metadata must identify a packaged distribution."
    }
    $expectedBackends = if ($Profile -eq "full") {
        @("llvm", "cranelift", "c")
    } else {
        @($Profile)
    }
    $actualBackends = @($metadata.available_backends)
    if (($actualBackends -join ",") -ne ($expectedBackends -join ",")) {
        throw "Package metadata backends '$($actualBackends -join ", ")' do not match profile '$Profile'."
    }
    $expectedDefault = if ($Profile -eq "full") { "llvm" } else { $Profile }
    if ($metadata.default_backend -ne $expectedDefault) {
        throw "Package metadata default backend '$($metadata.default_backend)' does not match profile '$Profile' (expected '$expectedDefault')."
    }
    $expectedCompilerDigest = [string]$metadata.component_digests.'oscan.exe'
    if ($expectedCompilerDigest -notmatch '^[0-9a-fA-F]{64}$') {
        throw "Package metadata does not declare a valid SHA-256 digest for oscan.exe."
    }
} elseif ($Uninstall -and $Profile) {
    $version = $null
} elseif ($Profile -eq "dev") {
    $version = "current"
    $expectedCompilerDigest = $null
} else {
    throw "Source bundle must contain schema-2 oscan-package.json; development installs must pass -Profile dev."
}
if (-not $Uninstall) {
    if ($version -notmatch '^[A-Za-z0-9][A-Za-z0-9._+-]*$') {
        throw "Package metadata contains unsafe version '$version'."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $SourceDir "oscan.exe"))) {
        throw "Source bundle must contain oscan.exe."
    }
    if ($expectedCompilerDigest) {
        $actualCompilerDigest = (Get-FileHash -LiteralPath (Join-Path $SourceDir "oscan.exe") -Algorithm SHA256).Hash
        if ($actualCompilerDigest -ne $expectedCompilerDigest) {
            throw "oscan.exe digest mismatch: package metadata has $expectedCompilerDigest, actual is $actualCompilerDigest."
        }
    }
}

$profilesRoot = Join-Path $InstallRoot "profiles"
$profileRoot = Join-Path $profilesRoot $Profile
$qualifiedShim = Join-Path $BinDir "oscan-$Profile.cmd"
$defaultShim = Join-Path $BinDir "oscan.cmd"
$defaultState = Join-Path $InstallRoot "default-profile"
$archiveMarkerBase = Join-Path $env:LOCALAPPDATA "Programs\Oscan"
$archiveDefaultMarker = Join-Path $archiveMarkerBase "archive-default"
$archiveOwnerRoot = Join-Path $archiveMarkerBase "archive-defaults"
$archiveOwnerBytes = [System.Text.Encoding]::UTF8.GetBytes(
    "$($InstallRoot.ToUpperInvariant())`n$($BinDir.ToUpperInvariant())"
)
$archiveOwnerHash = [System.Security.Cryptography.SHA256]::Create()
try {
    $archiveOwnerName = -join (
        $archiveOwnerHash.ComputeHash($archiveOwnerBytes) |
            ForEach-Object { $_.ToString("x2") }
    )
} finally {
    $archiveOwnerHash.Dispose()
}
$archiveOwnerMarker = Join-Path $archiveOwnerRoot $archiveOwnerName
$selectorOwnerId = $archiveOwnerName
$qualifiedOwner = Join-Path $BinDir ".oscan-$Profile.owner"
$defaultOwner = Join-Path $BinDir ".oscan-default.owner"
$qualifiedLegacyOwned = Test-LegacyProfileShimOwnedByRoot `
    -BinDir $BinDir `
    -InstallRoot $InstallRoot `
    -ProfilesRoot $profilesRoot `
    -Profile $Profile
$qualifiedOwnership = Get-SelectorOwnership `
    -CommandPath $qualifiedShim `
    -OwnerPath $qualifiedOwner `
    -OwnerId $selectorOwnerId `
    -LegacyOwned $qualifiedLegacyOwned
$selected = if (Test-Path -LiteralPath $defaultState) {
    (Get-Content -LiteralPath $defaultState -Raw).Trim()
} else {
    ""
}
if ($selected -and $selected -notin @("full", "llvm", "cranelift", "c", "dev")) {
    throw "Default-profile state contains unknown profile '$selected'."
}
$selectedQualifiedOwnership = "Absent"
if ($selected) {
    $selectedQualifiedShim = Join-Path $BinDir "oscan-$selected.cmd"
    $selectedQualifiedOwner = Join-Path $BinDir ".oscan-$selected.owner"
    $selectedLegacyOwned = Test-LegacyProfileShimOwnedByRoot `
        -BinDir $BinDir `
        -InstallRoot $InstallRoot `
        -ProfilesRoot $profilesRoot `
        -Profile $selected
    $selectedQualifiedOwnership = Get-SelectorOwnership `
        -CommandPath $selectedQualifiedShim `
        -OwnerPath $selectedQualifiedOwner `
        -OwnerId $selectorOwnerId `
        -LegacyOwned $selectedLegacyOwned
}
$defaultLegacyOwned = $false
if ($selected -and
    $selectedQualifiedOwnership -eq "Current" -and
    (Test-Path -LiteralPath $defaultShim -PathType Leaf)) {
    $defaultLegacyOwned = (Get-Content -LiteralPath $defaultShim -Raw).
        Contains("oscan-$selected.cmd")
}
$defaultOwnership = Get-SelectorOwnership `
    -CommandPath $defaultShim `
    -OwnerPath $defaultOwner `
    -OwnerId $selectorOwnerId `
    -LegacyOwned $defaultLegacyOwned

if ($Uninstall) {
    $uninstallBackup = Join-Path $profilesRoot (
        ".$Profile-uninstall-" + [guid]::NewGuid().ToString("N")
    )
    $payloadStagedForRemoval = $false
    $qualifiedShimRemoved = $false
    $qualifiedOwnerRemoved = $false
    $defaultShimRemoved = $false
    $defaultOwnerRemoved = $false
    $stateRemoved = $false
    $ownerMarkerRemoved = $false
    $defaultMarkerRemoved = $false
    $uninstallCommitted = $false

    $qualifiedExisted = Test-Path -LiteralPath $qualifiedShim -PathType Leaf
    $qualifiedContent = if ($qualifiedExisted) {
        Get-Content -LiteralPath $qualifiedShim -Raw
    } else { "" }
    $qualifiedOwnerExisted = Test-Path -LiteralPath $qualifiedOwner -PathType Leaf
    $qualifiedOwnerContent = if ($qualifiedOwnerExisted) {
        Get-Content -LiteralPath $qualifiedOwner -Raw
    } else { "" }
    $defaultExisted = Test-Path -LiteralPath $defaultShim -PathType Leaf
    $defaultContent = if ($defaultExisted) {
        Get-Content -LiteralPath $defaultShim -Raw
    } else { "" }
    $defaultOwnerExisted = Test-Path -LiteralPath $defaultOwner -PathType Leaf
    $defaultOwnerContent = if ($defaultOwnerExisted) {
        Get-Content -LiteralPath $defaultOwner -Raw
    } else { "" }
    $stateExisted = Test-Path -LiteralPath $defaultState -PathType Leaf
    $stateContent = if ($stateExisted) {
        Get-Content -LiteralPath $defaultState -Raw
    } else { "" }
    $ownerMarkerExisted = Test-Path -LiteralPath $archiveOwnerMarker -PathType Leaf
    $ownerMarkerContent = if ($ownerMarkerExisted) {
        Get-Content -LiteralPath $archiveOwnerMarker -Raw
    } else { "" }
    $defaultMarkerExisted = Test-Path -LiteralPath $archiveDefaultMarker -PathType Leaf
    $defaultMarkerContent = if ($defaultMarkerExisted) {
        Get-Content -LiteralPath $archiveDefaultMarker -Raw
    } else { "" }

    try {
        if (Test-Path -LiteralPath $profileRoot) {
            Move-Item -LiteralPath $profileRoot -Destination $uninstallBackup -ErrorAction Stop
            $payloadStagedForRemoval = $true
        }
        if ($qualifiedOwnership -eq "Current") {
            if ($qualifiedExisted) {
                Remove-Item -LiteralPath $qualifiedShim -Force -ErrorAction Stop
                $qualifiedShimRemoved = $true
            }
            if ($qualifiedOwnerExisted) {
                Remove-Item -LiteralPath $qualifiedOwner -Force -ErrorAction Stop
                $qualifiedOwnerRemoved = $true
            }
        }
        if ($selected -eq $Profile) {
            if ($defaultOwnership -eq "Current") {
                if ($defaultExisted) {
                    Remove-Item -LiteralPath $defaultShim -Force -ErrorAction Stop
                    $defaultShimRemoved = $true
                }
                if ($defaultOwnerExisted) {
                    Remove-Item -LiteralPath $defaultOwner -Force -ErrorAction Stop
                    $defaultOwnerRemoved = $true
                }
            }
            if ($stateExisted) {
                Remove-Item -LiteralPath $defaultState -Force -ErrorAction Stop
                $stateRemoved = $true
            }
            if ($ownerMarkerExisted) {
                Remove-Item -LiteralPath $archiveOwnerMarker -Force -ErrorAction Stop
                $ownerMarkerRemoved = $true
            }
            $otherArchiveOwners = if (Test-Path -LiteralPath $archiveOwnerRoot) {
                @(Get-ChildItem -LiteralPath $archiveOwnerRoot -File -ErrorAction Stop)
            } else {
                @()
            }
            if ($otherArchiveOwners.Count -eq 0 -and $defaultMarkerExisted) {
                Remove-Item -LiteralPath $archiveDefaultMarker -Force -ErrorAction Stop
                $defaultMarkerRemoved = $true
            }
        }
        $uninstallCommitted = $true
    } catch {
        $uninstallError = $_
        $rollbackErrors = @()
        if ($payloadStagedForRemoval -and (Test-Path -LiteralPath $uninstallBackup)) {
            try {
                Move-Item -LiteralPath $uninstallBackup -Destination $profileRoot -ErrorAction Stop
            } catch {
                $rollbackErrors += "restore profile payload: $($_.Exception.Message)"
            }
        }
        foreach ($restore in @(
                [pscustomobject]@{ Path = $qualifiedShim; Existed = $qualifiedExisted; Content = $qualifiedContent; Changed = $qualifiedShimRemoved },
                [pscustomobject]@{ Path = $qualifiedOwner; Existed = $qualifiedOwnerExisted; Content = $qualifiedOwnerContent; Changed = $qualifiedOwnerRemoved },
                [pscustomobject]@{ Path = $defaultShim; Existed = $defaultExisted; Content = $defaultContent; Changed = $defaultShimRemoved },
                [pscustomobject]@{ Path = $defaultOwner; Existed = $defaultOwnerExisted; Content = $defaultOwnerContent; Changed = $defaultOwnerRemoved },
                [pscustomobject]@{ Path = $defaultState; Existed = $stateExisted; Content = $stateContent; Changed = $stateRemoved },
                [pscustomobject]@{ Path = $archiveOwnerMarker; Existed = $ownerMarkerExisted; Content = $ownerMarkerContent; Changed = $ownerMarkerRemoved },
                [pscustomobject]@{ Path = $archiveDefaultMarker; Existed = $defaultMarkerExisted; Content = $defaultMarkerContent; Changed = $defaultMarkerRemoved }
            )) {
            if ($restore.Changed) {
                try {
                    Restore-AtomicFile `
                        -Path ([string]$restore.Path) `
                        -Existed ([bool]$restore.Existed) `
                        -Content ([string]$restore.Content)
                } catch {
                    $rollbackErrors += "restore '$($restore.Path)': $($_.Exception.Message)"
                }
            }
        }
        if ($rollbackErrors.Count -gt 0) {
            throw "Uninstalling profile '$Profile' failed: $($uninstallError.Exception.Message). Rollback also failed: $($rollbackErrors -join '; ')."
        }
        throw $uninstallError
    }
    if ($uninstallCommitted -and (Test-Path -LiteralPath $uninstallBackup)) {
        try {
            Remove-Item -LiteralPath $uninstallBackup -Recurse -Force -ErrorAction Stop
        } catch {
            Write-Warning "The profile was deactivated, but its staged payload could not be deleted from '$uninstallBackup': $($_.Exception.Message)"
        }
    }
    if ($qualifiedOwnership -in @("Foreign", "Unmanaged")) {
        Write-Warning "Preserved '$qualifiedShim' because it is not owned by install root '$InstallRoot'."
    }
    if ($selected -eq $Profile) {
        Write-Warning "Removed the selected '$Profile' profile. The unqualified oscan command now has no default for this install root; reinstall or use -SetDefault with another profile."
    }
    Write-Host "Uninstalled Oscan profile '$Profile'; other profiles were preserved."
    return
}

$selectedBeforeInstall = $selected
if ($qualifiedOwnership -in @("Foreign", "Unmanaged")) {
    throw "Refusing to replace qualified command '$qualifiedShim' because it is not owned by install root '$InstallRoot'. Use a different -BinDir or uninstall its owning profile first."
}
if ($selectedBeforeInstall -and $defaultOwnership -notin @("Current")) {
    throw "Default-profile state at '$defaultState' does not own the shared selector '$defaultShim'. Repair or remove the stale state before installing into this root."
}
$willSelectProfile = $SetDefault -or
    (-not $selectedBeforeInstall -and -not (Test-Path -LiteralPath $defaultShim))
if ($willSelectProfile -and $defaultOwnership -in @("Foreign", "Unmanaged")) {
    throw "Refusing to replace default command '$defaultShim' because it is not owned by install root '$InstallRoot'. Uninstall its owning default or use a different -BinDir."
}
if ($willSelectProfile -and -not $AllowMsiCommandConflict) {
    if (Test-OscanMsiInstalled) {
        throw "The legacy LLVM MSI is installed and already owns an unqualified oscan command. Installing an archive default would make PATH order ambiguous. Install the qualified profile without -SetDefault after creating an archive selector explicitly, uninstall the MSI, or pass -AllowMsiCommandConflict to acknowledge the collision."
    }
}

$versionDir = Join-Path $profileRoot $version
$staged = Join-Path $profileRoot (".install-$version-" + [guid]::NewGuid().ToString("N"))
$backup = Join-Path $profileRoot (".backup-$version-" + [guid]::NewGuid().ToString("N"))
$qualifiedExisted = Test-Path -LiteralPath $qualifiedShim
$qualifiedContent = if ($qualifiedExisted) {
    Get-Content -LiteralPath $qualifiedShim -Raw
} else {
    ""
}
$qualifiedOwnerExisted = Test-Path -LiteralPath $qualifiedOwner
$qualifiedOwnerContent = if ($qualifiedOwnerExisted) {
    Get-Content -LiteralPath $qualifiedOwner -Raw
} else {
    ""
}
$defaultExisted = Test-Path -LiteralPath $defaultShim
$defaultContent = if ($defaultExisted) {
    Get-Content -LiteralPath $defaultShim -Raw
} else {
    ""
}
$defaultOwnerExisted = Test-Path -LiteralPath $defaultOwner
$defaultOwnerContent = if ($defaultOwnerExisted) {
    Get-Content -LiteralPath $defaultOwner -Raw
} else {
    ""
}
$stateExisted = Test-Path -LiteralPath $defaultState
$stateContent = if ($stateExisted) {
    Get-Content -LiteralPath $defaultState -Raw
} else {
    ""
}
$markerExisted = Test-Path -LiteralPath $archiveDefaultMarker
$markerContent = if ($markerExisted) {
    Get-Content -LiteralPath $archiveDefaultMarker -Raw
} else {
    ""
}
$ownerMarkerExisted = Test-Path -LiteralPath $archiveOwnerMarker
$ownerMarkerContent = if ($ownerMarkerExisted) {
    Get-Content -LiteralPath $archiveOwnerMarker -Raw
} else {
    ""
}
$payloadActivated = $false
$backupCreated = $false
$qualifiedChanged = $false
$defaultChanged = $false
$defaultOwnerChanged = $false
$markerChanged = $false
$profileJunctionCreated = $false
$committed = $false
New-Item -ItemType Directory -Path $profileRoot -Force | Out-Null
try {
    Copy-InstallTree -Source $SourceDir -Destination $staged
    if (-not (Test-Path -LiteralPath (Join-Path $staged "oscan.exe"))) {
        throw "Staged profile is missing oscan.exe."
    }
    if ($expectedCompilerDigest) {
        $stagedDigest = (Get-FileHash -LiteralPath (Join-Path $staged "oscan.exe") -Algorithm SHA256).Hash
        if ($stagedDigest -ne $expectedCompilerDigest) {
            throw "Staged oscan.exe digest mismatch: package metadata has $expectedCompilerDigest, actual is $stagedDigest."
        }
    }
    if ($metadata -and -not (Test-Path -LiteralPath (Join-Path $staged "oscan-package.json"))) {
        throw "Staged profile is missing oscan-package.json."
    }

    if (Test-Path -LiteralPath $versionDir) {
        $backupCreated = $true
        Move-Item -LiteralPath $versionDir -Destination $backup
    }
    $payloadActivated = $true
    Move-Item -LiteralPath $staged -Destination $versionDir

    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    $defaultBinDir = [System.IO.Path]::GetFullPath((Join-Path $InstallRoot "bin")).TrimEnd('\')
    if ([string]::Equals(
            $BinDir.TrimEnd('\'),
            $defaultBinDir,
            [System.StringComparison]::OrdinalIgnoreCase)) {
        $relativeExecutable = "..\profiles\$Profile\$version\oscan.exe"
    } else {
        $profileJunction = Join-Path $BinDir ".oscan-profiles"
        $profileJunctionCreated = Ensure-ProfileJunction -Path $profileJunction -Target $profilesRoot
        $relativeExecutable = ".oscan-profiles\$Profile\$version\oscan.exe"
    }
    $qualifiedChanged = $true
    Write-ProfileShim -Path $qualifiedShim -RelativeExecutable $relativeExecutable
    Write-AtomicText -Path $qualifiedOwner -Content "$selectorOwnerId`r`n"

    $selected = if (Test-Path -LiteralPath $defaultState) {
        (Get-Content -LiteralPath $defaultState -Raw).Trim()
    } else {
        ""
    }
    $defaultExists = Test-Path -LiteralPath $defaultShim
    if ($SetDefault -or (-not $selected -and -not $defaultExists)) {
        $defaultChanged = $true
        Write-DefaultShim -Path $defaultShim -SelectedProfile $Profile
        Write-AtomicText -Path $defaultState -Content "$Profile`r`n"
        $selected = $Profile
    }
    if ($selected -and
        (Test-Path -LiteralPath $defaultShim) -and
        ($defaultChanged -or $defaultOwnership -eq "Current")) {
        $defaultOwnerChanged = $true
        Write-AtomicText -Path $defaultOwner -Content "$selectorOwnerId`r`n"
        $markerChanged = $true
        Write-AtomicText -Path $archiveOwnerMarker -Content "archive-default-owner`r`n"
        Write-AtomicText -Path $archiveDefaultMarker -Content "archive-default`r`n"
    }

    $committed = $true
    Get-ChildItem -LiteralPath $profileRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ne $versionDir } |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    $installError = $_
    if ($committed) {
        throw $installError
    }
    $rollbackErrors = @()
    if ($payloadActivated -and (Test-Path -LiteralPath $versionDir)) {
        try {
            Remove-Item -LiteralPath $versionDir -Recurse -Force -ErrorAction Stop
        } catch {
            $rollbackErrors += "remove new payload: $($_.Exception.Message)"
        }
    }
    if ($backupCreated -and (Test-Path -LiteralPath $backup)) {
        try {
            Move-Item -LiteralPath $backup -Destination $versionDir -ErrorAction Stop
        } catch {
            $rollbackErrors += "restore previous payload: $($_.Exception.Message)"
        }
    }
    if ($qualifiedChanged) {
        try {
            Restore-AtomicFile -Path $qualifiedShim -Existed $qualifiedExisted -Content $qualifiedContent
            Restore-AtomicFile -Path $qualifiedOwner -Existed $qualifiedOwnerExisted -Content $qualifiedOwnerContent
        } catch {
            $rollbackErrors += "restore qualified command: $($_.Exception.Message)"
        }
    }
    if ($defaultChanged) {
        try {
            Restore-AtomicFile -Path $defaultShim -Existed $defaultExisted -Content $defaultContent
            Restore-AtomicFile -Path $defaultState -Existed $stateExisted -Content $stateContent
        } catch {
            $rollbackErrors += "restore default selector: $($_.Exception.Message)"
        }
    }
    if ($defaultOwnerChanged) {
        try {
            Restore-AtomicFile -Path $defaultOwner -Existed $defaultOwnerExisted -Content $defaultOwnerContent
        } catch {
            $rollbackErrors += "restore default selector ownership: $($_.Exception.Message)"
        }
    }
    if ($markerChanged) {
        try {
            Restore-AtomicFile -Path $archiveOwnerMarker -Existed $ownerMarkerExisted -Content $ownerMarkerContent
            Restore-AtomicFile -Path $archiveDefaultMarker -Existed $markerExisted -Content $markerContent
        } catch {
            $rollbackErrors += "restore MSI-conflict marker: $($_.Exception.Message)"
        }
    }
    if ($profileJunctionCreated -and (Test-Path -LiteralPath $profileJunction)) {
        try {
            Remove-Item -LiteralPath $profileJunction -Force -ErrorAction Stop
        } catch {
            $rollbackErrors += "remove profile-link junction: $($_.Exception.Message)"
        }
    }
    if ($rollbackErrors.Count -gt 0) {
        throw "Installing profile '$Profile' failed: $($installError.Exception.Message). Rollback also failed: $($rollbackErrors -join '; ')."
    }
    throw $installError
} finally {
    Remove-Item -LiteralPath $staged -Recurse -Force -ErrorAction SilentlyContinue
    if ($committed) {
        Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ((Test-Path -LiteralPath (Join-Path $InstallRoot "oscan.exe")) -and
    (Join-Path $InstallRoot "oscan.exe") -ne (Join-Path $SourceDir "oscan.exe")) {
    Write-Warning "A legacy flat Oscan install remains at $InstallRoot\oscan.exe. The stable bin directory takes precedence on PATH; remove the legacy files after confirming the new profile works."
}
if (-not $NoPathUpdate) {
    Add-UserPathEntry -Entry $BinDir -LegacyEntry $InstallRoot
}

Write-Host "Installed Oscan profile '$Profile' to $versionDir"
Write-Host "Qualified command: $(Join-Path $BinDir "oscan-$Profile.cmd")"
if ($selected -eq $Profile) {
    Write-Host "Default command: $(Join-Path $BinDir "oscan.cmd") -> oscan-$Profile"
} else {
    Write-Host "Default remains '$selected'. Re-run with -SetDefault to select '$Profile'."
}
} finally {
    if ($selectorLock) {
        $selectorLock.Dispose()
        Remove-Item -LiteralPath $selectorLockPath -Force -ErrorAction SilentlyContinue
    }
    $installLock.Dispose()
    Remove-Item -LiteralPath $installLockPath -Force -ErrorAction SilentlyContinue
}
