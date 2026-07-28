#!/usr/bin/env pwsh
#
# Compile every .osc example with each backend that can produce a host
# executable, then show their sizes side by side.

param(
    [string]$Oscan = "",
    [string]$SourceDirectory = "examples",
    [string]$OutputDirectory = "tests\build\sample-backend-matrix"
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
$IsWindowsHost = $env:OS -eq "Windows_NT"
$ExecutableSuffix = if ($IsWindowsHost) { ".exe" } else { "" }
$Backends = @(
    [PSCustomObject]@{ Name = "llvm"; DisplayName = "LLVM" },
    [PSCustomObject]@{ Name = "native"; DisplayName = "Cranelift/native" },
    [PSCustomObject]@{ Name = "c"; DisplayName = "C" }
)

function Resolve-FromRoot {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $Root $Path))
}

function Resolve-Oscan {
    param([string]$RequestedPath)

    if ($RequestedPath) {
        $candidate = Resolve-FromRoot $RequestedPath
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Oscan compiler not found: $candidate"
        }
        return $candidate
    }

    $names = if ($IsWindowsHost) { @("oscan.exe", "oscan") } else { @("oscan", "oscan.exe") }
    foreach ($configuration in @("release", "debug")) {
        foreach ($name in $names) {
            $candidate = Join-Path $Root "target\$configuration\$name"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                return [System.IO.Path]::GetFullPath($candidate)
            }
        }
    }
    throw "Oscan compiler not found. Pass -Oscan or build target\release first."
}

function Get-SafeSampleNames {
    param(
        [Parameter(Mandatory = $true)][System.IO.FileInfo[]]$Sources,
        [Parameter(Mandatory = $true)][string]$SourceRoot
    )

    $entries = foreach ($source in $Sources) {
        $relative = [System.IO.Path]::GetRelativePath($SourceRoot, $source.FullName)
        $withoutExtension = [System.IO.Path]::ChangeExtension($relative, $null)
        $safe = $withoutExtension -replace '[\\/]+', '-' -replace '[^A-Za-z0-9_.-]', '_'
        if ([string]::IsNullOrWhiteSpace($safe)) {
            $safe = "sample"
        }
        [PSCustomObject]@{
            Source = $source
            RelativePath = $relative.Replace('\', '/')
            SafeName = $safe
        }
    }

    foreach ($group in ($entries | Group-Object { $_.SafeName.ToUpperInvariant() })) {
        $ordered = @($group.Group | Sort-Object `
            @{ Expression = { $_.RelativePath.ToUpperInvariant() } }, `
            @{ Expression = { $_.RelativePath } })
        if ($ordered.Count -eq 1) {
            $ordered[0] | Add-Member -NotePropertyName OutputName -NotePropertyValue $ordered[0].SafeName
            continue
        }
        for ($index = 0; $index -lt $ordered.Count; $index++) {
            $ordered[$index] | Add-Member -NotePropertyName OutputName -NotePropertyValue "$($ordered[$index].SafeName)--$($index + 1)"
        }
    }

    return @($entries | Sort-Object `
        @{ Expression = { $_.RelativePath.ToUpperInvariant() } }, `
        @{ Expression = { $_.RelativePath } })
}

function Test-ExecutableArtifact {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return $false
    }
    if ((Get-Item -LiteralPath $Path).Length -le 0) {
        return $false
    }
    if ([System.IO.Path]::GetExtension($Path) -in @(".c", ".ll")) {
        return $false
    }

    $header = [System.IO.File]::ReadAllBytes($Path)
    if ($IsWindowsHost) {
        return $header.Length -ge 2 -and $header[0] -eq 0x4d -and $header[1] -eq 0x5a
    }
    return $header.Length -ge 4 -and
        $header[0] -eq 0x7f -and $header[1] -eq 0x45 -and
        $header[2] -eq 0x4c -and $header[3] -eq 0x46
}

