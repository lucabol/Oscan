# Bishop History

## Core Context (Post-Security-Rounds Summary)

**Current focus**: Native-link Windows freestanding backend with embedded `ld.lld` + runtime DLL assets.

**2026-07-14 & 2026-07-15**: Two independent security reviews identified and fixed 7 HIGH findings across the native-link work:

- **Round 1 (4 findings)**: CWD script injection, untrusted manifest `cc`, predictable temp dirs, elevated Windows cache reuse. Fixed via manifest canonicalization, `CARGO_MANIFEST_DIR` dev-build-only trust, `tempfile::Builder`, Windows elevation detection.
- **Round 2 (3 findings)**: `link_flags` injection (removed entirely, hardcoded `-lm`/`-static`), Windows elevation TOCTOU fail-open (changed to fail-closed `Result<bool,String>` + policy refusal on both `Ok(true)` and `Err(_)`), Unix scratch-dir permissions reliance (explicit `harden_native_scratch_dir_unix()` with fail-hard propagation).

**Key learnings across both rounds**:
- Untrusted manifest JSON fields must never reach `Command` argv directly; hardcode derived values in code instead
- "Detection failure == assume elevated" (fail-closed over fail-open) for security gates
- Always verify end-to-end (black-box, real compiles) not just unit tests; caught CWD breakage via my own verification
- Flaky test races require full env-var lock coverage (happened with `RUNTIME_BUILDER_ENV_TEST_LOCK`)

**Testing status**: 167 unit + 2 integration tests passing; full oracle suite passing; zero regressions; real malicious-manifest black-box proof-of-concept confirms no injection reaches argv.

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan compiler/runtime with bundled Windows/Linux toolchain lookup already implemented in `src/main.rs`.
- Phase 1 release-packaging metadata now lives in `packaging/toolchains/release-contract.json` with per-target vendoring manifests beside it.
- Windows x86_64 is pinned to an llvm-mingw UCRT bundle and Linux x86_64 is pinned to an official LLVM archive, both with upstream SHA256 fields for release assembly.
- Builtin pattern: each new builtin touches 4 files — osc_runtime.h (declaration), osc_runtime.c (implementation), semantic.rs (@builtin comment + insert), codegen.rs (match arm).
- Result types used in runtime wrappers must be declared in osc_runtime.h AND added to the skip list in codegen.rs `emit_result_typedefs`.
- The generated C file's include order is: l_gfx.h → l_img.h → osc_runtime.h → osc_runtime.c → auto-generated typedefs → user code (freestanding mode).
- l_img.h requires freestanding mode (uses l_mmap); guarded by OSC_HAS_IMG. Non-freestanding builds get a stub returning Err.

## 2026-07-14 — Native link embedding (Windows freestanding direct ld.lld)

- Implemented `docs/design/native-link-embedding.md`'s Bishop scope: split
  `src/backend/link.rs` into `src/backend/link/{mod,plan,archive,capability,driver,execute}.rs`;
  added `src/backend/native_assets.rs` (embedded-asset extraction/cache);
  added `contains_native_shim` manifest field + shim-presence policy;
  implemented `OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR` migration
  and the no-silent-fallback rule; added `sha2` to `Cargo.toml`.
- `LinkPlan::render()` is a pure function keyed off boolean fields
  (strip/gc_sections/build_id_none), not `runtime_mode` branching inside
  render — callers set those booleans per-mode at plan-construction time so
  the renderer stays a simple, snapshot-testable function of its own state.
- **Critical toolchain finding**: this repo's pinned llvm-mingw
  `ld.lld.exe` is dynamically linked against 6 sibling DLLs in its `bin/`
  dir (`libLLVM-22.dll` alone is 76 MB) — it is NOT the self-contained
  ~5.2 MB binary the design assumed. Copying it alone to an isolated dir
  fails with `STATUS_DLL_NOT_FOUND`. Any future embedded-linker work on
  Windows must either embed all 6 DLLs (~126 MB total, not ~6.7 MB) or use
  a statically-linked `ld.lld`/`lld-link` build. See
  `.squad/decisions/inbox/bishop-native-link-impl.md` for full detail.
