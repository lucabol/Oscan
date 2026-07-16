# Ripley History

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan language/compiler with embedded runtime and planned GitHub Release packaging.

### 2026-07-14 — Native link embedding design (Windows freestanding)
- Converted the approved "remove C-compiler/linker dependency" research into a file-level
  design at `docs/design/native-link-embedding.md`; decision summary in
  `.squad/decisions/inbox/ripley-native-link-design.md`.
- Verified in-checkout: `release_tools.py build_runtime_archive` loops `sources` and `ar rcs`es
  them, so adding `osc_native_shim.c` to each mode's `sources` precompiles the shim for free.
  `build.rs` already exists (version stamping); `main.rs` already embeds runtime C via
  `include_str!` (precedent for compile-time embedding). No `sha2`/`dirs` in Cargo.lock yet.
- Key design calls: split `link.rs` → `src/backend/link/` dir + new `native_assets.rs`;
  `LinkerFlavor{MingwDirect,CompilerDriver}`; MingwDirect passes import libs + builtins as
  absolute positional archives (no `-l`/`-L`, no `-nostdlib`/`-no-pie`); content-addressed
  atomic cache under `%LOCALAPPDATA%`; NO-silent-fallback when embedded assets are claimed.
- Ownership: Bishop owns all `src/**` + `Cargo.toml`; Hicks owns packaging JSON, `release_tools.py`,
  `build.rs`, workflows, prepare/assemble scripts. Shared coupling = exact JSON field + generated
  symbol names (design §8.3). Reviewer gate: parity (hello == 6,656 B), KERNEL32-only, no-compiler
  proof, cache concurrency/corruption, no-silent-fallback.
- Reminder to self: I am the reviewer gate for Bishop/Hicks; check their diffs against §8.3 names
  and the §2.3 "nothing regresses" table.

## Learnings

- Native-link embedding: my original Windows asset list (8 files / ~6.7 MB) was
  wrong. `ld.lld.exe` is NOT a static standalone binary — it dynamically links
  against 5 sibling DLLs (`libLLVM-22.dll`, `libc++.dll`, `libwinpthread-1.dll`,
  `libunwind.dll`, `libffi-8.dll`) that must be embedded and co-located with it
  under `bin/` so Windows' module-directory DLL search resolves them with no PATH
  manipulation. Corrected set: **13 files, ≈85.4 MB** (still only ~22.6% of the
  378.8 MB full toolchain). `libclang-cpp.dll` (~47 MB) is only needed by clang,
  not the linker — excluded.
- Byte-parity (6,656 B hello.exe) was unchanged; the fix was purely "ship 5 more
  files." Lesson for validation: file-hash equality of an extracted binary is
  necessary but NOT sufficient — a dynamically-linked linker can hash-match and
  still fail to launch (STATUS_DLL_NOT_FOUND). Always isolation-test: move the
  pinned toolchain away AND scrub toolchain `bin/` from PATH, since a stray PATH
  entry masks missing-sibling-DLL bugs. Optional post-extraction `--version` smoke
  invocation is cheap defense-in-depth.

## Learnings (reviewer gate — final sign-off pass, 2026-07-14)
- GO for Windows x86-64 freestanding self-contained native linking. Reconfirmed a
  clean baseline myself: `cargo check --all-targets` clean (2 known build.rs
  warnings only), `cargo test --quiet` = 140+2 passing.
- Spot-check strategy that paid off: rather than re-reading every file, I sampled
  the exact coupling points a design-vs-code drift would hide in — (1) the §7.3
  no-silent-fallback string in `driver.rs` + its wiring on BOTH the extraction and
  the `execute::run` failure paths in `mod.rs`; (2) the MingwDirect forbidden-flags
  guard + exact-argv snapshot in `plan.rs`; (3) `is_mingw_eligible`'s Freestanding
  gate; (4) the field-name ABI (`contains_native_shim`/`native_shim_member`/
  `install_subpath`/`linker_runtime`) across the Rust reader and Python writer
  simultaneously. All consistent.
- Verification lesson reinforced: a green `cargo test` proves the pure/plan layer
  but NOT the embedded path (real toolchain is release-staged only). Kept that gap
  explicit in the verdict rather than letting the passing suite imply full coverage.
- Two honest gaps worth carrying forward as follow-ups, not blockers: the
  `@response.rsp` long-argv path is still unexercised, and `smoke-release.ps1`'s
  isolation is PATH-stub-only (true toolchain-rename isolation is proven only
  against the dev build via `tests/native_link_isolation.tests.ps1`, not the
  packaged tree).

## Learnings — Linux ElfDirect design ratification (2026-07-15)

- Ratified §10 (Linux x86-64 ElfDirect) as addendum to
  `docs/design/native-link-embedding.md`. Key design calls:
  - `LinkerFlavor::ElfDirect` added to enum; reuses existing pinned
    `linux-x86_64.json` toolchain's `x86_64-linux-musl-ld` (static, ~2.78 MB).
  - argv: `-s -m elf_x86_64 -static --gc-sections --build-id=none -o <out> <objs> <archive>`.
    No builtins, no system libs, no entry flag — byte-identical to CompilerDriver path.
  - Embedded payload: 1 file / ~2.78 MB (vs Windows 13 files / ~85.4 MB).
  - Unix elevation policy via `geteuid() != getuid()`, fed to existing
    `check_elevation_policy` — no new security machinery.
  - CWD/PATH hardening: not needed (linker is static; existing early-return
    for non-MingwDirect in `apply_windows_dll_search_hardening` is sufficient).
  - AArch64/RISC-V64 explicitly deferred (no runtime archives, no pinned
    cross-toolchains — §1.2 updated with precise blocker reasoning).
