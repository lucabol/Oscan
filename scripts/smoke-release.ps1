[CmdletBinding(DefaultParameterSetName = "Archive")]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("windows-x86_64", "linux-x86_64", "macos-x86_64")]
    [string]$Target,

    # Which package profile this archive is.
    [Parameter(Mandatory = $true)]
    [Alias("Backend")]
    [ValidateSet("full", "llvm", "cranelift", "c")]
    [string]$Profile,

    [Parameter(Mandatory = $true, ParameterSetName = "Archive")]
    [string]$ArchivePath,

    # An already installed/extracted package, used to smoke the payload that
    # was actually recovered from the MSI.
    [Parameter(Mandatory = $true, ParameterSetName = "Installed")]
    [string]$InstalledPackageDir,

    # The release version this package claims to be. Checked against the
    # archive name (when present), package metadata, and compiler --version.
    [string]$Version,

    [string]$ScratchDir,

    [string]$ContractPath
)

# Variant-aware release smoke test.
#
# Every published artifact is one (target, profile) pair, so this script
# takes the profile explicitly and checks *that* variant's promises:
#
#   * the archive is the contract's archive (name, suffix, archive root);
#   * the package contains exactly the components the contract declares and
#     none of the components it does not (verified by
#     'release_tools.py verify-package-layout', which the shell smoke test
#     runs too, so both check identical facts);
#   * the installed compiler reports the variant's identity through
#     --version; and
#   * the packaged compiler really works from the package alone: object
#     packages compile and link with no C toolchain reachable at all, and
#     refuse — by name, with no fallback — everything they do not contain.

$ErrorActionPreference = "Stop"
$Backend = $Profile
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($PSCmdlet.ParameterSetName -eq "Archive") {
    if (-not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
        throw "-ArchivePath must name a packaged release archive file, not '$ArchivePath'."
    }
    $ArchivePath = (Resolve-Path -LiteralPath $ArchivePath).Path
} else {
    if (-not (Test-Path -LiteralPath $InstalledPackageDir -PathType Container)) {
        throw "-InstalledPackageDir must name an installed package directory, not '$InstalledPackageDir'."
    }
    $InstalledPackageDir = (Resolve-Path -LiteralPath $InstalledPackageDir).Path
}
if (-not $ContractPath) {
    $ContractPath = Join-Path $RepoRoot "packaging/toolchains/release-contract.json"
}

$platform = $Target.Split("-", 2)[0]
$contract = Get-Content $ContractPath -Raw | ConvertFrom-Json -AsHashtable
if ($contract["schema_version"] -ne 3) {
    throw "Release contract $ContractPath is not schema 3 (got '$($contract["schema_version"])')."
}
if (-not $contract["variants"].ContainsKey($Target)) {
    $known = ($contract["variants"].Keys | Sort-Object) -join ", "
    throw "Release contract does not define target '$Target' (known: $known)."
}
$targetSpec = $contract["variants"][$Target]
if (-not $targetSpec["profiles"].ContainsKey($Profile)) {
    $known = ($targetSpec["profiles"].Keys | Sort-Object) -join ", "
    throw "Release contract does not define profile '$Profile' for target '$Target' (known: $known)."
}
$variant = $targetSpec["profiles"][$Profile]
$profileSpec = $contract["profiles"][$Profile]
$availableBackends = @($profileSpec["backends"])
$defaultBackend = [string]$profileSpec["default_backend"]
$backendKind = if ($Profile -eq "full") { "full" } else { [string]$contract["backends"][$Profile]["kind"] }
$isFullPackage = $Profile -eq "full"
$isObjectPackage = $backendKind -eq "object"
$isObjectCapable = $availableBackends -contains "llvm" -or $availableBackends -contains "cranelift"
$linkMode = [string]($variant["link_mode"] ?? "")
$usesSidecar = $linkMode -eq "sidecar"
$requiresHostCompiler = [bool]($variant["requires_host_compiler"] ?? $false)
$binaryName = [string]$targetSpec["binary_name"]
# Where a sidecar object package keeps its verified native-link assets, taken
# from the contract rather than restated here.
$sidecarSpec = $contract["components"]["direct_link_sidecar"]
$sidecarDirName = ([string]$sidecarSpec["position"]).Split("/")[-1]
$sidecarManifestName = [string]$sidecarSpec["manifest_name"]
$expectedArchiveSuffix = switch ([string]$targetSpec["archive_format"]) {
    "zip" { ".zip" }
    "tar.gz" { ".tar.gz" }
    "tar.xz" { ".tar.xz" }
    default { throw "Unsupported archive format '$($targetSpec["archive_format"])' for $Target." }
}
if ($ArchivePath -and -not $ArchivePath.EndsWith($expectedArchiveSuffix)) {
    throw "Archive '$ArchivePath' does not match the contract format '$expectedArchiveSuffix' for $Target."
}

function Resolve-PythonCommand {
    # Windows machines often expose a `python3` App Execution Alias stub that
    # resolves but refuses to run, so a candidate is only accepted once it has
    # actually reported a version.
    $candidates = if ($env:OS -eq "Windows_NT") { @("python", "python3") } else { @("python3", "python") }
    foreach ($candidate in $candidates) {
        $command = Get-Command $candidate -ErrorAction SilentlyContinue
        if (-not $command) {
            continue
        }
        $version = & $command.Source --version 2>&1 | Out-String
        if ($LASTEXITCODE -eq 0 -and $version -match "Python 3") {
            return $command.Source
        }
    }
    throw "no working Python 3 interpreter found on PATH (tried $($candidates -join ', ')); the release smoke test needs one to verify the package layout."
}

