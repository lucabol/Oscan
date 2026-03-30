# Decision: New Built-in Functions (Bitwise, String Ops, CLI Args)

**Author:** Trinity (Compiler Dev)
**Date:** 2025-07-18
**Status:** PROPOSED

## Summary

Added 11 new built-in functions plus string indexing and string comparison operators to the compiler and runtime.

## Key Decisions

### Bitwise functions emit inline C (not runtime calls)
- `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot` are emitted as inline C expressions with `uint32_t` casts
- This avoids function call overhead and prevents C undefined behavior on signed integer shifts/complement
- These are pure functions (no side effects)

### String indexing returns i32 (byte value), strings are immutable
- `s[i]` returns the byte value as `i32`, not a character type
- Assignment to string index (`s[i] = val`) is a compile error
- Bounds checking via `osc_str_check_index()` runtime function

### String comparisons use lexicographic `osc_str_compare`
- Returns negative/zero/positive like `strcmp` but works on length-prefixed `osc_str`
- Comparison operators `<`, `>`, `<=`, `>=` emit `(osc_str_compare(a, b) <op> 0)`

### CLI args use global variables set by main wrapper
- `osc_global_argc`/`osc_global_argv` set in `main()` before arena creation
- `arg_get` returns `osc_str` (wraps argv pointer, no copy needed)
- Both are `fn!` (impure) since they access global state

## Impact

- **Morpheus:** Runtime extended with 8 new C functions + 2 globals. All implementations in `osc_runtime.c`.
- **Tank:** Can now write tests for bitwise ops, string indexing, string comparison, str_find/str_slice/str_from_i32, and CLI args.
- **No parser changes** — all features use existing syntax (function calls, indexing, comparison operators).