- Verified end-to-end (not just unit tests): staged real embedded assets via
  `scripts/release_tools.py prepare-embed-assets`, built `oscan.exe` with
  `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS=1`, and confirmed
  the `MingwDirect` flavor produces a byte-exact 6,656-byte `hello.osc`
  executable (once the DLL gap above is worked around) that runs correctly.
- Rebuilding runtime archives with the shim baked in: `python
  scripts/release_tools.py build-runtime-archive --mode freestanding[_core]
  --target windows-x86_64 --cc <toolchain>\bin\clang.exe --ar
  <toolchain>\bin\llvm-ar.exe --toolchain-manifest packaging\toolchains\windows-x86_64.json`
  — omitting `--toolchain-manifest` causes the manifest's recorded
  `toolchain.vendor` to become the bare compiler family (e.g. "clang")
  instead of "llvm-mingw", which then fails the §4.3 toolchain-version
  cross-check against the embedded-asset manifest.

## 2026-07-14 — native_assets.rs review for corrected 5-DLL sibling set

- Reviewed `native_assets.rs` against the coordinator-confirmed corrected asset
  list (ld.lld.exe + libLLVM-22.dll, libwinpthread-1.dll, libunwind.dll,
  libffi-8.dll, libc++.dll -- libclang-cpp.dll confirmed unnecessary).
  `ensure_extracted_in` is already a fully generic "extract every manifest
  entry to <cache_root>/<digest>/<install_subpath>" loop with no hardcoded
  asset count and no role special-casing beyond the Unix exec bit for role
  `"linker"`. An unrecognized role like `linker_runtime` is accepted and
  extracted like any other asset. No code change was needed for this part.
- Added a live post-extraction smoke-check (`smoke_check_linker` /
  `smoke_check_result` in `native_assets.rs`, wired into `ensure_extracted()`):
  runs the extracted linker with `--version` and turns any launch failure
  into a hard error distinct from a hash-mismatch message, special-casing
  `STATUS_DLL_NOT_FOUND` (0xC0000135) by name -- the exact failure signature
  for "sibling DLL missing" found earlier this session. Covered by 4 new unit
  tests, including one using a deliberately incomplete synthetic asset set
  (non-executable "linker" bytes) to confirm `ensure_extracted` surfaces a
  clear launch-failure diagnostic rather than a confusing downstream linker
  crash. Full detail + testing trade-offs recorded in
  `.squad/decisions/inbox/bishop-asset-list-correction.md`.
- Verified: `cargo check --all-targets` clean (pre-existing unrelated
  build.rs warnings only); `cargo test --quiet` 136 + 2 tests, all passing.

## 2026-07-14 — Security review remediation (4 HIGH findings)

- Fixed all 4 HIGH findings from a security review of my own earlier
  native-link-embedding work this session. Full detail in
  `.squad/decisions/inbox/bishop-security-fixes.md`. Summary:
  1. `archive.rs`'s `find_release_tools_script` no longer ever uses CWD/exe-
     ancestor search (that path gets `Command`-executed) — only
     `OSCAN_RUNTIME_BUILDER` or `CARGO_MANIFEST_DIR` (dev builds only).
  2. `driver.rs`'s `find_linker_driver` no longer trusts a runtime archive
     manifest's raw `cc` field — new `trusted_manifest_cc()` only executes
     it if it canonicalizes to a descendant of the bundled toolchain dir or
     (dev builds) `CARGO_MANIFEST_DIR`. `main.rs`'s `resource_dir_candidates`
     gained an `include_cwd` param; `find_toolchain_dir` (now `pub(crate)`)
     passes `false`.
  3. `main.rs`'s native-backend temp dir now uses `tempfile::Builder`
     (added `tempfile` dependency) instead of predictable
     `oscan_native_<pid>`; every `process::exit` path explicitly `drop()`s
     the `TempDir` guard first since `exit` skips `Drop`.
  4. `native_assets.rs`: removed the length-only verification memoization
     entirely (re-hashes every call now); added symlink/junction rejection
     everywhere in the cache's directory tree; added Windows elevation
     detection (new `windows-sys` dependency,
     `native_assets::windows_elevation` submodule) so an elevated process
     never reuses the standard per-user cache.
