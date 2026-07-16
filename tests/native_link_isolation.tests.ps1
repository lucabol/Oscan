param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    # Relative to the repo root; the pinned toolchain directory that
    # OSCAN_EMBED_ASSETS_DIR was staged from. Renamed away (not just PATH
    # -stubbed) for the duration of this test.
    [string]$ToolchainDir = "build\toolchain-windows-x86_64"
)

# Formalizes the true-isolation proof from
# docs/design/native-link-embedding.md §9 item 1a: file-hash equality of the
# extracted linker is necessary but NOT sufficient, because ld.lld.exe is
# dynamically linked against 5 sibling DLLs. A stray toolchain bin/ directory
# left on PATH can satisfy the Windows loader's DLL search from the wrong
# place and mask a missing-sibling-DLL bug in the embedded asset set. Blocking
# cc/gcc/clang/cl by name (as scripts/smoke-release.ps1's
# Invoke-NoHostCompilerCommand does) is necessary but not this specific proof
# on its own -- this test additionally renames the entire pinned toolchain
# directory away so no toolchain bin/ can be reachable by any means (PATH or
# otherwise), then confirms a real native compile+link+run of examples/hello.osc
# still succeeds using this oscan.exe's embedded linker.

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir

function Assert-NativeLinkIsolation {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$toolchainPath = Join-Path $RepoRoot $ToolchainDir
$renamedPath = "$toolchainPath.isolation-test-renamed"

if (Test-Path -LiteralPath $renamedPath) {
    throw ("leftover renamed toolchain dir from a previous failed isolation-test run: " +
        "'$renamedPath' -- rename it back to '$toolchainPath' manually before re-running this test")
}

$buildRoot = Join-Path $ScriptDir "build\native-link-isolation"
[void](New-Item -ItemType Directory -Path $buildRoot -Force)
$helloSource = Join-Path $ScriptDir "..\examples\hello.osc"
$exePath = Join-Path $buildRoot "isolation_hello.exe"
Remove-Item -LiteralPath $exePath -Force -ErrorAction SilentlyContinue

# Pre-flight (no isolation yet): confirm this binary actually embeds native-link
# assets before we pay the cost of renaming the toolchain dir. A dev build
# (EMBEDDED_ASSETS_PRESENT == false) cannot pass this test by construction --
# fail with a clear, specific message rather than a confusing isolated-compile
# failure below.
$preflight = & $compiler --backend native -o (Join-Path $buildRoot "preflight_hello.exe") $helloSource 2>&1 | Out-String
Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) "preflight native compile of examples/hello.osc failed: $preflight"
Assert-NativeLinkIsolation ($preflight -match '\(embedded\)') `
    ("this oscan.exe was not built with embedded native-link assets (no '(embedded)' link " +
     "source label seen); rebuild with OSCAN_EMBED_ASSETS_DIR/OSCAN_REQUIRE_EMBEDDED_ASSETS=1 " +
     "before running the true-isolation test. Link log: $preflight")

$savedPath = $env:PATH
$toolchainRenamed = $false
$blockedNames = @('cc.exe', 'gcc.exe', 'clang.exe', 'cl.exe')

try {
    if (Test-Path -LiteralPath $toolchainPath) {
        Rename-Item -LiteralPath $toolchainPath -NewName (Split-Path $renamedPath -Leaf)
        $toolchainRenamed = $true
    }

    # Scrub PATH of every directory that would resolve cc/gcc/clang/cl -- do not
    # assume the ambient PATH is already clean (a real gcc.exe reachable via a
    # package-manager install is a realistic false-negative source here).
    $entries = $savedPath -split [System.IO.Path]::PathSeparator | Where-Object { $_ -ne '' }
    $clean = @()
    foreach ($entry in $entries) {
        $blocked = $false
        foreach ($name in $blockedNames) {
            if (Test-Path -LiteralPath (Join-Path $entry $name) -ErrorAction SilentlyContinue) {
                $blocked = $true
                break
            }
        }
        if (-not $blocked) { $clean += $entry }
    }
    $env:PATH = ($clean -join [System.IO.Path]::PathSeparator)

    foreach ($name in @('cc', 'gcc', 'clang', 'cl')) {
        $found = Get-Command $name -ErrorAction SilentlyContinue
        Assert-NativeLinkIsolation (-not $found) `
            "expected '$name' to be unreachable after scrubbing PATH and renaming the toolchain dir, but found $($found.Source)"
    }
    Assert-NativeLinkIsolation (-not (Test-Path -LiteralPath $toolchainPath)) `
        "toolchain dir '$toolchainPath' is still present -- isolation setup did not take effect"

    $compile = & $compiler --backend native -o $exePath $helloSource 2>&1 | Out-String
    Assert-NativeLinkIsolation ($LASTEXITCODE -eq 0) `
        "native compile of examples/hello.osc failed with no C compiler/linker reachable and the toolchain dir renamed away: $compile"
    Assert-NativeLinkIsolation ($compile -match '\(embedded\)') `
        "native link under isolation did not report using the embedded linker (got: $compile)"
    Assert-NativeLinkIsolation (Test-Path -LiteralPath $exePath -PathType Leaf) "no executable was produced"

    $size = (Get-Item -LiteralPath $exePath).Length
    Assert-NativeLinkIsolation ($size -eq 6656) "expected exactly 6656 bytes for hello.osc via MingwDirect, got $size"

    $run = & $exePath
    $runExit = $LASTEXITCODE
    Assert-NativeLinkIsolation ($runExit -eq 0) "isolated hello.exe exited with $runExit"
    $actual = ($run | Out-String).TrimEnd("`r", "`n")
    Assert-NativeLinkIsolation ($actual -eq "Hello, Oscan!") "isolated hello.exe stdout mismatch: got '$actual'"

    Write-Host "native link true-isolation test passed (toolchain dir renamed away + PATH scrubbed of cc/gcc/clang/cl; 6656 B; embedded ld.lld.exe used)"
} finally {
    $env:PATH = $savedPath
    if ($toolchainRenamed -and (Test-Path -LiteralPath $renamedPath)) {
        Rename-Item -LiteralPath $renamedPath -NewName (Split-Path $toolchainPath -Leaf)
    }
}
