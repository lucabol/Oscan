# Decisions — Oscan Project

**Last Updated:** 2026-03-27T13:05

---

## Phase 1: Language Specification (Neo)

### Decision: Language Specification and Design

**Author:** Neo (Lead Architect)  
**Date:** Phase 1  
**Status:** Implemented  

#### Context
Needed to establish the foundational language design for Oscan v0.1.

#### Decision
- **Memory Model** → Arena-based allocation
  - Single implicit arena per program with bulk deallocation on exit
  - LLMs never write memory management code
  - `arena_reset()` exposed for long-running programs
  - Runtime provides `osc_arena` with create/alloc/reset/destroy

- **Type System** → Explicit, no inference, no generics
  - All bindings require type annotations
  - No user-defined generics; only built-in `Result<T, E>`
  - Nominal typing for structs/enums
  - 6 explicit cast pairs, runtime-checked where narrowing

- **Function Model** → Pure/Impure separation
  - `fn` = pure (no side effects, no I/O, no extern calls)
  - `fn!` = side-effecting (may do anything)
  - No closures, no first-class functions, no methods

- **Error Handling** → Result + try
  - `Result<T, E>` built-in
  - `try` for propagation
  - Panics for programmer bugs

- **Syntax** → 18 keywords, LL(2) grammar
  - Keywords: fn, fn!, let, mut, struct, enum, if, else, while, for, in, match, return, try, extern, as, and, or, not + true, false, _
  - Logical operators as keywords
  - Trailing commas everywhere, mandatory braces, semicolons

- **Standard Library** → 18 micro-lib functions
  - 7 I/O, 4 string, 3 math, 2 array, 1 conversion, 1 memory

#### Impact
Specification is authoritative and unambiguous, serving as the sole reference for all subsequent phases.

---

## Phase 2: Compiler Infrastructure (Trinity)

### Decision: Compiler Infrastructure Architecture

**Author:** Trinity (Compiler Dev)  
**Date:** Phase 2  
**Status:** Implemented  

#### Context
Needed to establish the foundational compiler pipeline for Oscan.

#### Decision
- **Language:** Rust with edition 2021.
- **Architecture:** Lexer → Token stream → Recursive descent parser → AST. Classic pipeline.
- **Struct literal ambiguity:** Resolved by a pre-scan pass that collects all struct/enum type names before parsing. When the parser sees `Ident {`, it checks if the identifier is a known type name to decide between struct literal and block expression.
- **Assignment detection:** Uses lookahead scanning past place expressions (`ident.field[index]...`) to find `=` before committing to assignment vs expression statement parsing.
- **`fn!` token:** Handled at the lexer level as a single `FnBang` token, not as `fn` + `!`. This follows the spec's directive that `fn!` is a single token.
- **Error handling:** All errors carry `Span` (line, column) for source location reporting. Single-error-and-stop strategy for now.

#### Impact
This establishes the foundation that the type checker and C code generator will build on. The AST types are the contract between phases.

---

## Phase 5: Runtime & Micro-Library (Morpheus)

### Decision: Runtime Architecture (Morpheus)

**Date:** 2025-07-15  
**Author:** Morpheus (Runtime Dev)  
**Status:** Implemented  

#### Context
Phase 5 required implementing the complete Oscan runtime as a C static library.

#### Decisions

**1. Arena growth via realloc-copy (not linked chunks)**
When the arena runs out of space, we allocate a new 2× buffer, copy, and free the old one. This keeps all arena memory contiguous, which simplifies pointer arithmetic and array relocation. Linked-chunk arenas would avoid copies but complicate `osc_array_push` (data might span chunks).

**2. 8-byte alignment for all arena allocations**
Every `osc_arena_alloc` rounds up to 8-byte boundaries. This satisfies alignment requirements for all Oscan types (i64, f64, pointers) without per-type alignment logic.

**3. Result type via macro, not void***
`OSC_RESULT_DECL(ok_type, err_type, name)` generates a concrete tagged union struct. The compiler will emit one `OSC_RESULT_DECL` per distinct `Result<T, E>` instantiation. This gives type safety and avoids casts.

**4. i32 overflow detection via i64 widening**
`osc_mul_i32` widens operands to i64 before multiplying, then checks if the result fits i32. Simple, correct, zero UB. For i64 mul, we use careful case-analysis since C99 has no portable 128-bit integer.

**5. Panic tests via fork()**
Test suite uses `fork()` to test that panics actually `exit(1)`. On Windows (no fork), these tests are skipped. This is acceptable since CI will run on Linux.

#### Impact
- Compiler codegen (Trinity) should target these exact function signatures.
- Generated `main()` must create `osc_global_arena` and destroy it on exit.
- All allocating micro-lib functions receive the arena as a hidden first parameter.

---

## Phase 7A: Test Infrastructure (Tank)

### Decision: Test Infrastructure Conventions

**Author:** Tank (Tester/QA)  
**Date:** 2025-07-14  
**Status:** Implemented  

#### Context
Created the Phase 7A test suite. Establishing conventions for how tests are organized and run.

#### Decisions

1. **Test naming**: `.osc` filename matches `.expected` filename (e.g., `fibonacci.osc` → `fibonacci.expected`).
2. **Negative tests use exit code**: The test runner checks only that the compiler returns non-zero for negative tests. We don't yet assert specific error messages (can add later).
3. **Expected output files**: Use exact byte-level comparison (no regex). This means runtime output formatting must be deterministic.
4. **FFI test assumes**: `c_abs` maps to C `abs()`. The compiler or a shim must handle this name mapping.
5. **Build artifacts go in `tests/build/`**: This directory is gitignored (via `.gitkeep`). Test runners create it if missing.

#### Impact
All team members writing the compiler/runtime should ensure their output format matches the `.expected` files exactly.

---

## Decision Archive

All major architectural and implementation decisions for Oscan v0.1 are documented above. This file serves as the authoritative decision log for the project.

Last decision entry: 2026-03-27T13:05 (Phases 2, 5, 7A complete)
