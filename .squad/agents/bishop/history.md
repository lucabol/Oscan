# Bishop History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan compiler/runtime with bundled Windows/Linux toolchain lookup already implemented in `src/main.rs`.
- Phase 1 release-packaging metadata now lives in `packaging/toolchains/release-contract.json` with per-target vendoring manifests beside it.
- Windows x86_64 is pinned to an llvm-mingw UCRT bundle and Linux x86_64 is pinned to an official LLVM archive, both with upstream SHA256 fields for release assembly.
