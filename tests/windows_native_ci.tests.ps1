param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [string]$RuntimeArchiveDir = "build\runtime-archives\windows-x86_64",

    [string]$ToolchainDir = "build\toolchain-windows-x86_64",

    [string]$StandardUserStateDir,

    [switch]$StandardUserChild
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir

function Initialize-StandardUserEnvironment {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StateDir
    )

    $state = (Resolve-Path -LiteralPath $StateDir).Path
    $tempDir = Join-Path $state "temp"
    $cacheDir = Join-Path $state "cache"

    foreach ($directory in @($tempDir, $cacheDir)) {
        if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
            throw "standard-user state directory does not exist: $directory"
        }
    }

    # Start-Process -Credential inherits the elevated runner's environment,
    # even when its profile is loaded. Set these in the credentialed process
    # before anything can consult runneradmin's inaccessible TEMP or cache.
    $env:TEMP = $tempDir
    $env:TMP = $tempDir
    $env:OSCAN_NATIVE_ASSET_CACHE_DIR = $cacheDir

    foreach ($directory in @($tempDir, $cacheDir)) {
        $probe = Join-Path $directory ".oscan-write-probe-$PID-$([guid]::NewGuid().ToString('N'))"
        try {
            [IO.File]::WriteAllText($probe, "ok")
        } finally {
            Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
        }
    }

    $effectiveTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $expectedTemp = [IO.Path]::GetFullPath($tempDir + [IO.Path]::DirectorySeparatorChar)
    if (-not $effectiveTemp.Equals($expectedTemp, [StringComparison]::OrdinalIgnoreCase)) {
        throw "standard-user TEMP override was not applied (expected '$expectedTemp', got '$effectiveTemp')"
    }
}

function ConvertTo-PowerShellSingleQuotedLiteral {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value
    )

    return "'" + $Value.Replace("'", "''") + "'"
}

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
        if ([string]::IsNullOrWhiteSpace($StandardUserStateDir)) {
            throw "-StandardUserStateDir is required with -StandardUserChild"
        }
        Initialize-StandardUserEnvironment -StateDir $StandardUserStateDir
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
    $stateDir = Join-Path $logDir "state-$([guid]::NewGuid().ToString('N'))"

    try {
        & icacls.exe $RepoRoot /grant "${account}:(OI)(CI)RX" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant repository read access to $account" }
        & icacls.exe (Join-Path $RepoRoot "build") /grant "${account}:(OI)(CI)M" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant build-directory access to $account" }
        & icacls.exe (Join-Path $ScriptDir "build") /grant "${account}:(OI)(CI)M" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "failed to grant test-build access to $account" }

        [void](New-Item -ItemType Directory -Path $stateDir -Force)

        $currentSid = [Security.Principal.WindowsIdentity]::GetCurrent().User
        $standardUserSid = ([Security.Principal.NTAccount]::new($account)).Translate(
            [Security.Principal.SecurityIdentifier]
        )
        $systemSid = [Security.Principal.SecurityIdentifier]::new("S-1-5-18")
        $stateAcl = [Security.AccessControl.DirectorySecurity]::new()
        $stateAcl.SetOwner($currentSid)
        $stateAcl.SetAccessRuleProtection($true, $false)
        $inheritance = [Security.AccessControl.InheritanceFlags]"ContainerInherit, ObjectInherit"
        $propagation = [Security.AccessControl.PropagationFlags]::None
        $allow = [Security.AccessControl.AccessControlType]::Allow
        $stateAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $currentSid, [Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance, $propagation, $allow
        ))
        $stateAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $systemSid, [Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance, $propagation, $allow
        ))
        $stateAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $standardUserSid, [Security.AccessControl.FileSystemRights]::Modify,
            $inheritance, $propagation, $allow
        ))
        Set-Acl -LiteralPath $stateDir -AclObject $stateAcl
        [void](New-Item -ItemType Directory -Path (Join-Path $stateDir "temp"))
        [void](New-Item -ItemType Directory -Path (Join-Path $stateDir "cache"))

        $scriptLiteral = ConvertTo-PowerShellSingleQuotedLiteral $PSCommandPath
        $compilerLiteral = ConvertTo-PowerShellSingleQuotedLiteral $compiler
        $archiveLiteral = ConvertTo-PowerShellSingleQuotedLiteral (
            (Resolve-Path -LiteralPath $RuntimeArchiveDir).Path
        )
        $toolchainLiteral = ConvertTo-PowerShellSingleQuotedLiteral (
            (Resolve-Path -LiteralPath $ToolchainDir).Path
        )
        $stateLiteral = ConvertTo-PowerShellSingleQuotedLiteral $stateDir
        $childCommand = (
            "& $scriptLiteral -Oscan $compilerLiteral " +
            "-RuntimeArchiveDir $archiveLiteral -ToolchainDir $toolchainLiteral " +
            "-StandardUserStateDir $stateLiteral -StandardUserChild"
        )
        $encodedChildCommand = [Convert]::ToBase64String(
            [Text.Encoding]::Unicode.GetBytes($childCommand)
        )
        $arguments = @(
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy", "Bypass",
            "-EncodedCommand", $encodedChildCommand
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
        Remove-Item -LiteralPath $stateDir -Recurse -Force -ErrorAction SilentlyContinue
        & icacls.exe $RepoRoot /remove:g $account /T /Q 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "failed to remove repository ACL entries for $account"
        }
        Remove-LocalUser -Name $userName -ErrorAction SilentlyContinue
    }
} finally {
    Pop-Location
}