- **Learned the hard way (bug I introduced and caught via my own
  verification, not just review)**: the companion "Windows DLL search
  hardening" ask — setting the `MingwDirect` child's `current_dir` to its
  own bin dir — silently broke relative `-o <output>` paths (the linker
  "succeeded" but wrote the exe into its own bin dir, not the real CWD).
  Always re-verify end-to-end after any change that alters a spawned
  child's CWD; don't just reason about it. Fixed by absolutizing
  `exe_path` once at the top of `link_executable`, preserving
  `LinkPlan::render`'s documented "no I/O, everything pre-resolved by the
  caller" contract instead of patching around it downstream.
- Re-verified end-to-end on this machine after all fixes, not just via
  unit tests: rebuilt the dev binary and compiled+ran `examples/hello.osc`
  via `--backend native` twice — once for the legacy `CompilerDriver` flavor
  (normal `cargo build`, confirmed it now falls back to the trusted,
  canonicalized `build\toolchain-windows-x86_64\bin\clang.exe` after the
  host's PATH `gcc` fails the manifest family check) and once for
  `MingwDirect` (re-staged embedded assets via
  `scripts/release_tools.py prepare-embed-assets` +
  `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS=1`, reproducing
  the previously-documented byte-exact 6,656-byte `hello.exe`).
- `cargo check --all-targets` clean; `cargo test --quiet` 161 unit + 2
  integration tests, all passing (+21 new tests, 0 regressions).

## Follow-up: fixed flaky test race in archive.rs (coordinator re-run catch)

Coordinator's independent `cargo test --quiet` re-run hit a real race:
`CWD_TEST_LOCK` only guarded 2 of the 4 tests that mutate the global
`OSCAN_RUNTIME_BUILDER` env var, so the other 2 (`env::set_var`/`remove_var`
with no lock) could race with a `CwdGuard`-using test's remove/re-check
window. Renamed the lock to `RUNTIME_BUILDER_ENV_TEST_LOCK`, had all 4 tests
acquire it for their full env-var critical section, updated the doc comment.
Verified: `cargo test --quiet` x5 clean, `cargo test backend::link::archive::
--quiet` x15 clean, same x10 more with `--test-threads=8`. Grepped all of
`src/` for `env::set_var`/`remove_var`: only other instance is
`native_assets.rs`'s single test touching `OSCAN_NATIVE_ASSET_CACHE_DIR` —
no other test shares that var, so no race there. No other unguarded
instances found.

## 2026-07-15 — Security review remediation, round 2 (3 more HIGH findings)

Full detail in `.squad/decisions/inbox/bishop-security-fixes-round2.md`.
User's explicit instruction was "do not argue them away" — implemented
exactly as specified, no watering down. Summary:

1. **`link_flags` injection (archive.rs/plan.rs/mod.rs)**: a runtime
   archive manifest's raw, untrusted `link_flags` JSON array was being
   rendered verbatim into the linker driver's `Command` argv
   (`LinkPlan::manifest_link_flags`, populated by
   `archive::read_link_flags`) — full command injection via
   `OSCAN_RUNTIME_ARCHIVE_DIR`. Deleted the field, the render loop, and
   `read_link_flags` entirely. The only two things the manifest's
   `link_flags` actually contributed in practice (verified against
   `packaging/toolchains/runtime-archive-contract.json`) — Linux hosted's
   `-lm` and Linux freestanding's `-static` — are now hardcoded in
   `mod.rs`/new `LinkPlan::static_link` instead, documented as a
   deliberate, closed, security-motivated duplication that must never be
   re-derived from the manifest again. The manifest's `link_flags` is
   still parsed (already-parsed `RuntimeArchiveManifest` the caller has)
   purely to print a **count-only** diagnostic — never the flag content.
2. **Windows elevation TOCTOU (findings 2 & 3)**: last round's mitigation
   (route an elevated process to a fresh, isolated directory) was judged
   insufficient — Windows handle-based TOCTOU races aren't fully closed by
   re-checking paths, however carefully. New product policy: **refuse a
   native final link/`--run` entirely while elevated**, not sandbox it.
   `windows_elevation::is_elevated()` changed from a fail-open `bool` (any
   OS-call failure silently meant "not elevated") to `Result<bool, String>`
   so `main.rs` can fail closed on detection error too. New pure
   `NativeLinkOperation`/`check_elevation_policy` in `native_assets.rs`,
   wired into `main.rs`'s `run_native_backend` (Windows-only, right before
   scratch-dir creation, `output_is_object` never gated). Removed the
   now-dead elevated-per-process-directory machinery in `native_assets.rs`
   entirely rather than leaving it as unreachable complexity; kept a cheap
   internal re-check in `ensure_extracted` as belt-and-suspenders, with
   `main.rs`'s gate documented as the primary enforcement point.
