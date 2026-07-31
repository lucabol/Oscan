param(
    [ValidateSet("all", "oscan", "rust", "typescript", "csharp", "common-lisp")]
    [string]$Language = "all",
    [switch]$NoBuild,
    [string]$Json
)

$ErrorActionPreference = "Stop"
$arguments = @("$PSScriptRoot\harness\suite.py", "test", "--language", $Language)
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
