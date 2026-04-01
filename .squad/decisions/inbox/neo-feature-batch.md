# Feature Batch Prioritization & Doc Sync: String Interpolation First

**Author:** Neo (Lead Architect)  
**Date:** 2026-03-28  
**Status:** DECIDED  

---

## Executive Briefing

The research report decisively identifies **string interpolation** as the highest-leverage next feature. Current documentation has three blocking inconsistencies: (1) Guide still marks `+=` and `break`/`continue` as unsupported when they work, (2) Guide marks string interpolation as unsupported (correct but outdated for planning), and (3) README builtin count and category descriptions need refresh. 

**Decision:** 
1. **Doc Sync First** — Fix guide/spec/README to reflect actual language state (compound assign, break, continue exist and work)
2. **Implement String Interpolation** — Add `"...{expr}..."` syntax for clean output construction
3. **Test & Example Refresh** — Add interpolation tests; refresh examples that use string concat

---

## Phase 0: Doc Sync (24–48 hours)

### 3 Sync Tasks (High Priority)

#### Task 0.1: Guide Update — Compound Assignment & Control Flow
**Owner:** Neo (with Trinity review)  
**Scope:**
- Remove "No compound assignment" from Gotchas section
- Remove "No break/continue" from Gotchas section  
- Add subsection "Compound Assignment" under Variables with examples: `x += 1`, `y *= 2`
- Add subsection "Loop Control" under Control Flow with examples: `break` and `continue` in loops
- Verify against spec §5.7 precedence table and grammar

**Definition of Done:**
- Guide reflects actual language (all three features work)
- Examples are compilable and match test cases
- No contradictions with spec

---

#### Task 0.2: README Builtin Table & Count
**Owner:** Trinity (with Neo review)  
**Scope:**
- Audit builtin count: spec §10 shows ~156 functions; README claims "~156" (ok, but verify)
- Verify categories and counts match spec tables
- Add missing extended string functions if not listed
- Update status section if needed (currently says "v0.1+")
- Verify example list is accurate (currently ~17 CLI + 7 graphics = 24 total)

**Definition of Done:**
- Builtin table matches spec exactly or notes discrepancies
- No dangling references to missing or phantom functions
- Example list is complete and accurate

---

#### Task 0.3: Spec Refresh — Appendix B Future Features
**Owner:** Neo (as final reviewer)  
**Scope:**
- §1.1 Keywords table: verify 24 count (fn, fn!, let, mut, struct, enum, if, else, while, for, in, match, return, break, continue, try, use, extern, as, and, or, not, true, false = 24) ✓
- Appendix B: add string interpolation as **Phase 1 Committed Feature** (not just future consideration)
- Footnote the rationale: ergonomic bottleneck in output-heavy programs, existing infrastructure supports lowering
- Leave whole-file I/O and async I/O as Phase 2+ candidates

**Definition of Done:**
- Appendix B clarifies which features are committed for v0.2
- String interpolation is marked as Phase 1 (next after doc sync)
- No grammar changes yet (interpolation syntax still reserved for Phase 1)

---

## Phase 1: String Interpolation (Committed Next Feature)

### Feature Shape: Minimal Interpolation MVP

**Syntax:** `"text {expr} text"`  
**Scope:**
- Expressions in braces: `{x}`, `{x + 1}`, `{func(a, b)}`
- Automatic conversion for: `i32`, `i64`, `f64`, `bool`, `str`
- Nested braces not allowed (to avoid escape complexity)

**Implementation tasks:**

#### Task 1.1: Lexer & Parser (Trinity)
**Owner:** Trinity  
**Scope:**
- Extend lexer to handle interpolated strings: scan `"` → collect segments and `{...}` subexpressions
- Emit new token type: `StringInterp(Vec<StringSegment>)` where segment is either `Literal(str)` or `Expr(Expr)`
- Extend parser: `parse_string_lit()` → returns `Expr::StringInterp(Vec<InterpolatedPart>)`
- Add negative tests: malformed braces `"{"`, `"}"`, unclosed `"{expr"`, nested `"{{"`

**Definition of Done:**
- Lexer produces correct segments for test cases
- Parser builds `StringInterp` AST node
- Negative tests confirm error rejection
- All positive interpolation examples parse correctly

---

#### Task 1.2: Semantic Analysis (Trinity)
**Owner:** Trinity  
**Scope:**
- Type-check each interpolated expression
- Verify expressions have types in allowed set: `i32`, `i64`, `f64`, `bool`, `str`
- Reject unsupported types (structs, enums, unit, arrays, Result)
- Enforce immutability: expressions cannot call `fn!` or use mutable bindings

