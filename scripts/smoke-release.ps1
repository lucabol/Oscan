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
$expectedCompilerSource = if ($expectsBundled) { "bundled" } else { "host" }

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

$SampleSource = Join-Path $ScratchDir "hello.osc"
$SampleOutput = Join-Path $ScratchDir ("hello" + $(if ($platform -eq "windows") { ".exe" } else { "" }))
$CompileLog = Join-Path $ScratchDir "compile.stderr.txt"
Set-Content -Path $SampleSource -Encoding UTF8 -NoNewline -Value @'
fn! main() {
    println("Hello, Release!");
}
'@

# Diagnostics: verify toolchain health before compile
if ($expectsBundled -and $platform -ne "windows") {
    $tcBin = Join-Path $InstallDir "toolchain" "bin"
    Write-Host "Toolchain bin dir: $tcBin"
    if (Test-Path $tcBin) {
        Get-ChildItem $tcBin | ForEach-Object { Write-Host "  $($_.Name) $(if ($_.LinkType) { '-> ' + $_.Target } else { '' })" }
    }
    $gccBin = Join-Path $tcBin "x86_64-linux-musl-gcc"
    if (Test-Path $gccBin) {
        Write-Host "GCC version check:"
        $verOut = & $gccBin --version 2>&1 | Out-String
        Write-Host $verOut
    }
}

Push-Location $ScratchDir
try {
    $compileArgs = @()
    if ($requiresHostCompiler) {
        $compileArgs += "--libc"
    }
    $compileArgs += @($SampleSource, "-o", $SampleOutput)
    Write-Host "Running: $OscanCommand $($compileArgs -join ' ')"
    if ($platform -ne "windows") {
        # Use timeout to prevent hanging (5 minutes max)
        $timeoutSec = 300
        $proc = Start-Process -FilePath $OscanCommand -ArgumentList $compileArgs -RedirectStandardError $CompileLog -PassThru -NoNewWindow
        if (-not $proc.WaitForExit($timeoutSec * 1000)) {
            $proc.Kill()
            throw "Smoke compile timed out after ${timeoutSec}s.`nStderr so far:`n$((Get-Content $CompileLog -Raw -ErrorAction SilentlyContinue))"
        }
        if ($proc.ExitCode -ne 0) {
            throw "Release smoke compile failed (exit $($proc.ExitCode)):`n$((Get-Content $CompileLog -Raw))"
        }
    } else {
        & $OscanCommand @compileArgs 2> $CompileLog
        if ($LASTEXITCODE -ne 0) {
            throw "Release smoke compile failed:`n$((Get-Content $CompileLog -Raw))"
        }
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

Write-Host "Release smoke test passed for $ArchivePath"