function Get-DefaultScratchDir {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$Target,
        [Parameter(Mandatory = $true)][string]$Backend
    )

    if ($env:OS -eq "Windows_NT") {
        $baseDir = if ($env:RUNNER_TEMP) {
            $env:RUNNER_TEMP
        } elseif ($env:TEMP) {
            $env:TEMP
        } else {
            Join-Path $RepoRoot "target"
        }
        return Join-Path (Join-Path $baseDir "oscan-release-smoke") "$Target-$Backend"
    }

    return Join-Path (Join-Path (Join-Path $RepoRoot "target") "release-smoke") "$Target-$Backend"
}

$Python = Resolve-PythonCommand
$ReleaseTools = Join-Path $PSScriptRoot "release_tools.py"

if (-not $ScratchDir) {
    $ScratchDir = Get-DefaultScratchDir -RepoRoot $RepoRoot -Target $Target -Backend $Backend
}
if (Test-Path $ScratchDir) {
    Remove-Item $ScratchDir -Recurse -Force
}
New-Item -ItemType Directory -Path $ScratchDir -Force | Out-Null
$ScratchDir = (Resolve-Path -LiteralPath $ScratchDir).Path

# Every override that could make a packaged compiler behave like a
# development checkout: the compiler, linker and LLVM provider overrides,
# the native-asset cache, the runtime-archive directory, and the
# runtime-archive builder opt-in. They are removed for *every* packaged
# invocation below, so anything that works here works from the package
# alone. In particular OSCAN_RUNTIME_ARCHIVE_DIR stays unset: a sidecar
# package must use its fixed executable-relative archives, while a strict
# package must use the archives compiled into its executable.
$ScrubbedEnvironmentNames = @(
    "OSCAN_NO_TOOLCHAIN",
    "OSCAN_CC",
    "OSCAN_TOOLCHAIN_DIR",
    "OSCAN_LLVM_LIB",
    "OSCAN_LLVM_DIR",
    "OSCAN_NATIVE_LINKER",
    "OSCAN_NATIVE_LINKER_FLAVOR",
    "OSCAN_NATIVE_ASSET_CACHE_DIR",
    "OSCAN_RUNTIME_ARCHIVE_DIR",
    "OSCAN_RUNTIME_BUILDER",
    "OSCAN_ARCHIVE_CC",
    "OSCAN_ARCHIVE_AR",
    "CC",
    "CXX",
    "LD"
)

# The compiler discovers a bundled C toolchain by walking the package
# directory, never PATH, so making the well-known host tool names
# unusable proves a package is genuinely self-contained: a regression to a
# host compiler fails loudly here instead of "working" because the runner
# happens to have build-essential/Xcode CLT installed.
#
# Object packages additionally block the host *linkers*: sidecar variants run
# their verified packaged linker by absolute path and strict variants invoke
# LLD in-process, so neither may need PATH. A C package drives its bundled
# compiler, which finds its own sibling linker, so only compilers are blocked
# there.
function New-BlockedHostToolDir {
    param(
        [Parameter(Mandatory = $true)][string]$ScratchDir,
        [switch]$IncludeLinkers
    )

    $blockDir = Join-Path $ScratchDir "blocked-host-tools"
    New-Item -ItemType Directory -Path $blockDir -Force | Out-Null
    if ($env:OS -eq "Windows_NT") {
        # These only catch a PATHEXT-driven lookup (a shell, or a tool that
        # spells the extension out); a process launched with the bare name
        # 'gcc' is resolved by appending '.exe' and continuing down PATH, so
        # the isolation in Get-NoHostToolPath is what actually blocks a host
        # toolchain here.
        $names = @("cc", "gcc", "g++", "clang", "clang++", "cl")
        if ($IncludeLinkers) {
            $names += @("ld", "lld", "ld.lld", "link")
        }
        foreach ($name in $names) {
            Set-Content -Path (Join-Path $blockDir "$name.cmd") -Encoding ASCII -Value "@echo off`r`nexit /b 127"
        }
    } else {
        $names = @("cc", "gcc", "g++", "clang", "clang++", "x86_64-linux-musl-gcc")
        if ($IncludeLinkers) {
            $names += @("ld", "ld.lld", "lld", "x86_64-linux-musl-ld")
        }
        foreach ($name in $names) {
            $stub = Join-Path $blockDir $name
            Set-Content -Path $stub -Encoding ASCII -NoNewline -Value "#!/bin/sh`nexit 127`n"
            & chmod +x $stub
        }
    }
    return $blockDir
}

function Get-NoHostToolPath {
    <#
        The PATH a packaged invocation runs with when no host tool may be
        reachable.

        Windows: PATH *becomes* the blocker directory. A `.cmd` stub cannot
        shadow a host `gcc.exe`, because a process started with the bare name
        'gcc' is resolved by appending executable extensions and continuing
        down PATH — so leaving the real PATH in place would still let a host
        toolchain be found. Nothing is lost by isolating it: the smoke test
        runs the installed compiler and the programs it produces by absolute
        path, and a package resolves its own linker, provider and runtime
        archives relative to the installed executable.

        POSIX: the stubs are real executables and do shadow the host tools by
        name, so the rest of PATH stays available for ordinary utilities.
    #>
    param(
        [Parameter(Mandatory = $true)][string]$BlockDir,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$SavedPath
    )

    if ($env:OS -eq "Windows_NT") {
        return $BlockDir
    }
    return "$BlockDir$([System.IO.Path]::PathSeparator)$SavedPath"
}

