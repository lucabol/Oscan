# Squad Decisions

## Active Decisions

### 1. Oscan Language Specification v0.1

**Author:** Neo (Lead Architect)  
**Date:** 2025-07-14  
**Status:** ACTIVE  

#### Summary

Completed the full language specification for Oscan v0.1. The spec is at `docs/spec/oscan-spec.md` and serves as the sole reference for Trinity (compiler), Morpheus (runtime), and Tank (tests).

#### Key Decisions

**Memory Model → Arena-Based Allocation**
- Single implicit arena per program, bulk deallocation on exit
- LLMs never write memory management code
- `arena_reset()` exposed for advanced use in long-running programs
- Runtime provides `osc_arena` with create/alloc/reset/destroy

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

Comprehensive audit of `docs/spec/oscan-spec.md` identified 9 inconsistencies, 7 gaps, and 5 ambiguities across spec, guide, and compiler. Applied 10 fixes to establish spec as source of truth.

#### Key Fixes Applied

- **Purity:** `i32_to_str` changed to `fn!` (arena-allocating functions must be impure)
- **try_expr grammar:** Changed from greedy postfix_expr to restricted name paths
- **Float division:** IEEE 754 behavior for 1.0/0.0 → Inf (consistent with overflow handling)
- **Negative patterns:** Extended literal_pattern to allow `-` prefix for negative numeric matches
- **Reserved words:** `Result` documented as reserved type name (cannot define custom struct/enum Result)
- **Empty structs:** Now explicitly permitted with nominal type semantics
- **while/for semicolons:** Grammar updated to show optional trailing semicolons (`;`?)
- **as cast operator:** Added to precedence table at level 9
- **Project rename:** Oscan → Oscan; runtime symbols changed from `bc_` to `osc_` prefix

#### Guide Alignment

Neo applied 11 corresponding fixes to `docs/guide.md`:
- Split string functions (Pure: str_len/str_eq; Impure: str_concat/str_to_cstr)
- Removed phantom functions (i64_to_str, f64_to_str, str_to_i32 — do not exist)
- Split array functions (Pure: len; Impure: push)
- Fixed trailing semicolon guidance (optional, not required)
- Added unit type, str_to_cstr, recursive structures, parameter passing semantics

#### Impact

Spec and guide now aligned. Neo should verify negative literal pattern support in parser; Tank should add test coverage.

---

### 3. Empty Array Literal Element Size Fix

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-17  
**Status:** APPLIED  

#### Problem

Empty array literals like `let mut arr: [i32] = []` generated `osc_array_new(_arena, 1, 0)` with hardcoded `elem_size=1`, causing silent memory corruption when elements ≥2 bytes were pushed.

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
- **Project context:** Oscan (formerly Oscan) — references updated accordingly

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

Implemented semantic analysis (Phase 3) and C code generation (Phase 4), completing the Oscan compiler pipeline from `.osc` source to working C99 output.

#### Key Architecture Decisions

- **Type re-derivation in codegen:** No typed AST — code generator re-derives types using type_of() with symbol table access
- **Result<T,E>:** Uses runtime BC_RESULT_DECL macro; each unique combination gets a typedef
- **All arrays are dynamic:** Both fixed-size and dynamic Oscan arrays represented as `osc_array*` in C
- **Anti-shadowing scope:** Within function only (parameters can shadow top-level constants)
- **Micro-lib mapping:** 18 functions hard-coded to bc_-prefixed C runtime counterparts

#### Impact

Full pipeline operational: `oscan input.osc -o output.c` produces valid C99 linking against `osc_runtime.c`. All spec examples work end-to-end.

---

### 7. Hostname Support Integration via laststanding DNS

**Authors:** Morpheus, Trinity, Neo  
**Date:** 2026-04-01  
**Status:** APPROVED  

#### Summary

Integrated transparent hostname resolution into Oscan's socket builtins. Updated `deps/laststanding` to v5b3c0cd (adds `l_resolve` for IPv4 hostname lookup). Runtime now resolves hostnames inside `socket_connect` and `socket_sendto` before calling platform socket primitives.

#### Key Decisions

- **Language surface unchanged.** Existing `addr: str` parameter naturally expands from "IPv4 text" to "hostname or IPv4 text" at runtime.
- **Freestanding:** Uses `l_resolve(hostname, ip_out)` from laststanding; resolves before `l_socket_connect()` / `l_socket_sendto()`.
- **Libc:** Uses `getaddrinfo(..., AF_INET, ...)` for IPv4 resolution on Windows/POSIX; shared helper `osc_socket_lookup_ipv4()`.
- **Port validation:** All paths validate port is in range `0..65535` on entry.
- **Backward compatible:** Numeric IPv4 literals still work unchanged.
- **Test coverage:** `tests/positive/socket_hostnames.osc` validates `"localhost"` resolution in both TCP and UDP modes.

