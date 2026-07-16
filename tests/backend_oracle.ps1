# Shared helpers for comparing a selectable backend with the C backend.

# Windows can transiently report ERROR_PATH_NOT_FOUND/ERROR_ACCESS_DENIED for a
# split second after a directory is deleted and immediately recreated (real-time
# antivirus scanning is a common cause), and that filesystem churn can spill over
# into unrelated concurrent file reads (e.g. a just-spawned compiler process
# resolving its own source-file argument). This differential harness recreates a
# per-test run directory for *every* test, so retry the recreate step a few times
# before giving up rather than letting a purely transient hiccup fail a test.
function Invoke-WithRetry {
    param(
        [Parameter(Mandatory = $true)][scriptblock]$Action,
        [int]$MaxAttempts = 5,
        [int]$DelayMilliseconds = 100
    )
    $attempt = 0
    while ($true) {
        $attempt++
        try {
            & $Action
            return
        } catch {
            if ($attempt -ge $MaxAttempts) { throw }
            Start-Sleep -Milliseconds ($DelayMilliseconds * $attempt)
        }
    }
}

function Normalize-OracleText {
    param([AllowNull()][string]$Text)

    if ($null -eq $Text) { return "" }
    return $Text.Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n")
}

# Compares normalized actual stdout against a primary expected file, falling
# back to an optional secondary ("libc"-style, reduced-capability) expected
# file when the primary doesn't match. A handful of builtins are legitimately
# unavailable or behave differently outside the primary Windows freestanding
# environment: canvas/clipboard windowing defaults differ between Windows and
# POSIX's backend-selection logic (see runtime/osc_runtime.c's OSC_HAS_GFX
# path), img/svg/tt asset decoding is only wired up for x86_64/Windows targets
# (see src/codegen.rs's emit_includes), and tls_fetch needs real outbound
# network access that a sandboxed/offline runner may not provide. Every one of
# those constrained contexts deterministically reproduces the exact text
# already established for hosted/libc mode (tests/expected_libc/<name>.expected
# — e.g. "canvas_alive() -> false" or "not supported in this build"), so
# accepting either exact match still rejects any *other* divergence (a crash,
# a wrong error, partial output, ...) rather than loosening the comparison.
# This mirrors the existing convention used by the Windows libc test phase and
# by .github/workflows/ci.yml's Linux job (which prefers expected_libc for
# tls_fetch specifically because that CI runner has no outbound network) —
# except here both files are tried, since a WSL/ARM environment that *does*
# have working network/graphics should still be held to the primary
# (Windows-derived) expected output.
function Test-ExpectedOutputMatch {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$ActualRaw,
        [Parameter(Mandatory = $true)][string]$PrimaryExpectedFile,
        [string]$FallbackExpectedFile
    )

    $actual = Normalize-OracleText $ActualRaw
    if (Test-Path -LiteralPath $PrimaryExpectedFile -PathType Leaf) {
        $primary = Normalize-OracleText (Get-Content -LiteralPath $PrimaryExpectedFile -Raw)
        if ($actual -eq $primary) { return $true }
    }
    if ($FallbackExpectedFile -and (Test-Path -LiteralPath $FallbackExpectedFile -PathType Leaf)) {
        $fallback = Normalize-OracleText (Get-Content -LiteralPath $FallbackExpectedFile -Raw)
        if ($actual -eq $fallback) { return $true }
    }
    return $false
}

function Invoke-OracleProcess {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$WorkingDirectory
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add([string]$argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "could not start '$FilePath'"
        }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        return [PSCustomObject]@{
            ExitCode = $process.ExitCode
            Stdout = Normalize-OracleText $stdoutTask.GetAwaiter().GetResult()
            Stderr = Normalize-OracleText $stderrTask.GetAwaiter().GetResult()
        }
    } finally {
        $process.Dispose()
    }
}

