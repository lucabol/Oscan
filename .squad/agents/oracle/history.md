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

### 2026-04-01 — Hostname documentation alignment (APPROVED batch)
- **Task:** Update user-facing documentation to reflect approved hostname support.
- **Trigger:** Tank QA approved hostname integration (socket_hostnames.osc regression green).
- **README.md (line 202):** Old wording removed conservative "until hostname QA is green" caveat. Now: "TCP sockets with hostname support".
- **examples/http_client.osc:** Header and usage updated to reflect hostname capability. Parameter changed from `<ip>` to `<hostname|ip>`. Example changed from numeric IPv4 (93.184.216.34) to practical hostname (example.com).
- **Rationale:** Hostname support is implemented, tested, approved. Spec already documents behavior. User docs now align with reality.
- **Decision merged:** `.squad/decisions.md` entry #9 (User-Facing Documentation Alignment: Hostname Support)
- **Orchestration log:** `.squad/orchestration-log/2026-04-01T10-54-28Z-oracle.md`

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

## Learnings

### Session: README Examples Links (Luca Bolognese Request)
- **Task**: Updated README.md examples sections to use markdown links instead of plain code-formatted filenames
- **Changes**: 
  - Graphics examples in "### Examples" section: Converted code formatting to markdown links pointing to xamples/gfx/
  - CLI examples in "## CLI Examples" section: Converted all 18 examples from code formatting to markdown links pointing to xamples/
  - All descriptions preserved unchanged
- **Pattern**: When documenting code examples in user-facing docs, markdown links to actual files are more discoverable than plain text references
- **Links format**: Used relative paths from root (xamples/filename.osc and xamples/gfx/filename.osc) to enable GitHub and static site viewing

### Session: Hostname Support Documentation (Luca Bolognese Request)
- **Task**: Updated README.md and examples/http_client.osc to reflect approved hostname support (previously marked as "under QA")
- **Changes**:
  - README.md line 202: Changed http_client description from "TCP sockets; use IPv4 literals until hostname QA is green" to "TCP sockets with hostname support"
  - examples/http_client.osc: Updated header comments and usage examples to reflect hostname support; changed from IPv4-only literal (93.184.216.34) to hostname (example.com); parameter descriptions updated to show <hostname|ip> instead of just <ip>
- **Spec Status**: Confirmed docs/spec/oscan-spec.md already correctly documents hostname support in socket_connect and socket_sendto (spec parameter shows ddr: str with note "IPv4 address or hostname")
- **Tests**: Verified tests/README.md already references socket_hostnames.osc test for hostname regression coverage; no updates needed

### Session: README Structural Refactoring (Luca Bolognese Request)
- **Task**: Rethink and rewrite README.md as front door for the project — stronger positioning, better flow, curated examples instead of reference-heavy inventory.
- **Changes**:
   - **Opening:** Reduced from 12 feature bullets to 3-sentence hook
   - **Language Highlights:** Distilled to 6 scannable bullet points with clear rationale
   - **Quick Look:** Single coherent Fibonacci example with inline pattern glossary
   - **Install & Build:** Streamlined to 4 lines + one-command build
   - **Getting Started:** Hello World + 3 essential commands
   - **What You Can Build:** Intent-driven use cases (CLI, network, graphics, data)
   - **Examples:** Reorganized 25 examples into 3 groups (CLI utilities, network, graphics/games)
   - **Learn More:** Three deep links replacing verbose explanations
   - **Testing:** Practical commands + verified test counts
   - **Project Structure:** One-line summary table replacing 30-line listing
   - **Removed:** "Why Oscan?" section, string interpolation deep-dive, 139-function table, freestanding runtime explanation
- **Accuracy Verified:**
   - All example links checked and confirmed (25 files exist)
   - Test count (85: 53 unit + 65 positive + 20 negative) verified
   - Platform count (4: Windows/Linux/macOS/ARM64) confirmed
   - Feature claims (21 reserved words, ~130 builtins) match spec
- **Metrics:** Original ~1400 lines → Refactored ~167 lines (88% reduction)
- **Decision document:** .squad/decisions/inbox/oracle-readme-rethink.md captures full rationale

### 2026-04-01 — README Structural Refactoring APPROVED (Team Batch)
- **Status:** APPROVED by Neo (Lead Architect) after Trinity's corrective patch
- **Initial outcome:** Submitted README rewrite for review; Neo identified 6 factual errors + 2 omissions
  - Factual errors: reserved word count (21→24), test counts (53→62, 65→74, 20→26), example count (21→18), missing --libc flag
  - Omissions: "zero undefined behavior" and "order-independent definitions" absent from highlights
- **Trinity corrective action:** Independently patched all 8 issues against live codebase
- **Re-review outcome:** All corrections verified by Neo against source files. README now factually accurate and structurally sound.
- **Decision merged:** .squad/decisions.md entry #10 (README.md Structural Refactoring) — consolidated review/rereview records
- **Orchestration log:** .squad/orchestration-log/2026-04-01T11-50-20Z-oracle.md
- **Session log:** .squad/log/2026-04-01T11-50-20Z-readme-rethink.md
- **Inbox cleanup:** All three decision inbox files merged and deleted post-approval
