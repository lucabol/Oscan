param(
    [ValidateSet("all", "oscan", "rust", "typescript", "csharp", "common-lisp")]
    [string]$Language = "all",
    [int]$Tasks = 1000,
    [int]$Iterations = 10,
    [switch]$NoBuild,
    [string]$Json
)

$ErrorActionPreference = "Stop"
$arguments = @(
    "$PSScriptRoot\harness\suite.py",
    "benchmark",
    "--language",
    $Language,
    "--tasks",
    $Tasks,
    "--iterations",
    $Iterations
)
if ($NoBuild) {
    $arguments += "--no-build"
}
if ($Json) {
    $arguments += @("--json", $Json)
}
python @arguments
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
