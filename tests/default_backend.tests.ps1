param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [ValidateSet("llvm", "native")]
    [string]$ExpectedBackend = "llvm",

    [ValidateSet("embedded", "bundled", "host", "override", "any")]
    [string]$ExpectedLinkSource = "embedded"
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir
. (Join-Path $ScriptDir "backend_oracle.ps1")

function Assert-DefaultBackend {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$hostOs = if ($env:OS -eq "Windows_NT") {
    "windows"
} elseif ($IsLinux) {
    "linux"
} else {
    throw "implicit object-backend integration coverage only supports Windows and Linux hosts"
}
$hostArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
Assert-DefaultBackend ($hostArch -eq [System.Runtime.InteropServices.Architecture]::X64) `
    "implicit object-backend integration coverage requires an x86_64 host, got $hostArch"

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$helloSource = (Resolve-Path -LiteralPath (Join-Path $RepoRoot "examples\hello.osc")).Path
$buildRoot = Join-Path $ScriptDir "build\default-backend-$ExpectedBackend"
Remove-Item -LiteralPath $buildRoot -Recurse -Force -ErrorAction SilentlyContinue
[void](New-Item -ItemType Directory -Path $buildRoot -Force)

$suffix = if ($hostOs -eq "windows") { ".exe" } else { "" }
$executable = Join-Path $buildRoot "implicit_default_hello$suffix"
$compileArguments = @("--verbose", $helloSource, "-o", $executable)

# Keep this invocation an integration proof of the implicit policy: adding any
# backend or target selector would turn it into explicit-backend coverage.
foreach ($forbidden in @("--backend", "--native-target", "--target")) {
    Assert-DefaultBackend ($compileArguments -notcontains $forbidden) `
        "default-backend probe must not pass $forbidden"
}
Assert-DefaultBackend (-not $executable.EndsWith(".c", [System.StringComparison]::OrdinalIgnoreCase)) `
    "default-backend probe output must request an executable, not C source"

$compile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments $compileArguments `
    -WorkingDirectory $RepoRoot
$compileLog = Normalize-OracleText "$($compile.Stdout)`n$($compile.Stderr)"

Assert-DefaultBackend ($compile.ExitCode -eq 0) `
    "implicit default-backend compile failed with exit $($compile.ExitCode): $compileLog"
Assert-DefaultBackend (Test-Path -LiteralPath $executable -PathType Leaf) `
    "implicit default-backend compile succeeded without producing '$executable'"
Assert-DefaultBackend ((Get-Item -LiteralPath $executable).Length -gt 0) `
    "implicit default-backend compile produced an empty executable"
Assert-DefaultBackend ($compileLog -match "(?m)^\[verbose\] $ExpectedBackend backend target:") `
    "implicit compile selected a backend other than '$ExpectedBackend': $compileLog"
$linkPattern = if ($ExpectedLinkSource -eq "any") {
    '(?m)^Linking freestanding executable with .+ \([^)]+\)\.\.\.$'
} else {
    "(?m)^Linking freestanding executable with .+ \($ExpectedLinkSource\)\.\.\.$"
}
Assert-DefaultBackend ($compileLog -match $linkPattern) `
    "implicit compile did not report the expected '$ExpectedLinkSource' link source: $compileLog"
Assert-DefaultBackend ($compileLog -notmatch '(?m)^Compiling with ') `
    "implicit compile unexpectedly reported the C backend compiler path: $compileLog"
Assert-DefaultBackend (@(Get-ChildItem -LiteralPath $buildRoot -Filter "*.c" -File).Count -eq 0) `
    "implicit compile unexpectedly left C source in '$buildRoot'"

$header = [System.IO.File]::ReadAllBytes($executable)
if ($hostOs -eq "windows") {
    Assert-DefaultBackend ($header.Length -ge 2 -and $header[0] -eq 0x4d -and $header[1] -eq 0x5a) `
        "implicit default backend did not produce a PE executable"
} else {
    Assert-DefaultBackend (
        $header.Length -ge 20 -and
        $header[0] -eq 0x7f -and $header[1] -eq 0x45 -and
        $header[2] -eq 0x4c -and $header[3] -eq 0x46 -and
        $header[4] -eq 2 -and $header[18] -eq 0x3e -and $header[19] -eq 0
    ) "implicit default backend did not produce an ELF64 x86_64 executable"
}

$run = Invoke-OracleProcess -FilePath $executable -WorkingDirectory $buildRoot
Assert-DefaultBackend ($run.ExitCode -eq 0) `
    "implicit default-backend executable exited with $($run.ExitCode): $($run.Stderr)"
Assert-DefaultBackend ($run.Stdout -eq "Hello, Oscan!") `
    "implicit default-backend executable stdout mismatch: got '$($run.Stdout)'"
Assert-DefaultBackend ($run.Stderr -eq "") `
    "implicit default-backend executable wrote unexpected stderr: $($run.Stderr)"

Write-Host "implicit $ExpectedBackend default test passed ($hostOs-x86_64; executable ran; $ExpectedLinkSource link source reported)"
