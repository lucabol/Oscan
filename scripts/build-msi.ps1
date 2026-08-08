param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [string]$Version,

    [string]$OutputPath,

    # Where the harvested WiX fragment is written (defaults to a temp file).
    [string]$HarvestPath,

    # Harvest the staged bundle into a WiX fragment and stop. Lets the
    # harvesting rule — which payload directories and root files a package
    # contributes — be exercised without WiX or a Windows installer build.
    [switch]$HarvestOnly
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$WxsPath = Join-Path $RepoRoot "packaging/windows/oscan.wxs"
$ContractPath = Join-Path $RepoRoot "packaging/toolchains/release-contract.json"

if (-not (Test-Path $BundleDir)) {
    throw "Bundle directory not found: $BundleDir"
}
$BundleDir = (Resolve-Path -LiteralPath $BundleDir).Path
if (-not (Test-Path (Join-Path $BundleDir "oscan.exe"))) {
    throw "Bundle directory must contain oscan.exe"
}
$packageMetadataPath = Join-Path $BundleDir "oscan-package.json"
if (-not (Test-Path -LiteralPath $packageMetadataPath)) {
    throw "Bundle directory must contain oscan-package.json"
}
$packageMetadata = Get-Content -LiteralPath $packageMetadataPath -Raw | ConvertFrom-Json
if ($packageMetadata.schema_version -ne 2) {
    throw "The MSI requires schema-2 package metadata; got schema '$($packageMetadata.schema_version)'."
}
$Profile = [string]$packageMetadata.profile
if ($Profile -notin @("full", "llvm", "cranelift", "c")) {
    throw "The package metadata has unsupported MSI profile '$Profile'."
}
if ($packageMetadata.package_id -ne "oscan-$Profile") {
    throw "Package metadata package_id '$($packageMetadata.package_id)' does not match profile '$Profile'."
}
if ($packageMetadata.target -ne "windows-x86_64") {
    throw "The MSI builder requires a windows-x86_64 package; got '$($packageMetadata.target)'."
}
if ($packageMetadata.is_distribution -ne $true) {
    throw "The MSI builder requires distribution package metadata."
}
if (-not (Test-Path -LiteralPath $ContractPath -PathType Leaf)) {
    throw "Release contract not found: $ContractPath"
}
$releaseContract = Get-Content -LiteralPath $ContractPath -Raw | ConvertFrom-Json -AsHashtable
$profileSpec = $releaseContract["variants"]["windows-x86_64"]["profiles"][$Profile]
if (-not $profileSpec) {
    throw "The release contract does not declare Windows profile '$Profile'."
}
foreach ($field in @("msi_name_template", "msi_product_name", "msi_upgrade_code")) {
    if (-not $profileSpec[$field]) {
        throw "The release contract Windows profile '$Profile' is missing '$field'."
    }
}
$ProductName = [string]$profileSpec["msi_product_name"]
$UpgradeCode = ([guid]([string]$profileSpec["msi_upgrade_code"])).ToString().ToUpperInvariant()
if ($Profile -eq "llvm" -and
    $UpgradeCode -ne "F7A3B2C1-4D5E-6F78-9A0B-1C2D3E4F5A6B") {
    throw "The LLVM MSI must preserve its historical UpgradeCode for in-place migration."
}
if (-not (Test-Path $WxsPath)) {
    throw "WiX source not found: $WxsPath"
}
if (-not $HarvestOnly) {
    if (-not $Version) {
        throw "-Version is required when building an MSI"
    }
    if (-not $OutputPath) {
        throw "-OutputPath is required when building an MSI"
    }
    if ($Version -ne [string]$packageMetadata.version) {
        throw "MSI version '$Version' does not match package metadata version '$($packageMetadata.version)'."
    }
    if ($Version -notmatch '^\d+\.\d+\.\d+$') {
        throw "MSI version '$Version' must contain exactly three numeric components."
    }

    # Install WiX dotnet tool if not present
    $wixCmd = Get-Command wix -ErrorAction SilentlyContinue
    if (-not $wixCmd) {
        Write-Host "Installing WiX Toolset..."
        dotnet tool install --global wix
        if ($LASTEXITCODE -ne 0) { throw "Failed to install WiX" }
    }

    # Accept EULA and install UI extension
    wix eula accept wix7 2>$null
    wix extension add WixToolset.UI.wixext 2>$null
}

