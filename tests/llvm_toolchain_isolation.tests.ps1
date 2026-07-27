param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [Parameter(Mandatory = $true)]
    [string]$RuntimeArchiveDir,

    [Parameter(Mandatory = $true)]
    [string]$LlvmLibrary
)

# Proves the *direct* LLVM backend needs no C toolchain at all: with an
# empty PATH, an unusable OSCAN_CC, and no discoverable clang/gcc/cl/ld,
# `--backend llvm` must still compile and link a working executable using
# only Oscan's packaged LLVM code generator (loaded in-process) and the
# embedded direct linker. It must also leave no C/intermediate artifact
# behind, and it must survive the strict no-toolchain profile.

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
$llvmLib = (Resolve-Path -LiteralPath $LlvmLibrary).Path
$helloSource = (Resolve-Path -LiteralPath (Join-Path $RepoRoot "examples\hello.osc")).Path
$buildRoot = Join-Path $ScriptDir "build\llvm-toolchain-isolation"
Remove-Item -LiteralPath $buildRoot -Recurse -Force -ErrorAction SilentlyContinue
[void](New-Item -ItemType Directory -Path $buildRoot -Force)
$emptyPath = Join-Path $buildRoot "empty-path"
[void](New-Item -ItemType Directory -Path $emptyPath -Force)

$executable = Join-Path $buildRoot "hello$(Get-OracleExecutableSuffix)"
Remove-Item -LiteralPath $executable -Force -ErrorAction SilentlyContinue

$environmentNames = @(
    "PATH",
    "OSCAN_CC",
    "OSCAN_LLVM_LIB",
    "OSCAN_LLVM_DIR",
    "OSCAN_TOOLCHAIN_DIR",
    "OSCAN_NATIVE_LINKER",
    "OSCAN_NATIVE_LINKER_FLAVOR",
    "OSCAN_RUNTIME_ARCHIVE_DIR",
    "OSCAN_NO_TOOLCHAIN"
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
    $env:OSCAN_LLVM_LIB = $llvmLib
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $runtimeArchives
    # The strict no-toolchain profile turns every remaining route to a
    # host C compiler into a hard error, so a pass here is proof rather
    # than a coincidence of PATH scrubbing.
    $env:OSCAN_NO_TOOLCHAIN = "1"
    foreach ($name in @(
        "OSCAN_LLVM_DIR",
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

    foreach ($name in @("cc", "gcc", "clang", "cl", "ld", "ld.lld", "llvm-as", "opt", "llc")) {
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
    Assert-LlvmIsolation ($compileLog -match "(?m)^\[verbose\] LLVM code generator: .*\(LLVM \d+\.\d+\.\d+, targets: ") `
        "isolated compile did not load the packaged LLVM code generator: $compileLog"
    Assert-LlvmIsolation ($compileLog -match "(?m)^Linking freestanding executable with .* \(embedded\)\.\.\.$") `
        "isolated LLVM compile did not use the embedded direct linker: $compileLog"
    Assert-LlvmIsolation ($compileLog -notmatch "(?m)^Compiling with ") `
        "isolated LLVM compile unexpectedly invoked the C backend: $compileLog"
    Assert-LlvmIsolation (
        $compileLog -notmatch "(?m)^Linking .* \((host|bundled|override)\)\.\.\.$"
    ) "isolated LLVM compile unexpectedly used a compiler-driver link: $compileLog"

    # No C source, header, textual IR, bitcode, or assembly may survive an
    # end-to-end LLVM build: the whole pipeline is in-process.
    $leftovers = Get-ChildItem -LiteralPath $buildRoot -File -Recurse |
        Where-Object { $_.Extension -in @(".c", ".h", ".i", ".ll", ".bc", ".s", ".obj", ".o") }
    Assert-LlvmIsolation ($null -eq $leftovers -or $leftovers.Count -eq 0) `
        "isolated LLVM compile left C/intermediate artifacts behind: $($leftovers.FullName -join ', ')"

    $run = Invoke-OracleProcess -FilePath $executable -WorkingDirectory $buildRoot
    Assert-LlvmIsolation ($run.ExitCode -eq 0) `
        "isolated LLVM executable exited with $($run.ExitCode): $($run.Stderr)"
    Assert-LlvmIsolation ($run.Stdout -eq "Hello, Oscan!") `
        "isolated LLVM executable stdout mismatch: got '$($run.Stdout)'"
    Assert-LlvmIsolation ($run.Stderr -eq "") `
        "isolated LLVM executable wrote unexpected stderr: $($run.Stderr)"

    # Textual IR emission must work in the same isolation, and must carry
    # no poison-generating flags and no memcpy intrinsic.
    $ir = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @("--backend", "llvm", "--emit-llvm-ir", $helloSource) `
        -WorkingDirectory $buildRoot
    Assert-LlvmIsolation ($ir.ExitCode -eq 0) `
        "isolated LLVM IR emission failed with exit $($ir.ExitCode): $($ir.Stderr)"
    Assert-LlvmIsolation ($ir.Stdout -match "define i32 @main\(i32 %arg0, ptr %arg1\)") `
        "isolated LLVM IR did not contain Oscan's own entry wrapper"
    Assert-LlvmIsolation ($ir.Stdout -notmatch "program\.c") `
        "isolated LLVM IR referenced a generated C translation unit"
    foreach ($banned in @(" nsw ", " nuw ", "inbounds", "llvm\.memcpy", "llvm\.memmove")) {
        Assert-LlvmIsolation ($ir.Stdout -notmatch $banned) `
            "isolated LLVM IR contained '$banned', which the conservative poison/freestanding policy forbids"
    }

    # And the C backend really is refused under the strict profile rather
    # than quietly used as a fallback.
    $refused = Invoke-OracleProcess `
        -FilePath $compiler `
        -Arguments @("--backend", "c", "--emit-c", $helloSource) `
        -WorkingDirectory $buildRoot
    Assert-LlvmIsolation ($refused.ExitCode -ne 0) `
        "strict no-toolchain profile unexpectedly allowed the C backend"
    Assert-LlvmIsolation ($refused.Stderr -match "OSCAN_NO_TOOLCHAIN=1") `
        "strict no-toolchain refusal did not name the profile: $($refused.Stderr)"

    $size = (Get-Item -LiteralPath $executable).Length
    Write-Host "LLVM toolchain isolation test passed (empty PATH; unusable OSCAN_CC; in-process packaged code generator; embedded direct linker; strict no-toolchain profile; $size bytes)"
} finally {
    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable(
            $name,
            $savedEnvironment[$name],
            [System.EnvironmentVariableTarget]::Process
        )
    }
}