- Sections added/modified: §1.1, §1.2, §8.4, §9 deferred list, §10 (10.1–10.11).
- Decision entry: `.squad/decisions/inbox/ripley-linux-elfdirect-design.md`.
- Ownership: Bishop owns plan.rs/mod.rs/driver.rs/unix_elevation.rs/main.rs;
  Hicks owns release_tools.py/.github/workflows/.gitignore/smoke-release.ps1.

## Learnings — Final sign-off: Cross-linker sidecar fix + aarch64/riscv64 native linking (2026-07-16)

**GO** — ready to commit and release. This session completed freestanding
`--backend native` final linking on linux-aarch64/linux-riscv64 without external
tooling, fixed a major cross-linker sidecar bug (completely non-functional since
initial implementation), patched a second packaging gap, and added
`--extra-obj`/`--extra-lib` CLI support.

**Key reasoning:**

- The cross-linker sidecar override mechanism (`OSCAN_NATIVE_LINKER` +
  `OSCAN_NATIVE_LINKER_FLAVOR=elf/mingw`) was advertised as a feature but never
  worked — `src/backend/link/mod.rs`'s blanket `if !target.is_host()` gate
  rejected all non-host targets before ever consulting the override env vars.
  Root cause traced to the design doc's own buggy §11.4 pseudocode (faithfully
  implemented, but wrong). Coordinator's live end-to-end validation (attempting
  to actually *use* a packaged sidecar) caught this blind spot.

- Fix is surgical and correct: new `cross_link_permitted()` pure predicate with
  7 unit tests covering all branch combinations (explicit override with matching
  flavor, explicit override with mismatched flavor, FLAVOR alone without LINKER,
  embedded assets matching target, embedded assets for different target, no
  override and no assets). Logic exactly matches design §11.4's corrected
  pseudocode. `driver::env_var_nonempty` made `pub(super)` so `mod.rs` can
  check override presence. Zero test regressions (189+4 passing, up from 186+3).

- Second packaging gap (also found via live validation): `release.yml`'s sidecar
  staging only copied the `ld` binary, never the runtime archive. Since
  `--backend native` cross-linking needs BOTH a linker override AND a runtime
  archive (`OSCAN_RUNTIME_ARCHIVE_DIR`), a user with only the downloaded release
  had no way to supply one. Fixed by also copying
  `build\runtime-archives\linux-{target}\*` into each
  `build\cross-linker-sidecars\linux-{target}\` directory. Live-validated: the
  fully realistic bundle (linker + runtime archive together, no external files)
  successfully cross-linked and QEMU-executed a real program.

- I independently verified the actual code changes myself (not just decisions.md
  prose): `cross_link_permitted()` implementation (lines 252-264 of mod.rs)
  exactly matches §11.4, all 7 unit tests present (lines 1079-1167), visibility
  change in driver.rs confirmed (line 503), release.yml diff complete (aarch64
  + riscv64 toolchain fetch, runtime archive builds, sidecar staging with BOTH
  files copied, inline bugfix comment explaining why). Ran `cargo test --release
  --quiet` myself in WSL — 189+4 tests passing, zero failures.

- Vasquez independently validated all fixes (black-box, including sidecar override
  regression coverage), all-pass. Newt updated docs (README, guide, spec) with
  accurate aarch64/riscv64 native support scoping and new CLI flags. CI coverage
  now includes required (non-continue-on-error) sidecar-override steps in both
  `native-link-embedding-smoke-linux-{aarch64,riscv64}` jobs, closing the blind
  spot that let this ship undetected.

**Accepted gaps (I agree both are shippable as-is):**

1. No Rust test asserts `--extra-obj`/`--extra-lib` values literally appear in
   `LinkPlan` argv — only E2E FFI proof (precompiled C → Oscan successfully calls
   it). Stronger evidence than argv string-matching; CLI parsing independently
   covered. Bishop declined this earlier; I agree it's non-blocking.

2. C backend (default) has no existence/extension validation for
   `--extra-obj`/`--extra-lib` parity with native backend's §12.2 checks. I agree:
   C backend already delegates all link diagnostics to `cc` (missing libs, bad
   objects, etc.), so delegating these specific diagnostics is consistent. Native
   backend's stricter validation is appropriate because it owns the full link
   argv; C backend doesn't.

**Residual risks:** None blocking ship. The two accepted gaps are legitimate
design trade-offs, not omissions. Cross-linker sidecar mechanism is now
comprehensively tested (unit + CI + live validation). Packaging bug is fixed
with clear inline documentation.

**Lesson reinforced from earlier passes:** Live end-to-end validation (actually
*using* a packaged feature, not just checking its files exist) catches blind
spots that passing unit tests + code review miss. The sidecar mechanism's unit
tests were green, its packaging looked plausible, but it was completely
non-functional until the Coordinator attempted a real cross-link with extracted
sidecars. "Does it work in realistic end-user hands?" is a different, necessary
question from "does the code match the design?"