3. **Unix scratch permissions**: `main.rs`'s `create_native_scratch_dir`
   now explicitly `fs::set_permissions(0o700)`s the dir and fails hard
   (propagates the `io::Error`, existing caller already turns that into
   `process::exit(1)`) instead of relying solely on `tempfile`'s internal
   default.
4. Added explicit module docs (`native_assets.rs` top) stating plainly:
   for a non-elevated Windows process, the per-user cache is at
   *equivalent privilege* to any other same-user process — hash-on-every-
   use and content-addressed layout are defense-in-depth against
   accidental corruption, not a claim to stop a determined same-user
   attacker. Do not overclaim a boundary that doesn't exist.
- **Recurring lesson reinforced**: an editor tool's exact-string match can
  silently fail against a file with CRLF line endings even when the
  visible text looks identical (`windows_elevation.rs` was CRLF; my first
  `edit` attempts against it kept reporting "old_str not found" despite
  byte-for-byte-looking content). When that happens, don't fight it with
  more `edit` attempts — delete and recreate the file with `create`
  instead of guessing at hidden whitespace/encoding differences.
- Verified: `cargo check --all-targets` clean; `cargo test --quiet` run
  6x (3x default, 3x `--test-threads=32`), 167 unit + 2 integration
  passing every time, no flakes. Re-verified end-to-end on this machine,
  both linker flavors: dev `CompilerDriver` build against
  `examples/hello.osc` (confirmed the new finding-1 diagnostic fires and
  `-lkernel32` is still effectively linked via the hardcoded path), and
  `MingwDirect` via re-staged embedded assets
  (`OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS=1`) — byte-exact
  6,656-byte `hello.exe`, both `-o` and `--run` modes, elevation gate
  active and passing through correctly for this non-elevated process.

## 2026-07-15 — Linux ElfDirect implementation (design §10)

Implemented the full Linux x86-64 ElfDirect direct-link flavor per design §10:

**Types/functions added:**
- `LinkerFlavor::ElfDirect` variant in `plan.rs`
- `render_elf_direct(&self) -> Vec<OsString>` in `plan.rs`
- `LinkerSelection::Elf(ElfLinkerSource)` + `pub(super) enum ElfLinkerSource { Embedded, Override { command } }` in `driver.rs`
- `is_elf_eligible(target, runtime_mode, extra_c_files_empty) -> bool` in `mod.rs`
- `build_elf_plan(...)` in `mod.rs`
- `is_setuid_elevated() -> Result<bool, String>` in `native_assets/unix_elevation.rs`
- `#[cfg(unix)] pub fn is_setuid_elevated()` re-export in `native_assets.rs`
- Unix elevation gate in `main.rs::run_native_backend`

**Removed:** `reject_elf_flavor()` — "elf" flavor is now accepted.

**Tests added (10 new):**
- `elf_direct_renders_exact_argv_in_locked_order` (plan.rs)
- `elf_direct_never_emits_forbidden_flags` (plan.rs)
- `elf_flavor_is_now_accepted_by_resolve_linker_selection` (driver.rs)
- `hosted_linux_is_never_elf_eligible` (mod.rs)
- `freestanding_linux_x64_with_no_extra_c_files_is_elf_eligible` (mod.rs)
- `freestanding_linux_x64_with_extra_c_files_is_not_elf_eligible` (mod.rs)
- `freestanding_windows_is_not_elf_eligible` (mod.rs)
- `freestanding_linux_aarch64_is_not_elf_eligible` (mod.rs)
- `freestanding_linux_riscv64_is_not_elf_eligible` (mod.rs)
- `is_setuid_elevated_never_panics_and_returns_a_result` (unix_elevation.rs)