function Get-OracleFixtureSnapshot {
    param([Parameter(Mandatory = $true)][string]$Directory)

    $snapshot = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Directory -PathType Container)) {
        return $snapshot
    }

    $root = [System.IO.Path]::GetFullPath($Directory)
    foreach ($file in Get-ChildItem -LiteralPath $root -File -Recurse -Force | Sort-Object FullName) {
        $relativePath = [System.IO.Path]::GetRelativePath($root, $file.FullName).Replace('\', '/')
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $snapshot[$relativePath] = "$($file.Length):$hash"
    }
    return $snapshot
}

function Compare-OracleFixtureSnapshots {
    param(
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Actual
    )

    $differences = [System.Collections.Generic.List[string]]::new()
    $paths = @($Expected.Keys) + @($Actual.Keys) | Sort-Object -Unique
    foreach ($path in $paths) {
        if (-not $Expected.Contains($path)) {
            $differences.Add("unexpected fixture '$path'")
        } elseif (-not $Actual.Contains($path)) {
            $differences.Add("missing fixture '$path'")
        } elseif ($Expected[$path] -ne $Actual[$path]) {
            $differences.Add("fixture content mismatch '$path'")
        }
    }
    return @($differences)
}

function Copy-OracleInputFixtures {
    param(
        [Parameter(Mandatory = $true)][string]$FixtureDirectory,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not (Test-Path -LiteralPath $FixtureDirectory -PathType Container)) { return }
    foreach ($item in Get-ChildItem -LiteralPath $FixtureDirectory -Force) {
        Copy-Item -LiteralPath $item.FullName -Destination $Destination -Recurse -Force
    }
}

function Get-OracleExecutableSuffix {
    if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) {
        return ".exe"
    }
    return ""
}

function ConvertTo-OracleBackendTag {
    param([Parameter(Mandatory = $true)][string]$Backend)
    return ($Backend -replace '[^A-Za-z0-9_.-]', '_')
}

function Assert-OracleBackendAvailable {
    param(
        [Parameter(Mandatory = $true)][string]$Compiler,
        [Parameter(Mandatory = $true)][string]$Backend,
        [string]$BackendOption = "--backend"
    )

    if ($Backend -eq "c") { return }
    $compilerPath = (Resolve-Path -LiteralPath $Compiler).Path
    $help = Invoke-OracleProcess -FilePath $compilerPath -Arguments @("--help") -WorkingDirectory (Get-Location).Path
    if ("$($help.Stdout)`n$($help.Stderr)" -notmatch [regex]::Escape($BackendOption)) {
        throw "backend '$Backend' was selected, but this compiler does not advertise $BackendOption. The differential oracle is opt-in; omit -Backend until the native backend is available."
    }
}

