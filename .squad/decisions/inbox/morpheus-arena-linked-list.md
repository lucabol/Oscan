# Decision: Linked-List Arena Blocks (Fix Dangling Pointer SEGFAULT)

**Author:** Morpheus (Runtime Dev)  
**Date:** 2025-07-18  
**Status:** APPLIED  
**Severity:** CRITICAL BUG FIX  

## Problem

The monolithic arena buffer used `malloc(new) → memcpy → free(old)` on growth, invalidating ALL previously returned pointers. Any program allocating >1MB from the arena would SEGFAULT, because `osc_array_push` (and any code holding arena-allocated pointers) would dereference freed memory after arena growth.

Reported by Tank via `tank-arena-growth.md`.

## Solution: Linked-List of Fixed Blocks

Replaced the single growable buffer with a linked list of fixed-size blocks:

```c
typedef struct osc_arena_block {
    uint8_t *data;
    size_t   used;
    size_t   capacity;
    struct osc_arena_block *next;
} osc_arena_block;

typedef struct {
    osc_arena_block *head;
    osc_arena_block *current;
    size_t          block_size;
} osc_arena;
```

**Key guarantees:**
- Once a block is allocated, it is NEVER freed or moved until `osc_arena_destroy`
- All pointers returned by `osc_arena_alloc` remain valid for the arena's lifetime
- New blocks are `max(block_size, requested_size)` — oversized requests still fit in one block
- `osc_arena_reset` resets `used` on all blocks but keeps them allocated for reuse

## Impact

- **osc_runtime.h:** `osc_arena` struct changed from `{data, used, capacity}` to `{head, current, block_size}` with new `osc_arena_block` type
- **osc_runtime.c:** Arena functions rewritten for block-based allocation
- **test_runtime.c:** Arena tests updated for new struct layout; 2 new tests added (pointer validity after growth, multi-block reset). Total: 78 tests, all passing.
- **Codegen:** No changes needed — codegen only calls `osc_arena_create/destroy/reset_global`, never accesses struct internals
- **New test:** `arena_stress_200k.osc` pushes 200K i32 elements (~1.6MB), forcing multiple arena blocks. Passes on WSL/GCC.
- **Existing test:** `arena_growth.osc` (60K elements) continues to pass
- **All 48 integration tests pass** (32 positive + 16 negative)

## Alternatives Considered

- **Option B (offset-based):** Would require pervasive API changes — all users must dereference via `arena->data + offset`
- **Option C (bigger default):** Bandaid, doesn't fix the bug
- **Option D (pointer fixup in push):** Fragile, only fixes `osc_array_push`, not other code holding stale pointers
