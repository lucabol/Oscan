# Tank — Interpolation MVP test verdict

## Decision

I added the interpolation MVP conformance suite now, even though the compiler does not implement interpolation yet. Keeping these tests active is intentional: they define the acceptance bar for the feature instead of letting an incomplete Trinity revision land silently.

## Coverage added

- **Positive:** `interpolation_i32`, `interpolation_i64`, `interpolation_f64`, `interpolation_bool`, `interpolation_str`, `interpolation_segments`, `interpolation_realistic`, `interpolation_nested`
- **Negative:** `interpolation_unsupported_struct`, `interpolation_unsupported_array`, `interpolation_impure_call`, `interpolation_unclosed_expr`, `interpolation_extra_closing_brace`

These cover:

- `str`, `i32`, `i64`, `f64`, and `bool` interpolation
- multi-segment interpolated strings
- escapes adjacent to interpolation (`\"`, `\\`, `\n`)
- realistic usage patterns (request/log formatting)
- nested pure expressions in interpolation
- rejection of unsupported embedded types
- rejection of malformed interpolation syntax
- rejection of impure calls inside interpolation

## Test oracle notes

- **`f64` interpolation should match `str_from_f64` formatting**, not `print_f64` formatting. Expected outputs are trimmed (`3.5`, `12.5`) rather than fixed six-decimal prints (`3.500000`, `12.500000`) because interpolation lowers through string conversion helpers.

## Reviewer stance

I would **reject** any interpolation-related revision that lands without all of the new positive tests passing and all negative tests being rejected.

## Current validation status

After rebuilding the current source tree and running the existing integration suite:

- **My 8 new positive tests pass**
- **3 of 5 new negative tests pass**
- **Still failing:** `interpolation_impure_call`, `interpolation_extra_closing_brace`

That means the MVP is close, but not releasable yet.

### Required fixes by owner

1. **Trinity** must implement lexer/parser/semantic/codegen support so:
   - `{expr}` inside string literals keeps working across the full supported surface
   - only `str`, `i32`, `i64`, `f64`, and `bool` are accepted
   - impure calls inside interpolation are rejected (`interpolation_impure_call`)
   - stray closing braces are rejected with clear diagnostics (`interpolation_extra_closing_brace`)

2. **Neo** must complete doc sync before calling the feature done:
   - `docs/spec/oscan-spec.md`
   - `docs/guide.md`
   - `README.md`

3. **Morpheus** does **not** need new runtime formatting machinery for MVP unless Trinity finds a missing conversion/runtime bug while wiring lowering.

## Expected follow-up

When Trinity lands interpolation, rerun:

```powershell
Set-Location tests
.\run_tests.ps1 -Oscan ..\target\release\oscan.exe
```

Release should stay blocked until the interpolation files above are green and the docs no longer claim interpolation is unsupported.

## Additional reviewer note

There is also a concurrent draft test already present in the workspace, `tests/positive/string_interpolation.osc`, that still fails because it expects `9000000000 as i64` to survive an out-of-range `i32` literal cast path. That case needs to be rewritten by its owner or tightened by Trinity with a proper literal-range diagnostic, but it is separate from the two MVP blockers above.
