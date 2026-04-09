param(
    [Parameter(Mandatory = $true)]
    [string]$BundleDir,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$WxsPath = Join-Path $RepoRoot "packaging\windows\oscan.wxs"

if (-not (Test-Path $BundleDir)) {
    throw "Bundle directory not found: $BundleDir"
}
if (-not (Test-Path (Join-Path $BundleDir "oscan.exe"))) {
    throw "Bundle directory must contain oscan.exe"
}
if (-not (Test-Path $WxsPath)) {
    throw "WiX source not found: $WxsPath"
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

# Harvest the toolchain directory into a WiX fragment
$ToolchainDir = Join-Path $BundleDir "toolchain"
$HarvestWxs = Join-Path $env:TEMP "oscan-toolchain-harvest.wxs"

if (Test-Path $ToolchainDir) {
    Write-Host "Harvesting toolchain directory..."

    # Build a WiX fragment with all toolchain files
    $fragment = @'
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Fragment>
    <DirectoryRef Id="INSTALLFOLDER">
      <Directory Id="ToolchainDir" Name="toolchain">
'@

    $componentIds = @()
    $counter = 0

    function Add-DirectoryContent {
        param(
            [string]$SourceDir,
            [string]$ParentId,
            [int]$Indent,
            [ref]$Counter,
            [ref]$Components,
            [System.Text.StringBuilder]$Builder
        )
        $pad = "        " * $Indent

        foreach ($item in Get-ChildItem $SourceDir -ErrorAction SilentlyContinue | Sort-Object Name) {
            if ($item.PSIsContainer) {
                $dirId = "dir_$($Counter.Value)"
                $Counter.Value++
                $Builder.AppendLine("$pad<Directory Id=`"$dirId`" Name=`"$($item.Name)`">") | Out-Null
                Add-DirectoryContent -SourceDir $item.FullName -ParentId $dirId -Indent ($Indent + 1) -Counter $Counter -Components $Components -Builder $Builder
                $Builder.AppendLine("$pad</Directory>") | Out-Null
            } else {
                $compId = "comp_$($Counter.Value)"
                $fileId = "file_$($Counter.Value)"
                $Counter.Value++
                $guid = [guid]::NewGuid().ToString().ToUpper()
                $relativePath = $item.FullName.Substring($BundleDir.Length).TrimStart('\')
                $Builder.AppendLine("$pad<Component Id=`"$compId`" Guid=`"$guid`">") | Out-Null
                $Builder.AppendLine("$pad  <File Id=`"$fileId`" Source=`"`$(var.BundleDir)\$relativePath`" KeyPath=`"yes`" />") | Out-Null
                $Builder.AppendLine("$pad</Component>") | Out-Null
                $Components.Value += $compId
            }
        }
    }

    $sb = [System.Text.StringBuilder]::new()
    $sb.AppendLine($fragment) | Out-Null
    $counterRef = [ref]$counter
    $componentIdsRef = [ref]$componentIds
    Add-DirectoryContent -SourceDir $ToolchainDir -ParentId "ToolchainDir" -Indent 4 -Counter $counterRef -Components $componentIdsRef -Builder $sb

    $sb.AppendLine("      </Directory>") | Out-Null
    $sb.AppendLine("    </DirectoryRef>") | Out-Null
    $sb.AppendLine("    <ComponentGroup Id=`"ToolchainFiles`">") | Out-Null
    foreach ($id in $componentIds) {
        $sb.AppendLine("      <ComponentRef Id=`"$id`" />") | Out-Null
    }
    $sb.AppendLine("    </ComponentGroup>") | Out-Null
    $sb.AppendLine("  </Fragment>") | Out-Null
    $sb.AppendLine("</Wix>") | Out-Null

    Set-Content -Path $HarvestWxs -Value $sb.ToString() -Encoding UTF8
    Write-Host "Harvested $($componentIds.Count) toolchain files"
} else {
    # No toolchain — create empty component group
    $emptyFragment = @'
<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Fragment>
    <ComponentGroup Id="ToolchainFiles" />
  </Fragment>
</Wix>
'@
    Set-Content -Path $HarvestWxs -Value $emptyFragment -Encoding UTF8
    Write-Host "No toolchain directory — empty fragment"
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