$BlockedHostToolDir = New-BlockedHostToolDir -ScratchDir $ScratchDir -IncludeLinkers:$isObjectCapable

function Invoke-PackagedOscan {
    <#
        Run the installed compiler with a scrubbed environment and return
        its exit code. Nothing here throws on a non-zero exit: both the
        "must succeed" and the "must be refused" assertions below need the
        real exit code and the real diagnostics.
    #>
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [switch]$NoToolchainProfile,
        [switch]$BlockHostTools
    )

    $saved = @{}
    foreach ($name in $ScrubbedEnvironmentNames) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    $savedPath = $env:PATH
    try {
        if ($NoToolchainProfile) {
            $env:OSCAN_NO_TOOLCHAIN = "1"
        }
        if ($BlockHostTools) {
            $env:PATH = Get-NoHostToolPath -BlockDir $BlockedHostToolDir -SavedPath $savedPath
        }
        & $script:OscanCommand @Arguments 2> $LogPath | Out-Null
        return $LASTEXITCODE
    } finally {
        $env:PATH = $savedPath
        foreach ($name in $ScrubbedEnvironmentNames) {
            if ($null -eq $saved[$name]) {
                Remove-Item "Env:$name" -ErrorAction SilentlyContinue
            } else {
                Set-Item "Env:$name" -Value $saved[$name]
            }
        }
    }
}

function Get-LogText {
    param([Parameter(Mandatory = $true)][string]$LogPath)

    if (-not (Test-Path -LiteralPath $LogPath)) {
        return ""
    }
    $text = Get-Content -LiteralPath $LogPath -Raw
    if ($null -eq $text) {
        return ""
    }
    return $text
}

function Assert-LogMatches {
    param(
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$Pattern,
        [Parameter(Mandatory = $true)][string]$What
    )

    $text = Get-LogText -LogPath $LogPath
    if ($text -notmatch $Pattern) {
        throw "$What did not report /$Pattern/:`n$text"
    }
}

function Assert-ProgramRuns {
    param(
        [Parameter(Mandatory = $true)][string]$ExePath,
        [Parameter(Mandatory = $true)][string]$What
    )

    if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
        throw "$What produced no executable at $ExePath"
    }
    $actual = (& $ExePath 2>&1 | Out-String).Replace("`r`n", "`n").TrimEnd("`n")
    if ($LASTEXITCODE -ne 0) {
        throw "$What produced an executable that exited with $LASTEXITCODE"
    }
    if ($actual -ne "Hello, Release!") {
        throw "$What produced unexpected output: '$actual'"
    }
}

function Get-ComparablePath {
    <#
        A path in a form two spellings of the same directory compare equal
        in: Rust's `fs::canonicalize` yields a Windows verbatim path
        (`\\?\C:\...`) and resolves symlinks, so the reported directory and
        the one this script built by hand have to be normalized the same way
        before they can be compared.
    #>
    param([Parameter(Mandatory = $true)][string]$Path)

    $normalized = [System.IO.Path]::GetFullPath(($Path.Trim() -replace '^(\\\\\?\\|//\?/)', ''))
    if (Test-Path -LiteralPath $normalized) {
        $target = (Get-Item -LiteralPath $normalized -Force).ResolveLinkTarget($true)
        if ($target) {
            $normalized = $target.FullName
        }
    }
    return $normalized.TrimEnd([System.IO.Path]::DirectorySeparatorChar)
}

function Assert-PackagedInProcessLinking {
    param(
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$What
    )

    $text = Get-LogText -LogPath $LogPath
    if ($text -match "(?m)^\[verbose\] native-link assets:") {
        throw "$What resolved extractable native-link assets; the strict package must feed its embedded inputs directly to in-process LLD:`n$text"
    }
    if ($text -match "Compiling with ") {
        throw "$What invoked a C compiler instead of the strict in-process linker:`n$text"
    }
}

function Find-Dumpbin {
    $inPath = Get-Command dumpbin -ErrorAction SilentlyContinue
    if ($inPath) {
        return $inPath.Source
    }

    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        return $null
    }
    $vsPath = & $vswhere -latest -property installationPath 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $vsPath) {
        return $null
    }
    $candidate = Get-ChildItem `
        -Path (Join-Path $vsPath "VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe") `
        -File -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending |
        Select-Object -First 1
    if ($candidate) {
        return $candidate.FullName
    }
    return $null
}

