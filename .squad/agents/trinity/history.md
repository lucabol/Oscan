# Trinity — History

## Project Context
- **Project:** Babel-C — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Compiler implementation (lexer, parser, AST, type checker, C code generator)
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Primary output: C99/C11 source code from Babel-C input
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
- **Project structure:** `src/main.rs` (CLI), `src/token.rs` (tokens), `src/lexer.rs` (lexer), `src/ast.rs` (AST nodes), `src/parser.rs` (recursive descent parser), `src/error.rs` (error types).
- **Lexer design:** Single-pass character-by-character scanner. `fn!` handled as a single `FnBang` token by peeking after `fn`. All escape sequences (`\n`, `\t`, `\r`, `\\`, `\"`, `\0`) supported. Block comments non-nesting, line comments `//`.
- **Parser design:** Recursive descent with Pratt-style precedence for binary expressions. Two-pass approach — first pass collects struct/enum names so struct literals (`Name { ... }`) can be disambiguated from block expressions. Assignment detection uses lookahead scan past place expressions.
- **AST:** Complete coverage of all spec grammar constructs — functions (pure/impure), structs, enums, extern blocks, all statement types, all expression types including try/match/if-as-expression, all type annotations including Result<T,E> and arrays.
- **Operator precedence:** 7 levels implemented per spec (or < and < ==/!= < relational < additive < multiplicative < unary/not). Equality and relational are non-chaining (parsed as single optional right-hand side).
- **CLI:** `--dump-tokens` and `--dump-ast` flags for debugging. Reads `.bc` files.
- **Test coverage:** 17 lexer tests + 17 parser tests = 34 total. All passing, zero warnings, zero clippy issues.
- **Key decision:** `#[allow(dead_code)]` on `ast.rs` and `error.rs` since these types are constructed but not yet consumed by downstream phases (type checker, codegen).
