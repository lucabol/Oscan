# Focused tests for Resolve-WslNativeBatchRecords, the pure helper in
# test.ps1 that validates the single batched WSL call used by the
# native-backend cross-target phase (WSL Linux x64 <backend> cross-link).
# It must fail a name whose program's exit code doesn't match its declared
# expected exit code (0 unless overridden via $ExpectedExitCodes, mirroring
# Invoke-OracleBackendCase's tests/expected_exit/<name>.expected convention)
# even when stdout later matches, surface a failed wsl invocation instead of
# silently reporting "0 passed, 0 failed", and reject/repair missing,
# duplicate, out-of-set, and malformed records rather than accepting them.
#
# This does not require WSL to be installed: the helper takes plain strings
# and never shells out, so these tests run anywhere PowerShell runs.

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot ".." "test.ps1") -SourceOnly

function Assert-WslBatchTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Get-RecordByName($Result, [string]$Name) {
    return @($Result.Records | Where-Object { $_.Name -eq $Name })[0]
}

# ── 1. Success: every attempted name reports OK|name|0 ─────────────────
$success = Resolve-WslNativeBatchRecords `
    "OK|hello_world|0`nOK|fibonacci|0`n" `
    0 `
    @("hello_world", "fibonacci")
Assert-WslBatchTest ($success.Errors.Count -eq 0) "success case should have no errors"
Assert-WslBatchTest ($success.Records.Count -eq 2) "success case should have exactly 2 records"
foreach ($n in @("hello_world", "fibonacci")) {
    $r = Get-RecordByName $success $n
    Assert-WslBatchTest ($r.Status -eq 'OK') "success case: '$n' should be OK"
    Assert-WslBatchTest ([string]::IsNullOrEmpty($r.Detail)) "success case: '$n' should have empty detail"
}

# ── 2. Nonzero program exit must fail even though it "ran" ─────────────
# (stdout comparison happens separately in the caller; this only asserts
# that a nonzero exit code alone is enough to mark the record FAIL.)
$nonzeroExit = Resolve-WslNativeBatchRecords `
    "OK|hello_world|0`nOK|fibonacci|1`n" `
    0 `
    @("hello_world", "fibonacci")
$fib = Get-RecordByName $nonzeroExit "fibonacci"
Assert-WslBatchTest ($fib.Status -eq 'FAIL') "nonzero exit code must fail even if stdout later matches"
Assert-WslBatchTest ($fib.Detail -match 'exited 1') "nonzero exit detail should mention the exit code"
$hw = Get-RecordByName $nonzeroExit "hello_world"
Assert-WslBatchTest ($hw.Status -eq 'OK') "unrelated passing name must remain OK"

# ── 3. The wsl batch command itself failing must not be swallowed ──────
$batchFailed = Resolve-WslNativeBatchRecords `
    "" `
    1 `
    @("hello_world", "fibonacci")
Assert-WslBatchTest ($batchFailed.Errors.Count -ge 1) "a failed wsl invocation must be reported as an error"
Assert-WslBatchTest (($batchFailed.Errors -join "`n") -match 'wsl native batch command failed \(exit 1\)') `
    "batch failure error should mention the nonzero exit code"
foreach ($n in @("hello_world", "fibonacci")) {
    $r = Get-RecordByName $batchFailed $n
    Assert-WslBatchTest ($r.Status -eq 'FAIL') "'$n' must be FAIL when the whole batch call failed"
}

# ── 4. Fewer records than attempted (one attempted name never echoed) ──
$fewerRecords = Resolve-WslNativeBatchRecords `
    "OK|hello_world|0`n" `
    0 `
    @("hello_world", "fibonacci")
Assert-WslBatchTest ($fewerRecords.Records.Count -eq 2) "missing records must still produce one entry per attempted name"
$missing = Get-RecordByName $fewerRecords "fibonacci"
Assert-WslBatchTest ($missing.Status -eq 'FAIL') "an attempted name with no record must be FAIL"
Assert-WslBatchTest ($missing.Detail -eq 'missing batch result record') "missing record should say so"
Assert-WslBatchTest (($fewerRecords.Errors -join "`n") -match "missing batch result record for 'fibonacci'") `
    "missing record must also be reported as an error"

# ── 5. More records than attempted (unexpected/unattempted name) ──────
$extraRecord = Resolve-WslNativeBatchRecords `
    "OK|hello_world|0`nOK|not_attempted|0`n" `
    0 `
    @("hello_world")
Assert-WslBatchTest ($extraRecord.Records.Count -eq 1) "an unattempted name must not be accepted into Records"
Assert-WslBatchTest (($extraRecord.Errors -join "`n") -match "unattempted name 'not_attempted'") `
    "extra record for an unattempted name must be reported as an error"
$hwOnly = Get-RecordByName $extraRecord "hello_world"
Assert-WslBatchTest ($hwOnly.Status -eq 'OK') "the legitimately attempted name must still pass"

