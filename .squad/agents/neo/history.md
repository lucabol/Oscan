# Neo — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Language design, compiler/transpiler (parsing, AST, type checking, C code generation), C99/C11 output
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Babel-C is designed for LLMs as primary users, not humans
- Key principles: extreme minimalism, hallucination resistance, zero UB, one way to do everything
- Strict static typing, nominal types, no implicit coercion
- Immutable by default, explicit `mut` for mutation
- Errors as values (Result-like), forced handling, no exceptions
- Anti-shadowing mandate, strict lexical scoping
- Order-independent definitions (no forward declarations needed)
- Context-free grammar, single-pass parseable
- C-FFI for interoperability with existing C ecosystem

## Learnings

### 2025-07-14 — Phase 1: Language Specification Complete

**Key architecture decisions made:**
1. **Memory model: Arena-based allocation.** Single implicit arena, bulk deallocation. LLMs never write alloc/free. Rationale: deterministic, uniform, zero UB, simplest mental model.
2. **No type inference.** All bindings require explicit type annotations. LLM clarity over brevity.
3. **No generics except built-in `Result<T, E>`.** Keeps type system trivially simple for v0.1.
4. **`fn` vs `fn!` for purity.** Purity is syntactically visible; `fn` cannot call `fn!` or `extern`.
5. **`and`/`or`/`not` as keywords** (not `&&`/`||`/`!`). Better LLM tokenizer clarity.
6. **No methods, no OOP.** All operations are free functions. One calling convention.
7. **No `break`/`continue`.** Extreme minimalism — use boolean flags.
8. **No closures, no first-class functions.** Functions are not values.
9. **Mandatory explicit casts.** Only 6 permitted cast pairs, all runtime-checked where narrowing.
10. **Panics for programmer bugs** (overflow, bounds, division by zero). Errors-as-values (`Result`) for expected failures.

**Key file paths:**
- Language spec: `docs/spec/babel-c-spec.md`
- Requirements: `../requirements.md` (repo root parent)
- Runtime header (planned): `bc_runtime.h` / `bc_runtime.c`

**Patterns/preferences:**
- User (Luca) wants extreme minimalism — resist feature creep
- ≤20 keywords achieved (21 reserved words including true/false/_)
- Micro-lib kept to exactly 18 functions
- File extension: `.bc`
- Grammar is LL(2), recursive descent, no backtracking