function Invoke-OscanCompile {
    param(
        [Parameter(Mandatory = $true)][string]$Compiler,
        [Parameter(Mandatory = $true)][string]$Backend,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Output
    )

    $outputText = & $Compiler --backend $Backend $Source -o $Output 2>&1 | Out-String
    return [PSCustomObject]@{
        ExitCode = $LASTEXITCODE
        Output = $outputText.Trim()
    }
}

function Test-BackendAvailable {
    param(
        [Parameter(Mandatory = $true)][string]$Compiler,
        [Parameter(Mandatory = $true)][string]$Backend,
        [Parameter(Mandatory = $true)][string]$ProbeRoot
    )

    $probeSource = Join-Path $ProbeRoot "backend-probe.osc"
    $probeOutput = Join-Path $ProbeRoot "backend-probe-$Backend$ExecutableSuffix"
    Set-Content -LiteralPath $probeSource -NoNewline -Value @'
fn! main() {
    println("backend probe");
}
'@
    $compile = Invoke-OscanCompile -Compiler $Compiler -Backend $Backend -Source $probeSource -Output $probeOutput
    $available = $compile.ExitCode -eq 0 -and (Test-ExecutableArtifact $probeOutput)
    return [PSCustomObject]@{
        Available = $available
        Detail = if ($available) { "" } elseif ($compile.ExitCode -ne 0) {
            "availability probe exited $($compile.ExitCode): $($compile.Output)"
        } else {
            "availability probe did not produce a non-empty host executable"
        }
    }
}

$compiler = Resolve-Oscan $Oscan
$sourceRoot = Resolve-FromRoot $SourceDirectory
$outputRoot = Resolve-FromRoot $OutputDirectory

if (-not (Test-Path -LiteralPath $sourceRoot -PathType Container)) {
    throw "Source directory not found: $sourceRoot"
}

$sources = @(
    Get-ChildItem -LiteralPath $sourceRoot -Recurse -File -Filter "*.osc" |
        Sort-Object @{ Expression = { $_.FullName.ToUpperInvariant() } }, @{ Expression = { $_.FullName } }
)
if ($sources.Count -eq 0) {
    throw "No .osc samples found under: $sourceRoot"
}

[string]$sourceRelativeToOutput = [System.IO.Path]::GetRelativePath($outputRoot, $sourceRoot)
if (-not [System.IO.Path]::IsPathRooted($sourceRelativeToOutput) -and
    ($sourceRelativeToOutput -eq "." -or
     (-not $sourceRelativeToOutput.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)") -and
      $sourceRelativeToOutput -ne ".."))) {
    throw "Output directory must not contain the source directory: $outputRoot"
}

Remove-Item -LiteralPath $outputRoot -Recurse -Force -ErrorAction SilentlyContinue
[void](New-Item -ItemType Directory -Path $outputRoot -Force)
Write-Host "Output root: $outputRoot"

$samples = Get-SafeSampleNames -Sources $sources -SourceRoot $sourceRoot
$probeRoot = Join-Path $outputRoot ".backend-probe"
$availableBackends = [System.Collections.Generic.List[object]]::new()

Push-Location $Root
try {
    Remove-Item -LiteralPath $probeRoot -Recurse -Force -ErrorAction SilentlyContinue
    [void](New-Item -ItemType Directory -Path $probeRoot -Force)

    foreach ($backend in $Backends) {
        $probe = Test-BackendAvailable -Compiler $compiler -Backend $backend.Name -ProbeRoot $probeRoot
        if ($probe.Available) {
            $availableBackends.Add($backend)
            Write-Host "Available: $($backend.DisplayName)"
        } else {
            Write-Host "SKIP $($backend.DisplayName): $($probe.Detail)"
        }
    }
} finally {
    Remove-Item -LiteralPath $probeRoot -Recurse -Force -ErrorAction SilentlyContinue
    Pop-Location
}

if ($availableBackends.Count -eq 0) {
    throw "No backend could produce a host executable; see availability probe diagnostics above."
}

