# Oracle Spec Fixes — Applied Changes

**Author:** Oracle (Language Spec Specialist)  
**Date:** 2025-07-15  
**Status:** APPLIED  

## Summary

Fixed 10 issues in `docs/spec/babel-c-spec.md` from the audit report (oracle-spec-audit.md). All changes are spec-side only; no compiler or runtime code was modified.

## Decisions Made

### 1. `i32_to_str` is impure (`fn!`)

Any standard library function that returns `str` (arena-allocated) is `fn!`. This is consistent with `str_concat` and `str_to_cstr`. The compiler already implemented this correctly.

### 2. `try_expr` grammar uses restricted name path

Changed from `'try' postfix_expr '(' arg_list? ')'` to `'try' IDENT ('.' IDENT)* '(' arg_list? ')'`. This resolves the ambiguity where `postfix_expr` would greedily consume the call suffix. `try` now only applies to dotted name paths (e.g., `try foo(x)`, `try obj.method(x)`).

### 3. Float division by zero follows IEEE 754

`1.0 / 0.0` produces `Inf` per IEEE 754, consistent with how `+`, `-`, `*` handle float overflow. Only integer division by zero panics.

### 4. Negative literal patterns allowed in match

Grammar extended to `'-'? (INT_LIT | FLOAT_LIT)` in `literal_pattern`. Enables `match x { -1 => ..., _ => ... }`. The parser may need to be updated to support this if not already implemented.

### 5. `Result` is a reserved type name

Users cannot define `struct Result` or `enum Result`. Documented in §1.1 (reserved words note) and §3.3 (Result type section).

### 6. Empty structs are permitted

`struct Void {}` is grammatically valid and produces a distinct nominal type. Runtime size is implementation-defined.

### 7. Small struct threshold is implementation-defined

The spec no longer implies a specific threshold. The compiler decides stack vs. arena placement based on size; this is an optimization detail.

## Impact

- **Trinity (compiler):** Should verify negative literal patterns are supported in the parser. All other changes already match compiler behavior.
- **Tank (tests):** Should add test cases for: negative literal patterns, empty structs, `i32_to_str` called from `fn` (should fail), float division by zero producing Inf.
- **Neo (guide):** Guide fixes are happening in parallel; these spec changes establish the source of truth the guide should reference.
