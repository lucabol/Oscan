# Oracle — History

## Project Context
- **Project:** Oscan — LLM-optimized minimalist programming language transpiling to C99/C11
- **Tech Stack:** Rust compiler, C runtime, arena + stack hybrid memory model
- **User:** Luca Bolognese
- **Spec:** `docs/spec/oscan-spec.md` (1396 lines, comprehensive)
- **Requirements:** `../requirements.md` (original 80-line spec)
- **Key files:** src/codegen.rs, src/semantic.rs, src/parser.rs, src/lexer.rs, runtime/osc_runtime.h

## Core Context
- Oscan uses hybrid stack/arena allocation: value types on stack, dynamic types (arrays, strings) in arena
- Arena never frees until program exit — known limitation; `temp {}` block proposed but not implemented
- Empty array literal bug was found and fixed (elem_size was hardcoded to 1)
- All functions receive hidden `osc_arena* _arena` first parameter
- `fn!` denotes fallible functions (single FN_BANG token)
- Struct literal vs block ambiguity resolved by pre-scanning struct names
- 99 tests total: 53 unit + 30 positive integration + 16 negative integration

## Learnings
- **Spec audit completed (2025-07-15):** Found 9 inconsistencies, 7 gaps, 5 ambiguities. Full report in `.squad/decisions/inbox/oracle-spec-audit.md`.
- **Key purity issue:** str_concat and i32_to_str allocate on arena → must be fn!. Guide incorrectly marks them pure.
- **Guide has phantom functions:** i64_to_str, f64_to_str, str_to_i32 are in the guide but not in spec or compiler.
- **Grammar-parser divergence:** while/for trailing semicolons are optional in parser, absent from EBNF, "required" in guide.
- **try_expr EBNF is ambiguous:** postfix_expr greedily consumes call_suffix, conflicting with try_expr's own call syntax.
- **Compiler builtin registration:** semantic.rs lines 75-106 define all builtins; `false` = impure, `true` = pure in the `builtin()` helper.
- **Project rename:** Oscan → Oscan; all runtime symbols changed from `bc_` to `osc_` prefix.
- **Guide missing:** str_to_cstr, unit type, Result-returning main, recursive data patterns, negative literal patterns.
- **Spec fixes applied (2025-07-15):** Fixed 4 inconsistencies (IC-4/5/6/7), 4 ambiguities (AMB-1/2/3/4), and 2 gaps (GAP-4/7) in oscan-spec.md.
  - IC-5: `i32_to_str` changed to `fn!` (arena-allocating = impure).
  - IC-6: Heading changed to "Reserved Words (21 total)" with accurate note.
  - IC-7: `as` cast added to precedence table at prec 9; table renumbered 1–11.
  - IC-4: while/for grammar updated to `';'?`; grammar note documents if/match optional `;`.
  - AMB-1: `try_expr` grammar changed to `'try' IDENT ('.' IDENT)* '(' arg_list? ')'`.
  - AMB-2: Float division by zero now explicitly IEEE 754 (consistent with +, -, *).
  - AMB-3: Small struct threshold marked "implementation-defined."
  - AMB-4: Empty structs documented as permitted with nominal type semantics.
  - GAP-4: `literal_pattern` extended with optional leading `-` for negative numeric patterns.
  - GAP-7: `Result` documented as reserved type name in §1.1 and §3.3.
- **Verified totals post-fix:** 21 reserved words in table, 18 stdlib functions (6 pure, 12 impure), precedence table matches EBNF grammar.
- **Spec-to-test gap analysis completed (2025-07-15):** Mapped all 10 spec sections against 32 positive and 16 negative tests. Found 7 high-priority gaps with zero coverage: `Result` reserved name, `try` outside Result fn, assignment-as-expression, match arm type mismatch, 5 untested micro-lib functions (abs_i32, abs_f64, mod_i32, str_to_cstr, arena_reset isolated), negative literal patterns, and for-loop edge cases. Proposed 6 new positive + 4 new negative test files. Full report in `.squad/decisions/inbox/oracle-spec-gap-analysis.md`.
- **Test grouping strategy:** Minimized file count (10 total) since compilation is slow. Grouped by spec section: tokens/syntax (§1-2), types/casts (§3), declarations (§4), expressions (§5), scoping+errors (§6-7), micro-lib (§10). Four negative files target four distinct compile-error classes.