function Invoke-OracleBackendCase {
    param(
        [Parameter(Mandatory = $true)][string]$Compiler,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Backend,
        [Parameter(Mandatory = $true)][string]$BuildRoot,
        [Parameter(Mandatory = $true)][string]$RunRoot,
        [Parameter(Mandatory = $true)][string]$ExpectedFile,
        [string]$ExpectedStderrFile,
        [string]$ExpectedExitFile,
        [string]$FixtureRoot,
        [string]$ExpectedFixtureRoot,
        [string[]]$CompileArguments = @(),
        [string]$BackendOption = "--backend",
        [switch]$ExplicitBackend,
        [bool]$CompareExpectedStdout = $true
    )

    $compilerPath = (Resolve-Path -LiteralPath $Compiler).Path
    $sourcePath = (Resolve-Path -LiteralPath $Source).Path
    $buildPath = [System.IO.Path]::GetFullPath($BuildRoot)
    $runPath = [System.IO.Path]::GetFullPath($RunRoot)
    Invoke-WithRetry { [void](New-Item -ItemType Directory -Path $buildPath -Force) }
    Invoke-WithRetry {
        if (Test-Path -LiteralPath $runPath) {
            Remove-Item -LiteralPath $runPath -Recurse -Force
        }
        [void](New-Item -ItemType Directory -Path $runPath -Force)
    }

    if ($FixtureRoot) {
        Copy-OracleInputFixtures -FixtureDirectory (Join-Path $FixtureRoot $Name) -Destination $runPath
    }

    $backendTag = ConvertTo-OracleBackendTag $Backend
    # Never let a backend tag become the artifact's semantic extension:
    # on Unix, "$Name.c" tells Oscan to emit C source instead of an
    # executable. Keep the tag in the stem and reserve only the platform
    # executable suffix for the extension.
    $artifact = Join-Path $buildPath "$Name.backend-$backendTag$(Get-OracleExecutableSuffix)"
    $arguments = @($CompileArguments)
    if ($ExplicitBackend) {
        $arguments += @($BackendOption, $Backend)
    }
    $arguments += @($sourcePath, "-o", $artifact)
    $compile = Invoke-OracleProcess -FilePath $compilerPath -Arguments $arguments -WorkingDirectory $buildPath
    # A "cannot resolve <path>: ... path specified" compile failure on an
    # already-`Resolve-Path`-verified source file is a transient Windows
    # filesystem/AV-scanning hiccup, observed specifically under sustained
    # sequential load (long freestanding-mode differential runs write and
    # scan many files back-to-back) rather than a one-off blip — retry with
    # a longer backoff before giving up rather than failing the test.
    $retryAttempt = 0
    while ($compile.ExitCode -ne 0 -and $compile.Stderr -match 'cannot resolve.*path specified' -and $retryAttempt -lt 10) {
        $retryAttempt++
        Start-Sleep -Milliseconds (300 * $retryAttempt)
        $compile = Invoke-OracleProcess -FilePath $compilerPath -Arguments $arguments -WorkingDirectory $buildPath
    }

    $failures = [System.Collections.Generic.List[string]]::new()
    if ($compile.ExitCode -ne 0) {
        $failures.Add("$Backend compile failed (exit $($compile.ExitCode)): $($compile.Stderr)")
        return [PSCustomObject]@{
            Backend = $Backend; Compile = $compile; Run = $null
            Fixtures = [ordered]@{}; Failures = @($failures); Artifact = $artifact
        }
    }
    if (-not (Test-Path -LiteralPath $artifact -PathType Leaf)) {
        $failures.Add("$Backend compile succeeded without producing '$artifact'")
        return [PSCustomObject]@{
            Backend = $Backend; Compile = $compile; Run = $null
            Fixtures = [ordered]@{}; Failures = @($failures); Artifact = $artifact
        }
    }

    $run = Invoke-OracleProcess -FilePath $artifact -WorkingDirectory $runPath
    $fixtures = Get-OracleFixtureSnapshot $runPath
    $expectedExit = 0
    if ($ExpectedExitFile -and (Test-Path -LiteralPath $ExpectedExitFile -PathType Leaf)) {
        $expectedExit = [int](Get-Content -LiteralPath $ExpectedExitFile -Raw).Trim()
    }
    if ($run.ExitCode -ne $expectedExit) {
        $failures.Add("$Backend exit mismatch (expected $expectedExit, got $($run.ExitCode))")
    }

    if ($CompareExpectedStdout) {
        if (-not (Test-Path -LiteralPath $ExpectedFile -PathType Leaf)) {
            $failures.Add("missing expected stdout file '$ExpectedFile'")
        } else {
            $expectedStdout = Normalize-OracleText (Get-Content -LiteralPath $ExpectedFile -Raw)
            if ($run.Stdout -ne $expectedStdout) {
                $failures.Add("$Backend stdout differs from expected output")
            }
        }
    }

    if ($ExpectedStderrFile -and (Test-Path -LiteralPath $ExpectedStderrFile -PathType Leaf)) {
        $expectedStderr = Normalize-OracleText (Get-Content -LiteralPath $ExpectedStderrFile -Raw)
        if ($run.Stderr -ne $expectedStderr) {
            $failures.Add("$Backend stderr differs from expected output")
        }
    }

    $expectedFixtureDirectory = if ($ExpectedFixtureRoot) {
        Join-Path $ExpectedFixtureRoot $Name
    } else {
        $null
    }
    if ($expectedFixtureDirectory -and (Test-Path -LiteralPath $expectedFixtureDirectory -PathType Container)) {
        $expectedFixtures = Get-OracleFixtureSnapshot $expectedFixtureDirectory
        foreach ($difference in Compare-OracleFixtureSnapshots $expectedFixtures $fixtures) {
            $failures.Add("$Backend $difference")
        }
    }

    return [PSCustomObject]@{
        Backend = $Backend; Compile = $compile; Run = $run
        Fixtures = $fixtures; Failures = @($failures); Artifact = $artifact
    }
}

