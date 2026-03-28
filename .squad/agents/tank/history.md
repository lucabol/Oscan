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

### Mixed Allocation & Arena Corner-Case Tests (2025-07-14)
- Added 8 new positive tests covering mixed stack/arena allocation edge cases
- Test files: `mixed_alloc_struct_with_array.bc`, `mixed_alloc_struct_with_string.bc`, `mixed_alloc_nested_dynamic.bc`, `mixed_alloc_array_of_structs.bc`, `mixed_alloc_enum_dynamic.bc`, `mixed_alloc_return_dynamic.bc`, `arena_stress.bc`, `mixed_alloc_mutable_reassign.bc`
- **BUG FOUND:** Empty array literal `[]` generates `bc_array_new(_arena, 1, 0)` with hardcoded elem_size=1 regardless of declared type. Causes silent memory corruption when pushing i32/struct/etc. elements. Filed in `.squad/decisions/inbox/tank-allocation-bugs.md`.
- Existing `arrays.bc` test has a latent instance of this bug that passes by coincidence (single small value + zeroed arena memory)
- **Patterns that work correctly:** Structs with array fields (pre-initialized), nested struct field access (e.g. `out.inner.values[0]`), arrays of structs, enum variants with struct/string payloads, returning dynamic data from fn!, mutable struct reassignment with array fields, arena growth under stress (500 pushes, 50 string concats)
- **Purity constraint:** `push`, `str_concat`, `i32_to_str` are all impure — helper functions using these must be `fn!`, not `fn`
- `len` and `str_len` and `str_eq` are pure-safe
- All 46 tests pass (30 positive + 16 negative) after adding the 8 new tests

### Arena Growth Test & Critical Bug Discovery (2025-07-17)
- Created `arena_growth.bc`: 60K i32 pushes (14 array doublings, ~524KB arena usage), 1200 struct pushes with arena-allocated string fields, 4-phase data integrity verification
- Expected output: `arena_growth.expected` — verified matching on WSL/GCC
- **CRITICAL BUG FOUND:** Arena growth (>1MB) causes SEGFAULT. When `bc_arena_alloc` grows the buffer via malloc+memcpy+free, ALL existing pointers into the arena become dangling. On Linux/glibc, `free` on large allocations does `munmap` → immediate page fault. Confirmed with 150K pushes (exit code 139). Filed in `.squad/decisions/inbox/tank-arena-growth.md`.
- Root cause: `bc_array_push` holds `arr` (a `bc_array*` inside the arena). If `bc_arena_alloc` triggers arena growth during array doubling, `arr` points to freed memory. Both read (`arr->data` for memcpy source) and write (`arr->data = new_data`) are UB.
- Test adjusted to 60K elements to stay within 1MB arena so it passes, but the underlying bug is a time bomb for any program allocating >1MB
- Recommended fix: linked-list arena blocks (never free existing blocks) or offset-based allocation
- All 53 cargo tests pass with the new test added
