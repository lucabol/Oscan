# Interpolation Example Review

## When to use

Use this when reviewing a new Oscan example that is meant to showcase string interpolation.

## Validation pattern

1. Read team context first:
   - `.squad/agents/tank/history.md`
   - `.squad/decisions.md`
   - `.squad/identity/wisdom.md`
   - `.squad/identity/now.md`
2. Inspect the example source and verify it demonstrates:
   - supported interpolation types: `str`, `i32`, `i64`, `f64`, `bool`
   - at least one expression hole
   - pure helper calls inside interpolation, not `fn!`
   - escaped literal braces with `{{` / `}}`
3. Run the targeted example directly with the checked-in compiler binary:
   - prefer `target\debug\oscan.exe`
   - fallback to `target\release\oscan.exe`
   - command shape: `oscan examples\string_interpolation.osc --run`
4. Run the interpolation conformance suite:
   - positive: `tests\positive\*interpolation*.osc`
   - negative: `tests\negative\*interpolation*.osc`
5. Run `build-examples.ps1` to confirm the example is wired into the normal examples validation path.
6. Distinguish example-specific failures from unrelated existing example failures before approving or rejecting.

## Notes

- In this repo, `build-examples.ps1` is the established examples compile path.
- If `cargo` is unavailable, it is acceptable to validate with the existing `target\debug\oscan.exe` or `target\release\oscan.exe`.
- `f64` interpolation output should match string conversion formatting (`str_from_f64` style), not `print_f64`.