function Assert-StrictCompilerDependencies {
    param([Parameter(Mandatory = $true)][string]$ExePath)

    $dumpbin = Find-Dumpbin
    if ($dumpbin) {
        $raw = @(& $dumpbin /nologo /dependents $ExePath 2>&1)
        if ($LASTEXITCODE -ne 0) {
            throw "dumpbin could not inspect strict packaged compiler dependencies:`n$($raw -join "`n")"
        }
        $dllNames = @(
            $raw |
                Where-Object { $_ -match '^\s+\S+\.dll\s*$' } |
                ForEach-Object { $_.Trim() } |
                Sort-Object -Unique
        )
    } else {
        # PE import names are ASCII. Scanning every DLL reference is a
        # conservative fallback: it can reject more than the import table, but
        # it cannot overlook a forbidden imported runtime/provider DLL.
        $ascii = [System.Text.Encoding]::ASCII.GetString(
            [System.IO.File]::ReadAllBytes($ExePath)
        )
        $dllNames = @(
            [regex]::Matches($ascii, '(?i)[A-Z0-9_+.-]+\.dll') |
                ForEach-Object { $_.Value } |
                Sort-Object -Unique
        )
    }
    if ($dllNames.Count -eq 0) {
        throw "No DLL dependencies could be read from strict packaged compiler $ExePath"
    }

    $forbidden = @(
        $dllNames | Where-Object {
            $_ -match '^(?i)(msvcr[^.]*|ucrtbase|vcruntime[^.]*|msvcp[^.]*|api-ms-win-crt-[^.]+|libLLVM[^.]*|LLVM-C|libclang-cpp[^.]*|liblld[^.]*|lld[^.]*|libc\+\+|libunwind|libwinpthread[^.]*|libgcc_s[^.]*|libstdc\+\+[^.]*)\.dll$'
        }
    )
    if ($forbidden.Count -ne 0) {
        throw "Strict packaged compiler has forbidden dynamic CRT/provider/linker dependencies: $($forbidden -join ', ')"
    }
}

function Assert-PackagedSidecarAssets {
    <#
        An object package must resolve its linker and the rest of its
        native-link assets from the verified sidecar *inside the package that
        was just installed* — not from assets embedded in the binary, not
        from an extracted asset cache, and not from another copy that happens
        to exist on this machine. Matching the word "sidecar" alone would not
        show that, so the reported directory is required to be absolute and
        to be the installed package's own sidecar root.
    #>
    param(
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$SidecarRoot,
        [Parameter(Mandatory = $true)][string]$ManifestName,
        [Parameter(Mandatory = $true)][string]$What
    )

    $text = Get-LogText -LogPath $LogPath
    $reported = [regex]::Match(
        $text,
        '(?m)^\[verbose\] native-link assets: (?<source>\S+) \((?<dir>.+?)\)[ \t\r]*$'
    )
    if (-not $reported.Success) {
        throw "$What did not report which native-link assets it resolved:`n$text"
    }

    $source = $reported.Groups["source"].Value
    if ($source -ne "sidecar") {
        throw "$What resolved its native-link assets from '$source'; a packaged object build must use the verified sidecar beside its own executable:`n$text"
    }
    $reportedDir = $reported.Groups["dir"].Value
    if (-not [System.IO.Path]::IsPathRooted(($reportedDir -replace '^(\\\\\?\\|//\?/)', ''))) {
        throw "$What reported the relative native-link asset directory '$reportedDir'; packaged assets are always resolved through an absolute, executable-relative path."
    }
    $reportedFull = Get-ComparablePath -Path $reportedDir
    $expectedFull = Get-ComparablePath -Path $SidecarRoot
    $comparison = if ($env:OS -eq "Windows_NT") {
        [System.StringComparison]::OrdinalIgnoreCase
    } else {
        [System.StringComparison]::Ordinal
    }
    if (-not [string]::Equals($reportedFull, $expectedFull, $comparison)) {
        throw "$What used the native-link assets in '$reportedFull', which is not the installed package's sidecar directory '$expectedFull'."
    }
    if (-not (Test-Path -LiteralPath (Join-Path $reportedFull $ManifestName) -PathType Leaf)) {
        throw "$What reported '$reportedFull' as its sidecar root, but that directory contains no $ManifestName."
    }
}

function Assert-Refused {
    <#
        A packaged compiler must refuse what it does not contain by name,
        with an actionable diagnostic, and without falling back to anything
        else on the machine.
    #>
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$OutputPath,
        [Parameter(Mandatory = $true)][string[]]$ExpectedPatterns,
        [Parameter(Mandatory = $true)][string]$What
    )

    # Deliberately *not* run under OSCAN_NO_TOOLCHAIN=1: an object package
    # must refuse these because of what it is, not because of how it was
    # invoked.
    $exitCode = Invoke-PackagedOscan -Arguments $Arguments -LogPath $LogPath -BlockHostTools
    $text = Get-LogText -LogPath $LogPath
    if ($exitCode -eq 0) {
        throw "$What was accepted (exit 0) but must be refused:`n$text"
    }
    foreach ($pattern in $ExpectedPatterns) {
        if ($text -notmatch $pattern) {
            throw "$What was refused without the expected diagnostic /$pattern/:`n$text"
        }
    }
    if ($text -match "Compiling with ") {
        throw "$What fell back to a C compiler instead of refusing:`n$text"
    }
    if (Test-Path -LiteralPath $OutputPath) {
        throw "$What was refused but still produced $OutputPath"
    }
}

# --- extract/install ----------------------------------------------------------

