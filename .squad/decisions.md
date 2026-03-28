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

### 2. Specification Audit & Fixes

**Author:** Oracle (Language Spec Specialist)  
**Date:** 2025-07-15  
**Status:** APPLIED  

#### Summary

Comprehensive audit of `docs/spec/babel-c-spec.md` identified 9 inconsistencies, 7 gaps, and 5 ambiguities across spec, guide, and compiler. Applied 10 fixes to establish spec as source of truth.

#### Key Fixes Applied

- **Purity:** `i32_to_str` changed to `fn!` (arena-allocating functions must be impure)
- **try_expr grammar:** Changed from greedy postfix_expr to restricted name paths
- **Float division:** IEEE 754 behavior for 1.0/0.0 → Inf (consistent with overflow handling)
- **Negative patterns:** Extended literal_pattern to allow `-` prefix for negative numeric matches
- **Reserved words:** `Result` documented as reserved type name (cannot define custom struct/enum Result)
- **Empty structs:** Now explicitly permitted with nominal type semantics
- **while/for semicolons:** Grammar updated to show optional trailing semicolons (`;`?)
- **as cast operator:** Added to precedence table at level 9

#### Guide Alignment

Neo applied 11 corresponding fixes to `docs/guide.md`:
- Split string functions (Pure: str_len/str_eq; Impure: str_concat/str_to_cstr)
- Removed phantom functions (i64_to_str, f64_to_str, str_to_i32 — do not exist)
- Split array functions (Pure: len; Impure: push)
- Fixed trailing semicolon guidance (optional, not required)
- Added unit type, str_to_cstr, recursive structures, parameter passing semantics

#### Impact

Spec and guide now aligned. Trinity should verify negative literal pattern support in parser; Tank should add test coverage.

---

### 3. Empty Array Literal Element Size Fix

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-17  
**Status:** APPLIED  

#### Problem

Empty array literals like `let mut arr: [i32] = []` generated `bc_array_new(_arena, 1, 0)` with hardcoded `elem_size=1`, causing silent memory corruption when elements ≥2 bytes were pushed.

#### Solution

Added `expected_array_elem_type: Option<BcType>` to CodeGenerator. Binding's resolved type is set before emit_expr and used by emit_array_lit to compute correct elem_size when element list is empty.

#### Impact

- Fixes silent memory corruption in empty array initializations
- No API/syntax changes — purely internal codegen fix
- All 53 unit tests pass

---

### 4. CI/CD Workflow Design

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-17  
**Status:** ACTIVE  

#### Summary

Created `.github/workflows/ci.yml` with three parallel platform jobs (Linux/GCC, Windows/MSVC, macOS/Clang) running on push to main and PRs. Includes optional ARM64/QEMU cross-compilation job.

#### Key Decisions

- **Three separate jobs** (not matrix) — each platform has different shell scripts and C compilers
- **Windows MSVC setup** via `ilammy/msvc-dev-cmd@v1` — required for `cl.exe` availability
- **Integration tests inline** in workflow YAML — clear per-step GitHub Actions output
- **Cargo caching** on all platforms

#### Impact

All team members get CI feedback on PRs. Tank's test files exercised on every push across three platforms.

---

### 5. WSL/QEMU Local Cross-Platform Testing

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-15  
**Status:** ACTIVE  

#### Summary

Enhanced `test.ps1` with WSL-based Linux testing and ARM64/QEMU testing. Graceful skip if tools unavailable.

#### Key Decisions

- **WSL tests only positive integration tests** — negative tests are compiler-rejection only (platform-independent)
- **ARM64 uses `-static` linking** — required for QEMU user-mode emulation
- **Auto-detection with helpful skip messages** — if WSL or ARM tools missing, skip with install instructions
- **New flags:** `-SkipWSL`, `-SkipARM` for selective testing

#### Impact

`test.ps1` now covers 3 platforms locally: native Windows, Linux/GCC (WSL), ARM64 (QEMU)

---

### 6. Semantic Analysis & Code Generation (Trinity Phase 3+4)

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-15  
**Status:** ACTIVE  

#### Summary

Implemented semantic analysis (Phase 3) and C code generation (Phase 4), completing the Babel-C compiler pipeline from `.bc` source to working C99 output.

#### Key Architecture Decisions

- **Type re-derivation in codegen:** No typed AST — code generator re-derives types using type_of() with symbol table access
- **Result<T,E>:** Uses runtime BC_RESULT_DECL macro; each unique combination gets a typedef
- **All arrays are dynamic:** Both fixed-size and dynamic Babel-C arrays represented as `bc_array*` in C
- **Anti-shadowing scope:** Within function only (parameters can shadow top-level constants)
- **Micro-lib mapping:** 18 functions hard-coded to bc_-prefixed C runtime counterparts

#### Impact

Full pipeline operational: `babelc input.bc -o output.c` produces valid C99 linking against `bc_runtime.c`. All spec examples work end-to-end.

---

## Governance

- All meaningful changes require team consensus
- Document architectural decisions here
- Keep history focused on work, decisions focused on direction
