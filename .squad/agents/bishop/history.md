# Bishop History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan compiler/runtime with bundled Windows/Linux toolchain lookup already implemented in `src/main.rs`.
- Phase 1 release-packaging metadata now lives in `packaging/toolchains/release-contract.json` with per-target vendoring manifests beside it.
- Windows x86_64 is pinned to an llvm-mingw UCRT bundle and Linux x86_64 is pinned to an official LLVM archive, both with upstream SHA256 fields for release assembly.
- Builtin pattern: each new builtin touches 4 files — osc_runtime.h (declaration), osc_runtime.c (implementation), semantic.rs (@builtin comment + insert), codegen.rs (match arm).
- Result types used in runtime wrappers must be declared in osc_runtime.h AND added to the skip list in codegen.rs `emit_result_typedefs`.
- The generated C file's include order is: l_gfx.h → l_img.h → osc_runtime.h → osc_runtime.c → auto-generated typedefs → user code (freestanding mode).
- l_img.h requires freestanding mode (uses l_mmap); guarded by OSC_HAS_IMG. Non-freestanding builds get a stub returning Err.