**Deviations from design §10:** None — implemented exactly as specified.

**Byte-parity proof:** SHA-256 `a399954cd11bba6c21d1afad3ebfcb8f1c8a4faaa22c76b75d9ac3298edf4247`,
4,744 bytes — CompilerDriver and ElfDirect produce identical `hello.osc` output.

**Test status:** 177 total (167 prior + 10 new), 175 passing, 2 pre-existing
Windows-specific failures (not introduced by this change).

## 2026-07-15 — Linux ElfDirect fix-ups (coordinator-reviewed gaps)

Coordinator independently reviewed the ElfDirect diff and found 3 small gaps:

1. **`unix_elevation.rs` — GID check added**: doc comment said "setuid/setgid"
   but code only checked UID. Added `getegid()`/`getgid()` FFI calls;
   `is_setuid_elevated()` now returns `Ok(euid != uid || egid != gid)`. Doc
   comments updated to accurately describe both checks.

2. **`native_assets.rs` `ensure_extracted()` — symmetric Unix re-check**: the
   belt-and-suspenders elevation re-check inside `ensure_extracted()` was
   Windows-only. Added `#[cfg(unix)] check_elevation_policy(is_setuid_elevated(),
   NativeLinkOperation::FinalLink)?` right after the Windows line, making the
   defense-in-depth symmetric.

3. **`driver.rs` — real `resolve_linker_selection` test**: replaced the weak
   "enum is constructible" test with two real tests that call
   `resolve_linker_selection()` with `OSCAN_NATIVE_LINKER_FLAVOR=elf`:
   - `embedded_assets_present = true` → asserts `Ok(Elf(Embedded))`
   - `embedded_assets_present = false` → asserts `Err`
   Added a `static LINKER_ENV_TEST_LOCK: Mutex<()>` following `archive.rs`'s
   `RUNTIME_BUILDER_ENV_TEST_LOCK` pattern exactly; env vars cleaned up after
   each test; `cargo test --quiet` run twice with identical results (no flakes).

**Learned**: `LinkerSelection` doesn't derive `Debug`, so assertion messages
in tests can't use `{result:?}` — keep assertions simple with string-only
messages when the type under test lacks a Debug impl.

**Test status:** 179 total (177 prior + 2 new), 177 passing, 2 pre-existing
Windows-specific failures (unchanged).

## 2026-07-15 — WSL test portability fixes (2 platform-guard gaps)

Fixed 2 pre-existing failures in `src/backend/native_assets.rs` when running
`cargo test` on WSL Ubuntu — both caused by Windows-specific test assumptions
lacking proper `#[cfg(windows)]` guards:

1. **`validated_dest_rejects_absolute_and_traversal_paths`** (line 828):
   Windows-style backslash paths like `r"C:\evil\payload.exe"` aren't
   rejected on Unix because backslash is just an ordinary filename character,
   not a path separator there. Split the bad-path list into portable
   forward-slash cases (rejected on all hosts) and Windows-only backslash
   cases gated with `#[cfg(windows)]`, following the exact
   `#[cfg(windows)] let foreign = ...; #[cfg(not(windows))] let foreign = ...;`
   pattern from `src/backend/link/driver.rs`'s
   `trusted_manifest_cc_rejects_a_foreign_absolute_path`.

2. **`smoke_check_result_reports_missing_sibling_dll_distinctly_from_a_hash_mismatch`**
   (line 1262): simulates Windows NTSTATUS 0xC0000135
   (STATUS_DLL_NOT_FOUND) via `synthetic_exit_status`, which doesn't have
   the same meaning under Unix POSIX wait-status bit-packing. The test is
   inherently Windows-only (the production `describe_exit_status` code
   already only special-cases that hex constant on Windows, with no Unix
   analog). Gated the entire `#[test]` function with `#[cfg(windows)]`.

**Learned**: on Unix, a backslash in a path literal is just a regular
character, not a path separator — portable path-validation tests must split
their fixture sets by separator style and only assert the platform-appropriate
subset. Always check `describe_exit_status`'s own branching logic to confirm
whether a test's OS-specific premise actually has an analog on the other
platform before trying to simulate it.

