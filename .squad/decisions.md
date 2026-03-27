# Squad Decisions

## Active Decisions

### 1. Babel-C Language Specification v0.1

**Author:** Neo (Lead Architect)  
**Date:** 2025-07-14  
**Status:** ACTIVE  

#### Summary

Completed the full language specification for Babel-C v0.1. The spec is at `docs/spec/babel-c-spec.md` and serves as the sole reference for Trinity (compiler), Morpheus (runtime), and Tank (tests).

#### Key Decisions

**Memory Model → Arena-Based Allocation**
- Single implicit arena per program, bulk deallocation on exit
- LLMs never write memory management code
- `arena_reset()` exposed for advanced use in long-running programs
- Runtime provides `bc_arena` with create/alloc/reset/destroy

**Type System → Explicit, No Inference, No Generics**
- All bindings require type annotations (no inference)
- No user-defined generics; only built-in `Result<T, E>`
- Nominal typing for structs/enums, no subtyping
- 6 explicit cast pairs only, runtime-checked where narrowing

**Function Model → Pure/Impure Separation**
- `fn` = pure (no side effects, no I/O, no extern calls)
- `fn!` = side-effecting (may do anything)
- No closures, no first-class functions, no methods

**Error Handling → Result + try**
- `Result<T, E>` built-in, no way to access payload without match/try
- `try` for propagation (prefix on function call)
- Panics for programmer bugs (overflow, bounds, etc.)

**Syntax → 21 Reserved Words, LL(2) Grammar**
- Keywords: fn, fn!, let, mut, struct, enum, if, else, while, for, in, match, return, try, extern, as, and, or, not + true, false, _
- Logical operators as keywords (and/or/not)
- Trailing commas everywhere, mandatory braces, semicolons

**Standard Library → 18 Functions**
- 7 I/O, 4 string, 3 math, 2 array, 1 conversion, 1 memory

#### Impact

This spec enables parallel implementation by Trinity (compiler), Morpheus (runtime), and Tank (tests). No ambiguities should require clarification.

---

## Governance

- All meaningful changes require team consensus
- Document architectural decisions here
- Keep history focused on work, decisions focused on direction
