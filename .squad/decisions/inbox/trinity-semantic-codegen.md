# Trinity: Phase 3+4 Semantic Analysis & Code Generation

**Author:** Trinity (Compiler Dev)
**Date:** 2025-07-15
**Status:** PROPOSED

## Summary

Implemented semantic analysis (Phase 3) and C code generation (Phase 4), completing the Babel-C compiler pipeline from `.bc` source to working C99 output.

## Key Decisions

### Type System: Re-derive types in codegen, no typed AST
Rather than annotating the AST with types or maintaining a span-indexed type map, the code generator re-derives types using a `type_of()` function with access to symbol tables. This avoids AST modification and keeps phases cleanly separated.

### Result<T,E>: Uses runtime BC_RESULT_DECL macro
Each unique `Result<T,E>` combination gets a `BC_RESULT_DECL` typedef (e.g., `bc_result_i32_str`). `Result::Ok/Err` use C99 compound literals. The `try` keyword generates early-return with compatible error propagation.

### All arrays use bc_array* (dynamic)
Both fixed-size and dynamic Babel-C arrays are represented as `bc_array*` in C for simplicity. This simplifies codegen at the cost of some performance for fixed-size arrays.

### Anti-shadowing: Within-function scope only
Anti-shadowing checks apply within a single function's scope chain. Function parameters can shadow top-level constants (matching the spec's "does NOT apply across different functions" rule).

### Micro-lib mapping: 18 functions hard-coded
All 18 micro-lib functions are mapped to their `bc_` prefixed C runtime counterparts. `len` and `push` are special-cased in both the type checker (for generic element types) and codegen.

## Impact

The full pipeline is now operational: `babelc input.bc -o output.c` produces valid C99 that compiles with `gcc -std=c99 -Wall` and links against `bc_runtime.c`. All three spec examples (hello world, fibonacci, error handling) work end-to-end.
