# Trinity interpolation MVP

## Summary
Implemented compiler-side string interpolation for `"...{expr}..."` literals.

## Decisions
- Lexer uses segmented interpolation tokens (`InterpStringStart` / `InterpStringMiddle` / `InterpStringEnd`) so parsing stays single-pass and nested braces inside interpolation expressions remain balanced.
- Literal braces inside interpolated strings use doubled braces: `{{` and `}}`. A lone `}` inside a string literal is rejected.
- Interpolation hole types are limited to `str`, `i32`, `i64`, `f64`, and `bool`.
- Interpolation holes must remain pure expressions: they may call `fn`, but not `fn!`, `extern`, `push`, or `pop`.
- Codegen lowers interpolation to existing string builders/conversions (`str_concat`, `i32_to_str`, `str_from_i64`, `str_from_f64`, `str_from_bool`) with no large formatter runtime.

## Impact
- Grammar stays context-free and single-pass parseable.
- Docs/spec/guide/readme now describe interpolation, purity, type limits, and brace escaping.
- Validation passed with `cargo test` and the full `tests\\run_tests.ps1` suite.
