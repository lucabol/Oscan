param(
    [Parameter(Mandatory = $true)]
    [string]$OscanCommand,

    [Parameter(Mandatory = $true)]
    [string]$ScratchDir,

    [Parameter(Mandatory = $true)]
    [string]$RuntimeArchiveDir,

    [Parameter(Mandatory = $true)]
    [ValidateSet("freestanding", "hosted")]
    [string]$NativeSmokeMode,

    [string]$ExpectedNativeLinkSource
)

$ErrorActionPreference = "Stop"
$OscanCommand = (Resolve-Path -LiteralPath $OscanCommand).Path
$ScratchDir = (Resolve-Path -LiteralPath $ScratchDir).Path
$RuntimeArchiveDir = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
$SampleSource = Join-Path $ScratchDir "hello.osc"
$NativeOutput = Join-Path $ScratchDir "hello-native.exe"
$NativeLog = Join-Path $ScratchDir "native.stderr.txt"

if (-not (Test-Path -LiteralPath $SampleSource -PathType Leaf)) {
    throw "Release smoke sample source was not found at '$SampleSource'."
}

$blockDir = Join-Path $ScratchDir "no-host-native-compiler-path"
New-Item -ItemType Directory -Path $blockDir -Force | Out-Null
foreach ($name in @("cc", "gcc", "clang", "cl")) {
    Set-Content -Path (Join-Path $blockDir "$name.cmd") -Encoding ASCII -Value "@echo off`r`nexit /b 127"
}

$savedPath = $env:PATH
$savedRuntimeArchiveDir = $env:OSCAN_RUNTIME_ARCHIVE_DIR
try {
    $env:PATH = "$blockDir$([IO.Path]::PathSeparator)$savedPath"
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $RuntimeArchiveDir
    $nativeArgs = @()
    if ($NativeSmokeMode -eq "hosted") {
        $nativeArgs += "--libc"
    }
    $nativeArgs += @("--backend", "native", $SampleSource, "-o", $NativeOutput)
    & $OscanCommand @nativeArgs 2> $NativeLog
    if ($LASTEXITCODE -ne 0) {
        throw "Packaged --backend native smoke compile failed:`n$((Get-Content $NativeLog -Raw))"
    }
} finally {
    $env:PATH = $savedPath
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $savedRuntimeArchiveDir
}

$NativeCompileText = Get-Content $NativeLog -Raw
if ($ExpectedNativeLinkSource -and $NativeCompileText -notmatch "\b$ExpectedNativeLinkSource\b") {
    throw "Packaged native link did not use the expected linker source ('$ExpectedNativeLinkSource'):`n$NativeCompileText"
}

$NativeActual = & $NativeOutput 2>&1 | Out-String
$NativeActual = $NativeActual.TrimEnd("`r", "`n").Replace("`r`n", "`n")
if ($NativeActual -ne "Hello, Release!") {
    throw "Unexpected packaged native smoke output: '$NativeActual'"
}

$NativeAscii = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($NativeOutput))
if ($NativeAscii -notmatch "(?i)KERNEL32\.dll") {
    throw "Packaged native hello is missing its expected KERNEL32.dll import"
}
if ($NativeAscii -match "(?i)(msvcrt|ucrt|vcruntime|api-ms-win-crt|WS2_32\.dll|USER32\.dll|GDI32\.dll|Secur32\.dll|Crypt32\.dll)") {
    throw "Packaged native hello contains an unexpected CRT or optional Win32 DLL dependency"
}
