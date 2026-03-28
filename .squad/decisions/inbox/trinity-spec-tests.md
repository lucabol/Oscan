# Decision: Spec Gap-Analysis Positive Tests

**Date:** 2025-07-15
**Author:** Trinity (Compiler Dev)
**Status:** Implemented

## Summary

Created 6 positive test files covering gaps identified in Oracle's spec-to-test gap analysis. All 6 compile and pass.

## Files Created

| File | Spec Sections | Key Features Tested |
|------|--------------|-------------------|
| `spec_tokens_syntax` | §1-2 | Escape sequences, block comments, `return`, trailing commas, negative match patterns, string match, zero-arg fn |
| `spec_types_casts` | §3 | Cast arithmetic, cast precedence, truncation, enum tag equality, empty struct, Result via helper, array of arrays |
| `spec_declarations` | §4 | Zero-arg fn, single-field struct, single-variant enum, 3-payload variant, field order independence, forward fn ref |
| `spec_expressions` | §5 | i64 modulo, unary minus (f64/i64), variable-range for-loop, zero-iter loops, f64 match, deep if-else-if, logical precedence |
| `spec_scoping_errors` | §6-7 | Pattern reuse across arms, block scope isolation, for-loop var gone after loop, 3-level nesting, chained try (3 ops) |
| `spec_microlib` | §10 | abs_i32, abs_f64, mod_i32, str_to_cstr, str_len empty, str_eq false, concat empty, i32_to_str, arena_reset, push 10 |

## Compiler Limitations Discovered

These are NOT bugs in the tests — they're genuine compiler limitations that required test workarounds:

1. **Result<T, CustomEnum>** — codegen emits result typedef before enum typedef. Custom error enums in Result are unusable.
2. **Result::Ok(val) in let binding** — generates invalid C. Must use helper function.
3. **Forward struct in fn signatures** — `fn foo() -> LaterStruct` fails if LaterStruct defined after the function.
4. **`return` as last statement** — makes function body type `unit`. Must use bare expression as tail.

## Team Impact

- Morpheus: No runtime changes needed
- Oracle: Gap analysis items §1-7 and §10 now covered for positive paths
- Neo: Compiler bugs #1-4 above are potential fix targets
