# Executable regressions for rejecting child-arena values stored in outer arrays.

param(
    [Parameter(Mandatory = $true)][string]$Oscan,
    [ValidateSet("c", "cranelift", "llvm")][string[]]$Backends = @("c", "cranelift")
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $ScriptDir "backend_oracle.ps1")

$oscanPath = (Resolve-Path -LiteralPath $Oscan).Path
$workDir = Join-Path $ScriptDir "build\arena-store"
New-Item -ItemType Directory -Path $workDir -Force | Out-Null

$cases = @(
    [PSCustomObject]@{
        Name = "push"
        Source = @'
fn! main() {
    let mut outer: [[i32]] = [[1]];
    println("before");
    arena {
        push(outer, [2]);
    };
    println("after");
}
'@
    },
    [PSCustomObject]@{
        Name = "index"
        Source = @'
fn! main() {
    let mut outer: [[i32]] = [[1]];
    println("before");
    arena {
        outer[0] = [2];
    };
    println("after");
}
'@
    },
    [PSCustomObject]@{
        Name = "struct-field"
        Source = @'
struct Bucket {
    values: [i32],
}

fn! main() {
    let mut buckets: [Bucket] = [Bucket { values: [1] }];
    println("before");
    arena {
        buckets[0].values = [2];
    };
    println("after");
}
'@
    },
    [PSCustomObject]@{
        Name = "push-eval-once"
        ExpectedStdout = "before`nindex"
        Source = @'
fn! choose() -> i32 {
    println("index");
    0
}

fn! main() {
    let mut targets: [[[i32]]] = [[[1]], [[2]]];
    println("before");
    arena {
        push(targets[choose()], [3]);
    };
    println("after");
}
'@
    },
    [PSCustomObject]@{
        Name = "insert-eval-once"
        ExpectedStdout = "before`nindex"
        Source = @'
fn! choose() -> i32 {
    println("index");
    0
}

fn! main() {
    let mut targets: [[[i32]]] = [[[1]], [[2]]];
    println("before");
    arena {
        array_insert(targets[choose()], 0, [3]);
    };
    println("after");
}
'@
    },
    [PSCustomObject]@{
        Name = "nested-index-eval-once"
        ExpectedStdout = "before`nindex"
        Source = @'
fn! choose() -> i32 {
    println("index");
    0
}

fn! main() {
    let mut targets: [[[i32]]] = [[[1]], [[2]]];
    println("before");
    arena {
        targets[choose()][0] = [3];
    };
    println("after");
}
'@
    }
)

$expectedMessage = "arena-backed value cannot be stored in an array owned by another arena"
$failures = [System.Collections.Generic.List[string]]::new()

foreach ($case in $cases) {
    $sourcePath = Join-Path $workDir "$($case.Name).osc"
    Set-Content -LiteralPath $sourcePath -Value $case.Source -NoNewline

    foreach ($backend in $Backends) {
        $exe = Join-Path $workDir "$($case.Name).backend-$backend$(Get-OracleExecutableSuffix)"
        Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
        & $oscanPath --backend $backend $sourcePath -o $exe 2>$null
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $exe)) {
            $failures.Add("$($case.Name)/$backend failed to compile")
            continue
        }

        $run = Invoke-OracleProcess -FilePath $exe -WorkingDirectory $workDir
        if ($run.ExitCode -ne 1) {
            $failures.Add("$($case.Name)/$backend expected exit 1, got $($run.ExitCode)")
        }
        $expectedStdout = if ($case.PSObject.Properties.Name -contains "ExpectedStdout") {
            $case.ExpectedStdout
        } else {
            "before"
        }
        if ($run.Stdout -ne $expectedStdout) {
            $failures.Add("$($case.Name)/$backend expected stdout '$expectedStdout', got '$($run.Stdout)'")
        }
        $stderr = Normalize-OracleText $run.Stderr
        if ($stderr -notmatch "osc_runtime\.c:\d+: $([regex]::Escape($expectedMessage))$") {
            $failures.Add("$($case.Name)/$backend emitted unexpected stderr: '$stderr'")
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Host "arena store tests FAILED:" -ForegroundColor Red
    foreach ($failure in $failures) {
        Write-Host "  - $failure" -ForegroundColor Red
    }
    exit 1
}

Write-Host "arena store tests PASSED" -ForegroundColor Green
exit 0
