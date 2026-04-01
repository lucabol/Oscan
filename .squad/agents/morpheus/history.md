# Morpheus — History

## Project Context
- **Project:** Oscan — An LLM-optimized minimalist programming language that transpiles to C
- **Tech Stack:** Runtime implementation in C, memory management, FFI, standard library
- **User:** Luca Bolognese
- **Requirements:** See `../requirements.md` for full specification

## Core Context
- Runtime is pure C, no external dependencies beyond libc
- Memory model must be deterministic and uniform (one approach for all allocation)
- Micro-lib provides only: basic I/O, math primitives, memory interfaces
- No domain-specific modules (no JSON, HTTP, etc.)
- C-FFI allows seamless external C function calls
- Error-as-value runtime support (Result-like composite types)
- UB guards: bounds checking, integer overflow detection, null pointer guards
- Runtime overhead must be minimal

## Learnings

### 2026-04-01 — laststanding DNS wiring (APPROVED batch)
- **Files:** `deps/laststanding/l_os.h`, `runtime/osc_runtime.c`, `examples/http_client.osc`, `docs/spec/oscan-spec.md`
- **Dependency update:** Oscan's submodule checkout now points at `deps/laststanding` commit `5b3c0cd`, which is newer than the superproject's previously pinned `dd5282c`.
- **Runtime pattern:** Keep the language surface unchanged when a freestanding dependency grows a useful primitive. For networking, normalize `osc_str` to a bounded C buffer, validate the port once, then resolve hostnames before calling the lower socket primitive.
- **Implementation:** `socket_connect` and `socket_sendto` now accept hostnames as well as dotted IPv4 text in both freestanding (`l_resolve`) and libc (`getaddrinfo(AF_INET)`) modes.
- **Sample/docs:** `examples/http_client.osc` now documents `<host>` input and uses `example.com`; the spec now states socket connect/sendto accept hostnames too.
- **Validation:** `cargo test --quiet` passed (62/62), and `examples/http_client.osc` compiled in freestanding mode and successfully fetched a local `python -m http.server` endpoint via `localhost`.
- **Team batch approved:** Orchestration logs in `.squad/orchestration-log/2026-04-01T10-54-28Z-*.md`. Neo architecture review APPROVED. Tank re-review APPROVED (both freestanding and libc regressions green). Oracle updated user docs per Tank approval.
- **Decision merged:** `.squad/decisions.md` entry #7 (Hostname Support Integration via laststanding DNS)

### Interpolation MVP runtime prep
- **Files:** `runtime/osc_runtime.c`, `runtime/test_runtime.c`, `docs/spec/oscan-spec.md`, `docs/guide.md`, `README.md`
- **Runtime decision:** Interpolation MVP needs no new formatter helper. The existing arena-backed surface is enough: `str_concat`, `str_from_i32` / `i32_to_str`, `str_from_i64`, `str_from_f64`, and `str_from_bool`.
- **Implementation:** `osc_str_from_i32()` now delegates to `osc_i32_to_str()` so i32 stringification has one runtime implementation and one behavior path.
- **Validation:** Added runtime tests for i32 aliasing plus i64/f64/bool stringification, and adjusted panic tests for MSVC friendliness. Runtime test suite passes on Windows/MSVC (`cl`) with 82/82 passing.
- **Docs:** Synced spec, guide, and README so they all describe interpolation as upcoming syntax lowered onto existing string helpers rather than a new runtime subsystem.

### Phase 5: Runtime & Micro-Lib (completed)
- **Files:** `runtime/osc_runtime.h`, `runtime/osc_runtime.c`, `runtime/test_runtime.c`, `runtime/Makefile`
- **Arena allocator:** Single-arena model with 8-byte alignment, doubling growth strategy. `osc_arena_create / alloc / reset / destroy`. Global arena pointer (`osc_global_arena`) for generated main() to set.
- **Checked arithmetic:** i32 uses widening to i64 for mul overflow check. i64 uses careful case analysis (no portable 128-bit in C99). All ops detect overflow BEFORE it happens.
- **Strings:** Immutable `bc_str` = `{const char* data, int32_t len}`. Literals are zero-copy wraps. Concat/to_cstr allocate on arena.
- **Arrays:** Generic via `void* + elem_size`. Bounds-checked get/set panic on OOB. Push doubles capacity via arena realloc.
- **Result type:** `OSC_RESULT_DECL` macro generates tagged unions. `osc_result_str_str` is pre-declared for `read_line`.
- **Type casts:** f64→i32/i64 check NaN/Inf/range before cast. i64→i32 checks narrowing overflow. Widening casts are unconditional.
- **Panic handler:** `bc_panic(msg, file, line)` → stderr + `exit(1)`. `BC_PANIC(msg)` macro captures `__FILE__`/`__LINE__`.
- **Build:** C99, `-Wall -Wextra -Werror -pedantic -fsanitize=address,undefined`. Zero warnings on both GCC 13 and Clang 18.
- **Tests:** 78 assert-based tests (up from 76), panic tests use `fork()` on POSIX (skipped on Windows). All passing.
- **Files updated:** Runtime files renamed from `bc_*` to `osc_*`

