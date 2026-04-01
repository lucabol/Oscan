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
- `fn!` denotes impure functions (single FN_BANG token)
- Struct literal vs block ambiguity resolved by pre-scanning struct names
- **85 tests total:** 53 unit + 65 positive integration + 20 negative integration (updated 2025-07-19)
- 139 compiler-registered builtins across 13+ tiers (spec documents 140+)
- 35 example programs: 21 CLI utilities + 14 graphics demonstrations

## Learnings
- **Full doc sync completed (2025-07-19):** Audited README.md, docs/spec/oscan-spec.md, docs/guide.md, docs/test_suite.md against current code/tests/examples.
  - **README fixes:** Updated builtin count (~156→~139), examples count (~17→21 CLI), test count (81→85)
  - **guide.md status:** Already correct (compound assignment documented properly)
  - **spec status:** Fully accurate; no implementation divergences found; 139 builtins match 13 tiers
  - **test_suite.md fixes:** Updated test counts (38→85 total; 22→65 positive; 16→20 negative)
  - **Full audit report:** `.squad/decisions/inbox/oracle-doc-sync.md`
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
- **Spec v0.2 expansion (2025-07-18):** Added 4 feature groups to support CLI samples (grep, sort, hexdump, base64). Micro-lib grew from 18 to 36 functions (18 + 6 bitwise + 3 string + 2 args + 7 file I/O). Changes: (1) 6 bitwise functions in §10.3 (band/bor/bxor/bshl/bshr/bnot); (2) 3 new string functions in §10.2 + string indexing §5.12.1 + str comparison operators in §5.2; (3) §10.7 command-line arguments (arg_count/arg_get); (4) §10.8 file I/O (7 functions + STDIN/STDOUT/STDERR constants). Maintained spec style throughout.
