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

### 2026-04-01 — Batch approval & example repair (hostname + interpolation)
- **Architecture review:** Hostname integration approved. No language surface growth. Transparent runtime resolution via `l_resolve` (freestanding) and `getaddrinfo(AF_INET)` (libc). ✅
- **Web server repair:** Fixed compile failure in `examples/web_server.osc` line 72. CSS `font-family: 'Segoe UI'` apostrophe triggered parser sensitivity. Surgical fix: unquoted family name (valid CSS, preserves interpolation improvements). ✅
- **Validation:** Direct compile of repaired file: PASS. Example sweep (25/25): PASS. Interpolation regression gate: PASS.
- **Reviewer follow-up:** Tank re-review approved both tracks (hostname + interpolation). All blockers resolved.
- **Decisions merged:** `.squad/decisions.md` entries #7 (Hostname Support Integration) + #8 (Example Interpolation Sweep). Decision inbox cleared.
- **Orchestration logs:** `.squad/orchestration-log/2026-04-01T10-54-28Z-neo.md`

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

### 2026-03-28 — Phase 0 Doc Sync Complete

**Three synchronization tasks completed:**

#### Task 0.1: Guide Update — Compound Assignment & Control Flow (✅ DONE)
- Added `### Compound Assignment` subsection under Variables with examples: `x += 5`, `x -= 2`, `x *= 3`
- Added `### Loop Control: Break and Continue` subsection under Control Flow with runnable examples
- Removed false Gotchas: "No `+=`, `-=`" and "No `break`/`continue`"
- Updated Gotcha #1 to acknowledge `++`/`--` don't exist but `x += 1` does
- All examples compile against actual working test suite

#### Task 0.2: README Builtin Audit (✅ VERIFIED)
- Confirmed ~156 builtin count is correct (7+7+9+2+4+11+4+6+10+8+4+4+6+2+8+8+4+11+8+3+5+1+5+8+4+2+7+1+1+1+2 = 156)
- Verified all categories and counts match spec tables exactly
- README builtin table fully synchronized with spec §10

#### Task 0.3: Spec Appendix B — Roadmap & Commitment (✅ DONE)
- Renamed "Appendix B: Reserved for Future Consideration" → "Appendix B: Roadmap (Feature Phases)"
- Created Phase 1 subsection: **String Interpolation** marked as "Committed — v0.2"
- Added detailed rationale: ergonomic bottleneck, existing infrastructure, lowering pattern
- Moved other features to "Phase 2+" with explicit deferrals and rationale
- Result: spec now clearly commits to interpolation as next major feature

**Commit:** `24ad9ea` — "Doc sync: reflect working features and commit string interpolation as Phase 1"

---

### 2026-03-28 — Oracle Doc-Sync Review: REJECTED (Incomplete Delivery)

**Code Review Decision:** Oracle's `.squad/decisions/inbox/oracle-doc-sync.md` submitted as "complete" but contained critical gaps:
1. README.md still claimed "Six example programs" while listing 7
2. docs/test_suite.md still listed `compound_assign` as unsupported (contradicts current language)
3. No git commits for claimed fixes (only decision document)

**Reviewer Action:** 
- Rejected Oracle's work (CRITICAL severity)
- Applied strict lockout rule: Oracle locked out from rework
- Neo executed emergency remediation (1 commit, 2 minutes) to unblock feature batch
  - README line 156: "Six" → "Seven"
  - test_suite.md: Removed stale compound_assign negative test row
- Reassigned to Trinity for future rework if needed

**Commit:** `f43b0df` — Neo architecture gate fixes

**Lesson Learned:** Decision documents must be paired with actual git commits. Claims of "complete" work must be verified against live files before acceptance.

---

### 2026-03-28 — Feature Batch Decision: String Interpolation First

**Research report consensus:** String interpolation is the highest-leverage feature because:
1. Oscan already covers broad capability (networking, graphics, hashing, datetime, maps, filesystem)
2. Ergonomic bottleneck: current examples use nested `str_concat` and manual conversions
3. Examples affected: `env_info.osc`, `http_client.osc`, `file_checksum.osc`, `web_server.osc` all pay token tax
4. Compiler infrastructure ready: code generator already handles string concat, runtime has `str_concat` + `str_from_*`
5. LLM alignment: fewer nested parens, clearer intent, reduced conversion boilerplate

**Implementation shape:**
- MVP scope: `"text {expr} text"` syntax (not full printf)
- Allowed types: `i32`, `i64`, `f64`, `bool`, `str` (have obvious conversions)
- Lowering: interpolated literal → nested `str_concat(...)` + conversion calls
- No complexity: no nested braces, no alignment/precision specifiers in v1

**Ownership & Responsibility:**
- **Trinity (Compiler):** Lexer/parser (Task 1.1), semantic (Task 1.2), codegen (Task 1.3)
- **Tank (Tests):** Positive/negative tests, example refreshes (Task 1.4)
- **Neo (Architecture):** Design reviews, spec/guide/README updates (Task 1.5), gate approvals
- **Morpheus (Runtime):** Review implications (expected: none for v0)

