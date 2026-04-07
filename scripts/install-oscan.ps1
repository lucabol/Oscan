[CmdletBinding()]
param(
    [string]$SourceDir = $PSScriptRoot,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "Programs\oscan"),
    [string]$BinDir,
    [switch]$NoPathUpdate
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Add-UserPathEntry {
    param([Parameter(Mandatory = $true)][string]$Entry)

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($current) {
        $parts = $current.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries)
    }
    if ($parts -contains $Entry) {
        return
    }
    $updated = @($parts + $Entry) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $updated, "User")
    if (-not ($env:Path.Split(';', [System.StringSplitOptions]::RemoveEmptyEntries) -contains $Entry)) {
        $env:Path = "$env:Path;$Entry"
    }
}

$SourceDir = [System.IO.Path]::GetFullPath($SourceDir)
$InstallDir = [System.IO.Path]::GetFullPath($InstallDir)
if (-not (Test-Path (Join-Path $SourceDir "oscan.exe"))) {
    throw "Source bundle must contain oscan.exe"
}
if ([System.IO.Path]::GetPathRoot($InstallDir) -eq $InstallDir) {
    throw "Refusing to install into a drive root: $InstallDir"
}

if (Test-Path $InstallDir) {
    Remove-Item $InstallDir -Recurse -Force
}
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

$robocopy = Get-Command robocopy.exe -ErrorAction SilentlyContinue
if ($robocopy) {
    & $robocopy.Source $SourceDir $InstallDir /MIR /NFL /NDL /NJH /NJS /NP | Out-Null
    if ($LASTEXITCODE -gt 7) {
        throw "robocopy failed while installing to $InstallDir (exit code $LASTEXITCODE)."
    }
} else {
    Copy-Item (Join-Path $SourceDir "*") -Destination $InstallDir -Recurse -Force
}

$InstalledExe = Join-Path $InstallDir "oscan.exe"
$PathEntry = $InstallDir
if ($BinDir) {
    $BinDir = [System.IO.Path]::GetFullPath($BinDir)
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    $ShimPath = Join-Path $BinDir "oscan.cmd"
    $Shim = "@echo off`r`n`"$InstalledExe`" %*`r`n"
    Set-Content -Path $ShimPath -Value $Shim -Encoding ASCII -NoNewline
    $PathEntry = $BinDir
}

if (-not $NoPathUpdate) {
    Add-UserPathEntry -Entry $PathEntry
}

Write-Host "Installed Oscan to $InstallDir"
if (Test-Path (Join-Path $InstallDir "toolchain")) {
    Write-Host "Bundled toolchain installed next to oscan.exe"
}
if ($BinDir) {
    Write-Host "Shim available at $(Join-Path $BinDir 'oscan.cmd')"
} elseif ($NoPathUpdate) {
    Write-Host "Add $InstallDir to PATH to run oscan globally."
} else {
    Write-Host "User PATH updated with $PathEntry"
}
