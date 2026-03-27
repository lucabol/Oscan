# Morpheus — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
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
- **Files:** `runtime/bc_runtime.h`, `runtime/bc_runtime.c`, `runtime/test_runtime.c`, `runtime/Makefile`
- **Arena allocator:** Single-arena model with 8-byte alignment, doubling growth strategy. `bc_arena_create / alloc / reset / destroy`. Global arena pointer (`bc_global_arena`) for generated main() to set.
- **Checked arithmetic:** i32 uses widening to i64 for mul overflow check. i64 uses careful case analysis (no portable 128-bit in C99). All ops detect overflow BEFORE it happens.
- **Strings:** Immutable `bc_str` = `{const char* data, int32_t len}`. Literals are zero-copy wraps. Concat/to_cstr allocate on arena.
- **Arrays:** Generic via `void* + elem_size`. Bounds-checked get/set panic on OOB. Push doubles capacity via arena realloc.
- **Result type:** `BC_RESULT_DECL` macro generates tagged unions. `bc_result_str_str` is pre-declared for `read_line`.
- **Type casts:** f64→i32/i64 check NaN/Inf/range before cast. i64→i32 checks narrowing overflow. Widening casts are unconditional.
- **Panic handler:** `bc_panic(msg, file, line)` → stderr + `exit(1)`. `BC_PANIC(msg)` macro captures `__FILE__`/`__LINE__`.
- **Build:** C99, `-Wall -Wextra -Werror -pedantic -fsanitize=address,undefined`. Zero warnings on both GCC 13 and Clang 18.
- **Tests:** 76 assert-based tests, panic tests use `fork()` on POSIX (skipped on Windows). All passing.
