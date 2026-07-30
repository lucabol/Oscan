[CmdletBinding()]
param(
    [string]$CompilerRoot = (Join-Path $PSScriptRoot '..\build\strict\compilers'),
    [string]$ExamplesRoot = (Join-Path $PSScriptRoot '..\examples'),
    [string]$OutputRoot = (Join-Path $PSScriptRoot '..\build\strict\examples'),
    [string]$CCompiler
)

$ErrorActionPreference = 'Stop'
$compilerRootPath = (Resolve-Path $CompilerRoot).Path
$examplesRootPath = (Resolve-Path $ExamplesRoot).Path
$outputRootPath = [IO.Path]::GetFullPath($OutputRoot)
$backends = @(
    [pscustomobject]@{ Name = 'c'; Compiler = Join-Path $compilerRootPath 'c\oscan.exe' },
    [pscustomobject]@{ Name = 'cranelift'; Compiler = Join-Path $compilerRootPath 'cranelift\oscan.exe' },
    [pscustomobject]@{ Name = 'llvm'; Compiler = Join-Path $compilerRootPath 'llvm\oscan.exe' }
)
foreach ($backend in $backends) {
    if (-not (Test-Path $backend.Compiler -PathType Leaf)) {
        throw "Compiler not found: $($backend.Compiler)"
    }
}

$examples = Get-ChildItem $examplesRootPath -Recurse -File -Filter '*.osc' |
    Sort-Object FullName
if ($examples.Count -eq 0) {
    throw "No .osc examples found under $examplesRootPath"
}

$rows = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[string]]::new()
foreach ($backend in $backends) {
    $backendRoot = Join-Path $outputRootPath $backend.Name
    if (Test-Path $backendRoot) {
        Remove-Item -Recurse -Force $backendRoot
    }
    New-Item -ItemType Directory -Force $backendRoot | Out-Null

    foreach ($example in $examples) {
        $relative = [IO.Path]::GetRelativePath($examplesRootPath, $example.FullName)
        $relativeOutput = [IO.Path]::ChangeExtension($relative, '.exe')
        $output = Join-Path $backendRoot $relativeOutput
        New-Item -ItemType Directory -Force (Split-Path $output -Parent) | Out-Null

        $savedCompiler = $env:OSCAN_CC
        try {
            if ($backend.Name -eq 'c' -and $CCompiler) {
                $env:OSCAN_CC = $CCompiler
            }
            $diagnostics = & $backend.Compiler $example.FullName --backend $backend.Name -o $output 2>&1
            $exitCode = $LASTEXITCODE
        }
        finally {
            $env:OSCAN_CC = $savedCompiler
        }
        if ($exitCode -ne 0 -or -not (Test-Path $output -PathType Leaf)) {
            $message = "$($backend.Name):$relative"
            $failures.Add($message)
            Write-Error "$message failed`n$($diagnostics -join [Environment]::NewLine)" -ErrorAction Continue
            continue
        }
        $file = Get-Item $output
        $rows.Add([ordered]@{
            backend = $backend.Name
            source = $relative.Replace('\', '/')
            output = [IO.Path]::GetRelativePath($outputRootPath, $output).Replace('\', '/')
            size = $file.Length
            sha256 = (Get-FileHash $output -Algorithm SHA256).Hash.ToLowerInvariant()
        })
    }
}

$manifest = [ordered]@{
    schema_version = 1
    example_count = $examples.Count
    backend_count = $backends.Count
    output_count = $rows.Count
    compilers = @(
        foreach ($backend in $backends) {
            [ordered]@{
                backend = $backend.Name
                path = $backend.Compiler
                sha256 = (Get-FileHash $backend.Compiler -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    )
    outputs = $rows
}
New-Item -ItemType Directory -Force $outputRootPath | Out-Null
$manifest | ConvertTo-Json -Depth 5 |
    Set-Content -Encoding utf8 (Join-Path $outputRootPath 'matrix-manifest.json')

if ($failures.Count -ne 0) {
    throw "$($failures.Count) example builds failed: $($failures -join ', ')"
}
Write-Output (Join-Path $outputRootPath 'matrix-manifest.json')
