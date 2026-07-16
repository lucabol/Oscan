param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [string]$RuntimeArchiveDir = "build\runtime-archives\windows-x86_64",

    [string]$ToolchainDir = "build\toolchain-windows-x86_64",

    [switch]$StandardUserChild
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir

function Invoke-NativeValidation {
    $compiler = (Resolve-Path -LiteralPath $Oscan).Path
    $runtimeArchives = (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
    $env:OSCAN_RUNTIME_ARCHIVE_DIR = $runtimeArchives
    $env:OSCAN_TOOLCHAIN_DIR = (Resolve-Path -LiteralPath $ToolchainDir).Path

    & (Join-Path $ScriptDir "native_link_isolation.tests.ps1") `
        -Oscan $compiler `
        -ToolchainDir $ToolchainDir
    if ($LASTEXITCODE -ne 0) {
        throw "native-link isolation suite failed with exit code $LASTEXITCODE"
    }

    & (Join-Path $ScriptDir "run_tests.ps1") -Oscan $compiler -Backend native
    if ($LASTEXITCODE -ne 0) {
        throw "C-vs-native differential suite failed with exit code $LASTEXITCODE"
    }
}

Push-Location $RepoRoot
try {
    if ($StandardUserChild) {
        Invoke-NativeValidation
        exit 0
    }

    $compiler = (Resolve-Path -LiteralPath $Oscan).Path
    $probeOutput = & $compiler examples\hello.osc --backend native `
        -o tests\build\native_ci_elevation_probe.exe 2>&1 | Out-String
    $probeExit = $LASTEXITCODE

    if ($probeExit -eq 0) {
        Invoke-NativeValidation
        exit 0
    }
    if ($probeOutput -notmatch "running elevated \(Administrator\)") {
        throw "native-link preflight failed for a reason other than elevation: $probeOutput"
    }

    # GitHub-hosted Windows runners execute the runner service with an
    # elevated token. Product code must continue to reject that token, so run
    # the real final-link tests under a disposable standard-user token rather
    # than adding a CI-only bypass to Oscan.
    $userName = "oscan-ci-native"
    $account = "$env:COMPUTERNAME\$userName"
    $passwordText = "Oscan!Ci-$([guid]::NewGuid().ToString('N'))-aA1!"
    $password = ConvertTo-SecureString $passwordText -AsPlainText -Force
    $credential = [pscredential]::new($account, $password)

    if (Get-LocalUser -Name $userName -ErrorAction SilentlyContinue) {
        Remove-LocalUser -Name $userName
    }
    New-LocalUser -Name $userName -Password $password `
        -PasswordNeverExpires -UserMayNotChangePassword | Out-Null

    $logDir = Join-Path $ScriptDir "build\windows-native-standard-user"
    [void](New-Item -ItemType Directory -Path $logDir -Force)
    $stdoutPath = Join-Path $logDir "stdout.log"
    $stderrPath = Join-Path $logDir "stderr.log"

    try {
        & icacls.exe $RepoRoot /grant "${account}:(OI)(CI)RX" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant repository read access to $account" }
        & icacls.exe (Join-Path $RepoRoot "build") /grant "${account}:(OI)(CI)M" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant build-directory access to $account" }
        & icacls.exe (Join-Path $ScriptDir "build") /grant "${account}:(OI)(CI)M" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant test-build access to $account" }

        $arguments = @(
            "-NoProfile",
            "-ExecutionPolicy", "Bypass",
            "-File", $PSCommandPath,
            "-Oscan", $compiler,
            "-RuntimeArchiveDir", (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path,
            "-ToolchainDir", $ToolchainDir,
            "-StandardUserChild"
        )
        $process = Start-Process `
            -FilePath (Get-Command pwsh).Source `
            -ArgumentList $arguments `
            -Credential $credential `
            -LoadUserProfile `
            -WorkingDirectory $RepoRoot `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath `
            -Wait `
            -PassThru

        if (Test-Path -LiteralPath $stdoutPath) {
            Get-Content -LiteralPath $stdoutPath | Write-Host
        }
        if (Test-Path -LiteralPath $stderrPath) {
            Get-Content -LiteralPath $stderrPath | Write-Host
        }
        if ($process.ExitCode -ne 0) {
            throw "standard-user native validation failed with exit code $($process.ExitCode)"
        }
    } finally {
        Remove-LocalUser -Name $userName -ErrorAction SilentlyContinue
    }
} finally {
    Pop-Location
}
