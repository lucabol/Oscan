param(
    [Parameter(Mandatory = $true)]
    [string]$Oscan,

    [ValidateSet("cranelift", "llvm")]
    [string]$Backend = "cranelift"
)

$ErrorActionPreference = "Stop"
$ScriptDir = $PSScriptRoot
$RepoRoot = Split-Path -Parent $ScriptDir
. (Join-Path $ScriptDir "backend_oracle.ps1")

function Assert-NativeExternStr {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

$compiler = (Resolve-Path -LiteralPath $Oscan).Path
$buildRoot = Join-Path $ScriptDir "build\$Backend-extern-str-abi"
[void](New-Item -ItemType Directory -Path $buildRoot -Force)

$oscSource = Join-Path $buildRoot "extern-str.osc"
$bridgeSource = Join-Path $buildRoot "extern-str-bridge.c"
$cExe = Join-Path $buildRoot "extern-str-c$(Get-OracleExecutableSuffix)"
$backendExe = Join-Path $buildRoot "extern-str-$Backend$(Get-OracleExecutableSuffix)"

Set-Content -LiteralPath $oscSource -NoNewline -Value @'
struct UnusedPayload {
    text: str,
}

extern {
    fn! vlog_msg(tag: str, msg: str);
    fn! js_engine_eval(ctx: handle, code: str) -> str;
    fn! str_score(s: str, bias: i32) -> i32;
    fn! join2(a: str, b: str) -> str;
    fn! unused_missing_str(s: str) -> str;
    fn! unused_struct(p: UnusedPayload);
}

fn! main() {
    let ctx: handle = (7 as i64) as handle;
    vlog_msg("tag", "AB");

    let eval: str = js_engine_eval(ctx, "1+2");
    print("eval len=");
    print_i32(str_len(eval));
    println("");
    print("eval nul byte=");
    print_i32(eval[2]);
    println("");
    print("eval byte3=");
    print_i32(eval[3]);
    println("");

    print("score=");
    print_i32(str_score("AB", 5));
    println("");
    println(join2("left", "right"));
}
'@

Set-Content -LiteralPath $bridgeSource -NoNewline -Value @'
#include "osc_runtime.h"
#include <stdint.h>

static osc_str lit(const char *data, int32_t len) {
    osc_str s;
    s.data = data;
    s.len = len;
    return s;
}

void vlog_msg(osc_str tag, osc_str msg) {
    osc_print(tag);
    osc_print(lit(":len=", 5));
    osc_print_i32(msg.len);
    osc_print(lit(" b1=", 4));
    osc_print_i32((uint8_t)msg.data[1]);
    osc_println(lit("", 0));
}

osc_str js_engine_eval(uintptr_t ctx, osc_str code) {
    if (ctx == 7 && code.len == 3 && code.data[0] == '1' && code.data[1] == '+' && code.data[2] == '2') {
        return lit("ok\0native", 9);
    }
    return lit("bad", 3);
}

int32_t str_score(osc_str s, int32_t bias) {
    return s.len + (uint8_t)s.data[1] + bias;
}

osc_str join2(osc_str a, osc_str b) {
    if (a.len == 4 && b.len == 5 && a.data[0] == 'l' && b.data[0] == 'r') {
        return lit("joined", 6);
    }
    return lit("bad", 3);
}
'@

$expected = @'
tag:len=2 b1=66
eval len=9
eval nul byte=0
eval byte3=110
score=73
joined
'@ | ForEach-Object { Normalize-OracleText $_ }

$cCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @("--backend", "c", "--extra-c", $bridgeSource, $oscSource, "-o", $cExe) `
    -WorkingDirectory $RepoRoot
Assert-NativeExternStr ($cCompile.ExitCode -eq 0) "C backend extern-str compile failed: $($cCompile.Stderr)"
$cRun = Invoke-OracleProcess -FilePath $cExe -WorkingDirectory $buildRoot
Assert-NativeExternStr ($cRun.ExitCode -eq 0) "C backend extern-str executable failed: $($cRun.Stderr)"
Assert-NativeExternStr ($cRun.Stdout -eq $expected) "C backend extern-str output mismatch: '$($cRun.Stdout)'"

$backendCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @("--backend", $Backend, "--extra-c", $bridgeSource, $oscSource, "-o", $backendExe) `
    -WorkingDirectory $RepoRoot
Assert-NativeExternStr ($backendCompile.ExitCode -eq 0) "$Backend extern-str compile failed: $($backendCompile.Stderr)"
$backendRun = Invoke-OracleProcess -FilePath $backendExe -WorkingDirectory $buildRoot
Assert-NativeExternStr ($backendRun.ExitCode -eq 0) "$Backend extern-str executable failed: $($backendRun.Stderr)"
Assert-NativeExternStr ($backendRun.Stdout -eq $expected) "$Backend extern-str output mismatch: '$($backendRun.Stdout)'"

$objectCompiler = $env:OSCAN_CC
if (-not $objectCompiler) {
    $cmd = Get-Command gcc -ErrorAction SilentlyContinue
    if (-not $cmd) { $cmd = Get-Command clang -ErrorAction SilentlyContinue }
    if ($cmd) { $objectCompiler = $cmd.Source }
}
if ($objectCompiler) {
    $bridgeObject = Join-Path $buildRoot "extern-str-bridge.obj"
    $compileObj = Invoke-OracleProcess `
        -FilePath $objectCompiler `
        -Arguments @("-std=c99", "-I$($RepoRoot)\runtime", "-c", $bridgeSource, "-o", $bridgeObject) `
        -WorkingDirectory $RepoRoot
    if ($compileObj.ExitCode -eq 0) {
        $backendObjExe = Join-Path $buildRoot "extern-str-$Backend-obj$(Get-OracleExecutableSuffix)"
        $backendObjCompile = Invoke-OracleProcess `
            -FilePath $compiler `
            -Arguments @("--backend", $Backend, "--extra-obj", $bridgeObject, $oscSource, "-o", $backendObjExe) `
            -WorkingDirectory $RepoRoot
        Assert-NativeExternStr ($backendObjCompile.ExitCode -eq 0) "$Backend extern-str --extra-obj compile failed: $($backendObjCompile.Stderr)"
        $backendObjRun = Invoke-OracleProcess -FilePath $backendObjExe -WorkingDirectory $buildRoot
        Assert-NativeExternStr ($backendObjRun.ExitCode -eq 0) "$Backend extern-str --extra-obj executable failed: $($backendObjRun.Stderr)"
        Assert-NativeExternStr ($backendObjRun.Stdout -eq $expected) "$Backend extern-str --extra-obj output mismatch: '$($backendObjRun.Stdout)'"
    }
}

