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

## Learnings (2026-07-14, native-link-embedding docs)

- This session Bishop+Hicks removed the C-compiler/linker dependency from
  default `oscan --backend native` on **Windows x86-64 freestanding only** —
  `oscan.exe` now embeds `ld.lld` + 5 required runtime DLLs + 6 import libs +
  compiler-builtins (13 files, ≈85.4 MB, corrected mid-session from an
  earlier wrong 8-file/~6.7 MB estimate — `ld.lld.exe` is dynamically linked
  against `libLLVM-22.dll` and friends, not static) and extracts them to
  `%LOCALAPPDATA%\oscan\native-assets\` on first use.
- **Honesty-rule discipline (design §1.2) is the single most important thing
  to get right in any doc/CLI text about this feature**: always say "Windows
  freestanding native builds are self-contained" and always pair it with an
  explicit list of what still needs the external/bundled C toolchain — Linux
  native builds, hosted `--libc` mode, and explicit `--extra-c` sources, even
  on Windows. Never let the claim bleed into those.
- Updated README.md (new "Self-contained native builds (Windows)" subsection
  + supported-targets table + a caveat that a local dev `cargo build
  --release` does NOT embed the linker assets — only the release pipeline's
  staged build does), docs/releasing.md (new section on `prepare-embed-assets`
  and the corrected release step order: fetch toolchain → build runtime
  archives with shim → prepare-embed-assets → `cargo build --release`
  w/ `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS=1` → assemble),
  docs/guide.md and docs/spec/oscan-spec.md (both had a Windows/Linux
  "toolchain lookup" section identical in shape to README's — added a
  matching "Exception — Windows freestanding native builds" paragraph to
  each so all toolchain-lookup docs stay consistent), and `src/main.rs`'s
  `print_usage()` (added missing `OSCAN_NATIVE_LINKER`/
  `OSCAN_NATIVE_LINKER_FLAVOR`/`OSCAN_NATIVE_ASSET_CACHE_DIR` lines — they
  weren't documented in `--help` at all before this).
- Read the diagnostic strings in `src/backend/link/driver.rs` and
  `src/backend/link/execute.rs` (migration note, dev-build fallback note,
  no-silent-fallback hard error, "Linking {mode} executable with ...
  (embedded)") — all already clear and cross-referenced to design section
  numbers; left unchanged rather than churn Bishop's wording unnecessarily.
- When multiple docs (README/guide/spec) share the same lookup-order
  section almost verbatim, check all of them for staleness together, not
  just the one the task explicitly names — they drift apart otherwise.

## Learnings (2026-07-15, Linux x86-64 ElfDirect docs)

- Bishop, Hicks, and Vasquez completed and validated Linux x86-64 `ElfDirect`
  native-link embedding (mirroring the Windows `MingwDirect` feature from July
  14). **CRITICAL DISTINCTION**: this is about the **native backend** (`--backend
  native`, Cranelift AOT), NOT the **C backend** (default, transpiles to C). The
  C backend already supports ARM64/RISC-V64 Linux via
  `aarch64-linux-gnu-gcc`/`--target riscv64` and was unchanged by this work — I
  needed to be very careful not to conflate these two backends in the docs.
- Linux embeds **1 file (~2.78 MB)** vs Windows' 13 files (~85.4 MB) — the
  Linux linker (`x86_64-linux-musl-ld` from the pinned musl-cross toolchain) is
  a fully static binary with zero shared-library dependencies, while Windows'
  `ld.lld.exe` requires 5 sibling DLLs. This is a nice, concrete, honest detail
  worth highlighting.
- Cache location on Linux: `$XDG_CACHE_HOME/oscan/native-assets` or
  `$HOME/.cache/oscan/native-assets` (vs Windows' `%LOCALAPPDATA%\oscan\native-assets\`).
- Updated README.md (renamed "Self-contained native builds (Windows)" to
  "Windows & Linux", added payload-size contrast, updated bullet list to remove
  the "Linux native builds (still uses...)" bullet and replace it with "Linux
  AArch64/RISC-V64 **native backend** builds" with an explicit note that this is
  unrelated to the C backend's existing support, updated table row for Linux
  x86-64 to say "embedded linker" mirroring Windows, mentioned
  `OSCAN_NATIVE_LINKER_FLAVOR=elf` alongside `=mingw`).
- docs/releasing.md: added a sibling "Embedded native-link assets for
  self-contained Linux native builds" section covering the same
  `prepare-embed-assets --target linux-x86_64` workflow, the 1-file/~2.78 MB
  payload, the reordered release job steps, and the
  `native-link-embedding-smoke-linux` CI job.
- docs/guide.md and docs/spec/oscan-spec.md: renamed/expanded the "Exception —
  Windows freestanding native builds" paragraph to "Exception — Windows and
  Linux x86-64 freestanding native builds", with careful wording to distinguish
  native backend (limited to x86-64 on both platforms, with ARM64/RISC-V64
  object-only via Cranelift cross-codegen but no link) from C backend (already
  supports ARM64/RISC-V64 via cross-compilers).
- **Key judgment call**: framed the ARM64/RISC-V64 native-backend limitation as
  "object-only via Cranelift cross-codegen; final link isn't implemented for
  these targets yet, embedded or otherwise" to make it clear this is a native
  backend completeness gap, not something the Linux ElfDirect work broke. Added
  an explicit note "(note this is unrelated to the C backend's existing, working
  ARM64/RISC-V64 support...)" wherever this came up.
- Validated cli_help tests still pass after changes — they do (2 passed, no
  changes to main.rs needed this time because Bishop already updated CLI help
  text).

## Learnings (2026-07-15, accuracy fixes — cross-reference + asset path)

- Fixed two accuracy issues flagged during coordinator review:
  1. README.md line 157: updated cross-reference from `"Self-contained native builds (Windows)"` to `"Self-contained native builds (Windows & Linux)"` to match the actual renamed heading (fix reflects earlier session's heading update).
  2. docs/releasing.md line 419: corrected Linux asset staging path from `packaging/prebuilt/linux-x86_64/bin/` to `packaging/prebuilt/linux-x86_64/linker/` — validated against actual manifest in `scripts/release_tools.py` which shows `"install_subpath": "linker/x86_64-linux-musl-ld"` for Linux, not `bin/` (which is Windows-only convention).

## Learnings (2026-07-16, linux-aarch64/riscv64 native + --extra-obj/--extra-lib docs)

- Session shipped real embedded native (`--backend native`) freestanding cross-linking on linux-aarch64 and linux-riscv64 via a cross-linker sidecar mechanism (NOT self-contained like x86_64) + new CLI flags `--extra-obj` (precompiled `.o`/`.obj`) and `--extra-lib` (precompiled `.a`/`.lib`), both repeatable and compatible with both `--backend c` and `--backend native`.
- Updated README.md:
  - Added `--extra-obj` and `--extra-lib` to CLI options section (placed between `--extra-c` and `--extra-cflags` for logical grouping).
  - Replaced stale "Linux AArch64/RISC-V64 native backend... final link isn't implemented yet" with honest description of cross-linker sidecar support (see `docs/releasing.md` for setup).
  - Updated "Supported targets" table to clarify native vs C backend paths: added "(native backend)" labels, expanded notes to distinguish `--backend native --native-target linux-aarch64` (sidecar-based) from `--target riscv64` (C backend), validated that ARM64/RISC-V64 C backend (`aarch64-linux-gnu-gcc`) remains unchanged and separate.
- Updated docs/guide.md:
  - Added new "Linking Precompiled Objects and Libraries" section right after the existing "Linking Extra C Files" section.
  - Documented `--extra-obj` and `--extra-lib` usage with clear examples for both backends, including cross-linking example (`--backend native --extra-lib precompiled.a --native-target linux-aarch64`).
  - Noted these flags work without a C compiler (unlike `--extra-c`).
- Updated docs/spec/oscan-spec.md:
  - Expanded CLI Options table to include `--backend`, `--native-target`, `--extra-obj`, `--extra-lib`, and clarified `--target` (C backend only).
  - Split the exception paragraph into two logical parts: x86_64 self-contained story + new aarch64/riscv64 cross-linker sidecar story, each with clear scoping.
  - Emphasized the distinction between native backend (limited to x86_64 self-contained; aarch64/riscv64 via sidecars) vs C backend (already supported aarch64/riscv64 natively).
- **Key accuracy discipline:** avoided overclaiming — never said aarch64/riscv64 native builds are "self-contained" (they require sidecar distribution), never implied macOS native support, never claimed multi-target single-binary embedding (explicitly deferred per design §11.1 and honesty table §14).
- Validated that no changes were needed to docs/design/native-link-embedding.md or docs/releasing.md (already fully updated by Coordinator).
- All changes are precise, terse, and consistent with existing docs tone.