# Harvest the bundle's payload into a WiX fragment.
#
# Every profile MSI is cut from its already staged package. The payload differs
# by profile, so harvest everything except files declared directly by the .wxs
# and the archive install script, which has no meaning inside an MSI.
$RootFileExclusions = @("oscan.exe", "README-install.txt", "install.ps1")
if (-not $HarvestPath) {
    $HarvestPath = Join-Path ([System.IO.Path]::GetTempPath()) "oscan-bundle-harvest.wxs"
}
$HarvestWxs = $HarvestPath

$payloadDirectories = @(Get-ChildItem -LiteralPath $BundleDir -Directory | Sort-Object Name)
$rootFiles = @(
    Get-ChildItem -LiteralPath $BundleDir -File |
        Where-Object { $RootFileExclusions -notcontains $_.Name } |
        Sort-Object Name
)

$componentIds = @()
$counter = 0

function Add-DirectoryContent {
    param(
        [string]$SourceDir,
        [int]$Indent,
        [ref]$Counter,
        [ref]$Components,
        [System.Text.StringBuilder]$Builder
    )
    $pad = "        " * $Indent

    foreach ($item in Get-ChildItem -LiteralPath $SourceDir -ErrorAction SilentlyContinue | Sort-Object Name) {
        if ($item.PSIsContainer) {
            $dirId = "dir_$($Counter.Value)"
            $Counter.Value++
            $Builder.AppendLine("$pad<Directory Id=`"$dirId`" Name=`"$($item.Name)`">") | Out-Null
            Add-DirectoryContent -SourceDir $item.FullName -Indent ($Indent + 1) -Counter $Counter -Components $Components -Builder $Builder
            $Builder.AppendLine("$pad</Directory>") | Out-Null
        } else {
            $compId = "comp_$($Counter.Value)"
            $fileId = "file_$($Counter.Value)"
            $Counter.Value++
            $relativePath = $item.FullName.Substring($BundleDir.Length).TrimStart('\', '/').Replace('/', '\')
            $Builder.AppendLine("$pad<Component Id=`"$compId`" Guid=`"*`">") | Out-Null
            $Builder.AppendLine("$pad  <File Id=`"$fileId`" Source=`"`$(var.BundleDir)\$relativePath`" KeyPath=`"yes`" />") | Out-Null
            $Builder.AppendLine("$pad</Component>") | Out-Null
            $Components.Value += $compId
        }
    }
}

if ($payloadDirectories.Count -eq 0 -and $rootFiles.Count -eq 0) {
    Write-Host "Bundle has no additional payload — empty fragment"
    $emptyFragment = @'
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Fragment>
    <ComponentGroup Id="BundlePayload" />
  </Fragment>
</Wix>
'@
    Set-Content -Path $HarvestWxs -Value $emptyFragment -Encoding UTF8
} else {
    Write-Host "Harvesting bundle payload ($($payloadDirectories.Count) director(ies), $($rootFiles.Count) root file(s))..."
    $sb = [System.Text.StringBuilder]::new()
    $sb.AppendLine(@'
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Fragment>
    <DirectoryRef Id="InstallFolder">
'@) | Out-Null

    $counterRef = [ref]$counter
    $componentIdsRef = [ref]$componentIds
    foreach ($directory in $payloadDirectories) {
        $dirId = "dir_$($counterRef.Value)"
        $counterRef.Value++
        $sb.AppendLine("      <Directory Id=`"$dirId`" Name=`"$($directory.Name)`">") | Out-Null
        Add-DirectoryContent -SourceDir $directory.FullName -Indent 4 -Counter $counterRef -Components $componentIdsRef -Builder $sb
        $sb.AppendLine("      </Directory>") | Out-Null
    }
    foreach ($file in $rootFiles) {
        $compId = "comp_$($counterRef.Value)"
        $fileId = "file_$($counterRef.Value)"
        $counterRef.Value++
        $sb.AppendLine("      <Component Id=`"$compId`" Guid=`"*`">") | Out-Null
        $sb.AppendLine("        <File Id=`"$fileId`" Source=`"`$(var.BundleDir)\$($file.Name)`" KeyPath=`"yes`" />") | Out-Null
        $sb.AppendLine("      </Component>") | Out-Null
        $componentIdsRef.Value += $compId
    }

    $sb.AppendLine("    </DirectoryRef>") | Out-Null
    $sb.AppendLine("    <ComponentGroup Id=`"BundlePayload`">") | Out-Null
    foreach ($id in $componentIdsRef.Value) {
        $sb.AppendLine("      <ComponentRef Id=`"$id`" />") | Out-Null
    }
    $sb.AppendLine("    </ComponentGroup>") | Out-Null
    $sb.AppendLine("  </Fragment>") | Out-Null
    $sb.AppendLine("</Wix>") | Out-Null

    Set-Content -Path $HarvestWxs -Value $sb.ToString() -Encoding UTF8
    Write-Host "Harvested $($componentIdsRef.Value.Count) payload file(s)"
}

if ($HarvestOnly) {
    Write-Output $HarvestWxs
    return
}

# Build one product-family MSI. Profile payloads live under distinct folders;
# shared selector/PATH components are identical and reference-counted by MSI.
Write-Host "Building $ProductName MSI..."
$parentDir = Split-Path $OutputPath -Parent
if (-not (Test-Path $parentDir)) {
    New-Item -ItemType Directory -Path $parentDir -Force | Out-Null
}
$shimDir = Join-Path ([System.IO.Path]::GetTempPath()) "oscan-msi-$([guid]::NewGuid().ToString('N'))"
$profileShimPath = Join-Path $shimDir "oscan-$Profile.cmd"
$selectorShimPath = Join-Path $shimDir "oscan.cmd"
New-Item -ItemType Directory -Path $shimDir -Force | Out-Null
try {
    $profileShim = @"
@echo off
"%~dp0..\profiles\$Profile\$Version\oscan.exe" %*
@exit /b %ERRORLEVEL%
"@
    $selectorShim = @'
@echo off
if exist "%~dp0oscan-full.cmd" goto full
if exist "%~dp0oscan-llvm.cmd" goto llvm
if exist "%~dp0oscan-cranelift.cmd" goto cranelift
if exist "%~dp0oscan-c.cmd" goto c
echo Oscan has no installed MSI profile. 1>&2
exit /b 1
:full
call "%~dp0oscan-full.cmd" %*
exit /b %ERRORLEVEL%
:llvm
call "%~dp0oscan-llvm.cmd" %*
exit /b %ERRORLEVEL%
:cranelift
call "%~dp0oscan-cranelift.cmd" %*
exit /b %ERRORLEVEL%
:c
call "%~dp0oscan-c.cmd" %*
exit /b %ERRORLEVEL%
'@
    Set-Content -LiteralPath $profileShimPath -Value $profileShim -Encoding Ascii
    Set-Content -LiteralPath $selectorShimPath -Value $selectorShim -Encoding Ascii

    wix build $WxsPath $HarvestWxs `
        -arch x64 `
        -acceptEula wix7 `
        -ext WixToolset.UI.wixext `
        -b $RepoRoot `
        -d "BundleDir=$BundleDir" `
        -d "Version=$Version" `
        -d "Profile=$Profile" `
        -d "ProductName=$ProductName" `
        -d "UpgradeCode=$UpgradeCode" `
        -d "ProfileShimPath=$profileShimPath" `
        -d "SelectorShimPath=$selectorShimPath" `
        -out $OutputPath

    if ($LASTEXITCODE -ne 0) {
        throw "WiX build failed for profile '$Profile'"
    }
} finally {
    Remove-Item -LiteralPath $shimDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "MSI created: $OutputPath"
Write-Host "Size: $([math]::Round((Get-Item $OutputPath).Length / 1MB)) MB"