if ($Backend -eq "cranelift") {
$badStruct = Join-Path $buildRoot "extern-struct-bad.osc"
Set-Content -LiteralPath $badStruct -NoNewline -Value @'
struct Boxed {
    value: i32,
}

extern {
    fn! takes_boxed(v: Boxed);
}

fn! main() {
    takes_boxed(Boxed { value: 1 });
}
'@
$badStructCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @("--backend", "cranelift", $badStruct, "-o", (Join-Path $buildRoot "bad-struct$(Get-OracleExecutableSuffix)")) `
    -WorkingDirectory $RepoRoot
Assert-NativeExternStr ($badStructCompile.ExitCode -ne 0) "Cranelift struct extern unexpectedly compiled"
Assert-NativeExternStr ($badStructCompile.Stderr -match "structs still require an explicit C shim") `
    "Cranelift struct extern diagnostic was not explicit: $($badStructCompile.Stderr)"

$badResult = Join-Path $buildRoot "extern-result-bad.osc"
Set-Content -LiteralPath $badResult -NoNewline -Value @'
extern {
    fn! returns_result() -> Result<i32, str>;
}

fn! main() {
    let r: Result<i32, str> = returns_result();
    match r {
        Result::Ok(v) => print_i32(v),
        Result::Err(e) => println(e),
    };
}
'@
$badResultCompile = Invoke-OracleProcess `
    -FilePath $compiler `
    -Arguments @("--backend", "cranelift", $badResult, "-o", (Join-Path $buildRoot "bad-result$(Get-OracleExecutableSuffix)")) `
    -WorkingDirectory $RepoRoot
Assert-NativeExternStr ($badResultCompile.ExitCode -ne 0) "Cranelift Result extern unexpectedly compiled"
Assert-NativeExternStr ($badResultCompile.Stderr -match "Result still requires an explicit C shim") `
    "Cranelift Result extern diagnostic was not explicit: $($badResultCompile.Stderr)"
}

Write-Host "$Backend extern str ABI tests passed"