### Arena Linked-List Fix (critical bug fix)
- **Bug:** Monolithic arena buffer growth (`malloc→memcpy→free`) invalidated ALL previously returned pointers. Any program allocating >1MB SEGFAULT'd because `osc_array_push` held dangling pointers after arena realloc.
- **Fix:** Replaced single growable buffer with linked list of fixed-size blocks (`osc_arena_block`). Blocks are NEVER freed or moved until `osc_arena_destroy`. New blocks are `max(block_size, requested)`.
- **Struct change:** `osc_arena` went from `{data, used, capacity}` to `{head, current, block_size}` with separate `osc_arena_block` type. Public header change but codegen only uses opaque API (`create/alloc/reset/destroy`), so no compiler changes needed.
- **`osc_arena_reset`:** Walks all blocks, sets `used=0`, resets `current` to `head`. Blocks kept allocated for reuse.
- **Key insight:** Codegen (codegen.rs) never accesses arena struct fields directly — it only calls the C API functions. This made the struct layout change safe.
- **Naming:** All runtime symbols renamed from `bc_` prefix to `osc_` prefix for consistency with project rename.
- **Tests added:** 2 new C tests (pointer validity after growth, multi-block reset) + `arena_stress_200k.osc` integration test (200K pushes, ~1.6MB, forces multiple blocks).
- **Verified:** 53 Rust tests, 78 C runtime tests, 48 integration tests (32 positive + 16 negative) — all passing. WSL/GCC 200K push test passes with exit code 0.

### File I/O Built-in Functions
- **Files:** `runtime/osc_runtime.h`, `runtime/osc_runtime.c`, `src/semantic.rs`, `src/codegen.rs`
- **7 new built-ins:** `file_open_read`, `file_open_write`, `read_byte`, `write_byte`, `write_str`, `file_close`, `file_delete` — all impure (`fn!`)
- **Runtime dual-mode:** Freestanding uses `open_read()`/`read()`/`write()`/`close()`/`unlink()` from l_os.h. Libc mode uses POSIX fd APIs (`open`/`read`/`write`/`close`/`unlink` on Unix, `_open`/`_read`/`_write`/`_close`/`_unlink` on Windows via `<io.h>`/`<fcntl.h>`).
- **Design choice:** File descriptors as `int32_t` (not `FILE*`) — works on all platforms since fd values are small ints. Avoids 64-bit pointer truncation.
- **Path handling:** `osc_path_to_cstr()` helper does byte-by-byte copy (no `memcpy` dependency in freestanding), null-terminates into a 4096-byte stack buffer.
- **Verified:** 53 Rust unit tests pass, 38 positive + 21 negative integration tests pass (Win x64), smoke test writes/reads/deletes a file successfully.

### 2026-04-01 — Team Batch: Spec v0.2 Expansion & Inbox Consolidation

- **Specification v0.2 expansion finalized:** Decision merged from inbox documenting 4 feature groups expanding micro-lib from 18 to 36 functions
- **New runtime functions required (not yet implemented):**
  1. **Bitwise (6):** `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot` — emitted inline by codegen with unsigned semantics
  2. **String ops (3):** `str_find`, `str_from_i32`, `str_slice` — require runtime implementation
  3. **String indexing:** `osc_str_check_index` helper for bounds-checked byte access
  4. **String comparison:** `osc_str_compare` for lexicographic `<`, `>`, `<=`, `>=` on str types
  5. **CLI args (2):** Access globals `osc_global_argc` / `osc_global_argv` set by main wrapper
- **Status:** Decision documented, awaiting Trinity implementation and subsequent runtime porting
- **Microlib growth pattern:** Existing string helpers (`str_concat`, `str_from_*`) remain unchanged; new string ops layer on top

