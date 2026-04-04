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

### 2026-04-01 — README fact sync
- README fact patches should be verified from repo sources, not prior drafts: `docs/spec/oscan-spec.md` now states 24 reserved words, `examples\*.osc` contains 18 top-level programs, `examples\gfx\*.osc` contains 7 graphics programs, and the current test inventory is 62 unit / 74 positive / 26 negative.
- The CLI surface also includes `--libc` in `src\main.rs`, so user-facing command summaries should list it alongside `--run`, `--emit-c`, `--dump-ast`, and `--dump-tokens`.

### 2026-04-01 — Example interpolation sweep (APPROVED batch)
- **Task:** Apply string interpolation to example output formatting; reduce nested str_concat chains.
- **Scope:** 6 examples plus header/docs; skip examples where plain concat is clearer.
- **Brace escaping:** Any literal braces in strings escaped as `{{` / `}}` to avoid parser misinterpretation.
- **Files updated:** `env_info.osc`, `error_handling.osc`, `file_checksum.osc`, `http_client.osc`, `word_freq.osc`, `gfx/ui_demo.osc`, `web_server.osc`
- **Blocker resolved:** `web_server.osc` compile failure on CSS `font-family: 'Segoe UI'` — Neo applied surgical fix (unquoted font family).
- **Validation:** Initial 24/25 compile → Final 25/25 compile. Interpolation regression gate green.
- **Team batch approved:** Tank re-review APPROVED. Orchestration log in `.squad/orchestration-log/2026-04-01T10-54-28Z-trinity.md`. Decision merged to `.squad/decisions.md` entry #8 (Example Interpolation Sweep).

### DNS language stability (APPROVED batch)
- **Decision:** No language surface changes. Hostname resolution is runtime-internal expansion of existing `addr: str` parameter.
- **Trinity impact:** Examples/docs updated to reflect hostname capability; no compiler work required.
- **Rationale:** Keep language minimal. DNS support emerges naturally from runtime adaptation layer.
- **Decision merged:** `.squad/decisions.md` entry #7 (Hostname Support Integration).

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
- **Oracle gap-analysis positive tests added (6 files):** Created 6 test files from Oracle's spec-to-test gap analysis:
  1. `spec_tokens_syntax` — §1-2: escape sequences (`\\`, `\"`), `_`-prefixed identifiers, block comments, explicit `return`, trailing commas, empty array + len, negative literal patterns in match, match on string literals, zero-parameter function.
  2. `spec_types_casts` — §3: cast in arithmetic, cast precedence (`-5 as i64`), f64→i32 truncation, f64→i64 truncation, i32→f64 arithmetic, enum tag equality (== compares tags not payloads), empty struct construction, Result in let binding (via helper function), array of arrays access.
  3. `spec_declarations` — §4: zero-arg pure function, single-field struct, single-variant enum, 3-payload variant, field order independence in struct literal, trailing commas, forward function reference (calls function defined later).
  4. `spec_expressions` — §5: modulo on i64, unary minus on f64/i64, for-loop with variable range, zero-iteration for-loop (start==end), reverse range (start>end), match on f64 literal, deeply nested if-else-if, logical operator precedence (not/and/or combo), parenthesized expressions, complex arithmetic precedence.
  5. `spec_scoping_errors` — §6-7: pattern binding reuse across match arms (same `v` in different arms), inner block scope isolation, for-loop variable not visible after loop, nested 3-level blocks, top-level const used in function, chained try (3 operations), try error propagation.
  6. `spec_microlib` — §10: abs_i32 (negative/zero/positive), abs_f64 (negative/zero), mod_i32, str_to_cstr, str_len on empty, str_eq false case, str_concat with empty strings, i32_to_str (0 and negative), arena_reset + fresh allocation, len on fixed-size array, push 10 elements.
  All 6 compile and pass. Total positive tests: 38.
- **Compiler limitations found during testing:**
  - `return` statement: function body must still end with an expression for type inference; `return x;` as the last statement produces unit type. Workaround: use bare expression as tail, `return` only for early exits.
  - `Result::Ok(val)` in let binding: cannot construct Result directly in let binding position; compiler generates invalid C (void compound literal). Workaround: use a helper function that returns Result.
  - Forward struct reference in function signatures: `fn foo() -> LaterStruct` where `LaterStruct` is defined after the function fails with "undefined type". Forward references work for types used in main/other positions, just not function return type annotations.
  - `Result<T, CustomEnum>`: codegen emits `OSC_RESULT_DECL` before the enum typedef, causing C compilation error. Custom error enums in Result not usable.
  - Anti-shadowing is strict: pattern bindings in match arms shadow outer variables with same name, triggering error. Use different binding names when outer scope has conflicting names.
