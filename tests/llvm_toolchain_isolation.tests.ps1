param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [Parameter(Mandatory = $true)]
    [string]$RuntimeArchiveDir,

    [Parameter(Mandatory = $true)]
    [string]$LlvmClang
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir
. (Join-Path $ScriptDir "backend_oracle.ps1")

function Assert-LlvmIsolation {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$runtimeArchives = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
$clang = (Resolve-Path -LiteralPath $LlvmClang).Path
$helloSource = (Resolve-Path -LiteralPath (Join-Path $RepoRoot "examples\hello.osc")).Path
$buildRoot = Join-Path $ScriptDir "build\llvm-toolchain-isolation"
$emptyPath = Join-Path $buildRoot "empty-path"
[void](New-Item -ItemType Directory -Path $emptyPath -Force)

$executable = Join-Path $buildRoot "hello$(Get-OracleExecutableSuffix)"
Remove-Item -LiteralPath $executable -Force -ErrorAction SilentlyContinue

$environmentNames = @(
    "PATH",
    "OSCAN_CC",
    "OSCAN_LLVM_CLANG",
    "OSCAN_LLVM_TOOLCHAIN_DIR",
    "OSCAN_TOOLCHAIN_DIR",
    "OSCAN_NATIVE_LINKER",
    "OSCAN_NATIVE_LINKER_FLAVOR",
    "OSCAN_RUNTIME_ARCHIVE_DIR"
)
$savedEnvironment = @{}
foreach ($name in $environmentNames) {
    $savedEnvironment[$name] = [System.Environment]::GetEnvironmentVariable(
        $name,
        [System.EnvironmentVariableTarget]::Process
    )
}

try {
    $env:PATH = $emptyPath
    $env:OSCAN_CC = Join-Path $buildRoot "missing-c-compiler"
    $env:OSCAN_LLVM_CLANG = $clang
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $runtimeArchives
    foreach ($name in @(
        "OSCAN_LLVM_TOOLCHAIN_DIR",
        "OSCAN_TOOLCHAIN_DIR",
        "OSCAN_NATIVE_LINKER",
        "OSCAN_NATIVE_LINKER_FLAVOR"
    )) {
        [System.Environment]::SetEnvironmentVariable(
            $name,
            $null,
            [System.EnvironmentVariableTarget]::Process
        )
    }

    foreach ($name in @("cc", "gcc", "clang", "cl", "ld", "ld.lld")) {
        $resolved = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue
        Assert-LlvmIsolation (-not $resolved) `
            "expected '$name' to be unreachable on the isolated PATH, but found '$($resolved.Source)'"
    }

    $compile = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @("--backend", "llvm", "--verbose", $helloSource, "-o", $executable) `
        -WorkingDirectory $buildRoot
    $compileLog = Normalize-OracleText "$($compile.Stdout)`n$($compile.Stderr)"

    Assert-LlvmIsolation ($compile.ExitCode -eq 0) `
        "isolated LLVM compile failed with exit $($compile.ExitCode): $compileLog"
    Assert-LlvmIsolation (Test-Path -LiteralPath $executable -PathType Leaf) `
        "isolated LLVM compile succeeded without producing '$executable'"
    Assert-LlvmIsolation ((Get-Item -LiteralPath $executable).Length -gt 0) `
        "isolated LLVM compile produced an empty executable"
    Assert-LlvmIsolation ($compileLog -match "(?m)^\[verbose\] llvm backend target:") `
        "isolated compile did not select the LLVM backend: $compileLog"
    Assert-LlvmIsolation ($compileLog -match "(?m)^\[verbose\] LLVM toolchain: .* \(override,") `
        "isolated compile did not use the explicitly supplied LLVM toolchain: $compileLog"
    Assert-LlvmIsolation ($compileLog -match "(?m)^Linking freestanding executable with .* \(embedded\)\.\.\.$") `
        "isolated LLVM compile did not use the embedded direct linker: $compileLog"
    Assert-LlvmIsolation ($compileLog -notmatch "(?m)^Compiling with ") `
        "isolated LLVM compile unexpectedly invoked the C backend: $compileLog"
    Assert-LlvmIsolation (
        $compileLog -notmatch "(?m)^Linking .* \((host|bundled|override)\)\.\.\.$"
    ) "isolated LLVM compile unexpectedly used a compiler-driver link: $compileLog"

    $run = Invoke-OracleProcess -FilePath $executable -WorkingDirectory $buildRoot
    Assert-LlvmIsolation ($run.ExitCode -eq 0) `
        "isolated LLVM executable exited with $($run.ExitCode): $($run.Stderr)"
    Assert-LlvmIsolation ($run.Stdout -eq "Hello, Oscan!") `
        "isolated LLVM executable stdout mismatch: got '$($run.Stdout)'"
    Assert-LlvmIsolation ($run.Stderr -eq "") `
        "isolated LLVM executable wrote unexpected stderr: $($run.Stderr)"

    $size = (Get-Item -LiteralPath $executable).Length
    Write-Host "LLVM toolchain isolation test passed (empty PATH; unusable OSCAN_CC; explicit LLVM tool; embedded direct linker; $size bytes)"
} finally {
    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable(
            $name,
            $savedEnvironment[$name],
            [System.EnvironmentVariableTarget]::Process
        )
    }
}
