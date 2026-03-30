# Decision: Prefer clang over MSVC for C compilation on Windows

**Author:** Trinity (Compiler Dev)  
**Date:** 2025-07-14  
**Status:** Implemented

## Context
MSVC (cl.exe) takes ~8s per libc test compilation vs ~2s for clang. The compiler detection order was:
`gcc → clang (PATH) → cl.exe (PATH) → cl.exe (VS installation)`

On most Windows dev machines, clang isn't on PATH but VS-bundled clang exists. So cl.exe was always chosen.

## Decision
Changed `find_c_compiler()` priority to:
`gcc → clang (PATH) → VS-bundled clang → cl.exe (PATH) → cl.exe (VS installation)`

VS-bundled clang (from `VC\Tools\Llvm\x64\bin\clang.exe`) is now detected before cl.exe.

## Additional fix
Also fixed `-lm` flag: VS-bundled clang uses MSVC's linker (link.exe) which doesn't have a separate `libm`. Math functions are in ucrt on Windows. Removed `-lm` on Windows in `compile_with_gcc_or_clang`.

## Impact
- ~4x faster libc test compilations on Windows
- Freestanding mode unchanged (already used VS clang)
- Fallback to MSVC still works if clang is unavailable