# ── 6. Duplicate records for the same name must not be silently accepted ─
$duplicate = Resolve-WslNativeBatchRecords `
    "OK|hello_world|0`nOK|hello_world|0`n" `
    0 `
    @("hello_world")
Assert-WslBatchTest ($duplicate.Records.Count -eq 1) "a duplicated name must still yield exactly one record"
$dup = Get-RecordByName $duplicate "hello_world"
Assert-WslBatchTest ($dup.Status -eq 'FAIL') "a duplicate record must fail rather than keep the earlier OK"
Assert-WslBatchTest ($dup.Detail -eq 'duplicate batch record') "duplicate record should say so"
Assert-WslBatchTest (($duplicate.Errors -join "`n") -match "duplicate batch record for 'hello_world'") `
    "duplicate must also be reported as an error"

# ── 7. Malformed records must not be accepted at face value ────────────
# 7a. OK record with a non-numeric "exit code" field.
$badExit = Resolve-WslNativeBatchRecords `
    "OK|hello_world|not-a-number`n" `
    0 `
    @("hello_world")
$bad = Get-RecordByName $badExit "hello_world"
Assert-WslBatchTest ($bad.Status -eq 'FAIL') "a non-numeric exit code must fail, not be accepted as OK"
Assert-WslBatchTest (($badExit.Errors -join "`n") -match "malformed exit code for 'hello_world'") `
    "malformed exit code must be reported as an error"

# 7b. Record missing its third field entirely.
$missingField = Resolve-WslNativeBatchRecords `
    "OK|hello_world`n" `
    0 `
    @("hello_world")
$mf = Get-RecordByName $missingField "hello_world"
Assert-WslBatchTest ($mf.Status -eq 'FAIL') "a record missing its detail field must fail"
Assert-WslBatchTest (($missingField.Errors -join "`n") -match "missing detail field") `
    "missing-field record must be reported as an error"

# 7c. Record with an empty name must not be silently ignored.
$emptyName = Resolve-WslNativeBatchRecords `
    "OK||0`nOK|hello_world|0`n" `
    0 `
    @("hello_world")
Assert-WslBatchTest ($emptyName.Records.Count -eq 1) "an empty-name record must not create a phantom entry"
Assert-WslBatchTest (($emptyName.Errors -join "`n") -match 'malformed batch record \(no name\)') `
    "empty-name record must be reported as an error"
$hwEmpty = Get-RecordByName $emptyName "hello_world"
Assert-WslBatchTest ($hwEmpty.Status -eq 'OK') "a valid record alongside a malformed one must still pass"

# ── 8. Link-error FAIL records propagate their detail unchanged ────────
$linkError = Resolve-WslNativeBatchRecords `
    "FAIL|ffi|link error`n" `
    0 `
    @("ffi")
$le = Get-RecordByName $linkError "ffi"
Assert-WslBatchTest ($le.Status -eq 'FAIL') "an explicit FAIL record must remain FAIL"
Assert-WslBatchTest ($le.Detail -eq 'link error') "FAIL record detail must be preserved"

# ── 9. A declared nonzero expected exit code must be honored ───────────
# result_main_exit_code's tests/expected_exit file declares "1" — a program
# that actually exits 1 must be OK, not forced through a hardcoded "must be
# 0" assumption, while an unrelated name with no override still defaults to
# expecting 0.
$declaredExit = Resolve-WslNativeBatchRecords `
    "OK|result_main_exit_code|1`nOK|hello_world|0`n" `
    0 `
    @("result_main_exit_code", "hello_world") `
    @{ result_main_exit_code = 1 }
Assert-WslBatchTest ($declaredExit.Errors.Count -eq 0) "a declared nonzero exit code should not itself be an error"
$rmec = Get-RecordByName $declaredExit "result_main_exit_code"
Assert-WslBatchTest ($rmec.Status -eq 'OK') "an exit code matching its declared expected_exit must be OK"
Assert-WslBatchTest ([string]::IsNullOrEmpty($rmec.Detail)) "a correctly-matched declared exit code should have empty detail"
$hw2 = Get-RecordByName $declaredExit "hello_world"
Assert-WslBatchTest ($hw2.Status -eq 'OK') "a name with no expected-exit override must still default to expecting 0"

# ── 10. A declared expected exit code must still be enforced, not just allowed ─
# If the program's actual exit code doesn't match its declared expected exit
# (including exiting 0 when a nonzero exit was expected), it must fail with a
# detail that names both the actual and expected codes.
$wrongDeclaredExit = Resolve-WslNativeBatchRecords `
    "OK|result_main_exit_code|0`n" `
    0 `
    @("result_main_exit_code") `
    @{ result_main_exit_code = 1 }
$wrong = Get-RecordByName $wrongDeclaredExit "result_main_exit_code"
Assert-WslBatchTest ($wrong.Status -eq 'FAIL') "an exit code that contradicts its declared expected_exit must fail"
Assert-WslBatchTest ($wrong.Detail -match 'exited 0' -and $wrong.Detail -match 'expected 1') `
    "mismatched declared exit code detail should mention both the actual and expected codes"

Write-Host "wsl native batch harness tests passed"
