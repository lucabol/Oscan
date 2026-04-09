# Newt History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Install docs must distinguish Windows/Linux bundled releases from macOS host-toolchain installs.
- Updated install documentation across README.md, docs/guide.md, and docs/spec/oscan-spec.md to clearly separate bundled vs. host-compiler stories.
- Key messaging: toolchain/ is not in Git because it's a release artifact (generated during builds), not source code.
- Phase 1 release promise: Windows/Linux get full self-contained bundles; macOS requires Xcode CLT but ships binary-only archive.
- Documented honest upgrade/uninstall story for Phase 1 (manual extraction, no package manager yet).
- Emphasized GitHub Releases as canonical install surface; cargo-dist avoided as primary v1 path per locked decisions.
- Created Copilot instruction files for LLM-assisted Oscan development:
  - `.github/copilot-instructions.md` — static project-level context (~2 KB), auto-injected on every request.
  - `.github/instructions/oscan.instructions.md` — language reference (~25 KB), auto-injected for `*.osc` files.
  - `scripts/gen-copilot-instructions.py` — auto-generates the language reference from `src/semantic.rs` builtins and `examples/` files.
- Auto-generation approach: script extracts @builtin annotations (same regex as gen-builtin-table.py), reads 8 example files verbatim, combines with embedded critical-differences template, injects between marker comments.
- Supports `--inject` (update in place) and `--check` (CI verification) modes.
