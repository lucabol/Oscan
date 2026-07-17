param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [ValidateRange(2, 64)]
    [int]$NumJobs = 8
)

$ErrorActionPreference = "Stop"
$runningOnWindows = $env:OS -eq "Windows_NT"
$repoRootHost = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Invoke-LinuxShell {
    param([Parameter(Mandatory = $true)][string]$Command)
    if ($script:runningOnWindows) {
        & wsl -d Ubuntu -- bash -lc $Command
    } else {
        & bash -lc $Command
    }
}

function ConvertTo-WslPath {
    param([Parameter(Mandatory = $true)][string]$HostPath)
    $resolved = Get-Item (Resolve-Path $HostPath)
    $directory = if ($resolved.PSIsContainer) { $resolved.FullName } else { $resolved.DirectoryName }
    $linuxDirectory = (& wsl -d Ubuntu --cd $directory -- pwd) -join "`n"
    if ($LASTEXITCODE -ne 0) { throw "failed to translate '$HostPath' for WSL" }
    $linuxDirectory = $linuxDirectory.Trim()
    if ($resolved.PSIsContainer) { return $linuxDirectory }
    return "$linuxDirectory/$($resolved.Name)"
}

if ($runningOnWindows) {
    $linuxRepoRoot = ConvertTo-WslPath $repoRootHost
    if ($Oscan.StartsWith("/")) {
        $linuxOscan = $Oscan
    } else {
        $linuxOscan = ConvertTo-WslPath $Oscan
    }
} else {
    $linuxRepoRoot = $repoRootHost
    $linuxOscan = (Resolve-Path $Oscan).Path
}

$cacheDir = "$linuxRepoRoot/tests/build/native-link-concurrency-linux/cache"
Invoke-LinuxShell "rm -rf '$cacheDir' && mkdir -p '$cacheDir' && rm -f /tmp/concurrent_hello_*"
if ($LASTEXITCODE -ne 0) { throw "failed to initialize concurrency-test scratch paths" }

Write-Host "Racing $NumJobs concurrent native links against a cold cache..."
$jobs = @()
try {
    for ($i = 0; $i -lt $NumJobs; $i++) {
        $outPath = "/tmp/concurrent_hello_$i"
        $scriptBlock = {
            param($OscanPath, $CacheDir, $OutPath, $RepoRoot, $OnWindows)
            $command = "cd '$RepoRoot' && OSCAN_NATIVE_ASSET_CACHE_DIR='$CacheDir' '$OscanPath' --backend native -o '$OutPath' examples/hello.osc 2>&1"
            if ($OnWindows) {
                $output = & wsl -d Ubuntu -- bash -lc $command
            } else {
                $output = & bash -lc $command
            }
            if ($LASTEXITCODE -ne 0) {
                throw "native link for '$OutPath' failed with exit code $LASTEXITCODE`: $($output -join "`n")"
            }
            $output
        }
        $jobs += Start-Job -ScriptBlock $scriptBlock -ArgumentList $linuxOscan, $cacheDir, $outPath, $linuxRepoRoot, $runningOnWindows
    }

    $jobs | Wait-Job | Out-Null
    $failures = @()
    foreach ($job in $jobs) {
        $output = Receive-Job -Job $job -ErrorAction SilentlyContinue 2>&1
        if ($job.State -ne "Completed") {
            $failures += "job $($job.Id) ended in state $($job.State): $($output -join "`n")"
        }
    }
    if ($failures.Count -gt 0) {
        throw "Concurrent native links failed: $($failures -join '; ')"
    }

    $hashes = @()
    for ($i = 0; $i -lt $NumJobs; $i++) {
        $outPath = "/tmp/concurrent_hello_$i"
        $hashLine = (Invoke-LinuxShell "test -s '$outPath' && sha256sum '$outPath'") -join "`n"
        if ($LASTEXITCODE -ne 0) { throw "concurrent output '$outPath' is missing or empty" }
        $hashes += ($hashLine -split "\s+")[0]

        $actual = (Invoke-LinuxShell "'$outPath'") -join "`n"
        if ($LASTEXITCODE -ne 0 -or $actual.TrimEnd("`r", "`n") -ne "Hello, Oscan!") {
            throw "concurrent output '$outPath' did not run correctly: $actual"
        }
    }

    $uniqueHashes = @($hashes | Select-Object -Unique)
    if ($uniqueHashes.Count -ne 1) {
        throw "Expected byte-identical outputs, got $($uniqueHashes.Count) hashes: $($uniqueHashes -join ', ')"
    }

    $assetSetCount = ((Invoke-LinuxShell "find '$cacheDir' -mindepth 1 -maxdepth 1 -type d | wc -l") -join "`n").Trim()
    if ($LASTEXITCODE -ne 0 -or $assetSetCount -ne "1") {
        throw "Expected exactly one extracted asset-set directory, found '$assetSetCount'"
    }

    Write-Host "Linux native-link concurrency test passed ($NumJobs jobs, SHA-256 $($uniqueHashes[0]))"
} finally {
    foreach ($job in $jobs) {
        if ($job.State -eq "Running") { Stop-Job -Job $job }
        Remove-Job -Job $job -Force -ErrorAction SilentlyContinue
    }
    Invoke-LinuxShell "rm -f /tmp/concurrent_hello_*"
}
