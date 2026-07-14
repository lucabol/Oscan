param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$ArchivePath,

    [string]$ScratchDir,

    [string]$ContractPath
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$ArchivePath = (Resolve-Path $ArchivePath).Path
if (-not $ContractPath) {
    $ContractPath = Join-Path $RepoRoot "packaging\toolchains\release-contract.json"
}

$platform = $Target.Split("-", 2)[0]
$contract = Get-Content $ContractPath -Raw | ConvertFrom-Json -AsHashtable
$targetSpec = if ($contract["bundled_targets"].ContainsKey($Target)) {
    $contract["bundled_targets"][$Target]
} elseif ($contract["binary_only_targets"].ContainsKey($Target)) {
    $contract["binary_only_targets"][$Target]
} else {
    throw "Release contract does not define target '$Target'."
}
$expectedArchiveSuffix = switch ($targetSpec["archive_format"]) {
    "zip" { ".zip" }
    "tar.gz" { ".tar.gz" }
    "tar.xz" { ".tar.xz" }
    default { throw "Unsupported archive format '$($targetSpec["archive_format"])' for $Target" }
}
if (-not $ArchivePath.EndsWith($expectedArchiveSuffix)) {
    throw "Archive '$ArchivePath' does not match contract format '$expectedArchiveSuffix' for $Target."
}
$expectsBundled = $targetSpec["bundle_kind"] -eq "full"
$requiresHostCompiler = [bool]($targetSpec["requires_host_compiler"] ?? $false)
$expectedNoteFile = if ($targetSpec.ContainsKey("note_file")) { [string]$targetSpec["note_file"] } else { $null }
$nativeRuntimeModes = @($targetSpec["native_runtime_modes"])
$nativeSmokeMode = [string]$targetSpec["native_smoke_mode"]
$expectedCompilerSource = if ($expectsBundled) { "bundled" } else { "host" }

# Every bundled target (Windows, Linux) ships its own relocatable compiler
# under toolchain/, so a packaged bundle must never need a host C compiler.
# find_bundled_c_compiler_in_dir() walks the toolchain directory directly
# (never PATH), so shadowing just the well-known host compiler names on PATH
# with stubs that fail immediately proves the bundle is genuinely
# self-contained: if discovery ever regressed to silently falling back to a
# host compiler, this makes that regression fail loudly here instead of
# only "working" because the CI runner happens to have build-essential/Xcode
# CLT installed. The stub directory is *prepended* to the real PATH (not
# used to replace it), so every other tool (sh, dirname, tar, ...) still
# resolves normally through the rest of the inherited PATH.
function New-NoHostCompilerPathPrefix {
    param([Parameter(Mandatory = $true)][string]$ScratchDir)

    $blockDir = Join-Path $ScratchDir "no-host-compiler-path"
    New-Item -ItemType Directory -Path $blockDir -Force | Out-Null
    if ($env:OS -eq "Windows_NT") {
        foreach ($name in @("cc", "gcc", "clang", "cl")) {
            Set-Content -Path (Join-Path $blockDir "$name.cmd") -Encoding ASCII -Value "@echo off`r`nexit /b 127"
        }
    } else {
        foreach ($name in @("cc", "gcc", "clang")) {
            $stub = Join-Path $blockDir $name
            Set-Content -Path $stub -Encoding ASCII -NoNewline -Value "#!/bin/sh`nexit 127`n"
            & chmod +x $stub
        }
    }
    return $blockDir
}

function Invoke-NoHostCompilerCommand {
    param(
        [Parameter(Mandatory = $true)][string]$ScratchDir,
        [Parameter(Mandatory = $true)][scriptblock]$Body
    )

    $savedPath = $env:PATH
    try {
        $blockDir = New-NoHostCompilerPathPrefix -ScratchDir $ScratchDir
        $env:PATH = "$blockDir$([System.IO.Path]::PathSeparator)$savedPath"
        & $Body
    } finally {
        $env:PATH = $savedPath
    }
}

