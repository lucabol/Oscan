[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$LlvmSdk,

    [Parameter(Mandatory = $true)]
    [string]$LlvmSource,

    [Parameter(Mandatory = $true)]
    [string]$OutputDir,

    [string]$BuildDir = (Join-Path $PSScriptRoot '..\build\inprocess-lld')
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sdk = (Resolve-Path $LlvmSdk).Path
$source = (Resolve-Path $LlvmSource).Path
$output = [IO.Path]::GetFullPath($OutputDir)
$build = [IO.Path]::GetFullPath($BuildDir)
$patch = Join-Path $repoRoot 'toolchain\inprocess\lld-22.1.0-memory-inputs.patch'
$bridgeSource = Join-Path $repoRoot 'toolchain\inprocess\oscan_lld_bridge.cpp'
$dependencyShims = Join-Path $repoRoot 'toolchain\inprocess\llvm-dependency-shims.cmake'

foreach ($path in @(
    (Join-Path $sdk 'bin\llvm-config.exe'),
    (Join-Path $source 'lld\CMakeLists.txt'),
    $patch,
    $bridgeSource,
    $dependencyShims
)) {
    if (-not (Test-Path $path -PathType Leaf)) {
        throw "Required input not found: $path"
    }
}

$version = (& (Join-Path $sdk 'bin\llvm-config.exe') --version).Trim()
if ($version -ne '22.1.0') {
    throw "This patch is pinned to LLVM 22.1.0, but the SDK reports $version"
}

Push-Location $source
try {
    & git apply --ignore-space-change --check $patch 2>$null
    if ($LASTEXITCODE -eq 0) {
        & git apply --ignore-space-change --whitespace=fix $patch
        if ($LASTEXITCODE -ne 0) {
            throw 'Failed to apply the Oscan in-memory LLD patch'
        }
    }
    else {
        & git apply --ignore-space-change --reverse --check $patch 2>$null
        if ($LASTEXITCODE -ne 0) {
            throw 'LLVM source neither accepts the patch nor contains it already'
        }
    }
}
finally {
    Pop-Location
}

if (-not $env:VSCMD_VER) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere -PathType Leaf)) {
        throw 'Visual Studio vswhere.exe was not found'
    }
    $vsRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if (-not $vsRoot) {
        throw 'Visual Studio C++ build tools were not found'
    }
    $vsDevCmd = Join-Path $vsRoot 'Common7\Tools\VsDevCmd.bat'
    $environment = & $env:ComSpec /d /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set"
    if ($LASTEXITCODE -ne 0) {
        throw 'Failed to initialize the Visual Studio build environment'
    }
    foreach ($line in $environment) {
        $separator = $line.IndexOf('=')
        if ($separator -gt 0) {
            [Environment]::SetEnvironmentVariable(
                $line.Substring(0, $separator),
                $line.Substring($separator + 1),
                'Process'
            )
        }
    }
}

