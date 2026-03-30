# Spec Expansion: 4 Feature Groups for CLI Samples

**Author:** Oracle (Language Spec Specialist)  
**Date:** 2025-07-18  
**Status:** APPLIED  

## Summary

Expanded `docs/spec/oscan-spec.md` from 18 to 36 micro-lib functions to support CLI sample programs (grep, sort, hexdump, base64, etc.).

## Changes Applied

### 1. Bitwise Functions (§10.3 — renamed to "Math & Bitwise Functions (9)")
- 6 new pure functions: `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot`
- All `i32`-only with unsigned shift semantics

### 2. String Operations (§10.2 expanded + §5.12.1 + §5.2 updated)
- 3 new built-ins: `str_find` (pure), `str_from_i32` (fn!), `str_slice` (fn!)
- New §5.12.1 "String Indexing": `s[i]` returns byte as `i32`, read-only, bounds-checked
- §5.2: relational operators `<`, `>`, `<=`, `>=` now accept `str` operands (lexicographic)

### 3. Command-Line Arguments (new §10.7)
- 2 new functions: `arg_count()`, `arg_get(i)`
- Exposes C argc/argv

### 4. File I/O (new §10.8)
- 7 new functions: `file_open_read`, `file_open_write`, `read_byte`, `write_byte`, `write_str`, `file_close`, `file_delete`
- 3 constants: `STDIN`, `STDOUT`, `STDERR`
- Byte-at-a-time I/O to avoid buffer management complexity

## Impact

- **Trinity (Compiler):** Must register 20 new builtins in semantic.rs and emit corresponding C calls in codegen.rs. String indexing and str comparison operators need parser/semantic/codegen changes.
- **Morpheus (Runtime):** Must implement all new functions in osc_runtime.h/c. File I/O, arg handling, bitwise ops, string slicing.
- **Tank (Tests):** Needs new test coverage for all 4 feature groups.
- **Neo:** Guide needs updating to match.

## Design Notes

- `str_from_i32` overlaps with existing `i32_to_str` — both convert int to string. `str_from_i32` follows the `str_*` naming convention; `i32_to_str` remains for backward compatibility.
- Bitwise shift semantics are unsigned to avoid C undefined behavior on signed shifts.
- File I/O uses handles (i32) rather than opaque types for simplicity.
