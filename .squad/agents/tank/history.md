# Tank — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Test suite, compiler conformance testing, C sanitizers (ASan, UBSan)
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Tests cover: type system, scoping, mutability, control flow, error handling, FFI
- Negative tests: shadowing rejection, non-exhaustive match rejection, unhandled errors
- Generated C tested with GCC + Clang for portability
- Sanitizers verify zero UB in generated code
- Each requirement in the spec maps to at least one test case

## Learnings

### Phase 7A — Test Infrastructure (2025-07-14)
- Created full test suite: 20 positive tests, 14 negative tests, 20 expected output files
- Test runners: `run_tests.sh` (bash) and `run_tests.ps1` (PowerShell)
- Key directories: `tests/positive/`, `tests/negative/`, `tests/expected/`, `tests/build/`
- Every spec section §1-§10 has at least one positive and/or negative test
- Negative tests each target exactly ONE error for precise error-detection testing
- Coverage matrix documented in `tests/README.md`
- Expected outputs assume: `print_f64` prints IEEE 754 double repr, `print_i32`/`print_i64` print decimal
- FFI test (`ffi.bc`) declares `c_abs` — will need mapping to C stdlib `abs()` in generated code
- `order_independence.bc` validates two-pass name resolution (forward references)
- Purity tests: `purity.bc` (valid pure→pure chain) vs `purity_violation.bc` (pure calling fn!)
- Anti-shadowing tested across block boundaries, not across functions (per spec §6.2)
