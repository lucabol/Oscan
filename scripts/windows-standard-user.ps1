$ErrorActionPreference = "Stop"

function Invoke-WindowsStandardUserPowerShell {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$ScriptPath,

        [string[]]$ArgumentList = @(),

        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory,

        [Parameter(Mandatory = $true)]
        [string]$StateBaseDir,

        [string[]]$ReadOnlyPaths = @(),

        [string[]]$WritablePaths = @(),

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory,

        [string]$LogPrefix = "standard-user",

        [ValidateRange(1, 86400)]
        [int]$TimeoutSeconds = 900
    )

    if ($env:OS -ne "Windows_NT") {
        throw "Invoke-WindowsStandardUserPowerShell is only supported on Windows."
    }

    $script = (Resolve-Path -LiteralPath $ScriptPath).Path
    $working = (Resolve-Path -LiteralPath $WorkingDirectory).Path
    $stateBase = (Resolve-Path -LiteralPath $StateBaseDir).Path
    New-Item -ItemType Directory -Path $LogDirectory -Force | Out-Null
    $logs = (Resolve-Path -LiteralPath $LogDirectory).Path

    $suffix = [guid]::NewGuid().ToString("N").Substring(0, 8)
    $userName = "oscan-ci-$suffix"
    $account = "$env:COMPUTERNAME\$userName"
    $passwordText = "Oscan!Ci-$([guid]::NewGuid().ToString('N'))-aA1!"
    $password = ConvertTo-SecureString $passwordText -AsPlainText -Force
    $credential = [pscredential]::new($account, $password)
    $stateDir = Join-Path $stateBase "standard-user-$suffix"
    $stdoutPath = Join-Path $logs "$LogPrefix.stdout.log"
    $stderrPath = Join-Path $logs "$LogPrefix.stderr.log"
    $grantedPaths = [Collections.Generic.List[string]]::new()
    $createdUser = $false
    $process = $null

    function Grant-AccountAccess {
        param(
            [Parameter(Mandatory = $true)][string]$Path,
            [Parameter(Mandatory = $true)][ValidateSet("RX", "M")][string]$Access
        )

        $resolved = (Resolve-Path -LiteralPath $Path).Path
        # Track before icacls runs so a partial recursive grant is still
        # removed if icacls reports a failure partway through the tree.
        $grantedPaths.Add($resolved)
        & icacls.exe $resolved /grant "${account}:(OI)(CI)$Access" /T /Q | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "failed to grant $Access access to '$resolved' for $account"
        }
    }

    function Stop-ProcessTree {
        param([Parameter(Mandatory = $true)][int]$RootProcessId)

        $allProcesses = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
        $pending = [Collections.Generic.Queue[int]]::new()
        $processIds = [Collections.Generic.List[int]]::new()
        $pending.Enqueue($RootProcessId)
        while ($pending.Count -gt 0) {
            $parentId = $pending.Dequeue()
            $processIds.Add($parentId)
            foreach ($child in $allProcesses | Where-Object { $_.ParentProcessId -eq $parentId }) {
                $pending.Enqueue([int]$child.ProcessId)
            }
        }
        for ($index = $processIds.Count - 1; $index -ge 0; $index--) {
            Stop-Process -Id $processIds[$index] -Force -ErrorAction SilentlyContinue
        }
    }

    try {
        if (Get-LocalUser -Name $userName -ErrorAction SilentlyContinue) {
            throw "refusing to replace unexpectedly existing local user '$userName'"
        }
        New-LocalUser -Name $userName -Password $password `
            -PasswordNeverExpires -UserMayNotChangePassword | Out-Null
        $createdUser = $true

        New-Item -ItemType Directory -Path $stateDir -Force | Out-Null
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
        foreach ($sid in @($currentSid, $systemSid)) {
            $stateAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
                $sid, [Security.AccessControl.FileSystemRights]::FullControl,
                $inheritance, $propagation, $allow
            ))
        }
        $stateAcl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            $standardUserSid, [Security.AccessControl.FileSystemRights]::Modify,
            $inheritance, $propagation, $allow
        ))
        Set-Acl -LiteralPath $stateDir -AclObject $stateAcl

        $tempDir = Join-Path $stateDir "temp"
        $cacheDir = Join-Path $stateDir "cache"
        New-Item -ItemType Directory -Path $tempDir, $cacheDir | Out-Null

        foreach ($path in $ReadOnlyPaths) {
            Grant-AccountAccess -Path $path -Access RX
        }
        foreach ($path in $WritablePaths) {
            Grant-AccountAccess -Path $path -Access M
        }

        # Keep argv in JSON so spaces and quotes survive exactly, while the
        # CreateProcessWithLogonW command line stays below its 1,024-char cap.
        $payloadPath = Join-Path $stateDir "invocation.json"
        @{
            script_path = $script
            arguments = @($ArgumentList)
            expected_sid = $standardUserSid.Value
        } | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $payloadPath -Encoding UTF8

        $bootstrapPath = Join-Path $stateDir "invoke.ps1"
        @'
param([Parameter(Mandatory = $true)][string]$StateDir)
$ErrorActionPreference = "Stop"
try {
    $state = (Resolve-Path -LiteralPath $StateDir).Path
    $payload = Get-Content -LiteralPath (Join-Path $state "invocation.json") -Raw |
        ConvertFrom-Json
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    if ($identity.User.Value -ne [string]$payload.expected_sid) {
        throw "credentialed process SID did not match the disposable standard user"
    }
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw "credentialed process unexpectedly has an Administrator token"
    }
    $env:TEMP = Join-Path $state "temp"
    $env:TMP = $env:TEMP
    $env:OSCAN_NATIVE_ASSET_CACHE_DIR = Join-Path $state "cache"
    foreach ($directory in @($env:TEMP, $env:OSCAN_NATIVE_ASSET_CACHE_DIR)) {
        $probe = Join-Path $directory ".write-probe-$PID-$([guid]::NewGuid().ToString('N'))"
        try {
            [IO.File]::WriteAllText($probe, "ok")
        } finally {
            Remove-Item -LiteralPath $probe -Force -ErrorAction SilentlyContinue
        }
    }
    $effectiveTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    $expectedTemp = [IO.Path]::GetFullPath($env:TEMP + [IO.Path]::DirectorySeparatorChar)
    if (-not $effectiveTemp.Equals($expectedTemp, [StringComparison]::OrdinalIgnoreCase)) {
        throw "standard-user TEMP override was not applied (expected '$expectedTemp', got '$effectiveTemp')"
    }
    $scriptArguments = @($payload.arguments)
    & ([string]$payload.script_path) @scriptArguments
    if (-not $?) {
        throw "standard-user PowerShell script reported failure"
    }
    exit 0
} catch {
    Write-Error ($_ | Out-String)
    exit 1
}
'@ | Set-Content -LiteralPath $bootstrapPath -Encoding UTF8

        Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
        $arguments = @(
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy", "Bypass",
            "-File", "`"$bootstrapPath`"",
            "-StateDir", "`"$stateDir`""
        )
        $process = Start-Process `
            -FilePath (Get-Command pwsh).Source `
            -ArgumentList $arguments `
            -Credential $credential `
            -LoadUserProfile `
            -WorkingDirectory $working `
            -RedirectStandardOutput $stdoutPath `
            -RedirectStandardError $stderrPath `
            -PassThru

        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            Stop-ProcessTree -RootProcessId $process.Id
            $process.WaitForExit()
            throw "standard-user process timed out after $TimeoutSeconds seconds; logs: '$stdoutPath', '$stderrPath'"
        }
        $process.WaitForExit()

        if ($process.ExitCode -ne 0) {
            throw "standard-user process failed with exit code $($process.ExitCode); logs: '$stdoutPath', '$stderrPath'"
        }
    } finally {
        if ($process -and -not $process.HasExited) {
            Stop-ProcessTree -RootProcessId $process.Id
            $process.WaitForExit()
        }
        if (Test-Path -LiteralPath $stdoutPath) {
            Get-Content -LiteralPath $stdoutPath | Write-Host
        }
        if (Test-Path -LiteralPath $stderrPath) {
            Get-Content -LiteralPath $stderrPath | Write-Host
        }
        foreach ($path in ($grantedPaths | Select-Object -Unique)) {
            & icacls.exe $path /remove:g $account /T /Q 2>$null | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Warning "failed to remove ACL entries for $account from '$path'"
            }
        }
        Remove-Item -LiteralPath $stateDir -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $stateDir) {
            Write-Warning "failed to remove standard-user state directory '$stateDir'"
        }
        if ($createdUser) {
            Remove-LocalUser -Name $userName -ErrorAction SilentlyContinue
            if (Get-LocalUser -Name $userName -ErrorAction SilentlyContinue) {
                Write-Warning "failed to remove disposable local user '$userName'"
            }
        }
        $credential = $null
        $password = $null
        $passwordText = $null
    }
}