if ($PSCmdlet.ParameterSetName -eq "Archive") {
    $ExtractDir = Join-Path $ScratchDir "extract"
    New-Item -ItemType Directory -Path $ExtractDir -Force | Out-Null
    & tar -xf $ArchivePath -C $ExtractDir
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to extract $ArchivePath"
    }
    $bundleDirectories = @(Get-ChildItem -LiteralPath $ExtractDir -Directory)
    if ($bundleDirectories.Count -ne 1) {
        throw "Expected exactly one extracted bundle directory under $ExtractDir, found $($bundleDirectories.Count)."
    }
    $BundleDir = $bundleDirectories[0].FullName

    $layoutArgs = @(
        $ReleaseTools, "verify-package-layout",
        "--target", $Target,
        "--profile", $Profile,
        "--root", $BundleDir,
        "--stage", "extracted",
        "--archive", $ArchivePath,
        "--contract", $ContractPath
    )
    if ($Version) {
        $layoutArgs += @("--version", $Version)
    }
    & $Python @layoutArgs
    if ($LASTEXITCODE -ne 0) {
        throw "Extracted $Target/$Backend package does not match the release contract."
    }

    $packageMetadata = Get-Content (Join-Path $BundleDir "oscan-package.json") -Raw | ConvertFrom-Json
    $InstallRoot = Join-Path $ScratchDir "install"
    $InstallDir = Join-Path (Join-Path (Join-Path $InstallRoot "profiles") $Profile) ([string]$packageMetadata.version)
    $BinDir = Join-Path $ScratchDir "bin"
    if ($platform -eq "windows") {
        # install.ps1 is a PowerShell script with its own error handling; it
        # throws on failure, and $LASTEXITCODE here belongs to the robocopy it
        # ran internally (1 means "files copied"), so it must not be read.
        & (Join-Path $BundleDir "install.ps1") -InstallRoot $InstallRoot -BinDir $BinDir -NoPathUpdate
        # Compiles run the installed executable by absolute path. Inside a
        # no-host-tool body PATH is the blocker directory alone, which leaves
        # no interpreter for a .cmd shim. The shim is still exercised by the
        # --version check below with the ordinary PATH.
        $OscanCommand = Join-Path $InstallDir $binaryName
        $OscanShim = Join-Path $BinDir "oscan-$Profile.cmd"
        if (-not (Test-Path -LiteralPath $OscanShim -PathType Leaf)) {
            throw "install.ps1 did not create the oscan-$Profile.cmd shim in $BinDir"
        }
    } else {
        & sh (Join-Path $BundleDir "install.sh") --source-dir $BundleDir --install-root $InstallRoot --bin-dir $BinDir
        if ($LASTEXITCODE -ne 0) {
            throw "install.sh failed for $Target/$Backend"
        }
        # The bin-directory symlink, so every invocation also proves the
        # compiler resolves its package through its real executable path.
        $OscanCommand = Join-Path $BinDir "oscan-$Profile"
        $OscanShim = $OscanCommand
    }
} else {
    $InstallDir = $InstalledPackageDir
    $OscanCommand = Join-Path $InstallDir $binaryName
    $OscanShim = $OscanCommand
}
if (-not (Test-Path -LiteralPath $OscanCommand)) {
    throw "Installed oscan command was not found at $OscanCommand"
}

$installedLayoutArgs = @(
    $ReleaseTools, "verify-package-layout",
    "--target", $Target,
    "--profile", $Profile,
    "--root", $InstallDir,
    "--stage", "installed",
    "--contract", $ContractPath
)
if ($Version) {
    $installedLayoutArgs += @("--version", $Version)
}
& $Python @installedLayoutArgs
if ($LASTEXITCODE -ne 0) {
    throw "Installed $Target/$Backend package does not match the release contract."
}
if ($platform -eq "windows" -and $linkMode -eq "inprocess") {
    Assert-StrictCompilerDependencies -ExePath $OscanCommand
}

# --- identity ----------------------------------------------------------------

# Through the shim the installer put on PATH, so the installed entry point is
# covered too; the compiles below run the executable itself.
$VersionLog = Join-Path $ScratchDir "version.txt"
$versionText = (& $OscanShim --version 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0) {
    throw "Packaged 'oscan --version' failed:`n$versionText"
}
Set-Content -LiteralPath $VersionLog -Value $versionText
$backendList = $availableBackends -join ", "
foreach ($expected in @(
        "(?m)^backends: $([regex]::Escape($backendList))[ \t\r]*$",
        "(?m)^default-backend: $defaultBackend[ \t\r]*$",
        "(?m)^distribution: $Profile[ \t\r]*$",
        "(?m)^toolchain-free: $(if ([bool]$variant['toolchain_free']) { 'yes' } else { 'no' })[ \t\r]*$"
    )) {
    if ($versionText -notmatch $expected) {
        throw "Packaged 'oscan --version' does not report /$expected/:`n$versionText"
    }
}
if ($Version -and $versionText -notmatch [regex]::Escape($Version)) {
    throw "Packaged 'oscan --version' does not carry release version '$Version':`n$versionText"
}

# --- sample program ----------------------------------------------------------

$SampleSource = Join-Path $ScratchDir "hello.osc"
Set-Content -Path $SampleSource -Encoding UTF8 -NoNewline -Value @'
fn! main() {
    println("Hello, Release!");
}
'@
$exeSuffix = if ($platform -eq "windows") { ".exe" } else { "" }

if ($isObjectCapable -and $platform -eq "windows") {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $isWindowsAdministrator = $principal.IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator
    )
} else {
    $isWindowsAdministrator = $false
}
# Trusted CI/release opt-in: the object backends refuse a final link from an
# elevated process unless it is explicitly allowed, and the Windows release
# runner is elevated.
$ElevatedOptIn = if ($isWindowsAdministrator) { @("--allow-elevated-native-link") } else { @() }

