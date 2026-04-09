# Decisions

## 2026-04-07

### GitHub Releases are the canonical install surface
- Oscan should ship installable GitHub Releases instead of asking users to build from source for the primary path.

### Bundled toolchains are generated during release builds, not committed to git
- `toolchain/` belongs in staged release artifacts, not in repository source control.

### Phase 1 release promise
- Windows x86_64: self-contained release with bundled toolchain.
- Linux x86_64: self-contained release with bundled toolchain on tested targets.
- macOS: ship the `oscan` binary, but require Xcode Command Line Tools / host compiler.

### Release layout contract
- `oscan` / `oscan.exe` must remain a sibling of `toolchain/` in phase 1 release assets.

### Phase 1 Release Packaging Source of Truth (Bishop)
- Added `packaging/toolchains/` as release-packaging source of truth
- Windows uses llvm-mingw UCRT bundles; Linux uses official LLVM release archives
- `release-contract.json` is the workflow-facing metadata file
- macOS remains binary-only in phase 1
- Status: IMPLEMENTED

### Release Workflow Consumes Packaging Contract (Hicks)
- Release workflow and packaging scripts treat `release-contract.json` as source of truth
- `release.yml` derives packaging matrix from contract; `scripts/release_tools.py` resolves target metadata
- Smoke tests validate archive suffixes and installed bundle shape against contract metadata
- Status: PROPOSED

### Install Docs Update: Phase 1 Release Distribution Story (Newt)
- Updated README, guide, and spec with GitHub Releases as canonical surface
- Separate platform narratives: Windows/Linux full bundles, macOS binary-only
- Explained toolchain/ artifact split (not in Git, generated at release)
- Honest Phase 1 upgrade/uninstall story (manual extraction, future package managers)
- Status: IMPLEMENTED

## 2025-07-18

### `defer` Statement Implementation (Trinity)
- Implemented `defer` across full compiler pipeline (lexer → parser → AST → semantic → codegen)
- Function-level scope (not block-level), only in `fn!` functions
- Expression must be a function call; LIFO ordering for multiple defers
- Codegen uses string-based collection with save/restore for nested functions
- Status: IMPLEMENTED

### Remove `arena_reset()` from Language Surface (Trinity)
- Removed `arena_reset()` from language surface; internal `osc_arena_reset()` preserved
- Only source of use-after-free in the language; zero examples used it
- Gives Oscan clean memory safety story: no program-level use-after-free
- Status: PROPOSED

## 2025-07-17

### Copilot Instructions Auto-Generation (Newt)
- Created `.github/copilot-instructions.md` (~2 KB) and `.github/instructions/oscan.instructions.md` (~25 KB)
- `scripts/gen-copilot-instructions.py` auto-generates language reference from `src/semantic.rs` builtins + `examples/`
- `--check` mode enables CI verification that instructions are up to date
- Status: Implemented
