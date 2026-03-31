# build-examples.ps1 — Compile all Oscan examples
# Usage: pwsh -ExecutionPolicy Bypass -File build-examples.ps1

$ErrorActionPreference = "Continue"

# Find the compiler
$oscan = if (Test-Path "target\release\oscan.exe") { "target\release\oscan.exe" }
         elseif (Test-Path "target\debug\oscan.exe") { "target\debug\oscan.exe" }
         else { Write-Error "oscan.exe not found — run 'cargo build' first"; exit 1 }

$pass = 0; $fail = 0

# Compile examples/*.osc
foreach ($f in Get-ChildItem "examples\*.osc") {
    $name = $f.BaseName
    $out = "examples\${name}.exe"
    & $oscan $f.FullName -o $out 2>$null
    if ($LASTEXITCODE -eq 0) { Write-Host "  PASS: $name" -ForegroundColor Green; $pass++ }
    else { Write-Host "  FAIL: $name" -ForegroundColor Red; $fail++ }
}

# Compile examples/gfx/*.osc (graphics — freestanding only)
if (Test-Path "examples\gfx\*.osc") {
    foreach ($f in Get-ChildItem "examples\gfx\*.osc") {
        $name = $f.BaseName
        $out = "examples\gfx\${name}.exe"
        & $oscan $f.FullName -o $out 2>$null
        if ($LASTEXITCODE -eq 0) { Write-Host "  PASS: gfx/$name" -ForegroundColor Green; $pass++ }
        else { Write-Host "  FAIL: gfx/$name" -ForegroundColor Yellow; $fail++ }
    }
}

Write-Host ""
Write-Host "  $pass compiled, $fail failed" -ForegroundColor $(if ($fail -gt 0) { "Red" } else { "Green" })
