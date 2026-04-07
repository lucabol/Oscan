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