if ($isFullPackage) {
    # The full profile must exercise its deterministic default and every
    # explicitly selectable backend from the installed package alone.
    $defaultOutput = Join-Path $ScratchDir "hello-default$exeSuffix"
    $defaultLog = Join-Path $ScratchDir "default.stderr.txt"
    $defaultArgs = @("--verbose") + $ElevatedOptIn + @($SampleSource, "-o", $defaultOutput)
    $exitCode = Invoke-PackagedOscan -Arguments $defaultArgs -LogPath $defaultLog -BlockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged full-profile default compile failed:`n$(Get-LogText -LogPath $defaultLog)"
    }
    Assert-LogMatches -LogPath $defaultLog -Pattern "(?m)^\[verbose\] $defaultBackend backend target:" `
        -What "Packaged full-profile default compile"
    if ($usesSidecar) {
        Assert-PackagedSidecarAssets -LogPath $defaultLog `
            -SidecarRoot (Join-Path $InstallDir $sidecarDirName) `
            -ManifestName $sidecarManifestName `
            -What "Packaged full-profile default compile"
    } else {
        Assert-PackagedInProcessLinking -LogPath $defaultLog `
            -What "Packaged full-profile default compile"
    }
    Assert-ProgramRuns -ExePath $defaultOutput -What "Packaged full-profile default compile"

    foreach ($selectedBackend in $availableBackends) {
        $output = Join-Path $ScratchDir "hello-$selectedBackend$exeSuffix"
        $log = Join-Path $ScratchDir "$selectedBackend.stderr.txt"
        $backendOptIn = if ($selectedBackend -eq "c") { @() } else { $ElevatedOptIn }
        $arguments = @("--verbose", "--backend", $selectedBackend) + $backendOptIn +
            @($SampleSource, "-o", $output)
        $exitCode = Invoke-PackagedOscan -Arguments $arguments -LogPath $log -BlockHostTools
        if ($exitCode -ne 0) {
            throw "Packaged full-profile '--backend $selectedBackend' compile failed:`n$(Get-LogText -LogPath $log)"
        }
        if ($selectedBackend -eq "c") {
            Assert-LogMatches -LogPath $log -Pattern "Compiling with .+ \(bundled" `
                -What "Packaged full-profile C compile"
        } else {
            Assert-LogMatches -LogPath $log `
                -Pattern "(?m)^\[verbose\] $selectedBackend backend target:" `
                -What "Packaged full-profile $selectedBackend compile"
            if ($usesSidecar) {
                Assert-PackagedSidecarAssets -LogPath $log `
                    -SidecarRoot (Join-Path $InstallDir $sidecarDirName) `
                    -ManifestName $sidecarManifestName `
                    -What "Packaged full-profile $selectedBackend compile"
            } else {
                Assert-PackagedInProcessLinking -LogPath $log `
                    -What "Packaged full-profile $selectedBackend compile"
            }
        }
        Assert-ProgramRuns -ExePath $output -What "Packaged full-profile $selectedBackend compile"
    }

    $hostedOutput = Join-Path $ScratchDir "hello-hosted$exeSuffix"
    $hostedLog = Join-Path $ScratchDir "hosted.stderr.txt"
    $hostedArgs = @("--verbose", "--libc") + $ElevatedOptIn +
        @($SampleSource, "-o", $hostedOutput)
    $exitCode = Invoke-PackagedOscan -Arguments $hostedArgs -LogPath $hostedLog -BlockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged full-profile '--libc' compile failed:`n$(Get-LogText -LogPath $hostedLog)"
    }
    Assert-LogMatches -LogPath $hostedLog -Pattern "Linking hosted executable with .+ \(bundled\)" `
        -What "Packaged full-profile hosted compile"
    Assert-ProgramRuns -ExePath $hostedOutput -What "Packaged full-profile hosted compile"

    $extraCSource = Join-Path $ScratchDir "extra.c"
    Set-Content -Path $extraCSource -Encoding ASCII -Value "int oscan_smoke_extra(void) { return 0; }"
    $extraOutput = Join-Path $ScratchDir "hello-extra-c$exeSuffix"
    $extraLog = Join-Path $ScratchDir "extra-c.stderr.txt"
    $extraArgs = @("--verbose", "--extra-c", $extraCSource) + $ElevatedOptIn +
        @($SampleSource, "-o", $extraOutput)
    $exitCode = Invoke-PackagedOscan -Arguments $extraArgs -LogPath $extraLog -BlockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged full-profile '--extra-c' compile failed:`n$(Get-LogText -LogPath $extraLog)"
    }
    Assert-LogMatches -LogPath $extraLog -Pattern "Linking freestanding executable with .+ \(bundled\)" `
        -What "Packaged full-profile extra-C compile"
    Assert-ProgramRuns -ExePath $extraOutput -What "Packaged full-profile extra-C compile"
} elseif ($isObjectPackage) {
    # 1. The default backend: a distribution build defaults to the one
    #    backend it ships, deterministically and without probing.
    $defaultOutput = Join-Path $ScratchDir "hello-default$exeSuffix"
    $defaultLog = Join-Path $ScratchDir "default.stderr.txt"
    $defaultArgs = @("--verbose") + $ElevatedOptIn + @($SampleSource, "-o", $defaultOutput)
    $exitCode = Invoke-PackagedOscan -Arguments $defaultArgs -LogPath $defaultLog `
        -NoToolchainProfile -BlockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged $Backend default-backend compile failed:`n$(Get-LogText -LogPath $defaultLog)"
    }
    Assert-LogMatches -LogPath $defaultLog -Pattern "(?m)^\[verbose\] $Backend backend target:" `
        -What "Packaged $Backend default compile"
    if ($usesSidecar) {
        Assert-PackagedSidecarAssets -LogPath $defaultLog `
            -SidecarRoot (Join-Path $InstallDir $sidecarDirName) `
            -ManifestName $sidecarManifestName `
            -What "Packaged $Backend default compile"
    } else {
        Assert-PackagedInProcessLinking -LogPath $defaultLog `
            -What "Packaged $Backend default compile"
    }
    if ($Backend -eq "llvm") {
        $providerPattern = if ($linkMode -eq "inprocess") {
            "(?m)^\[verbose\] LLVM code generator: statically-linked-llvm-\d+\.\d+\.\d+ \(LLVM \d+\.\d+\.\d+, targets: "
        } else {
            "(?m)^\[verbose\] LLVM code generator: .+ \(LLVM \d+\.\d+\.\d+, targets: "
        }
        Assert-LogMatches -LogPath $defaultLog `
            -Pattern $providerPattern `
            -What "Packaged llvm default compile"
    }
    Assert-ProgramRuns -ExePath $defaultOutput -What "Packaged $Backend default compile"

    # 2. The same backend named explicitly.
    $explicitOutput = Join-Path $ScratchDir "hello-explicit$exeSuffix"
    $explicitLog = Join-Path $ScratchDir "explicit.stderr.txt"
    $explicitArgs = @("--verbose", "--backend", $Backend) + $ElevatedOptIn +
        @($SampleSource, "-o", $explicitOutput)
    $exitCode = Invoke-PackagedOscan -Arguments $explicitArgs -LogPath $explicitLog `
        -NoToolchainProfile -BlockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged '--backend $Backend' compile failed:`n$(Get-LogText -LogPath $explicitLog)"
    }
    Assert-LogMatches -LogPath $explicitLog -Pattern "(?m)^\[verbose\] $Backend backend target:" `
        -What "Packaged '--backend $Backend' compile"
    if ($usesSidecar) {
        Assert-PackagedSidecarAssets -LogPath $explicitLog `
            -SidecarRoot (Join-Path $InstallDir $sidecarDirName) `
            -ManifestName $sidecarManifestName `
            -What "Packaged '--backend $Backend' compile"
    } else {
        Assert-PackagedInProcessLinking -LogPath $explicitLog `
            -What "Packaged '--backend $Backend' compile"
    }
    Assert-ProgramRuns -ExePath $explicitOutput -What "Packaged '--backend $Backend' compile"

    # 3. Cranelift keeps accepting its deprecated spelling, with exactly one
    #    warning — the alias is a compatibility shim, never a package label.
    if ($Backend -eq "cranelift") {
        $aliasOutput = Join-Path $ScratchDir "hello-alias$exeSuffix"
        $aliasLog = Join-Path $ScratchDir "alias.stderr.txt"
        $aliasArgs = @("--backend", "native") + $ElevatedOptIn + @($SampleSource, "-o", $aliasOutput)
        $exitCode = Invoke-PackagedOscan -Arguments $aliasArgs -LogPath $aliasLog `
            -NoToolchainProfile -BlockHostTools
        if ($exitCode -ne 0) {
            throw "Packaged '--backend native' alias compile failed:`n$(Get-LogText -LogPath $aliasLog)"
        }
        Assert-LogMatches -LogPath $aliasLog `
            -Pattern "warning: '--backend native' is deprecated; use '--backend cranelift'" `
            -What "Packaged '--backend native' alias"
        Assert-ProgramRuns -ExePath $aliasOutput -What "Packaged '--backend native' alias compile"
    }

    # 4. Windows freestanding executables depend on KERNEL32 only: no CRT and
    #    no optional Win32 DLL crept in through the packaged linker.
    if ($platform -eq "windows") {
        $ascii = [System.Text.Encoding]::ASCII.GetString(
            [System.IO.File]::ReadAllBytes($defaultOutput)
        )
        if ($ascii -notmatch "(?i)KERNEL32\.dll") {
            throw "Packaged $Backend hello is missing its expected KERNEL32.dll import"
        }
        if ($ascii -match "(?i)(msvcrt|ucrt|vcruntime|api-ms-win-crt|WS2_32\.dll|USER32\.dll|GDI32\.dll|Secur32\.dll|Crypt32\.dll)") {
            throw "Packaged $Backend hello contains an unexpected CRT or optional Win32 DLL dependency"
        }
    }

    # 5. Everything this package does not contain is refused by name.
    $otherObjectBackend = if ($Backend -eq "llvm") { "cranelift" } else { "llvm" }
    Assert-Refused -Arguments (@("--backend", "c", $SampleSource, "-o", (Join-Path $ScratchDir "refused-c$exeSuffix"))) `
        -LogPath (Join-Path $ScratchDir "refused-backend-c.stderr.txt") `
        -OutputPath (Join-Path $ScratchDir "refused-c$exeSuffix") `
        -ExpectedPatterns @(
            "the c backend is not included in this compiler build",
            "this build includes: $Backend",
            "archive name ends in '-full' or '-c'"
        ) `
        -What "'--backend c' in the $Target/$Backend package"

    Assert-Refused -Arguments (@("--backend", $otherObjectBackend, $SampleSource, "-o", (Join-Path $ScratchDir "refused-other$exeSuffix"))) `
        -LogPath (Join-Path $ScratchDir "refused-other-backend.stderr.txt") `
        -OutputPath (Join-Path $ScratchDir "refused-other$exeSuffix") `
        -ExpectedPatterns @(
            "the $otherObjectBackend backend is not included in this compiler build",
            "archive name ends in '-full' or '-$otherObjectBackend'"
        ) `
        -What "'--backend $otherObjectBackend' in the $Target/$Backend package"

    Assert-Refused -Arguments (@("--libc") + $ElevatedOptIn + @($SampleSource, "-o", (Join-Path $ScratchDir "refused-libc$exeSuffix"))) `
        -LogPath (Join-Path $ScratchDir "refused-libc.stderr.txt") `
        -OutputPath (Join-Path $ScratchDir "refused-libc$exeSuffix") `
        -ExpectedPatterns @(
            "does not include the C backend",
            "refuses --libc",
            "install a package that includes the C backend"
        ) `
        -What "'--libc' in the $Target/$Backend package"

    $extraCSource = Join-Path $ScratchDir "extra.c"
    Set-Content -Path $extraCSource -Encoding ASCII -Value "int oscan_smoke_extra(void) { return 0; }"
    Assert-Refused -Arguments (@("--extra-c", $extraCSource) + $ElevatedOptIn + @($SampleSource, "-o", (Join-Path $ScratchDir "refused-extra$exeSuffix"))) `
        -LogPath (Join-Path $ScratchDir "refused-extra-c.stderr.txt") `
        -OutputPath (Join-Path $ScratchDir "refused-extra$exeSuffix") `
        -ExpectedPatterns @(
            "does not include the C backend",
            "refuses --extra-c"
        ) `
        -What "'--extra-c' in the $Target/$Backend package"

    Assert-Refused -Arguments (@($SampleSource, "-o", (Join-Path $ScratchDir "refused-output.c"))) `
        -LogPath (Join-Path $ScratchDir "refused-c-output.stderr.txt") `
        -OutputPath (Join-Path $ScratchDir "refused-output.c") `
        -ExpectedPatterns @(
            "the c backend is not included in this compiler build"
        ) `
        -What "C source output in the $Target/$Backend package"
} else {
    # A C package is the portability package: it emits C and needs a C
    # compiler for it. Windows and Linux bundle their own; macOS uses the
    # host Apple Command Line Tools.
    $expectedCompilerSource = if ($requiresHostCompiler) { "host" } else { "bundled" }
    $hostCompilerArgs = if ($requiresHostCompiler) { @("--libc") } else { @() }
    $blockHostTools = -not $requiresHostCompiler

    $defaultOutput = Join-Path $ScratchDir "hello-default$exeSuffix"
    $defaultLog = Join-Path $ScratchDir "default.stderr.txt"
    $exitCode = Invoke-PackagedOscan `
        -Arguments (@("--verbose") + $hostCompilerArgs + @($SampleSource, "-o", $defaultOutput)) `
        -LogPath $defaultLog -BlockHostTools:$blockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged c default-backend compile failed:`n$(Get-LogText -LogPath $defaultLog)"
    }
    Assert-LogMatches -LogPath $defaultLog -Pattern "Compiling with .+ \($expectedCompilerSource" `
        -What "Packaged c default compile"
    Assert-ProgramRuns -ExePath $defaultOutput -What "Packaged c default compile"

    $explicitOutput = Join-Path $ScratchDir "hello-explicit$exeSuffix"
    $explicitLog = Join-Path $ScratchDir "explicit.stderr.txt"
    $exitCode = Invoke-PackagedOscan `
        -Arguments (@("--backend", "c") + $hostCompilerArgs + @($SampleSource, "-o", $explicitOutput)) `
        -LogPath $explicitLog -BlockHostTools:$blockHostTools
    if ($exitCode -ne 0) {
        throw "Packaged '--backend c' compile failed:`n$(Get-LogText -LogPath $explicitLog)"
    }
    Assert-LogMatches -LogPath $explicitLog -Pattern "Compiling with .+ \($expectedCompilerSource" `
        -What "Packaged '--backend c' compile"
    Assert-ProgramRuns -ExePath $explicitOutput -What "Packaged '--backend c' compile"

    foreach ($missing in @("llvm", "cranelift")) {
        Assert-Refused -Arguments (@("--backend", $missing, $SampleSource, "-o", (Join-Path $ScratchDir "refused-$missing$exeSuffix"))) `
            -LogPath (Join-Path $ScratchDir "refused-$missing.stderr.txt") `
            -OutputPath (Join-Path $ScratchDir "refused-$missing$exeSuffix") `
            -ExpectedPatterns @(
                "the $missing backend is not included in this compiler build",
                "this build includes: c",
                "archive name ends in '-full' or '-$missing'"
            ) `
            -What "'--backend $missing' in the $Target/$Backend package"
    }
}

$smokedPackage = if ($ArchivePath) { $ArchivePath } else { $InstalledPackageDir }
Write-Host "Release smoke test passed for $Target/$Backend ($smokedPackage)"
$global:LASTEXITCODE = 0
