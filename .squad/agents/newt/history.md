# Newt History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Install docs must distinguish Windows/Linux bundled releases from macOS host-toolchain installs.
- Updated install documentation across README.md, docs/guide.md, and docs/spec/oscan-spec.md to clearly separate bundled vs. host-compiler stories.
- Key messaging: toolchain/ is not in Git because it's a release artifact (generated during builds), not source code.
- Phase 1 release promise: Windows/Linux get full self-contained bundles; macOS requires Xcode CLT but ships binary-only archive.
- Documented honest upgrade/uninstall story for Phase 1 (manual extraction, no package manager yet).
- Emphasized GitHub Releases as canonical install surface; cargo-dist avoided as primary v1 path per locked decisions.
