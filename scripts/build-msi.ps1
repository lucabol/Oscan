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

if (-not (Test-Path $BundleDir)) {
    throw "Bundle directory not found: $BundleDir"
}
$BundleDir = (Resolve-Path -LiteralPath $BundleDir).Path
if (-not (Test-Path (Join-Path $BundleDir "oscan.exe"))) {
    throw "Bundle directory must contain oscan.exe"
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
# Schema 2 packages are backend-specific, so the payload is no longer always
# `toolchain/`: an object package (llvm/cranelift) ships `native-link/` and
# `build/runtime-archives/<target>/` instead, and every package ships
# `oscan-package.json` plus its LICENSES tree at the root. Everything in the
# staged bundle is therefore harvested, except the two files the .wxs
# declares itself and the archive's own install script, which has no meaning
# inside an MSI (and mirrors/deletes its destination when run).
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
            $guid = [guid]::NewGuid().ToString().ToUpper()
            $relativePath = $item.FullName.Substring($BundleDir.Length).TrimStart('\', '/').Replace('/', '\')
            $Builder.AppendLine("$pad<Component Id=`"$compId`" Guid=`"$guid`">") | Out-Null
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
    <DirectoryRef Id="INSTALLFOLDER">
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
        $guid = [guid]::NewGuid().ToString().ToUpper()
        $sb.AppendLine("      <Component Id=`"$compId`" Guid=`"$guid`">") | Out-Null
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

# Build the MSI
Write-Host "Building MSI..."
$parentDir = Split-Path $OutputPath -Parent
if (-not (Test-Path $parentDir)) {
    New-Item -ItemType Directory -Path $parentDir -Force | Out-Null
}

wix build $WxsPath $HarvestWxs `
    -arch x64 `
    -acceptEula wix7 `
    -ext WixToolset.UI.wixext `
    -b $RepoRoot `
    -d "BundleDir=$BundleDir" `
    -d "Version=$Version" `
    -out $OutputPath

if ($LASTEXITCODE -ne 0) {
    throw "WiX build failed"
}

Write-Host "MSI created: $OutputPath"
Write-Host "Size: $([math]::Round((Get-Item $OutputPath).Length / 1MB)) MB"