- **New built-in functions added (Phase 5):** 11 new built-in functions across 3 groups:
  - **Bitwise (6 pure):** `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot` — emitted as inline C with unsigned casts to prevent UB.
  - **String (3):** `str_find` (pure), `str_from_i32` (fn!), `str_slice` (fn!) — call runtime `osc_str_*` functions.
  - **CLI args (2 fn!):** `arg_count`, `arg_get` — call `osc_arg_count()`/`osc_arg_get()` using globals set by main wrapper.
- **String indexing (`s[i]`):** Extended `Expr::Index` and `element_type` to allow `BcType::Str`, returning `i32` (byte value). Immutability enforced — `PlaceAccessor::Index` on str is a compile error. Codegen emits bounds-checked byte access via `osc_str_check_index`.
- **String comparison operators:** `<`, `>`, `<=`, `>=` now work on `str` types. Codegen emits `osc_str_compare(a, b) <op> 0` calls.
- **Main wrapper updated:** `emit_main_wrapper` now stores `argc`/`argv` into `osc_global_argc`/`osc_global_argv` instead of discarding them. Static globals emitted at top of every generated C file.
- **Runtime extended:** Added 8 C functions to `osc_runtime.h`/`.c`: `osc_str_compare`, `osc_str_find`, `osc_str_from_i32`, `osc_str_slice`, `osc_str_check_index`, `osc_arg_count`, `osc_arg_get`, plus global `osc_global_argc`/`osc_global_argv`. All 53 unit tests pass.
- **String interpolation MVP shipped:** Added segmented interpolation tokens in `src/token.rs` / `src/lexer.rs`, `Expr::InterpolatedString` in `src/ast.rs`, parser support in `src/parser.rs`, semantic checks for allowed hole types + pure-only embedded expressions in `src/semantic.rs`, and lowering to existing string builders/conversions in `src/codegen.rs`.
- **Interpolation syntax rules:** `"...{expr}..."` supports `str`, `i32`, `i64`, `f64`, and `bool` holes; literal braces are written as `{{` / `}}`; a lone `}` inside a string literal is rejected to keep interpolation boundaries unambiguous.
- **Purity model for holes:** Embedded interpolation expressions are stricter than surrounding `fn!` code — they may call only pure functions. This keeps formatted-string construction free of hidden side effects and matches the language philosophy.
- **Validation updated:** Added/covered interpolation unit tests plus end-to-end positive/negative test programs. Verified with `cargo test` and `tests\run_tests.ps1 -Oscan ..\target\debug\oscan.exe` (73 positive, 26 negative, 75 freestanding checks passing).
- **Interpolation example discovery:** User-facing examples live under `examples\*.osc` with a short header comment and run instructions; README's `## CLI Examples` section is the right discoverability surface for new CLI examples like `examples\string_interpolation.osc`.
- **Example interpolation sweep:** After interpolation shipped, examples with human-readable formatting should prefer `"text {value}"` over nested `str_concat(...)` chains, and any literal braces in strings (especially HTML/CSS/JSON) must be escaped as `{{` and `}}` to keep examples compiling.

### 2026-04-01 — Team Batch: README Example Links & Doc Sync Completion

- **README example link conversion:** Oracle updated README.md examples sections to use direct markdown links instead of plain code-formatted filenames. Graphics and CLI example sections now link directly to actual `.osc` files in `xamples/` and `xamples/gfx/` directories. This improves discoverability via GitHub and static sites.
- **Specification v0.2 expansion merged:** Decisions merged from inbox: 4 feature groups (bitwise, string ops, command-line args, file I/O) expanding micro-lib from 18 to 36 functions. Trinity action items: register new builtins, implement string indexing and str comparison operators.
- **Doc sync decision finalized:** Neo/Trinity sync initiative identified as Phase 0 priority (compound assignment, break/continue doc updates, README refresh) before string interpolation Phase 1 implementation.
- **Full documentation audit completed:** Oracle completed comprehensive audit of README, spec, guide, test_suite documentation. 3 out of 4 files updated with current counts. Spec verified as 100% accurate vs compiler — no implementation divergences.
- **laststanding DNS integration status:** No new Oscan builtin was needed. Existing `socket_connect(addr: str, ...)` and `socket_sendto(..., addr: str, ...)` already accept hostnames because `runtime/osc_runtime.c` normalizes `addr` through `l_resolve` in freestanding mode and `getaddrinfo(AF_INET)` in libc mode before connecting/sending.
- **Hostname coverage added:** `tests\positive\socket_hostnames.osc` is the focused end-to-end regression for hostname-aware TCP/UDP loopback, and `examples\http_client.osc` is the user-facing sample that should describe the first argument as `<host>` rather than `<ip>`.

