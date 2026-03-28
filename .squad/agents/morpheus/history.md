# Morpheus — History

## Project Context
- **Project:** Oscan — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Runtime implementation in C, memory management, FFI, standard library
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Runtime is pure C, no external dependencies beyond libc
- Memory model must be deterministic and uniform (one approach for all allocation)
- Micro-lib provides only: basic I/O, math primitives, memory interfaces
- No domain-specific modules (no JSON, HTTP, etc.)
- C-FFI allows seamless external C function calls
- Error-as-value runtime support (Result-like composite types)
- UB guards: bounds checking, integer overflow detection, null pointer guards
- Runtime overhead must be minimal

## Learnings

### Phase 5: Runtime & Micro-Lib (completed)
- **Files:** `runtime/osc_runtime.h`, `runtime/osc_runtime.c`, `runtime/test_runtime.c`, `runtime/Makefile`
- **Arena allocator:** Single-arena model with 8-byte alignment, doubling growth strategy. `osc_arena_create / alloc / reset / destroy`. Global arena pointer (`osc_global_arena`) for generated main() to set.
- **Checked arithmetic:** i32 uses widening to i64 for mul overflow check. i64 uses careful case analysis (no portable 128-bit in C99). All ops detect overflow BEFORE it happens.
- **Strings:** Immutable `bc_str` = `{const char* data, int32_t len}`. Literals are zero-copy wraps. Concat/to_cstr allocate on arena.
- **Arrays:** Generic via `void* + elem_size`. Bounds-checked get/set panic on OOB. Push doubles capacity via arena realloc.
- **Result type:** `OSC_RESULT_DECL` macro generates tagged unions. `osc_result_str_str` is pre-declared for `read_line`.
- **Type casts:** f64→i32/i64 check NaN/Inf/range before cast. i64→i32 checks narrowing overflow. Widening casts are unconditional.
- **Panic handler:** `bc_panic(msg, file, line)` → stderr + `exit(1)`. `BC_PANIC(msg)` macro captures `__FILE__`/`__LINE__`.
- **Build:** C99, `-Wall -Wextra -Werror -pedantic -fsanitize=address,undefined`. Zero warnings on both GCC 13 and Clang 18.
- **Tests:** 78 assert-based tests (up from 76), panic tests use `fork()` on POSIX (skipped on Windows). All passing.
- **Files updated:** Runtime files renamed from `bc_*` to `osc_*`

### Arena Linked-List Fix (critical bug fix)
- **Bug:** Monolithic arena buffer growth (`malloc→memcpy→free`) invalidated ALL previously returned pointers. Any program allocating >1MB SEGFAULT'd because `osc_array_push` held dangling pointers after arena realloc.
- **Fix:** Replaced single growable buffer with linked list of fixed-size blocks (`osc_arena_block`). Blocks are NEVER freed or moved until `osc_arena_destroy`. New blocks are `max(block_size, requested)`.
- **Struct change:** `osc_arena` went from `{data, used, capacity}` to `{head, current, block_size}` with separate `osc_arena_block` type. Public header change but codegen only uses opaque API (`create/alloc/reset/destroy`), so no compiler changes needed.
- **`osc_arena_reset`:** Walks all blocks, sets `used=0`, resets `current` to `head`. Blocks kept allocated for reuse.
- **Key insight:** Codegen (codegen.rs) never accesses arena struct fields directly — it only calls the C API functions. This made the struct layout change safe.
- **Naming:** All runtime symbols renamed from `bc_` prefix to `osc_` prefix for consistency with project rename.
- **Tests added:** 2 new C tests (pointer validity after growth, multi-block reset) + `arena_stress_200k.osc` integration test (200K pushes, ~1.6MB, forces multiple blocks).
- **Verified:** 53 Rust tests, 78 C runtime tests, 48 integration tests (32 positive + 16 negative) — all passing. WSL/GCC 200K push test passes with exit code 0.
