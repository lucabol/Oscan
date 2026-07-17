param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [int]$Concurrency = 6
)

# Genuine multi-process concurrency proof for docs/design/native-link-embedding.md
# §6.4/§9 item 4: several real `oscan.exe` processes racing a cold, shared
# native-asset cache directory must all succeed and produce byte-identical,
# runnable executables -- never a corrupted/partial extraction. This
# complements (does not replace) native_assets.rs's in-process
# `concurrent_extraction_of_the_same_asset_set_converges` unit test, which
# proves the same invariant with synthetic assets and real OS threads inside
# one process; this test proves it across real separate processes and the
# real embedded assets end to end.

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir

function Assert-NativeLinkConcurrency {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$helloSource = Join-Path $ScriptDir "..\examples\hello.osc"
$buildRoot = Join-Path $ScriptDir "build\native-link-concurrency"
Remove-Item -LiteralPath $buildRoot -Recurse -Force -ErrorAction SilentlyContinue
[void](New-Item -ItemType Directory -Path $buildRoot -Force)

# Isolated, cold cache dir -- never touches the real %LOCALAPPDATA% cache, so
# this test always starts from zero regardless of what earlier tests in this
# run already populated.
$cacheDir = Join-Path $buildRoot "cache"
[void](New-Item -ItemType Directory -Path $cacheDir -Force)

$savedCacheDir = $env:OSCAN_NATIVE_ASSET_CACHE_DIR
try {
    $env:OSCAN_NATIVE_ASSET_CACHE_DIR = $cacheDir

    $jobs = 0..($Concurrency - 1) | ForEach-Object {
        $index = $_
        $exePath = Join-Path $buildRoot "hello_$index.exe"
        Start-Job -ScriptBlock {
            param($compiler, $source, $exePath, $cacheDir)
            $env:OSCAN_NATIVE_ASSET_CACHE_DIR = $cacheDir
            $compileOutput = & $compiler --backend native -o $exePath $source 2>&1 | Out-String
            $compileExit = $LASTEXITCODE
            $runOutput = $null
            $runExit = $null
            if ($compileExit -eq 0 -and (Test-Path -LiteralPath $exePath)) {
                $runOutput = & $exePath 2>&1 | Out-String
                $runExit = $LASTEXITCODE
            }
            [PSCustomObject]@{
                Index         = $index
                CompileExit   = $compileExit
                CompileOutput = $compileOutput
                ExePath       = $exePath
                RunExit       = $runExit
                RunOutput     = $runOutput
            }
        } -ArgumentList $compiler, $helloSource, $exePath, $cacheDir
    }

    # Start all jobs as close together as possible, then wait -- this is the
    # actual race against the cold cache directory.
    $results = $jobs | Wait-Job | Receive-Job
    $jobs | Remove-Job -Force

    Assert-NativeLinkConcurrency ($results.Count -eq $Concurrency) `
        "expected $Concurrency job results, got $($results.Count)"

    $sizes = @()
    foreach ($r in $results) {
        Assert-NativeLinkConcurrency ($r.CompileExit -eq 0) `
            "concurrent compile #$($r.Index) failed with exit $($r.CompileExit): $($r.CompileOutput)"
        Assert-NativeLinkConcurrency (Test-Path -LiteralPath $r.ExePath -PathType Leaf) `
            "concurrent compile #$($r.Index) produced no executable"
        Assert-NativeLinkConcurrency ($r.RunExit -eq 0) `
            "concurrent hello.exe #$($r.Index) exited with $($r.RunExit)"
        $actual = ($r.RunOutput | Out-String).TrimEnd("`r", "`n")
        Assert-NativeLinkConcurrency ($actual -eq "Hello, Oscan!") `
            "concurrent hello.exe #$($r.Index) stdout mismatch: got '$actual'"
        $sizes += (Get-Item -LiteralPath $r.ExePath).Length
    }

    $distinctSizes = $sizes | Select-Object -Unique
    Assert-NativeLinkConcurrency ($distinctSizes.Count -eq 1 -and $distinctSizes[0] -eq 6656) `
        "expected every concurrently-linked hello.exe to be exactly 6656 bytes; got: $($sizes -join ', ')"

    # The cache must have converged to exactly one asset-set directory with a
    # single .complete marker -- no leftover corrupted/partial siblings from
    # the race.
    $setDirs = Get-ChildItem -LiteralPath $cacheDir -Directory
    Assert-NativeLinkConcurrency ($setDirs.Count -eq 1) `
        "expected exactly 1 asset-set cache directory after the race, found $($setDirs.Count): $($setDirs.Name -join ', ')"
    $completeMarker = Join-Path $setDirs[0].FullName ".complete"
    Assert-NativeLinkConcurrency (Test-Path -LiteralPath $completeMarker -PathType Leaf) `
        "cache set dir is missing its .complete marker after the concurrent race"

    Write-Host "native link concurrency test passed ($Concurrency concurrent processes raced a cold cache; all succeeded, byte-identical 6656 B output, cache converged to 1 set dir)"
} finally {
    $env:OSCAN_NATIVE_ASSET_CACHE_DIR = $savedCacheDir
}
