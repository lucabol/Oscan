# Bug Report: Arena Growth Causes SEGFAULT (Dangling Pointer)

**Author:** Tank (Tester)  
**Date:** 2025-07-17  
**Severity:** CRITICAL  
**Affects:** Any Babel-C program that allocates >1MB from the arena  

## Problem

When the arena's internal buffer exceeds its capacity (default 1MB), `bc_arena_alloc` grows the buffer by:
1. `malloc(new_capacity)` — allocates new buffer
2. `memcpy(new, old, used)` — copies data
3. `free(old)` — **frees the old buffer**

After step 3, **ALL pointers previously returned by `bc_arena_alloc` are dangling** — they point into the freed old buffer. On Linux/glibc, large allocations use `mmap`/`munmap`, so `free` unmaps the pages immediately → any access = **SIGSEGV**.

## Reproduction

```babel-c
fn! main() {
    let mut big: [i32] = [0];
    for i in 1..150000 {
        push(big, i);
    };
    print_i32(big[0]);
}
```

Compile and run on Linux (WSL):
```
babelc test.bc -o test.c
gcc -std=c99 test.c runtime/bc_runtime.c -Iruntime -o test -lm
./test
```

**Result:** Exit code 139 (SIGSEGV)

## Root Cause Analysis

In `bc_array_push(arena, arr, value)`:
1. When array needs to grow, calls `bc_arena_alloc(arena, new_size)`
2. If this triggers arena growth, `arena->data` is freed and replaced
3. `arr` (a `bc_array*` in the old arena buffer) is now a dangling pointer
4. Writing `arr->data = new_data` writes to freed memory → UB/SEGFAULT
5. Additionally, `arr->data` (the old array data pointer) also pointed into the freed arena, so `memcpy(new_data, arr->data, ...)` reads from freed memory

## Why Current Tests Don't Catch It

The default arena is 1MB. The existing `arena_stress.bc` test uses only 500 pushes (~4KB arena usage). The new `arena_growth.bc` test uses 65K pushes (~524KB) — heavy pressure but stays within 1MB.

## Recommended Fix Options

### Option A: Linked-list arena (recommended)
Replace single-buffer arena with a chain of fixed-size blocks. Never free/move existing blocks. Pointers remain valid forever.

```c
typedef struct bc_arena_block {
    uint8_t *data;
    size_t   used;
    size_t   capacity;
    struct bc_arena_block *next;
} bc_arena_block;
```

### Option B: Offset-based allocation
Store offsets instead of raw pointers. All arena-allocated data accessed via `arena->data + offset`. When arena grows, offsets remain valid.

### Option C: Increase default arena size
Bandaid fix: increase `BC_ARENA_DEFAULT_CAPACITY` to 64MB or 256MB. Doesn't fix the underlying bug — just makes it harder to trigger.

### Option D: Fix pointer update in bc_array_push
Detect arena growth inside `bc_array_push` by comparing `arena->data` before and after `bc_arena_alloc`. If it changed, recompute `arr` pointer as `new_arena_data + old_offset`. This is fragile but targeted.

## Impact

Any Babel-C program that allocates >1MB total (arrays, strings, structs combined) will SEGFAULT. This is a time bomb for real-world programs. The 1MB limit is surprisingly easy to hit with arrays of structs containing string fields.
