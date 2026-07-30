param(
    [ValidateSet("all", "oscan", "rust", "typescript", "csharp", "common-lisp")]
    [string]$Language = "all"
)

$ErrorActionPreference = "Stop"
python "$PSScriptRoot\harness\suite.py" build --language $Language
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}
