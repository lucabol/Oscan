$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$Script = Join-Path $RepoRoot "scripts\sample-backend-matrix.ps1"
$TestRoot = Join-Path $PSScriptRoot "build\sample-backend-matrix-test"

function Assert-MatrixTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

if ($env:OS -eq "Windows_NT") {
    $differentDriveRelative = "Z:\independent-source"
    $wouldBeRejected = -not [System.IO.Path]::IsPathRooted($differentDriveRelative) -and
        ($differentDriveRelative -eq "." -or
         (-not $differentDriveRelative.StartsWith("..$([System.IO.Path]::DirectorySeparatorChar)") -and
          $differentDriveRelative -ne ".."))
    Assert-MatrixTest (-not $wouldBeRejected) "a different-drive source path must not be treated as contained by the output root"
}

function Write-FakeCompiler {
    param([Parameter(Mandatory = $true)][string]$Path)

    if ($env:OS -eq "Windows_NT") {
        Set-Content -LiteralPath $Path -NoNewline -Value @'
@echo off
set "backend=%2"
echo %* | findstr /i /c:"--backend native" >nul && exit /b 21
if not "%OSCAN_MATRIX_FAIL_SAMPLE%"=="" (
  echo %* | findstr /i /c:"%OSCAN_MATRIX_FAIL_SAMPLE%" >nul && exit /b 22
)
:arguments
if "%~1"=="" exit /b 2
if "%~1"=="-o" (
  copy /y "%SystemRoot%\System32\where.exe" "%~2" >nul
  if errorlevel 1 exit /b 1
  exit /b 0
)
shift
goto arguments
'@
    } else {
        Set-Content -LiteralPath $Path -NoNewline -Value @'
#!/bin/sh
if [ "$2" = "native" ]; then
    exit 21
fi
if [ -n "$OSCAN_MATRIX_FAIL_SAMPLE" ]; then
    case "$*" in *"$OSCAN_MATRIX_FAIL_SAMPLE"*) exit 22 ;; esac
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
        & chmod +x $Path
    }
}

try {
    Remove-Item -LiteralPath $TestRoot -Recurse -Force -ErrorAction SilentlyContinue
    $sources = Join-Path $TestRoot "sources"
    [void](New-Item -ItemType Directory -Path (Join-Path $sources "nested") -Force)
    Set-Content -LiteralPath (Join-Path $sources "same name.osc") -NoNewline -Value "sample"
    Set-Content -LiteralPath (Join-Path $sources "same_name.osc") -NoNewline -Value "sample"
    Set-Content -LiteralPath (Join-Path $sources "nested\z.osc") -NoNewline -Value "sample"
    $compiler = Join-Path $TestRoot $(if ($env:OS -eq "Windows_NT") { "fake-compiler.cmd" } else { "fake-compiler.sh" })
    Write-FakeCompiler $compiler

    $output = Join-Path $TestRoot "output"
    [void](New-Item -ItemType Directory -Path (Join-Path $output "native") -Force)
    $staleSentinel = Join-Path $output "stale-artifact.txt"
    Set-Content -LiteralPath $staleSentinel -NoNewline -Value "stale"
    $run = & pwsh -NoProfile -File $Script -Oscan $compiler -SourceDirectory $sources -OutputDirectory $output 2>&1 | Out-String
    Assert-MatrixTest ($LASTEXITCODE -eq 0) "matrix should pass with C and LLVM fake backends: $run"
    Assert-MatrixTest ($run -match [regex]::Escape("Output root: $output")) "matrix did not print its absolute output root"
    Assert-MatrixTest (-not (Test-Path -LiteralPath $staleSentinel)) "matrix did not remove stale output-root artifacts"
    Assert-MatrixTest ($run -match "SKIP Cranelift/native") "matrix did not skip unavailable native backend"
    Assert-MatrixTest ($run -match "same name\.osc.*") "matrix table did not contain samples"

    $cArtifacts = @(Get-ChildItem -LiteralPath (Join-Path $output "c") -File)
    $llvmArtifacts = @(Get-ChildItem -LiteralPath (Join-Path $output "llvm") -File)
    Assert-MatrixTest ($cArtifacts.Count -eq 3 -and $llvmArtifacts.Count -eq 3) "matrix did not produce one artifact per sample/backend"
    Assert-MatrixTest (@($cArtifacts.Name | Select-Object -Unique).Count -eq 3) "sanitized sample names were not unique"
    Assert-MatrixTest (@($cArtifacts | Where-Object { $_.Extension -in @(".c", ".ll") }).Count -eq 0) "matrix emitted source instead of executables"
    Assert-MatrixTest (-not (Test-Path -LiteralPath (Join-Path $output "native"))) "matrix emitted artifacts for an unavailable backend"

    $env:OSCAN_MATRIX_FAIL_SAMPLE = "nested\z.osc"
    $failedOutput = Join-Path $TestRoot "failed-output"
    $failed = & pwsh -NoProfile -File $Script -Oscan $compiler -SourceDirectory $sources -OutputDirectory $failedOutput 2>&1 | Out-String
    Assert-MatrixTest ($LASTEXITCODE -ne 0) "matrix must fail when an available backend cannot compile a sample"
    Assert-MatrixTest ($failed -match "FAIL .*nested/z\.osc") "matrix did not report the explicit sample compile failure"
} finally {
    Remove-Item Env:\OSCAN_MATRIX_FAIL_SAMPLE -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $TestRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "sample backend matrix tests passed"