$cmake = Get-Command cmake.exe -ErrorAction Stop
$ninja = Get-Command ninja.exe -ErrorAction Stop
if (Test-Path $build) {
    Remove-Item -Recurse -Force $build
}
New-Item -ItemType Directory -Force -Path $build | Out-Null
$cmakeProgram = $cmake.Source.Replace('\', '/')
$ninjaProgram = $ninja.Source.Replace('\', '/')
$llvmCmakeDir = (Join-Path $sdk 'lib\cmake\llvm').Replace('\', '/')
$dependencyShimsCmake = $dependencyShims.Replace('\', '/')
& $cmakeProgram `
    -S (Join-Path $source 'lld') `
    -B $build `
    -G Ninja `
    "-DCMAKE_MAKE_PROGRAM=$ninjaProgram" `
    '-DCMAKE_BUILD_TYPE=Release' `
    "-DLLVM_DIR=$llvmCmakeDir" `
    "-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES=$dependencyShimsCmake" `
    '-DLLVM_TARGETS_TO_BUILD=X86' `
    '-DLLVM_INCLUDE_TESTS=OFF' `
    '-DLLD_INCLUDE_TESTS=OFF' `
    '-DLLD_BUILD_TOOLS=OFF' `
    '-DLLVM_USE_CRT_RELEASE=MT' `
    '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded'
if ($LASTEXITCODE -ne 0) {
    throw 'CMake configuration failed'
}
& $cmakeProgram --build $build --target lldCommon lldCOFF lldMinGW
if ($LASTEXITCODE -ne 0) {
    throw 'Patched LLD build failed'
}

$outputLib = Join-Path $output 'lib'
if (Test-Path $output) {
    Remove-Item -Recurse -Force $output
}
New-Item -ItemType Directory -Force -Path $outputLib | Out-Null
foreach ($name in @('lldCommon.lib', 'lldCOFF.lib', 'lldMinGW.lib')) {
    $match = Get-ChildItem $build -Recurse -Filter $name | Select-Object -First 1
    if (-not $match) {
        throw "Built library not found: $name"
    }
    Copy-Item $match.FullName (Join-Path $outputLib $name)
}

$bridgeObject = Join-Path $build 'oscan_lld_bridge.obj'
& cl.exe /nologo /c /O2 /MT /std:c++17 /EHsc /GR- `
    "/I$(Join-Path $source 'lld\include')" `
    "/I$(Join-Path $build 'include')" `
    "/I$(Join-Path $sdk 'include')" `
    $bridgeSource `
    "/Fo$bridgeObject"
if ($LASTEXITCODE -ne 0) {
    throw 'Oscan LLD bridge compilation failed'
}
& lib.exe /nologo "/OUT:$(Join-Path $outputLib 'oscan_lld_bridge.lib')" $bridgeObject
if ($LASTEXITCODE -ne 0) {
    throw 'Oscan LLD bridge archive creation failed'
}

$components = @(
    'x86codegen', 'x86asmparser', 'x86desc', 'x86info',
    'binaryformat', 'bitwriter', 'core', 'debuginfocodeview',
    'debuginfodwarf', 'debuginfomsf', 'debuginfopdb', 'demangle',
    'dtlto', 'libdriver', 'lto', 'mc', 'object', 'option', 'passes',
    'support', 'target', 'targetparser', 'windowsdriver'
)
$llvmConfig = Join-Path $sdk 'bin\llvm-config.exe'
$llvmLibraryPaths = ((& $llvmConfig --link-static --libs @components) -join ' ') -split '\s+' |
    Where-Object { $_ } |
    ForEach-Object { $_.Trim('"') }
$llvmNames = [Collections.Generic.List[string]]::new()
foreach ($libraryPath in $llvmLibraryPaths) {
    if (-not (Test-Path $libraryPath -PathType Leaf)) {
        throw "llvm-config returned a missing library: $libraryPath"
    }
    $name = Split-Path $libraryPath -Leaf
    if (-not $llvmNames.Contains($name)) {
        Copy-Item $libraryPath (Join-Path $outputLib $name)
        $llvmNames.Add($name)
    }
}
$llvmNames | Set-Content -Encoding ascii (Join-Path $output 'llvm-libraries.txt')

$systemNames = [Collections.Generic.List[string]]::new()
$systemLibraries = ((& $llvmConfig --link-static --system-libs @components) -join ' ') -split '\s+' |
    Where-Object { $_ -and $_ -notmatch '^xml2s\.lib$' } |
    ForEach-Object { $_.Trim('"') }
foreach ($name in $systemLibraries) {
    $sdkCopy = Join-Path (Join-Path $sdk 'lib') $name
    if (Test-Path $sdkCopy -PathType Leaf) {
        Copy-Item $sdkCopy (Join-Path $outputLib $name)
    }
    if (-not $systemNames.Contains($name)) {
        $systemNames.Add($name)
    }
}
$systemNames | Set-Content -Encoding ascii (Join-Path $output 'system-libraries.txt')

[ordered]@{
    schema_version = 1
    llvm_version = $version
    target = 'x86_64-pc-windows-msvc'
    patch = (Split-Path $patch -Leaf)
    llvm_libraries = $llvmNames.Count
    system_libraries = $systemNames.Count
} | ConvertTo-Json | Set-Content -Encoding ascii (Join-Path $output 'manifest.json')

Write-Output $output