function Get-DefaultScratchDir {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$Platform
    )

    if ($env:OS -eq "Windows_NT") {
        $baseDir = if ($env:RUNNER_TEMP) {
            $env:RUNNER_TEMP
        } elseif ($env:TEMP) {
            $env:TEMP
        } else {
            Join-Path $RepoRoot "target"
        }
        return Join-Path $baseDir "oscan-release-smoke\$Platform"
    }

    return Join-Path (Join-Path (Join-Path $RepoRoot "target") "release-smoke") $Platform
}

if (-not $ScratchDir) {
    $ScratchDir = Get-DefaultScratchDir -RepoRoot $RepoRoot -Platform $platform
}

if (Test-Path $ScratchDir) {
    Remove-Item $ScratchDir -Recurse -Force
}
New-Item -ItemType Directory -Path $ScratchDir -Force | Out-Null

$ExtractDir = Join-Path $ScratchDir "extract"
New-Item -ItemType Directory -Path $ExtractDir -Force | Out-Null

& tar -xf $ArchivePath -C $ExtractDir
if ($LASTEXITCODE -ne 0) {
    throw "Failed to extract $ArchivePath"
}

$BundleDir = Get-ChildItem $ExtractDir | Where-Object { $_.PSIsContainer } | Select-Object -First 1
if (-not $BundleDir) {
    throw "Expected an extracted bundle directory under $ExtractDir"
}

$InstallDir = Join-Path $ScratchDir "install"
$BinDir = Join-Path $ScratchDir "bin"
if ($platform -eq "windows") {
    & (Join-Path $BundleDir.FullName "install.ps1") -InstallDir $InstallDir -BinDir $BinDir -NoPathUpdate
    $OscanCommand = if (Test-Path (Join-Path $BinDir "oscan.cmd")) {
        Join-Path $BinDir "oscan.cmd"
    } else {
        Join-Path $InstallDir "oscan.exe"
    }
} else {
    & sh (Join-Path $BundleDir.FullName "install.sh") --source-dir $BundleDir.FullName --install-dir $InstallDir --bin-dir $BinDir
    if ($LASTEXITCODE -ne 0) {
        throw "install.sh failed for $Target"
    }
    $OscanCommand = Join-Path $BinDir "oscan"
}

if (-not (Test-Path $OscanCommand)) {
    throw "Installed oscan command was not found at $OscanCommand"
}
if ($expectsBundled -and -not (Test-Path (Join-Path $InstallDir "toolchain"))) {
    throw "Installed bundle is missing the sibling toolchain directory"
}
if ($expectedNoteFile -and -not (Test-Path (Join-Path $InstallDir $expectedNoteFile))) {
    throw "Installed bundle is missing the contract note file '$expectedNoteFile'"
}
if ($nativeRuntimeModes.Count -gt 0) {
    $RuntimeSourceDir = Join-Path $InstallDir "native-runtime"
    foreach ($sourceName in @("osc_native_shim.c", "osc_runtime.h")) {
        if (-not (Test-Path (Join-Path $RuntimeSourceDir $sourceName) -PathType Leaf)) {
            throw "Installed bundle is missing native runtime source '$sourceName'"
        }
    }
    $RuntimeArchiveDir = Join-Path $InstallDir "build\runtime-archives\$Target"
    foreach ($mode in $nativeRuntimeModes) {
        foreach ($suffix in @(".a", ".json")) {
            $runtimeAsset = Join-Path $RuntimeArchiveDir "libosc_runtime_$mode$suffix"
            if (-not (Test-Path $runtimeAsset -PathType Leaf)) {
                throw "Installed bundle is missing native runtime asset '$runtimeAsset'"
            }
        }
    }
}

$SampleSource = Join-Path $ScratchDir "hello.osc"
$SampleOutput = Join-Path $ScratchDir ("hello" + $(if ($platform -eq "windows") { ".exe" } else { "" }))
$CompileLog = Join-Path $ScratchDir "compile.stderr.txt"
Set-Content -Path $SampleSource -Encoding UTF8 -NoNewline -Value @'
fn! main() {
    println("Hello, Release!");
}
'@

