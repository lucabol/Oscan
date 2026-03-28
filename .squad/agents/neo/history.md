# Neo — History

## Project Context
- **Project:** Oscan — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Language design, compiler/transpiler (parsing, AST, type checking, C code generation), C99/C11 output
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Oscan is designed for LLMs as primary users, not humans
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
- Language spec: `docs/spec/oscan-spec.md`
- Requirements: `../requirements.md` (repo root parent)
- Runtime header (planned): `bc_runtime.h` / `bc_runtime.c`

**Patterns/preferences:**
- User (Luca) wants extreme minimalism — resist feature creep
- ≤20 keywords achieved (21 reserved words including true/false/_)
- Micro-lib kept to exactly 18 functions
- File extension: `.osc`
- Grammar is LL(2), recursive descent, no backtracking

### 2025-07-25 — README Rewrite & Language Guide

**What I learned from reading the test suite (22 positive, 16 negative tests):**

1. **Dynamic arrays are real.** `[i32]` (no size) creates a dynamic array; `push()` and `len()` work. Empty dynamic arrays `[]` are valid.
2. **Block expressions are fully supported.** `let x: i32 = { ... };` works; nested blocks return values.
3. **Top-level `let` bindings work as constants.** Including computed expressions like `let COMPUTED: i32 = 6 * 7;`.
4. **Order independence is complete.** main can reference functions, structs, enums, and constants defined later in the file.
5. **Enum variants can carry multi-field payloads.** e.g. `Rectangle(f64, f64)`, `Custom(i32, i32, i32)`.
6. **Match works on scalars too.** Not just enums — match on `i32`, `bool`, and `str` with `_` wildcard.
7. **Struct field mutation requires `let mut`.** `let mut c: Counter = ...; c.value = 42;` is the pattern.
8. **Negative tests confirm strict enforcement.** Shadowing, purity violations, compound assignment (`+=`), implicit coercion, non-exhaustive match, and unhandled Result all produce compile errors.
9. **FFI is practical.** Multiple `extern` blocks work; `sqrt`, `pow`, `floor`, `ceil`, `abs` all callable. Expressions can compose extern calls: `sqrt(pow(3.0, 2.0) + pow(4.0, 2.0))`.
10. **CLI defaults to exe compilation.** `oscan input.osc` → `input.exe`. The `-o foo.c` extension triggers transpile-only mode. `--emit-c` sends C to stdout.

**Files written:**
- `README.md` — complete rewrite (112 lines, accurate CLI, no Morpheus quote, no credits, no roadmap phases)
- `docs/guide.md` — concise language guide (341 lines, organized by topic with code examples)

### 2025-07-25 — Guide Fix: Oracle's Audit Report

**Inconsistencies fixed (IC):**
- **IC-1 (str_concat purity):** Split String functions into Pure (`str_len`, `str_eq`) and Impure (`str_concat`, `str_to_cstr`) sections to reflect that both allocate on the arena.
- **IC-2 (phantom conversion functions):** Removed non-existent functions `i64_to_str`, `f64_to_str`, and `str_to_i32` from the guide. Only `i32_to_str` exists, now marked as Impure.
- **IC-3 (Array purity header):** Split Array functions into Pure (`len`) and Impure (`push`) sections instead of claiming all are impure.
- **IC-4 (semicolon requirement):** Changed Gotcha #8 from "required" to "optional" — parser accepts trailing semicolons after control flow but doesn't require them.
- **IC-9 (% type restriction):** Added "(integers only)" note to remainder operator in the Operators section.

**Gaps filled (GAP):**
- **GAP-1 (str_to_cstr):** Added `str_to_cstr` to String Functions (Impure) table for C FFI interop.
- **GAP-2 (unit type):** Added `unit` type to the Primitives table with literal example `()`.
- **GAP-3 (main Result):** Updated main rules to note it can return `unit` (implicitly) or `Result<unit, str>` (explicit error handling).
- **GAP-5 (recursive structures):** Added "Recursive Data Structures" subsection under Enums explaining the dynamic array indirection pattern.
- **GAP-6 (parameter passing):** Added "Parameters and Passing Semantics" subsection explaining pass-by-value immutability and the absence of `mut` parameters.

**Ambiguities clarified (AMB):**
- **AMB-5 (struct field order):** Added note to Structs section: "Field order in struct literals does not need to match declaration order."

**Files modified:**
- `docs/guide.md` — fixed all 9 issues; guide now aligns with spec and compiler on purity, available functions, and semantics.

### 2026-03-28 — Documentation & Decisions Archive

**Orchestration:**
- Wrote `.squad/orchestration-log/2026-03-28T0520-oracle.md` documenting Oracle's spec audit and fixes
- Wrote `.squad/orchestration-log/2026-03-28T0520-neo.md` documenting Neo's guide alignment work
- Merged all inbox decisions into `.squad/decisions.md` (entries 2–6)
- Archived spec/guide alignment as complete ("Specification Audit & Fixes" decision)

