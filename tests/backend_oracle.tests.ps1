$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "backend_oracle.ps1")

function Assert-OracleTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

Assert-OracleTest ((Normalize-OracleText "a`r`nb`r`n") -eq "a`nb") "text normalization failed"
Assert-OracleTest ((Normalize-OracleText "a`rb`n") -eq "a`nb") "bare CR normalization failed"

# Test-ExpectedOutputMatch: primary expected file wins when it matches; an
# optional fallback (mirroring tests/expected_libc/<name>.expected) is only
# consulted when the primary doesn't match, and a genuine divergence must
# still fail rather than being masked by either file.
$matchRoot = Join-Path $PSScriptRoot "build\expected-output-match-test"
try {
    [void](New-Item -ItemType Directory -Path $matchRoot -Force)
    $primary = Join-Path $matchRoot "primary.expected"
    $fallback = Join-Path $matchRoot "fallback.expected"
    Set-Content -LiteralPath $primary -Value "true`ndone" -NoNewline
    Set-Content -LiteralPath $fallback -Value "false`ndone" -NoNewline

    Assert-OracleTest (Test-ExpectedOutputMatch -ActualRaw "true`r`ndone" -PrimaryExpectedFile $primary -FallbackExpectedFile $fallback) `
        "actual output matching the primary expected file should pass"
    Assert-OracleTest (Test-ExpectedOutputMatch -ActualRaw "false`r`ndone" -PrimaryExpectedFile $primary -FallbackExpectedFile $fallback) `
        "actual output matching only the fallback expected file should still pass"
    Assert-OracleTest (-not (Test-ExpectedOutputMatch -ActualRaw "garbage" -PrimaryExpectedFile $primary -FallbackExpectedFile $fallback)) `
        "actual output matching neither file must fail, not be masked by the fallback"
    Assert-OracleTest (-not (Test-ExpectedOutputMatch -ActualRaw "false`ndone" -PrimaryExpectedFile $primary -FallbackExpectedFile "$fallback.does-not-exist")) `
        "a nonexistent fallback file must not be treated as a match"
    Assert-OracleTest (-not (Test-ExpectedOutputMatch -ActualRaw "true`ndone" -PrimaryExpectedFile "$primary.does-not-exist" -FallbackExpectedFile $fallback)) `
        "a match against the fallback alone still requires the fallback file to actually match"
} finally {
    Remove-Item -LiteralPath $matchRoot -Recurse -Force -ErrorAction SilentlyContinue
}

$pwsh = (Get-Command pwsh).Source
$captured = Invoke-OracleProcess `
    -FilePath $pwsh `
    -Arguments @("-NoProfile", "-Command", '[Console]::Out.Write("out`r`n"); [Console]::Error.Write("err`r`n"); exit 7') `
    -WorkingDirectory $PSScriptRoot
Assert-OracleTest ($captured.ExitCode -eq 7) "process exit capture failed"
Assert-OracleTest ($captured.Stdout -eq "out") "process stdout capture failed"
Assert-OracleTest ($captured.Stderr -eq "err") "process stderr capture failed"

$root = Join-Path $PSScriptRoot "build\oracle-helper-test"
$left = Join-Path $root "left"
$right = Join-Path $root "right"
try {
    [void](New-Item -ItemType Directory -Path $left, $right -Force)
    Set-Content -LiteralPath (Join-Path $left "same.txt") -Value "same" -NoNewline
    Set-Content -LiteralPath (Join-Path $right "same.txt") -Value "same" -NoNewline
    $same = Compare-OracleFixtureSnapshots (Get-OracleFixtureSnapshot $left) (Get-OracleFixtureSnapshot $right)
    Assert-OracleTest ($same.Count -eq 0) "equal fixture snapshots differed"

    Set-Content -LiteralPath (Join-Path $right "same.txt") -Value "different" -NoNewline
    Set-Content -LiteralPath (Join-Path $right "extra.txt") -Value "extra" -NoNewline
    $different = Compare-OracleFixtureSnapshots (Get-OracleFixtureSnapshot $left) (Get-OracleFixtureSnapshot $right)
    Assert-OracleTest ($different.Count -eq 2) "fixture differences were not detected"

    $source = Join-Path $root "probe.osc"
    $expected = Join-Path $root "probe.expected"
    $expectedExit = Join-Path $root "probe.exit"
    Set-Content -LiteralPath $source -Value "probe" -NoNewline
    Set-Content -LiteralPath $expected -Value "" -NoNewline
    if ([System.Environment]::OSVersion.Platform -eq [System.PlatformID]::Win32NT) {
        $compiler = Join-Path $root "fake-compiler.cmd"
        Set-Content -LiteralPath $compiler -Value @"
@echo off
if "%~1"=="--help" (
  echo usage: fake-compiler --backend ^<name^>
  exit /b 0
)
set "out="
:loop
if "%~1"=="" goto copy
if "%~1"=="-o" (
  set "out=%~2"
  shift
  shift
  goto loop
)
shift
goto loop
:copy
copy /y "%SystemRoot%\System32\where.exe" "%out%" >nul
"@
        Set-Content -LiteralPath $expectedExit -Value "2" -NoNewline
    } else {
        $compiler = Join-Path $root "fake-compiler.sh"
        Set-Content -LiteralPath $compiler -Value @'
#!/bin/sh
if [ "$1" = "--help" ]; then
  echo "usage: fake-compiler --backend <name>"
  exit 0
fi
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    cp /bin/true "$2"
    chmod +x "$2"
    exit 0
  fi
  shift
done
exit 2
'@
        & chmod +x $compiler
        Set-Content -LiteralPath $expectedExit -Value "0" -NoNewline
    }

    Assert-OracleBackendAvailable -Compiler $compiler -Backend "native"
    $differential = Invoke-DifferentialBackendTest `
        -Compiler $compiler `
        -Source $source `
        -Name "probe" `
        -Backend "native" `
        -BuildRoot (Join-Path $root "artifacts") `
        -RunRoot (Join-Path $root "runs") `
        -ExpectedFile $expected `
        -ExpectedExitFile $expectedExit
    Assert-OracleTest $differential.Success "end-to-end differential comparison failed: $($differential.Failures -join '; ')"
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "backend oracle helper tests passed"