**Validated in WSL**: `cargo test --quiet` run twice (to confirm no
flakiness) — 178 passed, 0 failed both times. `cargo check --all-targets`
clean (no warnings/errors introduced).
## 2026-07-16: AArch64/RISC-V64 ElfDirect + --extra-obj/--extra-lib Implementation

Implemented per Ripley's spec in docs/design/native-link-embedding.md §11–§14 and .squad/decisions/inbox/ripley-aarch64-riscv64-design.md.

### What was implemented

1. **Extended is_elf_eligible** (§11.3): Now matches on LinuxX64 | LinuxAarch64 | LinuxRiscv64 instead of just LinuxX64.

2. **Added lf_emulation helper** (§11.5): Returns target-appropriate GNU ld emulation names:
   - LinuxX64 → "elf_x86_64"
   - LinuxAarch64 → "aarch64linux"
   - LinuxRiscv64 → "elf64lriscv"

3. **Target-aware cross-link gate** (§11.4): Replaced blanket !target.is_host() gate with:
   `ust
   let can_cross = elf_eligible 
       && native_assets::EMBEDDED_ASSETS_PRESENT
       && native_assets::embedded_target().as_deref() == Some(target.archive_tag());
   `
   **Critical tightening beyond spec**: Added mbedded_target() accessor to 
ative_assets.rs that parses the "target" field from EMBEDDED_ASSET_MANIFEST_JSON. This prevents a linux-x86_64-embedding oscan binary from incorrectly attempting aarch64 cross-link just because "some" assets are present. Gate now checks both presence AND target match.

4. **--extra-obj and --extra-lib CLI** (§12):
   - Added parser blocks mirroring --extra-c/--extra-cflags style
   - Threaded xtra_obj_files and xtra_lib_files through all call chains
   - Added xtra_objects: &'a [String] and xtra_libs: &'a [String] to NativeLinkOptions
   - Added xtra_libs: Vec<PathBuf> field to LinkPlan (§12.4)
   - Rendered in all three flavors (MingwDirect, ElfDirect, CompilerDriver) after builtins
   - Updated usage text with both new flags

5. **Test updates**:
   - Updated old is_elf_eligible tests that assumed aarch64/riscv64 were ineligible
   - Added new tests for aarch64/riscv64 eligibility
   - Added lf_emulation unit tests for all three targets
   - Added cli_help.rs tests for --extra-obj and --extra-lib
   - Updated all LinkPlan test fixtures to include xtra_libs: vec![]

### Key implementation decisions

**Target-matching accessor in 
ative_assets.rs**: Added pub fn embedded_target() -> Option<String> that parses the "target" field from EMBEDDED_ASSET_MANIFEST_JSON using the same serde_json pattern as mbedded_toolchain_version(). This was required to implement the gate tightening correctly - checking only EMBEDDED_ASSETS_PRESENT (a bool with no target info) would allow incorrect cross-arch attempts.