Push-Location $ScratchDir
try {
    $compileArgs = @()
    if ($requiresHostCompiler) {
        $compileArgs += "--libc"
    }
    $compileArgs += @($SampleSource, "-o", $SampleOutput)
    $compileInvocation = {
        & $OscanCommand @compileArgs 2> $CompileLog
        if ($LASTEXITCODE -ne 0) {
            throw "Release smoke compile failed:`n$((Get-Content $CompileLog -Raw))"
        }
    }
    if ($expectsBundled) {
        Invoke-NoHostCompilerCommand -ScratchDir $ScratchDir -Body $compileInvocation
    } else {
        & $compileInvocation
    }
} finally {
    Pop-Location
}

$CompileText = Get-Content $CompileLog -Raw
if ($CompileText -notmatch "\b$expectedCompilerSource\b") {
    throw "Expected compiler source '$expectedCompilerSource' during release smoke test.`n$CompileText"
}

$Actual = & $SampleOutput 2>&1 | Out-String
$Actual = $Actual.TrimEnd("`r", "`n").Replace("`r`n", "`n")
if ($Actual -ne "Hello, Release!") {
    throw "Unexpected smoke test output: '$Actual'"
}

if ($nativeRuntimeModes.Count -gt 0) {
    $NativeOutput = Join-Path $ScratchDir ("hello-native" + $(if ($platform -eq "windows") { ".exe" } else { "" }))
    $NativeLog = Join-Path $ScratchDir "native.stderr.txt"
    $savedRuntimeArchiveDir = $env:OSCAN_RUNTIME_ARCHIVE_DIR
    try {
        $env:OSCAN_RUNTIME_ARCHIVE_DIR = $RuntimeArchiveDir
        $nativeArgs = @()
        if ($nativeSmokeMode -eq "hosted") {
            $nativeArgs += "--libc"
        }
        $nativeArgs += @("--backend", "native", $SampleSource, "-o", $NativeOutput)
        $nativeInvocation = {
            & $OscanCommand @nativeArgs 2> $NativeLog
            if ($LASTEXITCODE -ne 0) {
                throw "Packaged --backend native smoke compile failed:`n$((Get-Content $NativeLog -Raw))"
            }
        }
        if ($expectsBundled) {
            Invoke-NoHostCompilerCommand -ScratchDir $ScratchDir -Body $nativeInvocation
        } else {
            & $nativeInvocation
        }
    } finally {
        $env:OSCAN_RUNTIME_ARCHIVE_DIR = $savedRuntimeArchiveDir
    }
    $NativeCompileText = Get-Content $NativeLog -Raw
    if ($expectsBundled -and $NativeCompileText -notmatch "\bbundled\b") {
        throw "Packaged native link did not use the relocatable bundled toolchain:`n$NativeCompileText"
    }

    $NativeActual = & $NativeOutput 2>&1 | Out-String
    $NativeActual = $NativeActual.TrimEnd("`r", "`n").Replace("`r`n", "`n")
    if ($NativeActual -ne "Hello, Release!") {
        throw "Unexpected packaged native smoke output: '$NativeActual'"
    }
    if ($platform -eq "windows") {
        $NativeAscii = [System.Text.Encoding]::ASCII.GetString(
            [System.IO.File]::ReadAllBytes($NativeOutput)
        )
        if ($NativeAscii -notmatch "(?i)KERNEL32\.dll") {
            throw "Packaged native hello is missing its expected KERNEL32.dll import"
        }
        if ($NativeAscii -match "(?i)(msvcrt|ucrt|vcruntime|api-ms-win-crt|WS2_32\.dll|USER32\.dll|GDI32\.dll|Secur32\.dll|Crypt32\.dll)") {
            throw "Packaged native hello contains an unexpected CRT or optional Win32 DLL dependency"
        }
    }
}

Write-Host "Release smoke test passed for $ArchivePath"
