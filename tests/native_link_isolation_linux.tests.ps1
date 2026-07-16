param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan
)

$ErrorActionPreference = "Stop"
$runningOnWindows = $env:OS -eq "Windows_NT"
$repoRootHost = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

function Assert-NativeLinkIsolation {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

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

$toolchainDir = "$linuxRepoRoot/build/toolchain-linux-x86_64"
$renamedDir = "$toolchainDir.isolation-test-renamed"
$stubDir = "$linuxRepoRoot/tests/build/native-link-isolation-linux/path-stubs"

$leftoverCheck = (Invoke-LinuxShell "test -d '$renamedDir' && echo exists || echo clean") -join "`n"
if ($leftoverCheck -match "exists") {
    throw "leftover renamed toolchain directory '$renamedDir'; restore it before rerunning this test"
}

$preflight = (Invoke-LinuxShell "cd '$linuxRepoRoot' && '$linuxOscan' --backend native -o /tmp/preflight_hello examples/hello.osc 2>&1") -join "`n"
Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) "preflight native compile failed: $preflight"
Assert-NativeLinkIsolation ($preflight -match "\(embedded\)") "preflight did not use embedded assets: $preflight"

$toolchainRenamed = $false
try {
    $renameCheck = (Invoke-LinuxShell "test -d '$toolchainDir' && mv '$toolchainDir' '$renamedDir' && echo renamed || echo absent") -join "`n"
    $toolchainRenamed = $renameCheck -match "renamed"

    Invoke-LinuxShell "rm -rf '$stubDir' && mkdir -p '$stubDir'"
    if ($LASTEXITCODE -ne 0) { throw "failed to create isolated PATH directory" }

    foreach ($name in @("cc", "gcc", "clang", "x86_64-linux-musl-gcc", "x86_64-linux-musl-ld", "ld", "ld.lld", "musl-gcc", "python", "python3")) {
        Invoke-LinuxShell "printf '#!/bin/sh\nexit 127\n' > '$stubDir/$name' && chmod +x '$stubDir/$name'"
        if ($LASTEXITCODE -ne 0) { throw "failed to create blocker for '$name'" }
    }

    $pathValue = "$stubDir`:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    $compile = (Invoke-LinuxShell "cd '$linuxRepoRoot' && env PATH='$pathValue' '$linuxOscan' --backend native -o /tmp/isolation_hello examples/hello.osc 2>&1") -join "`n"
    Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) "isolated native compile failed: $compile"
    Assert-NativeLinkIsolation ($compile -match "\(embedded\)") "isolated link did not use the embedded linker: $compile"

    $verify = (Invoke-LinuxShell "test -s /tmp/isolation_hello && ! readelf -l /tmp/isolation_hello | grep -qi interp && ! readelf -d /tmp/isolation_hello 2>/dev/null | grep -q NEEDED") -join "`n"
    Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) "isolated output is missing, empty, or dynamically linked: $verify"

    $run = (Invoke-LinuxShell "/tmp/isolation_hello 2>&1") -join "`n"
    Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) "isolated hello failed: $run"
    Assert-NativeLinkIsolation ($run.TrimEnd("`r", "`n") -eq "Hello, Oscan!") "isolated hello stdout mismatch: $run"

    $size = ((Invoke-LinuxShell "stat -c %s /tmp/isolation_hello") -join "`n").Trim()
    Write-Host "Linux native-link isolation test passed (embedded linker, static output, $size bytes)"
} finally {
    if ($toolchainRenamed) {
        Invoke-LinuxShell "test -d '$renamedDir' && mv '$renamedDir' '$toolchainDir'"
    }
    Invoke-LinuxShell "rm -rf '$stubDir' /tmp/preflight_hello /tmp/isolation_hello"
}