**Definition of Done:**
- Semantic checker validates interpolated expressions
- Negative tests catch unsupported types and `fn!` calls
- Error messages are clear

---

#### Task 1.3: Code Generation (Trinity)
**Owner:** Trinity  
**Scope:**
- Lower `StringInterp(parts)` to nested `osc_str_concat(_arena, ...)` calls
- Automatic lowering for each type:
  - `i32` / `i64` → `osc_str_from_i32(_arena, expr)` / `osc_str_from_i64(_arena, expr)`
  - `f64` → `osc_str_from_f64(_arena, expr)`
  - `bool` → `osc_str_from_bool(_arena, expr)`
  - `str` → pass through
- Generate readable C that matches manual concat pattern

**Definition of Done:**
- Code generator lowers interpolation to concat chains
- Generated C matches expected output
- All examples compile and run correctly

---

#### Task 1.4: Test Suite (Tank)
**Owner:** Tank  
**Scope:**
- **Positive tests:** interpolation with each type; nested function calls; mixed literals
- **Negative tests:** unsupported types; fn! in interpolation; malformed braces; unclosed expressions
- Examples refreshed: `env_info.osc`, `http_client.osc`, `web_server.osc` use interpolation where concat was verbose

**Definition of Done:**
- ≥8 positive integration tests
- ≥4 negative compiler-rejection tests  
- Examples compile and run; output matches expected
- All tests pass on 3 platforms (Windows, Linux, macOS)

---

#### Task 1.5: Documentation (Neo)
**Owner:** Neo (with Morpheus review)  
**Scope:**
- **Spec §5.5:** Add string interpolation subsection with EBNF grammar
- **Spec §10:** Update String (Core) table with interpolation note
- **Spec Appendix B:** Remove string interpolation from future; mark as implemented
- **Guide §2 Strings:** Add subsection "String Interpolation" with examples (i32, i64, f64, bool, str, nested calls)
- **Guide Gotchas:** Remove "No string interpolation" entry
- **README Quick Start:** Add interpolation example alongside hello.osc
- **README Builtins:** Verify string section matches spec

**Definition of Done:**
- All docs reflect interpolation as a shipping feature
- Examples compile and show readable output
- No references to "no interpolation" remain
- Spec grammar is unambiguous

---

## Ownership & Checkpoints

| Task | Owner | Duration | Blocker |
|------|-------|----------|---------|
| 0.1: Guide compound assign + control | Neo | 4h | None |
| 0.2: README builtin audit | Trinity | 4h | None |
| 0.3: Spec Appendix B commit | Neo | 2h | 0.1, 0.2 |
| 1.1: Lexer & Parser | Trinity | 8h | 0.3 |
| 1.2: Semantic | Trinity | 6h | 1.1 |
| 1.3: Codegen | Trinity | 8h | 1.2 |
| 1.4: Tests & examples | Tank | 12h | 1.3 |
| 1.5: Final docs | Neo | 4h | 1.4 |

**Reviewer Gates:**
- 0.1, 0.2: Neo gates doc sync
- 1.1, 1.2, 1.3: Neo gates compiler before Tank runs tests
- 1.4: Neo gates test results before doc finalization
- 1.5: Morpheus reviews runtime implications (none expected for v0)

---

## Rationale Summary

**Why string interpolation first:**
- Research report consensus: ergonomic bottleneck in output-heavy programs (CLI, HTTP, web server)
- Existing infrastructure: `str_concat`, `str_from_*` functions already implemented in runtime
- Compiler ready: AST, semantic, codegen already handle string expressions
- Fast payoff: improves ALL current examples that build formatted output
- LLM alignment: fewer nested parens, clearer intent, fewer manual conversion steps

**Why doc sync first:**
- Guide is out of date: marks working features (+=, break, continue) as unsupported
- README builtin list needs audit against spec
- Spec Appendix B needs to commit interpolation as next phase (not vague future)
- Clears cognitive load for Trinity to focus on compiler without doc noise

---

## Success Criteria

**At end of Phase 0 (Doc Sync):**
- ✅ Guide reflects all working features
- ✅ README builtin table matches spec
- ✅ Appendix B explicitly commits interpolation as Phase 1

**At end of Phase 1 (Interpolation):**
- ✅ `"Hello {name}, you are {age} years old"` works end-to-end
- ✅ All 4 primitive types + `str` convertible in interpolations
- ✅ Examples (env_info, http_client, web_server) use interpolation idiomatically
- ✅ All tests pass on 3 platforms
- ✅ Docs (spec, guide, README) are synchronized

---

## Next After Interpolation

The research report identified **whole-file I/O convenience** (`read_file`, `write_file`) as Phase 2, since current examples still contain byte-at-a-time boilerplate in file readers.

