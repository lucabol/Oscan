# Decisions Archive

Entries older than 2026-06-16 (archived from decisions.md on 2026-07-16).

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

## 2026-07-14

### Native Link Embedding — Architecture & Design (Ripley)
- Windows x86-64 freestanding now compiles without external C compiler/linker via embedded `ld.lld` + MinGW runtime assets
- Scope: Windows x86-64 freestanding, `--backend native`, no explicit `.c` files. Deferred: Linux, macOS, hosted mode, user `.c` input.
- `src/backend/link.rs` becomes `src/backend/link/` (directory module) with 6 submodules: `plan.rs` (LinkPlan/LinkerFlavor render), `archive.rs` (manifest inspection), `capability.rs`, `driver.rs` (migration table), `execute.rs`, `mod.rs`.
- New `src/backend/native_assets.rs`: content-addressed cache (`%LOCALAPPDATA%\oscan\native-assets\`), atomic writes, concurrency safety, `.complete` markers, sha256 verification, smoke-check linker launch.
- `runtime-archive-contract.json` schema bumped to v2: adds `contains_native_shim`, `native_shim_member` (derived from actual archive inspection), `schema_version`.
- 13-file embedded asset set for Windows (updated mid-session): `ld.lld.exe` + 6 MinGW import libraries + 5 `linker_runtime` DLLs (~85.4 MB total, 22.6% of full 378.8 MB toolchain).
- Status: IMPLEMENTED (with mid-session DLL-dependency correction)

### Native Link Embedding — Rust Implementation (Bishop)
- Split `src/backend/link.rs` into 6-module `link/` directory per design §2.1.
- `LinkPlan` and `LinkerFlavor::MingwDirect` fully implemented with exact argv rendering (locked by snapshot tests).
- `src/backend/native_assets.rs`: extraction, caching, path-traversal validation, atomic rename, Unix exec-bit handling, in-process sha256 memoization, `.complete` marker protocol.
- Detected 6-DLL dynamic-linking dependency in `ld.lld.exe` (initial design assumed 8 files / ~6.7 MB; actual 13 files / ~85.4 MB after coordinator verification).
- Added `smoke_check_linker` post-extraction (runs `--version`, special-cases `STATUS_DLL_NOT_FOUND` 0xC0000135 exit code for missing DLL diagnostics).
- `Cargo.toml` adds `sha2 = "0.10"` to both `[dependencies]` and `[build-dependencies]`.
- 130 unit tests + 2 integration tests passing; full end-to-end: 6,656-byte `hello.exe` with embedded linker.
- Status: IMPLEMENTED

### Native Link Embedding — Release Engineering & CI (Hicks)
- `runtime-archive-contract.json`: schema v2, `contains_native_shim` auto-detected from compiled archive membership, backward-compatible loader accepts schema 1 or 2.
- `scripts/release_tools.py`: new `prepare-embed-assets` subcommand stages 13 files (13-file set with new `linker_runtime` role for 5 DLLs; was 8 files until mid-session correction).
- `build.rs`: json_mini parser (no new dependencies), `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS` contract, dev builds default to `EMBEDDED_ASSETS_PRESENT=false` (no network, no required assets).
- Release `package` job reordered: fetch toolchain → build runtime archives (with precompiled shim) → `prepare-embed-assets` (13 files) → `cargo build --release` (embeds) → assemble.
- `assemble-release.ps1` gains optional `-PrebuiltRuntimeArchiveDir` to reuse pre-built archives, avoiding redundant rebuild.
- CI: new `native-link-embedding-smoke` job (non-blocking) validates embedded linker works after `cargo build --release`.
- Python test suite (45/45 tests) passing with corrected asset-count assertions (13 files, 5 linker_runtime entries).
- Status: IMPLEMENTED

### Native Link Embedding — Validation & Bug Fixes (Vasquez)
- Validated full end-to-end pipeline; found and fixed 3 bugs:
  1. Hosted mode (`--libc --backend native`) wrongly eligible for MingwDirect linker → extracted `is_mingw_eligible` check, added runtime-mode guard, 4 regression tests.
  2. Direct linker execution failure (e.g., broken linker binary) lacked no-silent-fallback diagnostic → wrapped `link_executable` error path in `no_silent_fallback_error` wrapper.
  3. `scripts/smoke-release.ps1` asserted stale "bundled" expectation → added per-target expected-link-source variable (embedded for Windows freestanding, bundled for other platforms).
- 2 new test suites formalizing coordinator's manual proof:
  - `tests/native_link_isolation.tests.ps1`: true toolchain removal (rename dir, PATH scrub of gcc/clang/cl), compile+run `hello.osc`, assert 6,656 B output, cache reuse.
  - `tests/native_link_concurrency.tests.ps1`: N concurrent `oscan.exe` processes (real `Start-Job` children), cold isolated cache, all produce byte-identical 6,656 B outputs, cache converges to 1 asset-set directory.
- Full validation checklist: PASS (isolation, cache reuse, concurrency, byte parity, import scanning, no-silent-fallback behavior).
- Rust tests (140 unit + 2 integration) and Python tests (45/45) passing; C oracle (99 positive + 35 negative + 96 freestanding) verified.
- Status: VALIDATED (3 bugs fixed, 2 new test suites added)

### Native Link Embedding — Documentation (Newt)
- `README.md`: new "Self-contained native builds (Windows)" subsection, updated "Supported targets" table Windows x86_64 row, caveat to "Build from Source" (local `cargo build` does not embed assets, still needs external toolchain).
- `docs/releasing.md`: new "## Embedded native-link assets for self-contained Windows native builds" section documenting schema v2, `prepare-embed-assets` step, reordered package job, 13-file ~85.4 MB asset count.
- `docs/guide.md` and `docs/spec/oscan-spec.md`: added "Exception — Windows freestanding native builds" paragraphs to existing "Native Toolchain Lookup" sections (no external compiler needed for this scope).
- `src/main.rs` `print_usage()`: added 3 lines documenting `OSCAN_NATIVE_LINKER`, `OSCAN_NATIVE_LINKER_FLAVOR`, `OSCAN_NATIVE_ASSET_CACHE_DIR` environment variables.
- All docs comply with honesty rule (design §1.2): scope precisely to Windows x86-64 freestanding --backend native, explicit non-coverage of Linux, macOS, hosted, user .c files.
- `cargo check` and `tests/cli_help.rs` passing (no behavior change, purely additive).
- Status: IMPLEMENTED

### Mid-Session Correction: `ld.lld.exe` Dynamic DLL Dependencies
- Initial design estimate: 8 files, ~6.7 MB.
- Coordinator discovered `ld.lld.exe` is dynamically linked against 6 sibling DLLs (not static); manual link of `hello.osc` required 13-file set (~85.4 MB) to produce working 6,656-byte executable.
- Ripley amended design doc §4.1–4.2 with corrected asset list, `linker_runtime` role vocabulary, byte accounting, and confirmation of byte-for-byte output parity.
- Bishop added `smoke_check_linker` to `native_assets.rs` (post-extraction launch test, special-casing `STATUS_DLL_NOT_FOUND` exit code for future similar failures).
- Hicks updated `EMBED_ASSET_SPECS["windows-x86_64"]` to 13 files with new `linker_runtime` role, re-staged against real toolchain, updated Python test assertions.
- All work remained correct; no silent fallback, no regressions; defense-in-depth check added.
- Status: CORRECTED (same session, all agents' halves updated in parallel)

### Native Link Embedding — Final Sign-off (Ripley)
- **Verdict:** GO — shippable for declared scope (Windows x86-64 freestanding self-contained native linking).
- **Verification:** `cargo check --all-targets` clean (2 pre-existing warnings); `cargo test --quiet` 140 unit + 2 integration, all passing.
- **Code-design match verified:** No-silent-fallback text wired for both extraction and execute failures (MingwDirect-only); `is_mingw_eligible` correctly gates on Windows/Freestanding/no-extra-c; field-name contract intact across Rust/Python; docs honestly scoped to Windows x86-64 freestanding.
- **End-to-end proven:** Coordinator verified toolchain dir renamed away, PATH scrubbed, `hello.osc` → exactly 6,656 B executable, cache populates and reuses correctly.
- **3 real bugs fixed and regression-guarded** (Vasquez: hosted-eligibility, no-fallback-diagnostic, stale-smoke-assertion).
- **Residual risks flagged (not blockers):** Response-file long-argv path untested; package-level true-isolation in `smoke-release.ps1` deferred (dev-level true-isolation proven); embedded asset path non-blocking CI; toolchain-bump maintenance surface noted.
## 2026-07-15

### Linux x86-64 ElfDirect: Architecture & Design (Ripley)

Extended the native-link-embedding design to Linux x86-64 freestanding. Reuse the pinned `packaging/toolchains/linux-x86_64.json` toolchain's `bin/x86_64-linux-musl-ld` (GNU ld 2.37, static 32-bit ELF binary, ~2.78 MB) as `LinkerFlavor::ElfDirect`. No new toolchain fetch/pin required; rejected standalone `ld.lld`/LLVM (~100+ MB, dynamically linked) as strictly worse on maintenance and payload.

**Exact argv:**
```
-s -m elf_x86_64 -static --gc-sections --build-id=none -o <output> <objects> <archive>
```

No `-nostdlib`, no `-l`/`-L`, no builtins, no system libs, no entry-point flag. Byte-identical output to current `CompilerDriver` path verified for `hello.osc`, `gfx_demo.osc`, and `tls_fetch.osc`.

**Embedded asset set:** 1 file (~2.78 MB) versus Windows' 13 files (~85.4 MB).

**AArch64/RISC-V64 blocker:** Explicitly out of scope. No runtime archives, no pinned cross-toolchains, no QEMU validation infrastructure.

- **Status:** IMPLEMENTED-DESIGN (ratified; implementation follows via Bishop/Hicks)

### Linux x86-64 ElfDirect: Rust Implementation (Bishop)

Split `src/backend/link.rs` into 6-module `link/` directory per design. Implemented `LinkerFlavor::ElfDirect` and `LinkerSelection::Elf` in `src/backend/link/{plan,mod,driver}.rs` with exact argv rendering matching design §10. Added `src/backend/native_assets/unix_elevation.rs` with `getegid()`/`getgid()` FFI for setuid detection on Unix.

**Key decisions:**
1. **No toolchain-version cross-check for Linux:** The Linux toolchain manifest's `version` field is a fixed distribution name (e.g., "musl-cross-make 2023-08-28"), not semver-style. Unlike Windows where drift is meaningful, the check would always pass vacuously. Documented in code comment; revisit if versioning evolves.
2. **`reject_elf_flavor` removed entirely:** Match arms in `resolve_linker_selection` now handle `"elf"` as concrete patterns alongside `"mingw"`, making a validation helper unnecessary.

**Byte-parity proof:** SHA-256 `a399954cd11bba6c21d1afad3ebfcb8f1c8a4faaa22c76b75d9ac3298edf4247`, 4,744 bytes. Both `CompilerDriver` (via musl-gcc) and `ElfDirect` (direct `x86_64-linux-musl-ld`) produce identical output for `examples/hello.osc`, matching the design document's own proven hash.

- **Status:** IMPLEMENTED

### Linux x86-64 ElfDirect: Rust Implementation — Bug Fixes (Bishop)

Three small gaps identified by coordinator's independent review. All three addressed exactly as specified:

1. **GID check in `unix_elevation.rs`:** Added `getegid()`/`getgid()` to FFI extern block; elevation check now `euid != uid || egid != gid` (not just UID).
2. **Symmetric Unix re-check in `ensure_extracted()`:** Added `#[cfg(unix)] check_elevation_policy(is_setuid_elevated(), ...)` right after existing `#[cfg(windows)]` line.
3. **Stronger `resolve_linker_selection` test:** Added `LINKER_ENV_TEST_LOCK: Mutex<()>` following `archive.rs` pattern; two new tests call `resolve_linker_selection()` with real env vars; kept original weak "enum constructible" test for compile-time check.

**Validation:** `cargo check --all-targets` clean; `cargo test --quiet` x2: 179 total passed, 2 pre-existing Windows-only failures (identical both runs, no new flakes).

- **Status:** IMPLEMENTED

### Linux x86-64 ElfDirect: Release Engineering & CI (Hicks)

Extended release tooling to Linux x86-64:

1. **BearSSL Injection Step:** KEEP existing "Inject pre-built libbearssl.a into bundle (Linux only)" unchanged. Serves as defense-in-depth for non-embedded scenarios (fallback `CompilerDriver` path, standalone `assemble-release.ps1` invocations). Redundant for default `ElfDirect` but necessary for robustness.
2. **Linux Embed Asset Spec:** Added `"linux-x86_64"` entry to `EMBED_ASSET_SPECS` in `scripts/release_tools.py`:
   - Single linker: `x86_64-linux-musl-ld` (~2.78 MB, static binary)
   - Flavor: `"elf"`, Emulation: `"elf_x86_64"`
   - Empty `linker_runtime` (no sibling DLLs — musl linker is fully static)
   - Empty `import_libs` (Linux freestanding has no import libraries)
   - **No `compiler_builtins` key** (musl toolchain supplies intrinsics via static linking)
3. **Conditional `compiler_builtins` Handling:** Modified `prepare_embed_assets()` to conditionally access via `.get()` instead of unconditional dict access. Windows still emits its `compiler_builtins` entry; Linux correctly omits it.
4. **smoke-release.ps1 Updates:** Updated `$expectedNativeLinkSource` to expect `"embedded"` for both Windows and Linux freestanding; extended non-Windows PATH stubbing to block `ld`, `x86_64-linux-musl-gcc`, `x86_64-linux-musl-ld`.
5. **CI Linux Smoke Test:** New `native-link-embedding-smoke-linux` job in `.github/workflows/ci.yml`. Mirrors Windows structure: fetch toolchain, build archives, stage assets, build with `OSCAN_REQUIRE_EMBEDDED_ASSETS=1`. PATH restriction via stub directory. Verification via `readelf -l` (no `PT_INTERP`) and `readelf -d` (no `NEEDED`) to prove static linking. Marked `continue-on-error: true`.

Python test suite: 48/48 passing with corrected asset-count assertions (1 file for Linux).

- **Status:** IMPLEMENTED

### Linux x86-64 ElfDirect: build.rs Asset Count Fix (Hicks)

**Problem:** `build.rs`'s `load_and_verify_embedded_assets()` had overly strict check `if manifest_assets.len() < 2 { return Err(...) }`. Assumed every target would have ≥1 asset besides linker. True for Windows (12 import libraries) but false for Linux (empty `"assets": []` array; just the linker — no import libraries, no compiler_builtins). This blocked the entire Linux ElfDirect feature from building with `OSCAN_REQUIRE_EMBEDDED_ASSETS=1`.

**Root cause:** The `linker` field is already mandatory (enforced lines above via `.ok_or("manifest is missing 'linker'")`), so `manifest_assets` can never be empty by construction. The `< 2` check was unjustified.

**Fix:** Removed the `if manifest_assets.len() < 2 { ... }` check entirely. Mandatory `linker` field ensures we have at least one asset.

**Verification:**
1. Linux build with embedded assets: ✓ Succeeded in 27s
2. End-to-end Linux ElfDirect link: ✓ Succeeded, output `Linking freestanding executable with .../x86_64-linux-musl-ld (embedded)...`
3. `cargo test`: 175 passed (2 pre-existing Windows-only failures unrelated)

- **Status:** FIXED

### Linux x86-64 ElfDirect: Validation (Vasquez)

Full validation battery executed; 10/11 checklist items passing, 1 packaged-smoke-test deferred (test-harness gap, not impl bug).

**Key results:**
1. ✅ Regenerate embedded assets + build: manifest shape correct, cargo build clean
2. ✅ Hosted-mode regression: `is_elf_eligible` correctly excludes `RuntimeMode::Hosted` (no duplication of Windows's historical bug)
3. ⚠️ True isolation proof script: script created and logic sound (coordinator fixed PowerShell array-to-bool parameter binding via `-join "`n"` wrapper); now passes cleanly
4. ✅ Concurrency test: 8 concurrent `oscan` processes raced cold cache, all 8 outputs byte-identical (SHA-256 `a399954cd11bba6c21d1afad3ebfcb8f1c8a4faaa22c76b75d9ac3298edf4247`), cache converged to 1 directory
5. ✅ Unit tests (corruption, concurrency, traversal): all asset-shape-agnostic (parameterized via `make_asset()`, cover 1-file Linux case by construction)
6. ✅ `OSCAN_NATIVE_LINKER_FLAVOR=elf` override path: code path confirmed, override correctly bypasses embedded extraction
7. ✅ AArch64/RISC-V64 regression: no regression; AArch64 link still fails with "not the host target" (unchanged)
8. ✅ Representative examples: `hello.osc` via embedded ElfDirect (4,744 B, fully static, runs correctly)
9. ✅ Python test suite: 48/48 passing
10. ⏳ Packaged release smoke test: blocked by prebuilt archive metadata gap (test-harness issue, not impl bug); should run in CI's full pipeline
11. ✅ `cargo check --all-targets` and `cargo test --quiet`: 179 passed (2 pre-existing Windows-only), no flakes/regressions

**No bugs found in implementation.** Pre-active search for historical bug classes (hosted-mode eligibility, PATH-stubbing gaps, missing sibling DLLs) found none. Linux's static `x86_64-linux-musl-ld` has zero shared-library dependencies (unlike Windows `ld.lld.exe`'s 6 DLLs), simplifying security profile.

**Confidence:** HIGH. Byte-parity proven, concurrency clean, isolation proven, full unit/integration test coverage.

- **Status:** VALIDATED (3 deferred items later completed by coordinator follow-up)

### Linux x86-64 ElfDirect: Documentation (Newt)

Documented the completed Linux x86-64 `ElfDirect` native-link embedding feature across README.md, docs/releasing.md, docs/guide.md, and docs/spec/oscan-spec.md, mirroring Windows `MingwDirect` documentation from 2026-07-14.

**Key changes:**
1. **README.md:** Renamed "Self-contained native builds (Windows)" → "Self-contained native builds (Windows & Linux)"; added payload-size bullet (1 file/~2.78 MB vs 13 files/~85.4 MB); removed "Linux native builds (still uses...)" bullet; replaced with "Linux AArch64/RISC-V64 **native backend** builds" with explicit C-backend clarification; updated supported-targets table row; added `OSCAN_NATIVE_LINKER_FLAVOR=elf` mention.
2. **docs/releasing.md:** New section "Embedded native-link assets for self-contained Linux native builds" parallel to Windows section; covered `prepare-embed-assets --target linux-x86_64`, payload details, reordered release job steps, `native-link-embedding-smoke-linux` CI job.
3. **docs/guide.md & docs/spec/oscan-spec.md:** Renamed/expanded "Exception — Windows freestanding native builds" → "Exception — Windows and Linux x86-64 freestanding native builds"; added Linux cache path, payload details, explicit ARM64/RISC-V64 native-backend clarification, `OSCAN_NATIVE_LINKER_FLAVOR=elf` mention.

**Backend disambiguation:** Explicitly noted "Linux AArch64/RISC-V64 **native backend** builds" wherever ARM64/RISC-V64 came up, with clarification that C backend's existing ARM64/RISC-V64 support is unrelated. Prevents reader confusion between native backend (Cranelift AOT) and C backend (default transpiler).

**Validation:** `cargo test --quiet --test cli_help`: 2/2 tests pass.

- **Status:** IMPLEMENTED

### Bugs Found & Fixed (Coordinator-Verified)

Coordinator's independent hands-on verification found and got fixed 6 bugs across the pipeline:
1. **build.rs asset-count bug (found by coordinator, fixed by Hicks):** Overly strict `manifest_assets.len() < 2` check blocked Linux builds; removed (see "build.rs Asset Count Fix" above).
2. **GID-check gap (found by coordinator, fixed by Bishop):** Elevation check on Unix only compared UIDs; added UID+GID check.
3. **Belt-and-suspenders symmetry gap (found by coordinator, fixed by Bishop):** Unix lacked re-check after extraction; added symmetric `#[cfg(unix)] check_elevation_policy()`.
4. **Pre-existing Windows-only test portability bug 1 (found by coordinator, fixed by Bishop):** Test `validated_dest_rejects_absolute_and_traversal_paths` hardcoded `C:\evil\...` without `#[cfg(windows)]` guard; Linux treats it as relative path. Fixed guard.
5. **Pre-existing Windows-only test portability bug 2 (found by coordinator, fixed by Bishop):** Test `smoke_check_result_reports_missing_sibling_dll_distinctly_from_a_hash_mismatch` tests Windows DLL behavior without `#[cfg(windows)]` guard. Fixed guard.
6. **Stale doc cross-reference (found by coordinator, fixed by Newt):** `README.md` referenced outdated design doc section number (§9.2 instead of §10). Fixed to §10.

All bugs genuinely closed; no regressions.

- **Status:** ALL FIXED

### Linux x86-64 ElfDirect: Final Sign-off (Ripley)

**Verdict:** GO — shippable for declared scope (Linux x86-64 freestanding self-contained native linking).

**Verification:**
- `cargo check --all-targets` clean
- `cargo test --quiet`: 180 unit tests + 2 integration tests, all passing
- Byte-parity proven: identical `hello.osc` output to existing `CompilerDriver` path (SHA-256 `a399954cd11bba6c21d1afad3ebfcb8f1c8a4faaa22c76b75d9ac3298edf4247`)
- True isolation proven: toolchain renamed away, PATH scrubbed, embedded `x86_64-linux-musl-ld` used, output correct
- Concurrency proven: 8 concurrent processes produce byte-identical outputs, cache converged correctly
- Full packaged-release smoke test: passes (CI validates fresh-built artifacts; dev-level isolation already proven)
- 93/99 representative test corpus passing; 6 remainders are pre-existing/unrelated native-backend codegen gaps or "hosted-mode-needs-a-compiler-as-designed"
- Security review: clean, no findings

**Code-design match verified:** `is_elf_eligible` correctly gates on Linux/Freestanding/no-extra-c; `render_elf_direct()` produces exact argv per design §10; docs honestly scoped to Linux x86-64 freestanding.

**3 real bugs fixed and regression-guarded** (coordinator-found, agent-fixed across Hicks/Bishop/Newt).

**Residual risks flagged (not blockers):** Response-file long-argv path untested (design deferred); ARM64/RISC-V64 explicit out-of-scope; hosted/`.c`-input unchanged.

- **Status:** SIGN-OFF APPROVED

## 2026-07-14

### Native Link Embedding — Final Sign-off (Ripley)

**Verdict:** GO — shippable for declared scope (Windows x86-64 freestanding self-contained native linking).

**Verification:** `cargo check --all-targets` clean (2 pre-existing warnings); `cargo test --quiet` 140 unit + 2 integration, all passing.

**Code-design match verified:** No-silent-fallback text wired for both extraction and execute failures (MingwDirect-only); `is_mingw_eligible` correctly gates on Windows/Freestanding/no-extra-c; field-name contract intact across Rust/Python; docs honestly scoped to Windows x86-64 freestanding.

**End-to-end proven:** Coordinator verified toolchain dir renamed away, PATH scrubbed, `hello.osc` → exactly 6,656 B executable, cache populates and reuses correctly.

**3 real bugs fixed and regression-guarded** (Vasquez: hosted-eligibility, no-fallback-diagnostic, stale-smoke-assertion).

**Residual risks flagged (not blockers):** Response-file long-argv path untested; package-level true-isolation in `smoke-release.ps1` deferred (dev-level true-isolation proven); embedded asset path non-blocking CI; toolchain-bump maintenance surface noted.

- **Status:** SIGN-OFF APPROVED

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

