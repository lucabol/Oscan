# Decision: `defer` Statement Implementation

**Author:** Trinity (Compiler Dev)
**Date:** 2025-07-18
**Status:** IMPLEMENTED

## Summary

Implemented the `defer` statement across the entire Oscan compiler pipeline (lexer → parser → AST → semantic analysis → code generation). The `defer` keyword registers a function call to execute when the enclosing function exits, with LIFO ordering for multiple defers.

## Design Decisions

### Function-level scope (not block-level)
Deferred expressions execute at function exit, not block exit. This matches Go's semantics and is much simpler to implement correctly. Block-level defer would require tracking block boundaries in codegen and handling complex nesting scenarios.

### Only in `fn!` functions
Since `defer` is inherently about side effects (cleanup actions), it is only permitted in impure functions (`fn!`). Using `defer` in a pure `fn` produces a compile error.

### Expression must be a function call
The deferred expression must be a `Call` expression. This prevents confusing patterns like `defer x + 1;` and ensures defers are used for their intended purpose: cleanup actions.

### Codegen strategy: string-based collection
The code generator collects C code strings for deferred expressions in a `Vec<String>`. At function end and before `return` statements, all collected strings are emitted in reverse order. For returns with values, the return expression is evaluated into a temp variable first, then defers execute, then the temp is returned. This avoids double-evaluation of the return expression.

### Save/restore across nested functions
`std::mem::take` is used to save and restore `deferred_exprs` around function emission, preventing nested function definitions from leaking deferred expressions into the outer function.

## Files Changed

| File | Change |
|------|--------|
| `src/token.rs` | Added `Defer` variant to `TokenKind` enum + Display impl |
| `src/lexer.rs` | Added `"defer"` keyword match |
| `src/ast.rs` | Added `DeferStmt` struct and `Stmt::Defer` variant |
| `src/parser.rs` | Added `parse_defer_stmt()`, updated `parse_stmt()` dispatch and `is_at_statement_start()` |
| `src/semantic.rs` | Added purity check, call-expression validation, interpolation purity walk for Defer |
| `src/codegen.rs` | Added `deferred_exprs` field, `emit_deferred_calls()`, `emit_deferred_before_return()`, updated `emit_function()` and `Stmt::Return` handling |
| `tests/positive/defer_basic.osc` | LIFO ordering test |
| `tests/positive/defer_return.osc` | Defer with early return test |
| `tests/expected/defer_basic.expected` | Expected output |
| `tests/expected/defer_return.expected` | Expected output |
| `tests/negative/defer_pure.osc` | Defer in pure fn rejection test |

## Validation

- 62 unit tests pass
- 25/25 examples compile
- Both positive defer tests produce correct output
- Negative test correctly rejected with clear error message

## Impact

- Adds 25th reserved keyword (`defer`) to the language
- Enables RAII-like cleanup patterns without manual try/finally
- No breaking changes to existing code