$results = @{}
$failed = $false
Push-Location $Root
try {
    foreach ($backend in $availableBackends) {
        $backendDirectory = Join-Path $outputRoot $backend.Name
        [void](New-Item -ItemType Directory -Path $backendDirectory -Force)

        foreach ($sample in $samples) {
            $artifact = Join-Path $backendDirectory "$($sample.OutputName)$ExecutableSuffix"
            Remove-Item -LiteralPath $artifact -Force -ErrorAction SilentlyContinue

            $compile = Invoke-OscanCompile `
                -Compiler $compiler `
                -Backend $backend.Name `
                -Source $sample.Source.FullName `
                -Output $artifact
            $key = "$($sample.RelativePath)`0$($backend.Name)"

            if ($compile.ExitCode -ne 0) {
                Write-Host "FAIL $($backend.DisplayName) $($sample.RelativePath): compile exited $($compile.ExitCode)"
                if ($compile.Output) { Write-Host $compile.Output }
                $results[$key] = [PSCustomObject]@{ Status = "FAIL"; Bytes = $null }
                $failed = $true
                continue
            }
            if (-not (Test-ExecutableArtifact $artifact)) {
                Write-Host "FAIL $($backend.DisplayName) $($sample.RelativePath): expected non-empty executable was not produced at $artifact"
                $results[$key] = [PSCustomObject]@{ Status = "FAIL"; Bytes = $null }
                $failed = $true
                continue
            }
            $results[$key] = [PSCustomObject]@{
                Status = "OK"
                Bytes = (Get-Item -LiteralPath $artifact).Length
            }
        }
    }
} finally {
    Pop-Location
}

$expectedArtifactCount = $samples.Count * $availableBackends.Count
$successfulArtifactCount = @($results.Values | Where-Object { $_.Status -eq "OK" }).Count
Write-Host ""
Write-Host "Compiled artifacts: $successfulArtifactCount/$expectedArtifactCount ($($samples.Count) samples x $($availableBackends.Count) backends)"
Write-Host ""
Write-Host "Executable size matrix (bytes)"
$sampleWidth = [Math]::Max(6, (($samples | ForEach-Object { $_.RelativePath.Length } | Measure-Object -Maximum).Maximum))
$columnWidth = 18
$header = ("{0,-$sampleWidth}" -f "Sample")
foreach ($backend in $availableBackends) {
    $header += (" {0,$columnWidth}" -f "$($backend.DisplayName) bytes")
}
Write-Host $header
Write-Host ("-" * $header.Length)

foreach ($sample in $samples) {
    $row = ("{0,-$sampleWidth}" -f $sample.RelativePath)
    foreach ($backend in $availableBackends) {
        $result = $results["$($sample.RelativePath)`0$($backend.Name)"]
        $value = if ($result.Status -eq "OK") { [string]$result.Bytes } else { "FAIL" }
        $row += (" {0,$columnWidth}" -f $value)
    }
    Write-Host $row
}

$totalRow = ("{0,-$sampleWidth}" -f "TOTAL")
$totals = @{}
foreach ($backend in $availableBackends) {
    $backendResults = @(
        foreach ($sample in $samples) {
            $results["$($sample.RelativePath)`0$($backend.Name)"]
        }
    )
    if ($backendResults.Status -contains "FAIL") {
        $totalRow += (" {0,$columnWidth}" -f "FAIL")
        continue
    }
    $total = ($backendResults | Measure-Object -Property Bytes -Sum).Sum
    $totals[$backend.Name] = [long]$total
    $totalRow += (" {0,$columnWidth}" -f $total)
}
Write-Host ("-" * $header.Length)
Write-Host $totalRow

if ($totals.ContainsKey("llvm") -and $totals.ContainsKey("c")) {
    $delta = $totals["llvm"] - $totals["c"]
    $percent = if ($totals["c"] -eq 0) {
        0.0
    } else {
        100.0 * $delta / $totals["c"]
    }
    Write-Host ("LLVM vs C aggregate: {0:+0;-0;0} bytes ({1:+0.00;-0.00;0.00}%)" -f $delta, $percent)
}

if ($failed) {
    Write-Host "sample-backend-matrix: FAILED"
    exit 1
}

Write-Host "sample-backend-matrix: PASSED"
