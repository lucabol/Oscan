# File I/O Built-in Functions

**Author:** Morpheus (Runtime Dev)  
**Date:** 2025-07-18  
**Status:** APPLIED  

## Summary

Added 7 file I/O built-in functions to the Oscan compiler and runtime: `file_open_read`, `file_open_write`, `read_byte`, `write_byte`, `write_str`, `file_close`, `file_delete`.

## Key Decisions

**File descriptors as int32_t (not FILE*)**
- `FILE*` is a pointer — casting to `int32_t` truncates on 64-bit systems. POSIX file descriptors are small ints and fit safely in `int32_t`.
- Both freestanding (`l_os.h`) and libc modes use fd-based APIs.

**Dual-mode runtime with platform abstraction**
- Freestanding: `open_read()`/`read()`/`write()`/`close()`/`unlink()` (l_os.h macros)
- Libc on Unix: `open()`/`read()`/`write()`/`close()`/`unlink()` from `<fcntl.h>`/`<unistd.h>`
- Libc on Windows: `_open()`/`_read()`/`_write()`/`_close()`/`_unlink()` from `<io.h>`/`<fcntl.h>`
- Platform differences abstracted via `OSC_OPEN`/`OSC_READ`/etc. macros.

**Path null-termination via byte-by-byte copy**
- `osc_str` is not null-terminated; C file APIs need C strings.
- Shared `osc_path_to_cstr()` helper avoids `memcpy` dependency in freestanding mode.
- 4096-byte stack buffer, silently truncates longer paths.

## Files Changed

- `runtime/osc_runtime.h` — 7 new function declarations
- `runtime/osc_runtime.c` — dual-mode implementations (freestanding + libc)
- `src/semantic.rs` — 7 new built-in registrations (all impure)
- `src/codegen.rs` — 7 new emit_call match arms

## Impact

Oscan programs can now perform basic file I/O. All existing tests pass with no regressions.