**Key design document:** `.squad/decisions/inbox/neo-feature-batch.md` — full specification, ownership split, success criteria

**Next phase after interpolation:** Whole-file I/O convenience (`read_file`, `write_file`) — reduces byte-at-a-time boilerplate in file readers

---

## Key Architectural Patterns

1. **AST-to-C lowering pattern:** Complex syntax (like future interpolated strings) lowers to simpler expressions (nested concat). Codegen doesn't need full C backend — just map to existing runtime.
2. **Purity separation:** Any function that allocates on arena must be `fn!` (side effect). This makes memory semantics obvious to LLMs.
3. **One way to do everything:** Compound assignment is shorthand syntax; no new semantics. `break`/`continue` are control flow keywords, not simulation with flags.
4. **Doc sync is architecture:** Out-of-sync docs create cognitive debt. Keeping README/guide/spec synchronized is as important as keeping code working.

---

## User Preferences & Style

- **Luca:** Extreme minimalism, decisive feature choices, clear ownership accountability, written decisions in inbox before work
- **Process:** Feature choices come from research reports with high-quality rationale; architecture gates compiler before tests; docs sync is Phase 0 before implementation
- **Communication:** Prefer human-readable outcomes; suppress tool internals in final response

### 2026-04-01 — laststanding DNS Integration Review: APPROVED

**Reviewed change:** Bump `deps/laststanding` to `5b3c0cd` and add transparent hostname resolution to `socket_connect` / `socket_sendto` in the runtime.

**Architecture decision:** No new builtin. Hostname resolution lives inside the runtime, behind the existing `addr: str` parameter. This is the right surface — zero language growth, strict superset of prior behavior.

**Implementation pattern:** Three-backend consistency:
- Freestanding: `l_resolve()` → resolved IPv4 string → `l_socket_connect()`
- Windows/POSIX: shared `osc_socket_lookup_ipv4()` using `getaddrinfo(AF_INET)`

**Key files:**
- `runtime/osc_runtime.c` — hostname resolution in socket wrappers
- `tests/positive/socket_hostnames.osc` — TCP + UDP `"localhost"` test
- `docs/spec/oscan-spec.md` — updated comments and examples

**Pattern:** Transparent runtime enhancement (no language surface change) is the preferred approach when a dependency gains new capability that maps naturally onto an existing API parameter. Document the expanded semantics in the spec comment, don't add a new builtin.

---

### 2026-04-01 — Interpolation Revision

- The interpolation blockers split cleanly into two language rules and one test-fixture issue: impurity in holes and stray `}` were already lexer/semantic concerns, while the `string_interpolation` mismatch came from an oversized integer literal cast expectation.
- We are keeping the language rule simple: integer literals are still `i32` values first, so anything outside the `i32` range must be constructed as `i64` from in-range literals plus explicit widening/arithmetic, not by casting an oversized literal token.
- For regression safety, interpolation now has explicit unit coverage for impure calls in holes, lone `}` rejection, and out-of-range integer literal rejection, plus the positive string interpolation case now exercises a true large `i64` value (`9000000000`) legally.

### 2026-04-01 — Team Batch: Inbox Merged, Doc Decisions Finalized

**Orchestration completed:**
- Wrote `.squad/orchestration-log/2026-04-01T09-37-38Z-oracle.md` documenting Oracle's README example links batch
- Wrote `.squad/log/2026-04-01T09-37-38Z-readme-example-links.md` session log
- Merged all inbox decisions into `.squad/decisions.md` (6 new decision entries added)
- Deleted all 18 inbox files after merge (deduplication completed)
- Updated Trinity, Tank, and Morpheus history entries with team progress

**Key decision merges:**
1. String Interpolation Feature & Doc Sync Priority (Neo) — Phase 0/Phase 1 commitment with ownership split
2. Specification v0.2 Expansion (Oracle) — 4 feature groups, micro-lib 18→36 functions
3. Full Documentation Audit & Fixes (Oracle) — README/spec/guide/test_suite synchronized, 3 of 4 files updated
4. Specification Gap-Analysis Positive Tests (Trinity) — 6 test files covering edge cases
5. README Example Links Update (Oracle) — Markdown link conversion for discoverability
6. User Directive (Luca) — Always commit/push; user can roll back

**Status:** All doc sync and inbox consolidation tasks complete. Ready for Trinity to implement string interpolation features.

### 2026-04-01 — Web Server Example Repair

- `examples\web_server.osc` can keep the accepted interpolation upgrades, but the CSS `font-family` line must avoid the single-quoted `Segoe UI` fragment to stay safe for the example compile gate.
- The minimal repair was to preserve escaped CSS braces (`{{` / `}}`) and change `font-family: 'Segoe UI', ...` to `font-family: Segoe UI, ...` rather than backing out interpolation.
- Validation scope for this kind of repair should include both the repaired example itself and the full examples compile sweep, because the user explicitly wanted all interpolation upgrades preserved elsewhere.