function Invoke-DifferentialBackendTest {
    param(
        [Parameter(Mandatory = $true)][string]$Compiler,
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Backend,
        [Parameter(Mandatory = $true)][string]$BuildRoot,
        [Parameter(Mandatory = $true)][string]$RunRoot,
        [Parameter(Mandatory = $true)][string]$ExpectedFile,
        [string]$ExpectedStderrFile,
        [string]$ExpectedExitFile,
        [string]$FixtureRoot,
        [string]$ExpectedFixtureRoot,
        [string[]]$CompileArguments = @(),
        [string]$BackendOption = "--backend",
        [bool]$CompareStderr = $true,
        [bool]$CompareExpectedStdout = $true
    )

    if ($Backend -eq "c") {
        throw "Invoke-DifferentialBackendTest requires a non-C backend"
    }
    $common = @{
        Compiler = $Compiler; Source = $Source; Name = $Name
        BuildRoot = $BuildRoot; ExpectedFile = $ExpectedFile
        ExpectedStderrFile = $ExpectedStderrFile; ExpectedExitFile = $ExpectedExitFile
        FixtureRoot = $FixtureRoot; ExpectedFixtureRoot = $ExpectedFixtureRoot
        CompileArguments = $CompileArguments; BackendOption = $BackendOption
        CompareExpectedStdout = $CompareExpectedStdout
        ExplicitBackend = $true
    }
    $oracle = Invoke-OracleBackendCase @common -Backend "c" -RunRoot (Join-Path $RunRoot "c")
    $candidate = Invoke-OracleBackendCase @common -Backend $Backend -RunRoot (Join-Path $RunRoot (ConvertTo-OracleBackendTag $Backend))

    $failures = [System.Collections.Generic.List[string]]::new()
    foreach ($failure in @($oracle.Failures) + @($candidate.Failures)) {
        $failures.Add($failure)
    }
    if ($oracle.Compile.ExitCode -ne $candidate.Compile.ExitCode) {
        $failures.Add("compile exit mismatch: c=$($oracle.Compile.ExitCode), $Backend=$($candidate.Compile.ExitCode)")
    }
    if ($oracle.Run -and $candidate.Run) {
        if ($oracle.Run.ExitCode -ne $candidate.Run.ExitCode) {
            $failures.Add("runtime exit mismatch: c=$($oracle.Run.ExitCode), $Backend=$($candidate.Run.ExitCode)")
        }
        if ($oracle.Run.Stdout -ne $candidate.Run.Stdout) {
            $failures.Add("stdout mismatch between c and $Backend")
        }
        if ($CompareStderr -and $oracle.Run.Stderr -ne $candidate.Run.Stderr) {
            $failures.Add("stderr mismatch between c and $Backend")
        }
        foreach ($difference in Compare-OracleFixtureSnapshots $oracle.Fixtures $candidate.Fixtures) {
            $failures.Add("$difference between c and $Backend")
        }
    }

    return [PSCustomObject]@{
        Name = $Name; Backend = $Backend; Oracle = $oracle; Candidate = $candidate
        Failures = @($failures); Success = ($failures.Count -eq 0)
    }
}
