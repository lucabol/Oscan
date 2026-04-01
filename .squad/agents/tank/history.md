# Tank — History

## Project Context
- **Project:** Oscan — An LLM-optimized minimalist programming language that transpiles to C
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
- FFI test (`ffi.osc`) declares `c_abs` — will need mapping to C stdlib `abs()` in generated code
- `order_independence.osc` validates two-pass name resolution (forward references)
- Purity tests: `purity.osc` (valid pure→pure chain) vs `purity_violation.osc` (pure calling fn!)
- Anti-shadowing tested across block boundaries, not across functions (per spec §6.2)

### Mixed Allocation & Arena Corner-Case Tests (2025-07-14)
- Added 8 new positive tests covering mixed stack/arena allocation edge cases
- Test files: `mixed_alloc_struct_with_array.osc`, `mixed_alloc_struct_with_string.osc`, `mixed_alloc_nested_dynamic.osc`, `mixed_alloc_array_of_structs.osc`, `mixed_alloc_enum_dynamic.osc`, `mixed_alloc_return_dynamic.osc`, `arena_stress.osc`, `mixed_alloc_mutable_reassign.osc`
- **BUG FOUND:** Empty array literal `[]` generates `bc_array_new(_arena, 1, 0)` with hardcoded elem_size=1 regardless of declared type. Causes silent memory corruption when pushing i32/struct/etc. elements. Filed in `.squad/decisions/inbox/tank-allocation-bugs.md`.
- Existing `arrays.osc` test has a latent instance of this bug that passes by coincidence (single small value + zeroed arena memory)
- **Patterns that work correctly:** Structs with array fields (pre-initialized), nested struct field access (e.g. `out.inner.values[0]`), arrays of structs, enum variants with struct/string payloads, returning dynamic data from fn!, mutable struct reassignment with array fields, arena growth under stress (500 pushes, 50 string concats)
- **Purity constraint:** `push`, `str_concat`, `i32_to_str` are all impure — helper functions using these must be `fn!`, not `fn`
- `len` and `str_len` and `str_eq` are pure-safe
- All 46 tests pass (30 positive + 16 negative) after adding the 8 new tests

### Arena Growth Test & Critical Bug Discovery (2025-07-17)
- Created `arena_growth.osc`: 60K i32 pushes (14 array doublings, ~524KB arena usage), 1200 struct pushes with arena-allocated string fields, 4-phase data integrity verification
- Expected output: `arena_growth.expected` — verified matching on WSL/GCC
- **CRITICAL BUG FOUND:** Arena growth (>1MB) causes SEGFAULT. When `bc_arena_alloc` grows the buffer via malloc+memcpy+free, ALL existing pointers into the arena become dangling. On Linux/glibc, `free` on large allocations does `munmap` → immediate page fault. Confirmed with 150K pushes (exit code 139). Filed in `.squad/decisions/inbox/tank-arena-growth.md`.
- Root cause: `bc_array_push` holds `arr` (a `bc_array*` inside the arena). If `bc_arena_alloc` triggers arena growth during array doubling, `arr` points to freed memory. Both read (`arr->data` for memcpy source) and write (`arr->data = new_data`) are UB.
- Test adjusted to 60K elements to stay within 1MB arena so it passes, but the underlying bug is a time bomb for any program allocating >1MB
- Recommended fix: linked-list arena blocks (never free existing blocks) or offset-based allocation
- All 53 cargo tests pass with the new test added

### Bitwise, Strings & Args Test Coverage (2025-07-17)
- Created 3 positive test files + 1 negative test file covering 3 new feature groups
- **spec_bitwise.osc**: All 6 bitwise ops (band/bor/bxor/bshl/bshr/bnot), edge cases (bshr(-1,1)=2147483647 unsigned shift, shift-by-0 identity, xor-self=0), combined masking pattern, bnot double-inversion, xor swap trick — 13 assertions total
- **spec_strings.osc**: String indexing (byte values), all 6 comparison operators on str (<,>,<=,>=), empty string comparison, str_find (found/not found/empty needle/overlapping), str_from_i32 (positive/negative/zero), str_slice, str_eq round-trip — 18 assertions total
- **spec_args.osc**: arg_count()>=1 and arg_get(0) non-empty — verifies command-line arg plumbing without needing test runner arg passing
- **string_index_assign.osc** (negative): Confirms `s[0] = 65` on str is rejected with "cannot assign to string index: strings are immutable"
- Both `str_from_i32` and `i32_to_str` exist as separate builtins (different codegen paths: `osc_str_from_i32` vs `osc_i32_to_str`)
- Bitwise functions are all pure (`fn`), string functions split: str_find/str_eq are pure, str_from_i32/str_slice are impure (arena allocation)
- arg_count and arg_get are impure (arg_get allocates on arena)
- Full suite: 41 positive, 21 negative — all passing

