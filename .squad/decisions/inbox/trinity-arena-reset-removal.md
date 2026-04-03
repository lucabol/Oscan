# Decision: Remove `arena_reset()` from Language Surface

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-18  
**Status:** PROPOSED  

## Summary

Removed `arena_reset()` from the Oscan language surface. The internal runtime function `osc_arena_reset()` is preserved for arena lifecycle management, but programs can no longer invoke it.

## Rationale

- `arena_reset()` was the **only** source of use-after-free in the language — calling it invalidated all dynamic arrays with no compile-time protection.
- Zero examples used it; the only reference was a synthetic test.
- Removing it gives Oscan a clean memory safety story: no program-level operation can cause use-after-free.

## Changes

- **Compiler:** Builtin registration and codegen arm removed from `semantic.rs` and `codegen.rs`.
- **Runtime:** `osc_arena_reset_global()` removed from `osc_runtime.h` and `osc_runtime.c`. Internal `osc_arena_reset()` preserved.
- **Tests:** `spec_microlib.osc` test block and expected output removed.
- **Docs:** Spec (§8.2, §8.3, §10) and guide updated to remove all references.

## Impact

- **Language users:** `arena_reset()` is no longer available. Long-running programs must rely on arena bulk-free at exit.
- **Runtime (Morpheus):** No changes needed; `osc_arena_reset()` internal API unchanged.
- **Future:** If arena reset is ever re-introduced, it should come with a borrow-checker or epoch-based safety mechanism.

## Validation

62 unit tests, 74 positive integration tests, 26 negative tests, 25 examples all pass.
