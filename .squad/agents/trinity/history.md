# Trinity — History

## Project Context
- **Project:** Oscan — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Compiler implementation (lexer, parser, AST, type checker, C code generator)
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Primary output: C99/C11 source code from Oscan input
- Grammar: context-free, single-pass parseable
- Type system: strict static, nominal, zero implicit coercion
- Anti-shadowing: same-name variables in nested scopes are compile errors
- Exhaustive branching: all enum/type match cases must be handled
- Error handling: Result-like types with forced unwrap/check
- Side-effect signatures on functions
- Order-independent definitions
- All generated C must guard against UB (bounds checks, overflow, null)

## Learnings
- **Phase 2 completed:** Full compiler infrastructure (lexer + parser + AST) implemented in Rust.
- **Project structure:** `src/main.rs` (CLI), `src/token.rs` (tokens), `src/lexer.rs` (lexer), `src/ast.rs` (AST nodes), `src/parser.rs` (recursive descent parser), `src/error.rs` (error types), `src/types.rs` (type representation), `src/semantic.rs` (semantic analysis), `src/codegen.rs` (C code generation).
- **Lexer design:** Single-pass character-by-character scanner. `fn!` handled as a single `FnBang` token by peeking after `fn`. All escape sequences (`\n`, `\t`, `\r`, `\\`, `\"`, `\0`) supported. Block comments non-nesting, line comments `//`.
- **Parser design:** Recursive descent with Pratt-style precedence for binary expressions. Two-pass approach — first pass collects struct/enum names so struct literals (`Name { ... }`) can be disambiguated from block expressions. Assignment detection uses lookahead scan past place expressions.
- **AST:** Complete coverage of all spec grammar constructs — functions (pure/impure), structs, enums, extern blocks, all statement types, all expression types including try/match/if-as-expression, all type annotations including Result<T,E> and arrays.
- **Operator precedence:** 7 levels implemented per spec (or < and < ==/!= < relational < additive < multiplicative < unary/not). Equality and relational are non-chaining (parsed as single optional right-hand side).
- **CLI:** `--dump-tokens`, `--dump-ast` flags for debugging, `-o output.c` for code generation. Reads `.osc` files.
- **Test coverage:** 17 lexer tests + 17 parser tests + 19 semantic tests = 53 total. All passing, zero warnings.
- **Key decision:** `#[allow(dead_code)]` on `ast.rs` and `error.rs` since AST span fields are used for error reporting but not all read by every pass.
- **Phase 3 completed:** Semantic analysis with two-pass approach (name collection + type checking). Covers: name resolution, anti-shadowing, type checking, purity enforcement, exhaustive match, try/Result propagation, mutability checks, cast validation.
- **Phase 4 completed:** C99 code generation targeting Morpheus's `osc_runtime.h`. All 18 micro-lib functions mapped. Arena parameter threading through all user-defined function calls. Checked arithmetic (`osc_add_i32` etc.), bounds-checked array access (`osc_array_get`), tagged unions for enums, `is_ok`-based Result types.
- **Type system design:** `BcType` enum represents all resolved types. Codegen uses a `type_of()` function to re-derive types from scope + symbol tables (no typed AST annotation needed).
- **Result type handling:** Result<T,E> generates BC_RESULT_DECL typedefs. Result::Ok/Err use compound literals. `try` generates early-return on error with compatible Result type. Type inference for Result constructors uses function return type or let-binding expected type.
- **Enum codegen:** Tagged unions with `int tag` + `union { struct { ... } VariantName; } data;`. Match on enums uses `switch(val.tag)`. Simple enums (no payloads) become `typedef int EnumName;`.
- **End-to-end verified:** hello_world, fibonacci, and error-handling spec examples all compile to correct C99 with gcc `-std=c99 -Wall` and produce expected output.
- **CI/CD workflow created:** `.github/workflows/ci.yml` — 4 parallel jobs (Linux/GCC, Windows/MSVC, macOS/Clang, ARM64/QEMU). Each runs `cargo test` (53 unit tests) + full integration suite (22 positive, 16 negative). Uses `ilammy/msvc-dev-cmd@v1` for MSVC env on Windows. Cargo caching enabled. ARM64/QEMU cross-compile job now active.
- **WSL + ARM local testing:** `test.ps1` extended with `-SkipWSL` and `-SkipARM` flags. WSL section compiles and runs positive tests inside WSL with gcc. ARM64 section cross-compiles with `aarch64-linux-gnu-gcc -static` and runs via `qemu-aarch64` in WSL. Both auto-detect availability and skip gracefully with install instructions. WSL path conversion: `C:\... → /mnt/c/...`. All WSL stderr piped through `ForEach-Object { "$_" }` per team decision.
- **Empty array literal elem_size bug fixed:** `emit_array_lit` in `codegen.rs` hardcoded `elem_size=1` for empty arrays (`bc_array_new(_arena, 1, 0)`), causing silent memory corruption for types larger than 1 byte. Fix: added `expected_array_elem_type: Option<BcType>` field to `CodeGenerator`. In `emit_stmt` for `Stmt::Let`, when the binding type is `BcType::Array(elem_ty)`, the element type is propagated before `emit_expr` so `emit_array_lit` can call `c_sizeof()` on it. Works for all element types (i32, i64, f64, bool, str, structs, enums, nested arrays). Verified with manual end-to-end tests and all 53 unit tests passing.