### 2026-04-01 — README Structural Refactoring Patch (Team Batch)
- **Trigger:** Neo identified 6 factual errors and 2 omissions in Oracle's README rewrite during review. Instead of waiting for Oracle revision, Trinity applied all corrections independently.
- **Corrections applied:**
  - Reserved word count: 21 → 24 (verified against src/lexer.rs:251–274)
  - Unit test count: 53 → 62 (verified against cargo test output)
  - Positive integration test count: 65 → 74 (verified against tests/positive/ file count)
  - Negative integration test count: 20 → 26 (verified against tests/negative/ file count)
  - Example count: 21 non-gfx → 18 (verified against examples/*.osc file count)
  - Added missing --libc CLI flag (verified in src/main.rs:71)
  - Added "Guarded C output" bullet (zero undefined behavior principle)
  - Added "Order-independent definitions" bullet (language design feature)
- **All corrections verified:** Every number cross-checked against live codebase sources before submission
- **Outcome:** Enabled Neo's immediate re-review approval (APPROVED status). README now factually accurate and structurally sound.
- **Decision merged:** .squad/decisions.md entry #10 (README.md Structural Refactoring) — consolidates review/rejection/approval cycle
- **Orchestration log:** .squad/orchestration-log/2026-04-01T11-50-20Z-trinity.md

### arena_reset() removal for memory safety
- **Task:** Surgically removed `arena_reset()` from the language surface to eliminate the only source of use-after-free.
- **Files changed (8):** `src/semantic.rs` (builtin registration), `src/codegen.rs` (codegen arm), `runtime/osc_runtime.h` (declaration), `runtime/osc_runtime.c` (function body), `tests/positive/spec_microlib.osc` + `tests/expected/spec_microlib.expected` (test + expected output), `docs/spec/oscan-spec.md` (§8.2, §8.3, §10), `docs/guide.md` (builtin table).
- **Preserved:** Internal `osc_arena_reset()` in runtime — still used for arena lifecycle management, just not exposed to Oscan programs.
- **Validation:** 62 unit tests pass, 74 positive + 26 negative integration tests pass, 25 examples compile. Pre-existing libc/spec_microlib printf formatting mismatch (`0` vs `0.0` for `abs_f64(0.0)`) is unrelated.
- **Key insight:** The `arena_reset()` builtin was the only way an Oscan program could trigger use-after-free. Removing it makes the language fully memory-safe at the surface level.

### defer statement implementation (full pipeline)
- **Task:** Implemented `defer` statement across all compiler phases: token, lexer, AST, parser, semantic analysis, and code generation.
- **Semantics:** `defer <call>;` registers a function call to execute when the enclosing function exits. Multiple defers execute in LIFO order. Only allowed in `fn!` (impure) functions. Deferred expression must be a function call.
- **Files changed (6 source):** `src/token.rs` (added `Defer` variant), `src/lexer.rs` (keyword match), `src/ast.rs` (DeferStmt struct + Stmt::Defer variant), `src/parser.rs` (parse_defer_stmt + statement dispatch), `src/semantic.rs` (purity check + call-only validation + interpolation purity walk), `src/codegen.rs` (deferred_exprs vec, LIFO emission at function end and before returns).
- **Codegen strategy:** Function-level defer collection. `deferred_exprs: Vec<String>` accumulates C code strings. At function end and before `return`, all defers are emitted in reverse order. For returns with values, the return expression is saved to a temp variable first, then defers run, then the temp is returned.
- **Tests added (3):** `tests/positive/defer_basic.osc` (LIFO ordering), `tests/positive/defer_return.osc` (defer + early return with value), `tests/negative/defer_pure.osc` (defer in pure fn rejected).
- **Validation:** 62 unit tests pass, 25/25 examples compile, both positive defer tests produce correct output, negative test correctly rejected.
- **Key insight:** The `saved_deferred = std::mem::take(&mut self.deferred_exprs)` pattern at function entry/exit prevents nested function definitions from leaking deferred expressions. The tail-expression return path also needs defer emission — the codegen saves the tail value to a temp, emits defers, then returns the temp.