#### Impact

- Users can pass hostnames to `socket_connect` / `socket_sendto` without language surface growth
- Consistent behavior across freestanding and libc backends
- Enables practical examples like HTTP client using hostnames (example.com, localhost)

#### Future Direction

If the language later needs explicit DNS APIs, richer error reporting, or IPv6 support, those should be designed as new language-level builtins rather than overloading the current transparent resolution.

---

### 8. Example Interpolation Sweep with Brace Escaping

**Authors:** Trinity, Neo, Tank  
**Date:** 2026-04-01  
**Status:** APPROVED  

#### Summary

Applied string interpolation to 6 example programs to reduce nested `str_concat(...)` chains in human-readable output. Fixed brace escaping in embedded CSS/JSON fragments. Repaired `examples/web_server.osc` compile failure.

#### Key Decisions

- **Scope:** Favor interpolation in example output, request formatting, status labels, and presentation strings.
- **Brace escaping:** Any literal braces in strings must be escaped as `{{` and `}}` to avoid parser misinterpretation.
- **Preservation:** Keep plain concatenation where it is still the clearest choice for incremental buffer assembly.

#### Examples Updated

- `env_info.osc` — interpolation applied
- `error_handling.osc` — interpolation applied
- `file_checksum.osc` — interpolation applied
- `http_client.osc` — interpolation + hostname example
- `word_freq.osc` — interpolation applied
- `gfx/ui_demo.osc` — interpolation applied
- `web_server.osc` — interpolation + CSS brace fix (unquoted font family)

#### Validation

- Initial: 24/25 examples compiled; `web_server.osc` failed on CSS `font-family: 'Segoe UI'`
- Repair: Replaced single-quoted family name with unquoted `Segoe UI` (valid CSS)
- Final: 25/25 examples compiled; interpolation regression gate green

#### Impact

- Examples showcase string interpolation feature naturally
- Reduced boilerplate in presentation strings
- No runtime behavior changes — purely syntax improvements

---

### 9. User-Facing Documentation Alignment: Hostname Support

**Author:** Oracle (Language Spec Specialist)  
**Date:** 2026-04-01  
**Status:** APPLIED  

#### Summary

Updated README.md and examples/http_client.osc to accurately reflect that hostname support in socket networking is now approved and operational. Removed conservative "until hostname QA is green" warning language.

#### Changes Applied

**README.md (line 202):**
- Old: "Simple HTTP GET client (TCP sockets; use IPv4 literals until hostname QA is green)"
- New: "Simple HTTP GET client (TCP sockets with hostname support)"

**examples/http_client.osc (header + usage):**
- Header comments: Updated from "use IPv4 literal" caveat to "with hostname support"
- Parameter specification: `<ip>` → `<hostname|ip>`
- Demonstration: Changed from IPv4 literal (93.184.216.34) to practical hostname (example.com)

#### Rationale

- Hostname support is now implemented, tested, and approved by Tank QA
- Specification (docs/spec/oscan-spec.md) already correctly documents hostname capability
- User-facing documentation now accurately reflects implemented, approved behavior
- No code or behavior changes — documentation alignment only

#### Impact

- Users will no longer see conservative warnings about hostname support
- Example code reflects real-world usage patterns (hostnames are more practical than IP literals)
- No breaking changes; purely informational update

---

### 10. README.md Structural Refactoring

**Author:** Oracle (Language Spec Specialist)  
**Date:** 2026-04-01  
**Status:** APPROVED  

#### Summary

Restructured README.md into a cleaner, more scannable front door for the Oscan project. Reduced cumulative 1400+ lines to focused 167 lines (~88% reduction) while preserving all essential technical information.

#### Key Changes

- **Sharper opening:** 3-line pitch emphasizing LLM-optimized minimalist language
- **Highlights section:** 6 concise bullet points explaining why each feature matters
- **Quick code example:** Fibonacci + main with inline pattern glossary
- **Intent-driven use cases:** Help readers self-identify relevant applications
- **Grouped examples:** Three sections (CLI utilities, network programs, graphics/games) instead of exhaustive list
- **Removed:** "Why Oscan?" section, string interpolation deep-dive, 139-function table, freestanding runtime explanation

#### Verification

All links, test counts (62 unit + 74 positive + 26 negative), platform counts (4), and feature claims verified against live codebase.

#### Review Cycle

- **Initial submission:** Neo identified 6 factual errors and 2 omissions (REJECTED)
- **Corrections applied:** Trinity patched all issues independently
- **Re-review:** Neo verified all corrections against codebase (APPROVED)

#### Impact

- Reduced cognitive load for new users
- Improved discoverability of examples
- Maintained technical accuracy throughout

---

## Governance

- All meaningful changes require team consensus
- Document architectural decisions here
- Keep history focused on work, decisions focused on direction