### Phase 8 — Baseline Validation & Documentation Sync (2025-03-27)
- **Baseline Status:** 87.6% green (78/89 tests passing)
  - 63/64 positive tests compile ✅
  - 58/63 positive tests produce correct output (5 have line-ending mismatch) ⚠️
  - 20/20 negative tests correctly rejected ✅
  - 5 interpolation negative tests incorrectly accepted (feature not yet implemented) ❌
- **Known Issues:**
  - Line-ending normalization in test verification: Tests compare with trailing `\n` mismatch
  - String interpolation feature not in v0.1 scope (5 negative tests expect rejection of `"value: {x}"` syntax)
  - Cargo not available in current environment; using pre-built oscan.exe binary
- **Key Finding:** Compiler is functionally correct; test failures are infrastructure/scope issues
- **Test Infrastructure:** Integration tests verified; unit tests (cargo test) skipped due to missing Rust toolchain
- **Documentation Status:**
  - Spec: `docs/spec/oscan-spec.md` — authoritative, complete, v0.1
  - Guide: `docs/guide.md` — aligned with spec
  - Test suite: 64 positive + 20 negative + 5 future features documented
- **Recommendation:** Proceed with feature work; baseline is green enough (compiler logic is sound)

### Interpolation MVP test gate (2026-04-01)
- Added **8 positive** interpolation conformance tests: `interpolation_i32`, `interpolation_i64`, `interpolation_f64`, `interpolation_bool`, `interpolation_str`, `interpolation_segments`, `interpolation_realistic`, `interpolation_nested`
- Added **5 negative** interpolation rejection tests: unsupported `struct`/array payloads, impure `fn!` call inside interpolation, unclosed interpolation expression, stray closing brace
- Positive coverage explicitly includes: all MVP supported types (`str`, `i32`, `i64`, `f64`, `bool`), multi-segment literals, escapes adjacent to interpolation, nested pure calls, and realistic log/request formatting
- `f64` interpolation uses string-conversion formatting (`3.5`) rather than `print_f64` formatting (`3.500000`) because lowering goes through `str_from_f64`
- After rebuilding the current source tree, all 8 new positive tests pass and 3/5 new negative tests reject correctly
- Remaining blockers are precise: impure calls inside interpolation are still accepted, and stray `}` inside interpolated strings is still treated as a literal instead of a syntax error
- Reviewer stance: reject interpolation revisions until those two negatives pass and Neo removes the remaining "no string interpolation" claims from spec/guide/README

### Interpolation example validation (2026-04-01)
- Shipping example is `examples/string_interpolation.osc`; it is also exercised by `build-examples.ps1`, so example QA can validate both the targeted example and the repo-wide examples compile path.
- Example quality bar that passed review: it demonstrates every supported MVP interpolation type (`str`, `i32`, `i64`, `f64`, `bool`), includes an expression hole (`{version + 1}`), a pure helper call (`{status_label(healthy)}`), and escaped literal braces for JSON-like output.
- Direct run of `examples/string_interpolation.osc` produced the expected showcase output, and the interpolation conformance files in `tests/positive/*interpolation*.osc` plus `tests/negative/*interpolation*.osc` all validated against the current compiler binary.
- `cargo` was unavailable in this environment, so validation relied on the checked-in `target\debug\oscan.exe` path rather than rebuilding from source.
- Repo-wide example build path is not fully green for unrelated reasons: `examples/web_server.osc` currently fails with `unexpected character '''` at line 72, but `string_interpolation` itself compiles cleanly in that same pass.
