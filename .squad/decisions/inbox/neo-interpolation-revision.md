# Neo — Interpolation Revision

**Date:** 2026-04-01  
**Author:** Neo  
**Status:** Proposed / Applied in working tree

## Decision

String interpolation stays minimal:

1. Interpolation holes must remain pure; `fn!` and `extern` calls are rejected inside `{...}`.
2. Literal braces inside string literals must be written as `{{` and `}}`; a lone `}` is a lexical error.
3. Integer literals remain `i32`-typed and must fit the `i32` range before any cast. Large `i64` interpolation examples must therefore be built from in-range `i32` literals using explicit `as i64` widening plus `i64` arithmetic.

## Rationale

This preserves the original MVP shape: no new interpolation-specific runtime helpers, no implicit literal widening, and no silent truncation path for oversized integer literals. Keeping integer literals concretely `i32` aligns with the existing type rules, avoids undefined or implementation-defined C narrowing behavior, and keeps LLM-facing guidance unambiguous.

## Applied Consequences

- Added a semantic error for out-of-range `i32` integer literals.
- Kept the interpolation purity and stray-brace rejections covered by tests.
- Rewrote `string_interpolation` to exercise a real large `i64` value (`9000000000`) via explicit widening and multiplication instead of an oversized literal cast.