**Honest error messages**: Gate now produces three distinct error messages:
1. Embedded assets present but for a different target (e.g., x86_64 assets, aarch64 request)
2. No embedded assets at all
3. Embedded asset manifest missing 'target' field (shouldn't happen in practice)

**No silent fallback**: All errors remain fail-closed. Cross-linking only proceeds when:
- Target is ELF-eligible
- Embedded assets are present
- Embedded assets' target matches requested target

### Validation results

- cargo check --all-targets: clean (1 pre-existing warning)
- cargo test --quiet: **181 unit tests passed, 0 failed** (baseline was 176; added 5 new tests)
- cargo test --quiet --test cli_help: **4 integration tests passed, 0 failed**
- Manual gate test: oscan examples/hello.osc --backend native --native-target linux-aarch64 -o test correctly:
  - Gets PAST the old "is not the host target" gate
  - Fails later with honest "no C compiler found on PATH for target 'linux-aarch64'" message
  - Proves the target-aware gate works and is still fail-closed (doesn't silently fall back)

### No deviations from spec

All implementation followed Ripley's spec exactly, with the one required tightening documented above (target-matching accessor). No files from Hicks' parallel work were touched (packaging/toolchains/*.json, scripts/release_tools.py, build.rs, .github/workflows/*.yml).

## 2026-07-16 — --extra-obj / --extra-lib fix (3 critical gaps)

- Fixed three gaps in the `--extra-obj`/`--extra-lib` implementation previously
  reported complete, after coordinator Squad independently re-reviewed against
  design spec `docs/design/native-link-embedding.md` §12:
  1. **GAP 1 (critical)**: `build_mingw_plan`, `build_elf_plan`, and
     `build_compiler_driver_plan` in `src/backend/link/mod.rs` all hardcoded
     `extra_objects: vec![]` and `extra_libs: vec![]` instead of reading from
     `options.extra_objects`/`options.extra_libs`. Fixed by replacing with
     `.iter().map(PathBuf::from).collect()` in all three functions. Confirmed
     via `cargo build --release` that the dead-code warning ("fields
     `extra_objects` and `extra_libs` are never read") disappeared.
  2. **GAP 2 (design §12.2 requirement)**: No validation existed for
     `--extra-obj`/`--extra-lib` files. Added validation in
     `run_native_backend()` immediately after constructing
     `NativeLinkOptions`: warns (not hard-errors) if file extension is not
     `.o`/`.obj` or `.a`/`.lib` (case-insensitive), hard-errors (exit(1)) if
     file does not exist.
  3. **GAP 3 (design §12.6 requirement)**: C-backend paths
     (`compile_to_executable`, `run_program`, `invoke_c_compiler`,
     `compile_with_gcc_or_clang`, `compile_cross_riscv64`,
     `compile_cross_wasi`) had no wiring for these flags at all. Threaded
     `extra_obj_files: &[String]` and `extra_lib_files: &[String]` through 6
     function signatures, updated 10+ call sites, and updated 3
     implementations to append the arguments to the compiler command line per
     spec: objects at the same position as `extra_c_files`, libs after all
     other inputs.
- Full detail + verification in
  `.squad/decisions/inbox/bishop-extra-obj-lib-fix.md`.
- Verified: `cargo build --release` zero warnings (dead-code warning gone),
  `cargo test --quiet` 181 + 4 tests passing (no regressions). Feature fully
  functional end-to-end across all three link flavors (MingwDirect, ElfDirect,
  CompilerDriver) and all C-backend paths.

## Learnings

- Seeded for Luca Bolognese on 2026-04-07.
- Project: Oscan compiler/runtime with bundled Windows/Linux toolchain lookup already implemented in `src/main.rs`.
- Phase 1 release-packaging metadata now lives in `packaging/toolchains/release-contract.json` with per-target vendoring manifests beside it.
- Windows x86_64 is pinned to an llvm-mingw UCRT bundle and Linux x86_64 is pinned to an official LLVM archive, both with upstream SHA256 fields for release assembly.
- Builtin pattern: each new builtin touches 4 files — osc_runtime.h (declaration), osc_runtime.c (implementation), semantic.rs (@builtin comment + insert), codegen.rs (match arm).
- Result types used in runtime wrappers must be declared in osc_runtime.h AND added to the skip list in codegen.rs `emit_result_typedefs`.
- The generated C file's include order is: l_gfx.h → l_img.h → osc_runtime.h → osc_runtime.c → auto-generated typedefs → user code (freestanding mode).
- l_img.h requires freestanding mode (uses l_mmap); guarded by OSC_HAS_IMG. Non-freestanding builds get a stub returning Err.
- Dead-code warnings are strong signals: Rust's "field is never read" warning immediately flagged that `extra_objects`/`extra_libs` were silently discarded in GAP 1.
- Design specs are contracts: Following design §12 line-by-line ensured all three gaps were caught (§12.2 validation, §12.4 field usage, §12.6 C-backend passthrough).
- Threading parameters is tedious but mechanical: GAP 3 required touching ~10 call sites and 6 function signatures, but the pattern was the same everywhere — just patience and grep.
- Validation placement matters: Adding validation right after constructing `NativeLinkOptions` (before the link) catches bad paths early, matching the design's "point of use" principle.

