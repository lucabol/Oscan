# Native Link Embedding — Design Document

**Status:** APPROVED FOR IMPLEMENTATION (design pass, Ripley, 2026-07-14)
**Implements:** approved research report
`research/how-do-we-remove-the-dependency-on-the-c.md`
**Owners of implementation:** Bishop (Rust / toolchain integration), Hicks
(release engineering / Python / CI). Vasquez validates; Newt documents.

This document is the single, file-level contract for removing the
C-compiler/linker dependency from default `oscan --backend native`
(freestanding) builds on **Windows x86-64**. It is written so Bishop and Hicks
can implement in parallel with **zero file overlap**, and so "did you follow the
design" is mechanically checkable at review.

---

## 1. Scope

### 1.1 In scope now

- **Windows x86-64, freestanding** (`--backend native`, no `--libc`): compile +
  link a standalone `.exe` with **no** `clang`/`gcc`/`cc`/`cl` and **no**
  externally installed linker on `PATH`. The shipped `oscan.exe` carries its own
  linker and link inputs.
- **Linux x86-64, freestanding** (`--backend native`, no `--libc`): compile +
  link a standalone ELF binary with **no** `gcc`/`cc`/`musl-gcc` and **no**
  externally installed linker on `PATH`. The shipped `oscan` binary carries its
  own linker (a single ~2.78 MB static binary). See §10.
- **Precompiling `runtime/osc_native_shim.c`** into every runtime archive so no
  C compilation happens during a native build.
- **Direct MinGW-flavor `ld.lld`** invocation (`-m i386pep`, GNU flavor — **not**
  `lld-link`), reproducing the proven 6,656-byte `hello.osc` output.
- **Single-file embedding**: `ld.lld.exe`, its **5 required runtime DLLs**, the
  required MinGW import libraries, compiler-builtins, and an asset manifest are
  embedded into `oscan.exe` at compiler build time; extracted at runtime to a
  verified, concurrency-safe cache; invoked directly (no shell). The corrected
  minimal Windows asset set is **13 files, ≈ 85.4 MB** (see §4.1).
  > **`ld.lld.exe` is NOT a static standalone binary.** It dynamically links
  > against 5 sibling DLLs (`libLLVM-22.dll`, `libwinpthread-1.dll`,
  > `libunwind.dll`, `libffi-8.dll`, `libc++.dll`) that live in the toolchain's
  > `bin/`. Copied alone into an empty directory it fails to launch
  > (`STATUS_DLL_NOT_FOUND`). These DLLs MUST be embedded and co-located with
  > `ld.lld.exe` in the cache (same `bin/` subdirectory) so Windows' default
  > module-directory DLL search resolves them with **no PATH manipulation**.
  > `libclang-cpp.dll` (~47 MB) is confirmed **not** required by `ld.lld.exe` and
  > is deliberately excluded.
- A **cross-platform-shaped** asset/cache/plan abstraction (so Linux is a later
  data/parameter change, not a rewrite).

### 1.2 Explicitly deferred (must NOT be claimed as done by docs/CLI)

| Deferred item | Interim behavior |
|---|---|
| Linux AArch64 / RISC-V64 **direct/cross** link | Object-only via Cranelift cross-codegen; final link errors (`link_executable`'s `target.is_host()` gate) exactly as today — no runtime archive exists for these targets in `runtime-archive-contract.json`'s `freestanding`/`freestanding_core` `supported_targets` (only `linux-x86_64`/`windows-x86_64`), and no pinned musl cross-toolchain manifest exists under `packaging/toolchains/`. Implementing this is a comparable-sized follow-up (2 more pinned toolchains, cross-compiled BearSSL, runtime archives per arch, QEMU validation, and removing/extending the single-host-target `link_executable` gate) — explicitly out of scope for this pass. |
| macOS native target | No `NativeTarget` variant exists; out of scope entirely. |
| Hosted `--libc` mode direct-link | Keeps the diagnosed external C-toolchain driver path. |
| Explicit user-supplied `.c` files | Activates the external C-toolchain path, clearly diagnosed. |
| Embedding the full compiler-driver toolchain | Never embedded. Only the Windows ≈85.4 MB / 13-file + Linux ≈2.78 MB / 1-file minimal linker asset sets are embedded. |

**Honesty rule (requirement #6):** any doc/CLI text must say "Windows and Linux
x86-64 freestanding native builds are self-contained" and must **not** imply
AArch64, RISC-V64, macOS, hosted, or `.c`-input builds are toolchain-free.

---

## 2. `LinkPlan` design and module layout

### 2.1 Module layout — split `link.rs` into `src/backend/link/`

`src/backend/link.rs` (1145 lines) becomes a directory module. Rationale: the
research (§7) asks for six separable responsibilities; a single file cannot be
unit-tested cleanly because plan construction is entangled with `Command`
execution. The split isolates the **pure, unit-testable** plan/flavor code from
side-effecting discovery/execution.

| New file | Contents (moved/added) | Responsibility (§7 mapping) |
|---|---|---|
| `src/backend/link/mod.rs` | `pub fn link_executable`, `NativeLinkOptions`, orchestration, `pub use` re-exports. Preserves the existing module doc comment (lines 1–128) verbatim, extended with a "Direct linker & embedding" section. | Orchestration |
| `src/backend/link/archive.rs` | `find_or_build_runtime_archive`, `archive_name`, `FreestandingProfile`, `RuntimeArchiveManifest` + `parse_runtime_manifest`/`read_manifest`/`read_link_flags` (+ new shim fields). | Manifest resolution |
| `src/backend/link/capability.rs` | `detect_windows_feature_libs`, `program_needs_graphics_runtime`. | Capability analysis |
| `src/backend/link/plan.rs` | **New.** `LinkPlan`, `LinkerFlavor`, `LinkerExecutable`, `SystemLib`, pure `LinkPlan::render(&self) -> Vec<OsString>` per flavor. No I/O, no `Command`. | Plan construction + flavor rendering |
| `src/backend/link/driver.rs` | Legacy `find_linker_driver`, `LinkerDriver`, `LinkerFamily`, `validate_manifest_driver`, `find_compiler_builtins_lib`, `compile_shim_object` (now hosted/legacy-only). | Legacy compiler-driver flavor rendering + discovery |
| `src/backend/link/execute.rs` | Executes a rendered plan via `Command` (no shell), post-link validation (non-empty output, cleanup-on-failure), verbose logging. | Execution + post-link validation |
| `src/backend/native_assets.rs` | **New, top-level backend module** (not under `link/`). Embedded-asset store: generated-const access, extraction, content-addressed cache, digest verification, cross-platform cache-dir resolution. | Asset lifecycle (feeds `LinkerExecutable` + `SystemLib` paths into the plan) |

`native_assets.rs` is deliberately **not** under `link/` because it is a
process-level asset/cache concern reused independently of any one link and is the
natural home for the concurrency/security logic in §6.

### 2.2 Types (`src/backend/link/plan.rs`)

```rust
/// How a LinkPlan renders to a concrete linker invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkerFlavor {
    /// Direct MinGW-flavor `ld.lld` (`-m i386pep`), the Windows freestanding
    /// default when embedded assets are present. No compiler driver.
    MingwDirect,
    /// Legacy: invoke GCC/Clang as a linker driver (`cc obj shim runtime.a`).
    /// Transitional; hosted mode; explicit `.c` files; dev builds without
    /// embedded assets.
    CompilerDriver,
    // Reserved, NOT implemented in this pass — leave the match arms exhaustive
    // with an explicit `unimplemented!`/error so ELF is a data change later:
    // ElfDirect,
}

/// Where the linker binary came from — drives diagnostics and the
/// no-silent-fallback rule (§7).
#[derive(Debug, Clone)]
pub enum LinkerExecutable {
    /// Extracted from this oscan binary's embedded assets to `path`.
    Embedded { path: PathBuf },
    /// User override via OSCAN_NATIVE_LINKER (+ FLAVOR).
    Override { command: String },
    /// Legacy compiler driver discovered on host / recorded in archive manifest.
    CompilerDriver { command: String, source: crate::CompilerSource },
}

/// A resolved system/import library input (absolute path form preferred for
/// direct flavors; `-l` name form for the compiler driver).
#[derive(Debug, Clone)]
pub struct SystemLib {
    /// e.g. "kernel32"; used to render `-lkernel32` for CompilerDriver.
    pub name: &'static str,
    /// Absolute staged path (Some for MingwDirect: passed positionally).
    pub archive_path: Option<PathBuf>,
}

/// Pure, unit-testable description of a final native link. Construction does no
/// I/O; all paths are already resolved by the caller.
#[derive(Debug, Clone)]
pub struct LinkPlan {
    pub flavor: LinkerFlavor,
    pub linker: LinkerExecutable,
    pub target: NativeTarget,
    pub runtime_mode: RuntimeMode,
    pub output: PathBuf,
    pub objects: Vec<PathBuf>,      // program.obj, shim.o (when compiled locally)
    pub archives: Vec<PathBuf>,     // runtime archive(s), user .a
    pub system_libs: Vec<SystemLib>,// kernel32 + optional ws2_32/user32/...
    pub builtins: Option<PathBuf>,  // libclang_rt.builtins-x86_64.a
    pub search_paths: Vec<PathBuf>, // empty for MingwDirect (absolute inputs)
    pub extra_objects: Vec<PathBuf>,// user .c compiled outputs (driver only)
    pub entry: Option<String>,      // None today (archive supplies _start)
    pub gc_sections: bool,          // true
    pub strip: bool,                // true (-s)
    pub build_id_none: bool,        // true
    pub pie: bool,                  // false; -no-pie only rendered on non-Windows
    pub emulation: Option<&'static str>, // Some("i386pep") for MingwDirect
}

impl LinkPlan {
    /// Pure: renders argv (excluding argv[0], the linker). Unit-tested with
    /// snapshot/exact-vector assertions per flavor — this is the primary
    /// "did we build the right command" test surface Vasquez relies on.
    pub fn render(&self) -> Vec<std::ffi::OsString> { /* ... */ }
}
```

### 2.3 Behavior mapping — nothing regresses

The existing behaviors map onto the plan as follows. Bishop must preserve each:

| Existing behavior (today, in `link_executable`) | New home | Rule |
|---|---|---|
| `FreestandingProfile::Core` vs `Full` via `program_needs_graphics_runtime` | `capability.rs` → sets which archive path goes into `LinkPlan.archives` | Unchanged: Core only when freestanding, no `extra_c_files`, and scan negative. |
| `detect_windows_feature_libs` optional import libs | `capability.rs` → `LinkPlan.system_libs` | Unchanged selection logic. See §2.4 for the LLD-sees-all rule. |
| `-lkernel32` always | `capability.rs` seeds kernel32 unconditionally | Unchanged. |
| `--gc-sections`, `-s`, `--build-id=none` | `LinkPlan.gc_sections/strip/build_id_none` | Unchanged; rendered per flavor. |
| `-no-pie` on non-Windows only | `LinkPlan.pie=false`; renderer emits `-no-pie` only when `target != WindowsX64` and flavor is `CompilerDriver` (MingwDirect Windows never needs it) | Unchanged Windows output. |
| `-nostdlib` | Rendered by `CompilerDriver` flavor only (ld.lld doesn't take it; it never links CRT anyway) | Windows byte-identical output preserved. |
| compiler-builtins re-link (`-print-libgcc-file-name`) | `LinkPlan.builtins` | MingwDirect: **embedded** `libclang_rt.builtins-x86_64.a` staged path. CompilerDriver: `find_compiler_builtins_lib`. Rendered **last**, after archives. |
| Link input ordering (program, shim, user .c, runtime.a, libs, builtins) | Renderer preserves exact left-to-right order | Static archive resolution depends on it. |

### 2.4 MingwDirect argument rendering (exact)

`render()` for `MingwDirect` produces, in order:

```
-s -m i386pep -Bdynamic --gc-sections --build-id=none
-o <output>
<program.obj> <shim member is inside runtime.a — no separate shim.o>
<runtime archive absolute path>
<kernel32.a> [<ws2_32.a> <user32.a> <gdi32.a> <secur32.a> <crypt32.a> as selected]
<libclang_rt.builtins-x86_64.a>
```

Notes locked by this design:

- **No `-nostdlib`, no `-no-pie`, no `-fuse-ld`, no `-l`/`-L`.** Import libs and
  builtins are passed as **absolute positional archive inputs** (ld.lld/GNU ld
  accept `.a` positionally), matching the research's "exact staged path, not
  `-L` over a huge dir".
- **Shim is a member of the runtime archive** (§3), so there is no separate
  `shim.o` object for the MingwDirect/default path. `LinkPlan.objects` contains
  only `program.obj` in the default freestanding case.
- **LLD-sees-all-optional-imports rule preserved:** because the runtime archive
  is Clang-built, LLD diagnoses undefined imports in dead sections before GC.
  For MingwDirect, capability analysis must supply **all five** optional import
  libs whenever the linker family is LLD — i.e. MingwDirect always requests the
  full optional set (ws2_32/user32/gdi32/secur32/crypt32) exactly as the current
  `driver.linker_family == Lld` branch does today (`link.rs:911`). The
  per-symbol `detect_windows_feature_libs` narrowing stays only for the GNU-ld
  compiler-driver path. Final DLL dependency set stays KERNEL32-only after GC.
- Response file: if argv length risks the Windows command-line limit, `execute.rs`
  writes an `@response.rsp` file (UTF-8, quoted) in the per-link temp dir. Not
  expected for `hello.osc`, but required for large `system_libs`/user inputs.

---

## 3. Archive & manifest contract changes (the shim)

### 3.1 `runtime-archive-contract.json` (Hicks owns the file)

- Bump top-level `"schema_version"` **1 → 2**.
- Add `"osc_native_shim.c"` to **each** mode's `"sources"` array (`hosted`,
  `freestanding`, `freestanding_core`). Verified: `build_runtime_archive`
  (`release_tools.py:1864`) loops `mode_spec["sources"]`, compiles each with the
  mode's existing args (`hosted_compile_args`/`freestanding_compile_args`, which
  already match `compile_shim_object`'s flags including `-fno-jump-tables`), and
  `ar rcs`es them together — so this one array edit precompiles the shim into
  every archive with **no other Python change to the compile loop**.
- Add per-mode `"contains_native_shim": true`.

### 3.2 Per-archive manifest (`libosc_runtime_*.json`, written by `build_runtime_archive`)

`build_runtime_archive` (`release_tools.py:1902`) must add to the emitted
manifest dict:

- `"schema_version": 2` (bump from 1).
- `"contains_native_shim": true`.
- `"native_shim_member": "osc_native_shim.o"` (the `ar` member name = source
  stem + `.o`; keep in sync with the compile loop's `Path(src_name).stem + ".o"`).

`"sources"` already lists the compiled sources and will now include
`osc_native_shim.c` automatically — but Rust must key off the **explicit
boolean**, not string-matching `sources`.

### 3.3 Rust reader (`archive.rs`, Bishop owns)

`RuntimeArchiveManifest` gains:

```rust
contains_native_shim: bool, // parsed from "contains_native_shim"; false if absent
```

Parsing rule: absent/missing → `false` (a legacy pre-schema-2 archive).

### 3.4 Shim-presence policy (mechanically checkable)

| Situation | Freestanding (default native) | Hosted (`--libc`) |
|---|---|---|
| `contains_native_shim: true` | Link the archive directly; **do not** compile the shim locally; **do not** search for a compiler. | Same: use the embedded member. |
| `contains_native_shim: false`/absent (legacy archive) | **Hard, actionable error** (no compiler fallback): `error: runtime archive '<path>' predates the precompiled native shim (manifest contains_native_shim is false/absent); rebuild it with 'scripts/build-runtime-archive.ps1 -Mode freestanding' (or fetch a current release). The freestanding native backend no longer compiles osc_native_shim.c locally.` | **Diagnosed local fallback allowed**: emit a one-line warning and fall back to `compile_shim_object` (hosted already requires an external C toolchain per requirement #1). |

`compile_shim_object` therefore stays in `driver.rs` but is reachable **only**
from the hosted legacy-archive fallback path.

---

## 4. Embedded-asset manifest schema

### 4.1 Minimal Windows asset set (re-confirmed against this checkout)

Exactly 13 files, ≈ 85.44 MB total (85,442,946 B; measured
`build/toolchain-windows-x86_64/`, llvm-mingw 20260324):

| role | source (within toolchain) | bytes |
|---|---|---|
| `linker` | `bin/ld.lld.exe` | 5,219,840 |
| `linker_runtime` | `bin/libLLVM-22.dll` | 76,043,264 |
| `linker_runtime` | `bin/libc++.dll` | 2,094,080 |
| `linker_runtime` | `bin/libwinpthread-1.dll` | 274,944 |
| `linker_runtime` | `bin/libunwind.dll` | 203,264 |
| `linker_runtime` | `bin/libffi-8.dll` | 86,528 |
| `import_lib` (kernel32) | `x86_64-w64-mingw32/lib/libkernel32.a` | 578,742 |
| `import_lib` (ws2_32) | `x86_64-w64-mingw32/lib/libws2_32.a` | 170,984 |
| `import_lib` (user32) | `x86_64-w64-mingw32/lib/libuser32.a` | 207,984 |
| `import_lib` (gdi32) | `x86_64-w64-mingw32/lib/libgdi32.a` | 208,456 |
| `import_lib` (secur32) | `x86_64-w64-mingw32/lib/libsecur32.a` | 26,000 |
| `import_lib` (crypt32) | `x86_64-w64-mingw32/lib/libcrypt32.a` | 73,954 |
| `compiler_builtins` | `lib/clang/22/lib/windows/libclang_rt.builtins-x86_64.a` | 254,906 |

**`ld.lld.exe` is dynamically linked**, not static: the 5 `linker_runtime` DLLs
above are its transitive load-time dependencies (verified by isolation-testing
`ld.lld.exe --version` in an empty directory and by a real end-to-end manual link
of `hello.osc`). **Hard requirement:** every `linker_runtime` DLL's
`install_subpath` MUST place it in the **same cache subdirectory as
`ld.lld.exe`** (i.e. all under `bin/`), because the mechanism that makes the
isolated linker launch is Windows' default DLL search of the directory containing
the loaded module — **no PATH entry is added or relied upon**. `libclang-cpp.dll`
(47,435,264 B) is confirmed empirically **unnecessary** for `ld.lld.exe` (it is
only needed by `clang.exe`/`clang++.exe`) and is excluded, saving ~47 MB.

**Not** `lib/clang/22/lib/linux/...` (wrong target). Directories are never
shipped; only these 13 files are staged.

### 4.2 Manifest JSON — `native-link-assets.json`

Staged (not committed) at
`packaging/prebuilt/<target>/native-link-assets.json` by the prepare tool
(§5.4). Shape reuses the runtime-archive manifest's `toolchain` provenance
philosophy (strict, explicit, digest-covered):

```json
{
  "schema_version": 1,
  "target": "windows-x86_64",
  "toolchain": {
    "vendor": "llvm-mingw",
    "version": "20260324",
    "archive_digest": { "algorithm": "sha256", "value": "<...>" }
  },
  "linker": {
    "role": "linker",
    "name": "ld.lld.exe",
    "install_subpath": "bin/ld.lld.exe",
    "flavor": "mingw",
    "emulation": "i386pep",
    "size": 5219840,
    "sha256": "<...>"
  },
  "assets": [
    { "role": "linker_runtime",    "name": "libLLVM-22.dll", "install_subpath": "bin/libLLVM-22.dll", "size": 76043264, "sha256": "<...>" },
    { "role": "import_lib",        "name": "libkernel32.a", "lib": "kernel32", "install_subpath": "lib/libkernel32.a", "size": 578742, "sha256": "<...>" },
    { "role": "import_lib",        "name": "libws2_32.a",   "lib": "ws2_32",   "install_subpath": "lib/libws2_32.a",   "size": 170984, "sha256": "<...>" },
    { "role": "import_lib",        "name": "libuser32.a",   "lib": "user32",   "install_subpath": "lib/libuser32.a",   "size": 207984, "sha256": "<...>" },
    { "role": "import_lib",        "name": "libgdi32.a",    "lib": "gdi32",    "install_subpath": "lib/libgdi32.a",    "size": 208456, "sha256": "<...>" },
    { "role": "import_lib",        "name": "libsecur32.a",  "lib": "secur32",  "install_subpath": "lib/libsecur32.a",  "size": 26000,  "sha256": "<...>" },
    { "role": "import_lib",        "name": "libcrypt32.a",  "lib": "crypt32",  "install_subpath": "lib/libcrypt32.a",  "size": 73954,  "sha256": "<...>" },
    { "role": "compiler_builtins", "name": "libclang_rt.builtins-x86_64.a", "install_subpath": "lib/clang/libclang_rt.builtins-x86_64.a", "size": 254906, "sha256": "<...>" }
  ]
}
```

**Field-name contract (Hicks writes ⇄ Bishop reads — must match exactly):**
`schema_version`, `target`, `toolchain.vendor`, `toolchain.version`,
`toolchain.archive_digest.algorithm`, `toolchain.archive_digest.value`,
`linker.install_subpath`, `linker.flavor`, `linker.emulation`, `linker.size`,
`linker.sha256`, and each `assets[]` entry's `role`, `name`, `lib` (import_lib
only), `install_subpath`, `size`, `sha256`.

- `lib` maps an `import_lib` to the `SystemLib.name` capability analysis emits
  (`kernel32` ⇄ the `-lkernel32`/positional-`libkernel32.a` choice).
- `role` ∈ **`linker` | `linker_runtime` | import_lib | compiler_builtins`**. A
  `linker_runtime` entry is a load-time DLL dependency of `ld.lld.exe`; its
  `install_subpath` MUST be under the **same subdirectory** as the linker
  (`bin/`) so the extracted linker launches without PATH manipulation (§4.1).
- `install_subpath` is the relative layout the runtime cache materializes; it is
  path-traversal-validated before use (§6). Only `install_subpath` (never an
  absolute or build-host path) crosses the manifest boundary.

### 4.3 Composition with the runtime-archive manifest

The two manifests are **separate files** but **cross-checked at link time**:
the embedded `native-link-assets.json`'s `toolchain.version` **must equal** the
selected runtime archive manifest's `toolchain.version`
(`libosc_runtime_freestanding.json` → `toolchain.version`, currently
`"20260324"`). Mismatch → hard error (`error: embedded linker toolchain
(llvm-mingw <A>) does not match runtime archive toolchain (llvm-mingw <B>); this
oscan build and its runtime archives were produced from different toolchains`).
This gives the same strict no-silent-drift guarantee as
`validate_runtime_archive_release_toolchain`.

---

## 5. `build.rs` contract (Hicks owns `build.rs`)

### 5.1 Env vars

| Env var | Meaning | Set by |
|---|---|---|
| `OSCAN_EMBED_ASSETS_DIR` | Absolute path to a staged dir containing `native-link-assets.json` + the 13 files at their `install_subpath`s. | The prepare tool / release workflow, before `cargo build`. |
| `OSCAN_REQUIRE_EMBEDDED_ASSETS` | `1`/`true` → build **fails** if any required asset for a required target is missing/incomplete/digest-mismatched. Unset/`0` → dev build, embedding optional. | Release workflow only. |

Both get `cargo:rerun-if-env-changed`. `OSCAN_EMBED_ASSETS_DIR`'s manifest gets
`cargo:rerun-if-changed`.

### 5.2 What `build.rs` generates

Writes `${OUT_DIR}/native_link_assets_generated.rs`, `include!`d by
`native_assets.rs`:

- `pub const EMBEDDED_ASSETS_PRESENT: bool` — `true` iff a full, digest-verified
  Windows asset set was embedded.
- When present: one `include_bytes!("<staged absolute path>")` const per asset,
  plus a static table `pub static EMBEDDED_ASSETS: &[EmbeddedAsset]` where
  `EmbeddedAsset { role, name, lib: Option<&str>, install_subpath, sha256:
  &str, len: usize, bytes: &'static [u8] }`, plus `pub static
  EMBEDDED_ASSET_MANIFEST_JSON: &str` (the verbatim `native-link-assets.json`).
- When absent: `EMBEDDED_ASSETS_PRESENT = false`, empty table, empty manifest.

`build.rs` **verifies sha256 of each staged file against the manifest at build
time** (fail-closed when `OSCAN_REQUIRE_EMBEDDED_ASSETS`), so a corrupt stage
never gets embedded. **No network access** in `build.rs`: it only reads the
already-staged dir. (`sha2` is available to `build.rs` as a normal dependency;
see §8 ownership — Bishop adds it to `[dependencies]`, `build.rs` may also use
it via `[build-dependencies]`.)

### 5.3 Clean-checkout dev build

With `OSCAN_EMBED_ASSETS_DIR` unset (ordinary `cargo build`):
`EMBEDDED_ASSETS_PRESENT = false`, nothing embedded, build succeeds. The runtime
then uses the legacy compiler-driver flavor (§7) and prints, on first native
link, `note: this oscan build has no embedded native linker (dev build); using
external C toolchain as linker driver`.

### 5.4 Canonical prepare tooling (Hicks)

Add a `release_tools.py` subcommand **`prepare-embed-assets`** (mirrors the
existing `fetch_toolchain`/`compute_digest` style, `release_tools.py:65,606`):

1. Given a pinned, already-fetched toolchain dir + target, copy the 13 files to
   `packaging/prebuilt/<target>/` under their `install_subpath`s.
2. Compute streaming sha256 for each; write `native-link-assets.json` (§4.2)
   with `toolchain.vendor/version` pulled from the toolchain manifest and the
   toolchain zip's `archive_digest`.
3. No network beyond what `fetch_toolchain` already does; if the toolchain isn't
   present, fail with an actionable "run fetch-toolchain first" message.

A thin `scripts/prepare-embed-assets.ps1`/`.sh` wraps it (mirrors
`build-runtime-archive.ps1`).

### 5.5 Release workflow ordering (Hicks; `.github/workflows/release.yml`)

Current `package` job order is **wrong** for embedding (`cargo build` at line
117 precedes asset prep in `assemble-release.ps1` at line 126). New order:

1. checkout / Rust / Python setup (unchanged).
2. **Fetch pinned toolchain** (existing fetch step / `assemble` prerequisite).
3. **Build runtime archives with the shim baked in** (`build-runtime-archive`,
   now compiling the shim member).
4. **`prepare-embed-assets`** → stages the 13 files + `native-link-assets.json`.
5. **`cargo build --release`** with `OSCAN_EMBED_ASSETS_DIR=packaging/prebuilt/<target>`
   and `OSCAN_REQUIRE_EMBEDDED_ASSETS=1` — this is the build that embeds and
   **fails loudly if assets are missing**.
6. **Assemble release asset** (`assemble-release.ps1`), packaging without
   requiring a full toolchain sidecar for default freestanding operation. The
   `toolchain/` sidecar stays **only** for hosted/`--extra-c`/legacy fallback
   (unchanged pruning; a different, coarser concern than the embed set).

CI (`ci.yml`) is unchanged in structure; its Windows job builds without
`OSCAN_EMBED_ASSETS_DIR` (dev/external path still exercised) — plus one **new**
optional CI job that runs the prepare tool and does an embedded smoke test, so
the embedded path has coverage without making every `cargo build` need assets.

---

## 6. Runtime extraction / cache (Bishop; `native_assets.rs`)

### 6.1 Cache location

- Windows: `%LOCALAPPDATA%\oscan\native-assets\`.
- Unix (abstraction, for later): `$XDG_CACHE_HOME/oscan/native-assets` else
  `$HOME/.cache/oscan/native-assets`.
- Override for tests/CI: `OSCAN_NATIVE_ASSET_CACHE_DIR`.

Resolver is hand-rolled (no new `dirs` dependency); it is small and lives in
`native_assets.rs`.

### 6.2 Content-addressed layout

Compute an **asset-set digest** = sha256 over the sorted list of
`(install_subpath, sha256)` from the embedded manifest. Materialize under:

```
<cache_root>/<asset_set_digest>/<install_subpath...>
<cache_root>/<asset_set_digest>/.complete   # marker, written last
```

Set-level addressing dedupes identical sets and makes a new toolchain a new
directory (never clobbers a live one).

### 6.3 Extraction algorithm (per asset)

1. Validate `install_subpath`: reject absolute paths, drive letters, and any
   `..`/`.` components; must be a strict relative path. Join under the set dir
   only after validation (path-traversal prevention).
2. If the destination exists: verify **size then sha256**; if it matches, reuse.
3. Otherwise write bytes to a sibling temp `.<name>.<pid>.<rand>.tmp` in the
   **same directory** (same filesystem → atomic rename), flush, on Unix `chmod
   0o755` for the `linker` role (else `0o644`), verify size+sha256 of the temp,
   then **atomic rename** onto the destination.
4. Windows rename-onto-existing race: if rename fails because the destination
   already exists, re-verify the existing destination; if it verifies, delete
   the temp and use it (a concurrent process won). If it does not verify, remove
   and retry once.
5. After all assets verify, write the `.complete` marker atomically.

### 6.4 Reuse & verification policy (requirement #8)

- A set dir is trusted for a link only if `.complete` exists **and** every
  asset's **size** matches. Full **sha256 is re-verified before each link**
  (honoring "validate before every reuse"), with an allowed **in-process
  memoization**: once a blob's sha256 is verified in this process, it need not
  be re-hashed again this process. Cross-process, first use re-verifies.
- Any size/hash mismatch or missing `.complete` ⇒ treat as cache miss ⇒
  re-extract (steps 3–5), overwriting via temp+rename.
- Concurrent extraction is safe by construction (temp-then-atomic-rename +
  set-level dir + `.complete` marker); no lock file required, but an advisory
  `.lock` (best-effort `create_new`) may reduce redundant work — optional, not
  required for correctness.

### 6.5 Cleanup on link failure (requirement #8)

`execute.rs`, on non-zero linker exit or post-link validation failure, **removes
any partial `exe_path`** so a failed link never leaves a success-shaped
executable. Temp files from a crashed extraction are `.tmp`-suffixed and are
cleaned opportunistically on the next run (glob-and-unlink stale `.tmp` older
than a threshold); they never satisfy a reuse check (no `.complete`).

---

## 7. `OSCAN_NATIVE_LINKER` migration & no-silent-fallback

### 7.1 Selection policy (default path)

For **Windows freestanding**, with no override:

```
EMBEDDED_ASSETS_PRESENT == true
   -> MingwDirect using extracted embedded linker + assets
EMBEDDED_ASSETS_PRESENT == false  (dev build)
   -> CompilerDriver (legacy), with the §5.3 note
```

For hosted / `.c` inputs / non-Windows targets: `CompilerDriver` (unchanged).

### 7.2 Env vars & backward compatibility

- `OSCAN_NATIVE_LINKER` **keeps its current meaning by default**: a
  GCC/Clang-style **driver command**. Existing users are not broken.
- New `OSCAN_NATIVE_LINKER_FLAVOR` ∈ `{compiler-driver, mingw, elf}`:

| `OSCAN_NATIVE_LINKER` | `OSCAN_NATIVE_LINKER_FLAVOR` | Result |
|---|---|---|
| set | unset | **Legacy compatibility**: treated as a compiler driver (current behavior) **plus** a one-line migration diagnostic: `note: OSCAN_NATIVE_LINKER is being interpreted as a C compiler driver for backward compatibility; set OSCAN_NATIVE_LINKER_FLAVOR=mingw to invoke a direct ld.lld instead.` |
| set | `compiler-driver` | Legacy driver, no diagnostic. |
| set | `mingw` | Invoke that binary **directly** as `ld.lld` (MingwDirect flavor). The override replaces only the linker; import libraries and builtins still come from embedded assets. |
| set | `elf` | Invoke that binary **directly** as GNU `ld` (ElfDirect flavor). Linux needs no additional linker assets, so this also works in a development build without embedded assets. |
| unset | (any) | `FLAVOR` alone selects the flavor for the default-resolved linker (embedded if present). |

### 7.3 No-silent-fallback rule (mechanically enforced by Bishop)

If `EMBEDDED_ASSETS_PRESENT == true` (the binary **claims** embedded assets) and
either extraction or the direct link **fails**, `link_executable` returns a
**hard error** and must **not** fall back to a compiler driver:

```
error: native link failed using this build's embedded linker: <reason>.
This oscan build embeds its own linker and will not silently fall back to an
external C toolchain. To override, set OSCAN_NATIVE_LINKER together with
OSCAN_NATIVE_LINKER_FLAVOR (e.g. =mingw for a direct ld.lld, or =compiler-driver
for the legacy path).
```

The dev-build fallback to `CompilerDriver` is permitted **only** when
`EMBEDDED_ASSETS_PRESENT == false`. This is a checkable invariant: the only code
path from a `true`-marker Windows freestanding build to `CompilerDriver` is an
explicit `OSCAN_NATIVE_LINKER_FLAVOR=compiler-driver`.

---

## 8. Task breakdown — Bishop vs Hicks (zero file overlap)

### 8.1 Bishop (Rust + toolchain integration)

| File / area | Work |
|---|---|
| `src/backend/link/` (new dir; split from `link.rs`) | Create `mod.rs`, `plan.rs`, `archive.rs`, `capability.rs`, `driver.rs`, `execute.rs` per §2.1. Implement `LinkPlan`/`LinkerFlavor`/`LinkerExecutable`/`render()` and the MingwDirect renderer (§2.4). Preserve the module doc comment; extend it. |
| `src/backend/native_assets.rs` (new) | Generated-const access (`include!` of `native_link_assets_generated.rs`), extraction + content-addressed cache + digest verification + cache-dir resolver (§6). |
| `src/backend/mod.rs` | Wire `mod native_assets;` and the `link` dir module; no behavior change beyond module wiring. |
| `Cargo.toml` | Add `sha2 = "0.10"` to `[dependencies]` **and** `[build-dependencies]` (build.rs also verifies). No `dirs` crate (hand-rolled resolver). Bishop owns the whole file. |
| Rust reader of `contains_native_shim` (§3.3) and `native-link-assets.json` fields (§4.2) | Must match the exact field names Hicks writes. |
| Unit tests | `plan.rs` render snapshots per flavor; `native_assets.rs` extraction corruption/concurrency/path-traversal tests. |

Bishop does **not** touch `runtime-archive-contract.json`, `release_tools.py`,
`build.rs`, workflows, or `assemble-release.*`.

### 8.2 Hicks (release engineering / Python / CI)

| File / area | Work |
|---|---|
| `packaging/toolchains/runtime-archive-contract.json` | Add `osc_native_shim.c` to each mode's `sources`; add `contains_native_shim: true` per mode; bump `schema_version` to 2 (§3.1). |
| `scripts/release_tools.py` | Emit `contains_native_shim`/`native_shim_member`/`schema_version: 2` in `build_runtime_archive` manifests (§3.2). Add `prepare-embed-assets` subcommand writing `native-link-assets.json` + staging the 13 files (§5.4). Optionally extend `validate_runtime_archive_release_toolchain` to assert the shim member. |
| `build.rs` | Read `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS`, verify sha256, generate `native_link_assets_generated.rs` (§5.2). Keep existing version-stamping logic. |
| `.github/workflows/release.yml` | Reorder `package` job per §5.5 (archives+prepare BEFORE `cargo build --release`; set the two env vars on the build step). |
| `.github/workflows/ci.yml` | Add one embedded-asset smoke job (prepare → build with embed → blocked-toolchain hello test). Leave existing jobs intact. |
| `scripts/assemble-release.ps1` / `.sh`, `scripts/prepare-embed-assets.ps1`/`.sh` | Thin wrapper for the new subcommand; ensure packaging no longer requires a full toolchain sidecar for default freestanding operation. |
| `packaging/prebuilt/<target>/native-link-assets.json` (generated, staged) | Produced by the prepare tool; field names per §4.2. `.gitignore` the staged binaries; keep `.gitkeep`. |

Hicks does **not** touch any file under `src/`, nor `Cargo.toml`.

### 8.3 Shared field-name contract (the only coupling)

These exact strings are the ABI between Hicks's writers and Bishop's readers.
Neither may rename without updating the other:

- Runtime-archive manifest: **`contains_native_shim`** (bool),
  **`native_shim_member`** (string), **`toolchain.version`** (string, used for
  cross-check in §4.3).
- `native-link-assets.json`: **`schema_version`, `target`, `toolchain.vendor`,
  `toolchain.version`, `toolchain.archive_digest.algorithm`,
  `toolchain.archive_digest.value`, `linker.install_subpath`, `linker.flavor`,
  `linker.emulation`, `linker.size`, `linker.sha256`**, and per `assets[]`
  entry: **`role`, `name`, `lib`, `install_subpath`, `size`, `sha256`**.
- `role` vocabulary: **`linker` | `linker_runtime` | `import_lib` | `compiler_builtins`**.
  (`linker_runtime` = a load-time DLL dependency of `ld.lld.exe`, co-located with
  the linker under `bin/` — see §4.1.)
- `linker.flavor` vocabulary: **`mingw`** (maps to `LinkerFlavor::MingwDirect`),
  **`elf`** (maps to `LinkerFlavor::ElfDirect`).
- build.rs generated symbols consumed by Rust: **`EMBEDDED_ASSETS_PRESENT`**,
  **`EMBEDDED_ASSETS`**, **`EMBEDDED_ASSET_MANIFEST_JSON`**, struct
  **`EmbeddedAsset { role, name, lib, install_subpath, sha256, len, bytes }`**.

### 8.4 Bishop/Hicks — Linux ElfDirect (this pass)

Zero-file-overlap split, mirroring §8.1/§8.2:

**Bishop** (Rust):

| File | Work |
|---|---|
| `src/backend/link/plan.rs` | Add `LinkerFlavor::ElfDirect` variant; implement `render_elf_direct()` per §10.3. |
| `src/backend/link/mod.rs` | Add `is_elf_eligible(target, runtime_mode, extra_c_files_empty) -> bool`; add `build_elf_plan()` orchestration. |
| `src/backend/link/driver.rs` | Add `LinkerSelection::Elf` variant; **remove** `reject_elf_flavor`'s rejection of `"elf"` string — wire the real flavor. |
| `src/backend/native_assets/unix_elevation.rs` (new) | Hand-rolled `extern "C"` `geteuid`/`getuid` declarations; `is_setuid_elevated() -> Result<bool, String>` (§10.7). |
| `src/main.rs` | `#[cfg(unix)]` elevation gate calling `check_elevation_policy` before native link (§10.7); extend `print_usage` text to mention Linux self-contained builds. |

**Hicks** (release engineering / Python / CI):

| File | Work |
|---|---|
| `scripts/release_tools.py` | Add `EMBED_ASSET_SPECS["linux-x86_64"]`: 1 asset (role `linker`, `x86_64-linux-musl-ld`, §10.5). |
| `.github/workflows/release.yml` | Reorder Linux matrix entry to run prepare-embed-assets before `cargo build --release`, mirroring the Windows entry. |
| `.github/workflows/ci.yml` | New Linux embedding smoke job (prepare → build with embed → blocked-toolchain hello test). |
| `.gitignore` | Add ignore rules for `packaging/prebuilt/linux-x86_64/native-link-assets.json` and `packaging/prebuilt/linux-x86_64/linker/`. Do NOT ignore the already-committed `libbearssl.a`/`.gitkeep` in that directory. |
| `scripts/smoke-release.ps1` | Extend `$expectedNativeLinkSource` to expect `"embedded"` for `linux-x86_64` freestanding too. |

---

## 9. Open risks / things for Vasquez to probe

1. **Byte-for-byte parity (found & fixed).** `hello.osc` must stay **exactly
   6,656 B** via MingwDirect. This was verified by a real end-to-end manual link:
   an isolated `ld.lld.exe` + its 5 `linker_runtime` DLLs + the 6 import libs +
   builtins, invoked with the exact §2.4 MingwDirect argument order against a real
   `hello.osc` object compiled by the current `oscan.exe`, produced an executable
   that is **exactly 6,656 B** and prints "Hello, Oscan!" / exit 0 — same
   byte-parity target as before. The fix was purely "ship 5 more files"; nothing
   about the link plan or argument rendering changed. Probe: size matrix +
   section-table/entry-point diff vs the legacy compiler-driver-linked binary. A
   drift here likely means a rendered argument order or a missing/extra flag (e.g.
   accidental `-nostdlib`/`-no-pie` leaking into MingwDirect).
1a. **The extracted linker must actually *launch*.** `ld.lld.exe` is dynamically
   linked; file-hash equality of the extracted linker is **necessary but was NOT
   sufficient** to catch the missing-sibling-DLL bug that this amendment fixes.
   Vasquez's validation MUST include a **true isolation test**: rename/move the
   entire pinned toolchain directory away **and** ensure `PATH` contains no
   toolchain `bin/` directory, then build+run `examples/hello.osc` native. Relying
   on the cache directory alone is insufficient — a stray toolchain `bin/` on
   `PATH` can mask a missing-sibling-DLL bug by satisfying the DLL search from the
   wrong place. **Recommended (Bishop's call, not mandated):** an optional
   post-extraction live smoke invocation (`ld.lld.exe --version` or similar) as
   defense-in-depth beyond hash verification, confirming the linker resolves its
   co-located `linker_runtime` DLLs and launches before the first real link.
2. **KERNEL32-only dependency after GC.** With all five optional import libs
   presented to LLD, confirm the final `hello.exe` imports only `KERNEL32.dll`;
   confirm socket/TLS/canvas programs import exactly their expected DLLs.
3. **No-compiler proof.** Rename/block `cc`/`gcc`/`clang`/`cl` **and** the
   toolchain dir, confirm the built `oscan.exe` still compiles+runs
   `examples/hello.osc` native, and that the archive manifest's recorded `cc`
   absolute path is genuinely unused on the freestanding path.
4. **Extraction concurrency/corruption.** N processes racing a cold cache;
   truncated/wrong-hash blobs; a partial (no `.complete`) set dir; path-traversal
   attempt via a crafted `install_subpath`. All must recover or hard-fail, never
   run a bad linker.
5. **No-silent-fallback.** With `EMBEDDED_ASSETS_PRESENT == true`, force a link
   failure (corrupt an extracted import lib after `.complete`, or point the
   linker at a bad object) and confirm a hard error — **not** a compiler-driver
   fallback.
6. **Migration compatibility.** Existing `OSCAN_NATIVE_LINKER=<clang>` (no
   FLAVOR) still links via the compiler driver and prints the migration note;
   `FLAVOR=mingw` invokes a direct ld.lld.
7. **Toolchain-version cross-check.** Mismatched embedded-asset vs runtime-archive
   `toolchain.version` produces the §4.3 hard error, not a mis-link.
8. **Response-file path.** Force a long argv (many `system_libs`/user inputs) and
   confirm the `@response.rsp` path quotes spaces/non-ASCII correctly.
9. **Clean-checkout dev build.** `cargo build` with no staged assets compiles,
   `EMBEDDED_ASSETS_PRESENT == false`, native link uses the external driver with
   the §5.3 note; `OSCAN_REQUIRE_EMBEDDED_ASSETS=1` with assets absent **fails**
   the build.

### Deliberately deferred (follow-ups, not this task)

- AArch64/RISC-V64 direct-link (blocked — see updated §1.2 table for exact reasons).
- Hosted `--libc` direct-link (`HostedLinkPlan`).
- Removing the compiler-driver flavor entirely (kept as transition/override).
- Cranelift unwind-info (`.pdata`/`.xdata`) — orthogonal, tracked separately.
- macOS native target enablement.

---

## 10. Linux x86-64 ElfDirect

This section mirrors §2's structure. It ratifies the direct GNU `ld` invocation
for Linux x86-64 freestanding builds, making those builds self-contained (no
external linker needed) in exactly the same sense as Windows builds after §2–§8.

### 10.1 `LinkerFlavor::ElfDirect` (enum extension)

The commented-out `// ElfDirect` placeholder in `plan.rs` (§2.2) becomes a real
variant:

```rust
pub enum LinkerFlavor {
    MingwDirect,
    CompilerDriver,
    /// Direct GNU ld invocation targeting ELF x86-64, the Linux freestanding
    /// default when embedded assets are present. No compiler driver.
    ElfDirect,
}
```

`LinkerExecutable` reuses the existing `Embedded { path }` and `Override`
variants unchanged — no new variant needed.

### 10.2 Linker choice and justification

The already-pinned `packaging/toolchains/linux-x86_64.json` toolchain provides
`bin/x86_64-linux-musl-ld` (GNU ld 2.37, from musl-cross-make/musl.cc). This
binary was empirically verified (WSL Ubuntu 24.04, extracted from the real
`x86_64-linux-musl-cross.tgz` at the pinned URL in `linux-x86_64.json`):

- `file`: reports "ELF 32-bit LSB pie executable, Intel 80386 ... static-pie
  linked, stripped"
- `ldd`: reports "statically linked" (zero shared-library dependencies)
- Size: 2,914,136 bytes (≈2.78 MiB)
- Runs correctly (`--version` → "GNU ld (GNU Binutils) 2.37") on x86_64 Linux
  hosts despite being a 32-bit binary (kernel IA32/COMPAT execve support — the
  near-universal default on x86_64 Linux)

**No new toolchain needs to be fetched or pinned.** The `linux-x86_64.json`
toolchain fetch that already happens for runtime-archive builds provides this
binary. This is a strict reuse of existing infrastructure.

**Rejected alternative — standalone `ld.lld`/LLVM toolchain for Linux:**
No such toolchain is pinned in this repo today. It would require a brand-new
~100+ MB toolchain fetch/pin/digest and, being dynamically linked in typical
distributions, would likely need sibling `.so` staging analogous to Windows'
5 DLLs — strictly worse on both maintenance surface and payload size than
reusing the already-pinned static musl-ld.

### 10.3 Exact argv — `render_elf_direct()`

`render()` for `ElfDirect` produces, in order:

```
-s -m elf_x86_64 -static --gc-sections --build-id=none
-o <output>
<program.o>
<runtime archive absolute path>
```

Notes locked by this design:

- **No `-nostdlib`, no `-no-pie`, no `-fuse-ld`, no `-l`/`-L`, no entry-point
  flag.** The runtime archive's `_start` (see `deps/laststanding/l_os.h`)
  resolves as ld's default entry symbol automatically, exactly as `MingwDirect`
  already documents for Windows's entry point.
- **No `system_libs`.** Linux freestanding has zero import libraries — the
  output is fully static, no libc at all.
- **No `builtins`/compiler-builtins re-link.** Empirically confirmed: unlike
  Windows (which needs `libclang_rt.builtins-x86_64.a`), the Linux freestanding
  runtime archive needs nothing from libgcc. This matches the *existing*
  `CompilerDriver` code in `src/backend/link/mod.rs`'s `build_compiler_driver_plan`,
  which only relinks builtins `if ... target == NativeTarget::WindowsX64`, never
  for Linux — already true today.
- **Shim is a member of the runtime archive** (§3), so `LinkPlan.objects`
  contains only `program.o` in the default freestanding case.

**Proof of byte-identical output:**

This exact argv, invoked directly via `x86_64-linux-musl-ld` with NO
gcc/collect2 driver involved at all, produced a SHA-256-**byte-identical**
executable to the existing `x86_64-linux-musl-gcc -nostdlib -s
-Wl,--gc-sections,--build-id=none -no-pie -static <obj> <archive> -o <out>`
(today's `CompilerDriver` argv, read verbatim from this build's own verbose link
log) for **three cases**:

1. `examples/hello.osc` — Core archive, 4,744 bytes, hash `a399954c...`
2. `examples/gfx/gfx_demo.osc` — Full archive (needs graphics), hash matched
3. `tests/positive/tls_fetch.osc` — Core archive (exercises sockets + TLS /
   BearSSL), hash matched; linked binary ran, correctly reaching a
   `connect_failed` fallback path consistent with this sandbox's network policy

Additionally proven:

- Linking with `env -i PATH=/nonexistent` (zero external tools reachable) still
  succeeds and the output still runs correctly.
- `readelf -l` shows no `PT_INTERP` segment; `readelf -d` reports "There is no
  dynamic section in this file" — fully static, no `NEEDED` entries possible.

### 10.4 BearSSL / TLS note

No special handling needed. `packaging/toolchains/runtime-archive-contract.json`'s
`targets.linux-x86_64.freestanding.embed_bearssl_from` already causes
`scripts/release_tools.py`'s `build_runtime_archive` to extract and merge
BearSSL's compiled objects directly into the runtime archive as `ar` members
(see `release_tools.py` around line 1936–1950, `extract_archive_members`). The
final link therefore never needs a separate TLS library reference; this was true
before this change and remains true.

### 10.5 Embedded asset manifest for Linux

**Exactly ONE file**, role `linker`:

| Field | Value |
|---|---|
| `role` | `linker` |
| `name` | `x86_64-linux-musl-ld` |
| `install_subpath` | `linker/x86_64-linux-musl-ld` |
| `flavor` | `elf` |
| `emulation` | `elf_x86_64` |

**No `linker_runtime` entries** (the binary is static — no DLL/`.so`
dependencies of any kind).
**No `import_lib` entries** (freestanding Linux has no import libraries).
**No `compiler_builtins` entry** (not needed — see §10.3).

**Payload/size comparison:**

| Target | Embedded files | Total size |
|---|---|---|
| Windows x86-64 | 13 (linker + 5 DLLs + 6 import libs + builtins) | ≈85.4 MB |
| Linux x86-64 | 1 (linker only) | ≈2.78 MB |

The Linux embedded payload is a **single ~2.78 MB static binary** versus
Windows' 13-file/~85.4 MB set. Callers/packagers should understand this
materially smaller, simpler embed for Linux.

### 10.6 Eligibility — `is_elf_eligible`

Mirror `is_mingw_eligible` (`src/backend/link/mod.rs`) with:

```rust
/// Returns true iff ElfDirect flavor should be used for this link.
pub fn is_elf_eligible(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    extra_c_files_empty: bool,
) -> bool {
    target == NativeTarget::LinuxX64
        && runtime_mode == RuntimeMode::Freestanding
        && extra_c_files_empty
}
```

Hosted (`--libc`) mode and any target other than `LinuxX64` always keep
`CompilerDriver` — same reasoning as Windows: the embedded asset set has no
CRT/libc, so a hosted link against it would fail with undefined libc symbols.

### 10.7 Unix privilege-elevation policy

Windows fails closed on process elevation (Administrator) before any final
native link (`src/backend/native_assets.rs`'s `check_elevation_policy` /
`NativeLinkOperation`, wired in `main.rs`'s `run_native_backend`).

Unix has no "Administrator" concept but has an analogous risk: a setuid/setgid
binary, or any process whose effective UID differs from its real UID, is running
with elevated privileges relative to the invoking user. The same "don't
trust/reuse the calling user's cache, and TOCTOU races are hard to fully close"
reasoning applies.

**Mandate:** add a `#[cfg(unix)]` gate in `main.rs::run_native_backend`,
parallel to the existing `#[cfg(windows)]` block, that calls:

```rust
// src/backend/native_assets/unix_elevation.rs (new file)

// Hand-rolled extern declarations — no new `libc` crate dependency,
// mirrors this codebase's existing "hand-rolled, no `dirs` dependency"
// convention for `cache_root()`.
extern "C" {
    fn geteuid() -> u32;
    fn getuid() -> u32;
}

/// Detects setuid/setgid elevation (euid != uid).
pub fn is_setuid_elevated() -> Result<bool, String> {
    let euid = unsafe { geteuid() };
    let uid = unsafe { getuid() };
    Ok(euid != uid)
}
```

This feeds into the **same** already-existing, already-tested
`check_elevation_policy` / `NativeLinkOperation::FinalLink` fail-closed policy
function. That function is already OS-agnostic — it takes a `Result<bool, String>`
and an operation enum; no Windows-specific logic in it. This is a small,
mechanical extension of already-built, already-hardened policy machinery — not
new security design.

### 10.8 CWD/PATH hardening (not needed)

Unlike Windows' `MingwDirect` — which must pin `current_dir`/`PATH` to the
linker's own `bin/` directory purely to satisfy the Windows DLL loader's search
order for its 5 sibling DLLs (see `execute.rs`'s
`apply_windows_dll_search_hardening`) — `ElfDirect` needs **NO**
current-dir/PATH manipulation:

- The linker binary is **static** (no DLL/`.so` search of any kind happens).
- `Command::new(embedded_absolute_path)` already never consults `PATH` to
  resolve the executable itself.

The existing `apply_windows_dll_search_hardening` function already returns early
for any non-`MingwDirect` flavor (`plan.flavor != LinkerFlavor::MingwDirect`
check), so `ElfDirect` already inherits the parent's normal CWD/PATH by
construction. **Already correct, no change needed.**

### 10.9 `OSCAN_NATIVE_LINKER_FLAVOR` value

`elf` — already reserved/reject-stubbed in `src/backend/link/driver.rs`'s
`reject_elf_flavor`. This pass **removes** that rejection and wires the real
`LinkerSelection::Elf` variant, mirroring `LinkerSelection::Mingw` /
`MingwLinkerSource`.

### 10.10 Fail-closed / no-silent-fallback

Identical rule as Windows (§7.3): if `EMBEDDED_ASSETS_PRESENT == true` and
extraction or the direct Linux link fails, `no_silent_fallback_error` fires; no
silent fallback to `CompilerDriver` except an explicit
`OSCAN_NATIVE_LINKER_FLAVOR=compiler-driver` override.

### 10.11 Selection policy (default path, Linux freestanding)

Mirrors §7.1:

```
EMBEDDED_ASSETS_PRESENT == true  AND  is_elf_eligible() == true
   -> ElfDirect using extracted embedded linker
EMBEDDED_ASSETS_PRESENT == false  (dev build)
   -> CompilerDriver (legacy), with the §5.3 note
```

For hosted / `.c` inputs / non-LinuxX64 targets: `CompilerDriver` (unchanged).

---

## 11. AArch64 / RISC-V64 ElfDirect

This section extends §10's Linux x86-64 ElfDirect design to two additional
Linux targets: `linux-aarch64` and `linux-riscv64`. After this pass, a Linux
x86-64 release `oscan` binary can `--backend native --native-target linux-aarch64`
(or `linux-riscv64`) and produce a fully linked, static, freestanding executable
**without the user installing any cross toolchain**.

### 11.1 Scope decision — single-target-per-build (recommended, rationale below)

**Decision:** this pass keeps the existing **single-target-per-build** embedded
asset model. Each `oscan` binary is built with `OSCAN_EMBED_ASSETS_DIR` staged
for **exactly one** target's linker assets, same as today. The standard
linux-x86_64 release oscan binary will NOT be able to cross-link aarch64/riscv64
out of the box unless the release workflow produces **additional oscan binaries**
(one per target) or **additional per-target linker artifact sidecar archives**
that the user unpacks into `OSCAN_NATIVE_ASSET_CACHE_DIR`.

**Why not multi-target bundling in one binary?**

1. **Binary size / build complexity.** Embedding 3 linkers (~2.78 + ~3.0 + ~3.1 MB
   = ~8.9 MB) vs 1 (~2.78 MB) into one binary triples the embedded payload. More
   critically, it requires `build.rs` to walk multiple target directories and
   generate a multi-target `EMBEDDED_ASSETS` table with target-keyed lookup,
   `native-link-assets.json` to change shape from single-object to array-of-
   objects, `ensure_extracted` to extract only the matching target's assets (not
   all 3), and `embedded_toolchain_version` to become target-aware. Every one of
   those changes touches security-reviewed, audit-hardened code paths (cache
   extraction, hash verification, elevation policy). The single-target model
   touches **none** of them — it extends the data (2 new `EMBED_ASSET_SPECS`
   entries, 2 new toolchain manifests, 2 new runtime-archive-contract targets)
   while leaving every Rust control-flow path unchanged.

2. **CI feasibility.** Today's release matrix already builds per-OS (Windows,
   Linux). Producing 3 Linux variant binaries (one embedding each target's
   linker) or 1 with all 3 is a CI matrix/packaging change either way.
   Single-target-per-build aligns with the existing model and requires no `build.rs`
   refactoring.

3. **Ship path.** The recommended ship path is: the Linux release archive
   includes the host-target oscan binary (embedding `linux-x86_64` linker, as
   today) **plus** the aarch64 and riscv64 linker binaries as sidecar files
   under a documented layout (e.g. `toolchain/cross-linkers/aarch64-linux-musl-ld`,
   `.../riscv64-linux-musl-ld`). A separate, follow-up pass can add a
   `--install-cross-linker <target>` or a multi-target bundling scheme if user
   demand warrants it. For now, cross-linking works by pointing
   `OSCAN_NATIVE_LINKER` at the sidecar linker binary and setting
   `OSCAN_NATIVE_LINKER_FLAVOR=elf`, which is already fully wired.

**Honesty rule (§1.2 update):** docs/CLI must NOT say "no external tooling
needed for aarch64/riscv64" unless the release archive actually ships those
linker binaries as sidecars. If they are shipped, docs may say "self-contained
cross-linking for linux-aarch64 and linux-riscv64 is included in this release."

### 11.2 Embedded-asset manifest — no schema change

`native-link-assets.json` keeps its current single-target shape (§4.2). A
linux-aarch64 build produces its own `native-link-assets.json` with
`"target": "linux-aarch64"`, containing one `linker` entry. Same for riscv64.
No new fields, no `targets: [...]` wrapper, no multi-target manifest.

The existing `embedded_toolchain_version()` / `toolchain_version_from_manifest()`
path is unchanged. The musl-cross linker toolchain's `version` field is a fixed
distribution name (same vendor/generation as x86-64), so the §10 "no cross-check"
documented simplification carries forward.

### 11.3 `is_elf_eligible` extension

Change `src/backend/link/mod.rs`'s `is_elf_eligible`:

```rust
pub(self) fn is_elf_eligible(
    target: NativeTarget,
    runtime_mode: RuntimeMode,
    extra_c_files_empty: bool,
) -> bool {
    matches!(
        target,
        NativeTarget::LinuxX64 | NativeTarget::LinuxAarch64 | NativeTarget::LinuxRiscv64
    ) && runtime_mode == RuntimeMode::Freestanding
      && extra_c_files_empty
}
```

### 11.4 Host-only gate removal — target-aware embedded-asset check

Replace the blanket `if !target.is_host() { return Err(...) }` at the top of
`link_executable` (`src/backend/link/mod.rs` ~line 282) with a target-aware
check that allows non-host targets **only** when they are elf-eligible **and**
this build has embedded assets. The new logic:

```rust
if !target.is_host() {
    // Allow cross-linking for ELF targets that have embedded linker assets.
    let can_cross = elf_eligible && native_assets::EMBEDDED_ASSETS_PRESENT;
    if !can_cross {
        return Err(format!(
            "'{}' is not the host target ({}); cross-linking requires a matching \
             embedded ELF linker asset in this oscan build (build with \
             OSCAN_EMBED_ASSETS_DIR staged for '{}', or set OSCAN_NATIVE_LINKER + \
             OSCAN_NATIVE_LINKER_FLAVOR=elf to use an external cross-linker)",
            target,
            NativeTarget::host(),
            target.archive_tag(),
        ));
    }
}
```

This preserves the existing loud error for truly unsupported cross-link
attempts (e.g. Windows host → Linux target, or a dev build with no embedded
assets), while allowing the embedded-linker path for aarch64/riscv64 when
the binary was built with assets for that target. When an override
`OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR=elf` is set, it bypasses
embedded-asset extraction entirely (the existing `LinkerSelection::Elf(Override)`
branch), so the gate only blocks the "no linker at all" case.

Note: move the `elf_eligible` computation (currently at ~line 314) **above**
this gate so it is available for the check. The `mingw_eligible` computation
can stay where it is or move alongside — no behavioral change either way.

### 11.5 Target-aware emulation in `build_elf_plan`

The hardcoded `emulation: Some("elf_x86_64")` in `build_elf_plan`
(`src/backend/link/mod.rs` ~line 525) must become target-aware. Add a helper:

```rust
fn elf_emulation(target: NativeTarget) -> &'static str {
    match target {
        NativeTarget::LinuxX64 => "elf_x86_64",
        NativeTarget::LinuxAarch64 => "aarch64linux",
        NativeTarget::LinuxRiscv64 => "elf64lriscv",
        _ => unreachable!("elf_emulation called for non-ELF target {target}"),
    }
}
```

And change the `build_elf_plan` field from:
```rust
emulation: Some("elf_x86_64"),
```
to:
```rust
emulation: Some(elf_emulation(target)),
```

The emulation names are verified from the respective toolchains' `lib/ldscripts/*.x`
files: `elf_x86_64`, `aarch64linux`, `elf64lriscv`.

### 11.6 Toolchain manifests — `linux-aarch64.json` / `linux-riscv64.json`

Create `packaging/toolchains/linux-aarch64.json` and
`packaging/toolchains/linux-riscv64.json`, mirroring `linux-x86_64.json`'s
exact `schema_version`/`bundle_kind`/`toolchain`/`stage` structure:

**`linux-aarch64.json`:**

| Field | Value |
|---|---|
| `schema_version` | `1` |
| `target` | `"linux-aarch64"` |
| `bundle_kind` | `"full"` |
| `toolchain.vendor` | `"musl.cc"` |
| `toolchain.version` | `"aarch64-linux-musl-cross"` |
| `toolchain.archive.url` | `https://github.com/lucabol/Oscan/releases/download/toolchains/aarch64-linux-musl-cross.tgz` |
| `toolchain.archive.type` | `"tgz"` |
| `toolchain.archive.digest.algorithm` | `"sha256"` |
| `toolchain.archive.digest.value` | `"c909817856d6ceda86aa510894fa3527eac7989f0ef6e87b5721c58737a06c38"` |
| `toolchain.extract.strip_components` | `1` |
| `toolchain.runtime.compiler.path` | `"bin/aarch64-linux-musl-gcc"` |
| `toolchain.runtime.compiler.family` | `"gcc"` |
| `toolchain.runtime.compiler.version` | `"11.2.1"` |
| `toolchain.runtime.compiler.target` | `"aarch64-linux-musl"` |
| `toolchain.runtime.compiler.size_flag` | `"-Os"` |
| `toolchain.runtime.archiver.path` | `"bin/aarch64-linux-musl-ar"` |
| `toolchain.runtime.archiver.family` | `"gnu-ar"` |
| `toolchain.runtime.archiver.version` | `"2.37"` |
| `toolchain.runtime.linker.path` | `"bin/aarch64-linux-musl-ld"` |
| `toolchain.runtime.linker.family` | `"gnu-ld"` |
| `toolchain.runtime.linker.version` | `"2.37"` |
| `toolchain.runtime.linker.driver_flags` | `[]` |
| `toolchain.runtime.abi` | `"musl"` |
| `toolchain.runtime.crt` | `"musl"` |
| `toolchain.prune.remove_globs` | (same list as x86_64) |
| `toolchain.prune.strip_debug` | `true` |
| `stage` | (same structure as x86_64) |
| Linker binary size (informational) | 3,119,472 bytes |

**`linux-riscv64.json`:**

| Field | Value |
|---|---|
| `schema_version` | `1` |
| `target` | `"linux-riscv64"` |
| `bundle_kind` | `"full"` |
| `toolchain.vendor` | `"musl.cc"` |
| `toolchain.version` | `"riscv64-linux-musl-cross"` |
| `toolchain.archive.url` | `https://github.com/lucabol/Oscan/releases/download/toolchains/riscv64-linux-musl-cross.tgz` |
| `toolchain.archive.type` | `"tgz"` |
| `toolchain.archive.digest.algorithm` | `"sha256"` |
| `toolchain.archive.digest.value` | `"db0bc413bd4a93f2012cc74b9ba0c4af29d8bc18b88e9c61998738ccb918604b"` |
| `toolchain.extract.strip_components` | `1` |
| `toolchain.runtime.compiler.path` | `"bin/riscv64-linux-musl-gcc"` |
| `toolchain.runtime.compiler.family` | `"gcc"` |
| `toolchain.runtime.compiler.version` | `"11.2.1"` |
| `toolchain.runtime.compiler.target` | `"riscv64-linux-musl"` |
| `toolchain.runtime.compiler.size_flag` | `"-Os"` |
| `toolchain.runtime.archiver.path` | `"bin/riscv64-linux-musl-ar"` |
| `toolchain.runtime.archiver.family` | `"gnu-ar"` |
| `toolchain.runtime.archiver.version` | `"2.37"` |
| `toolchain.runtime.linker.path` | `"bin/riscv64-linux-musl-ld"` |
| `toolchain.runtime.linker.family` | `"gnu-ld"` |
| `toolchain.runtime.linker.version` | `"2.37"` |
| `toolchain.runtime.linker.driver_flags` | `[]` |
| `toolchain.runtime.abi` | `"musl"` |
| `toolchain.runtime.crt` | `"musl"` |
| `toolchain.prune.remove_globs` | (same list as x86_64) |
| `toolchain.prune.strip_debug` | `true` |
| `stage` | (same structure as x86_64) |
| Linker binary size (informational) | 3,250,064 bytes |

Both tarballs are live on the repo's `toolchains` GitHub release; SHA-256
digests are re-verified. Both toolchains' internal layout (bin/, lib/ldscripts/,
prune globs) mirrors `x86_64-linux-musl-cross` exactly — same GCC/binutils 2.37
generation.

### 11.7 Runtime-archive contract extension

Add to `packaging/toolchains/runtime-archive-contract.json`'s `targets`:

```json
"linux-aarch64": {
  "release_toolchain": {
    "manifest": "linux-aarch64.json",
    "vendor": "musl.cc",
    "version": "aarch64-linux-musl-cross",
    "abi": "musl",
    "crt": "musl",
    "compiler_family": "gcc",
    "compiler_version": "11.2.1",
    "compiler_target": "aarch64-linux-musl",
    "compile_size_flag": "-Os",
    "archiver_family": "gnu-ar",
    "archiver_version": "2.37",
    "linker_family": "gnu-ld",
    "linker_version": "2.37",
    "linker_driver_flags": []
  },
  "hosted": { "link_flags": ["-lm"] },
  "freestanding": {
    "link_flags": ["-nostdlib", "-static", "-Wl,--gc-sections,--build-id=none"],
    "embed_bearssl_from": "packaging/prebuilt/linux-aarch64/libbearssl.a"
  },
  "freestanding_core": {
    "link_flags": ["-nostdlib", "-static", "-Wl,--gc-sections,--build-id=none"],
    "embed_bearssl_from": "packaging/prebuilt/linux-aarch64/libbearssl.a"
  }
},
"linux-riscv64": {
  "release_toolchain": {
    "manifest": "linux-riscv64.json",
    "vendor": "musl.cc",
    "version": "riscv64-linux-musl-cross",
    "abi": "musl",
    "crt": "musl",
    "compiler_family": "gcc",
    "compiler_version": "11.2.1",
    "compiler_target": "riscv64-linux-musl",
    "compile_size_flag": "-Os",
    "archiver_family": "gnu-ar",
    "archiver_version": "2.37",
    "linker_family": "gnu-ld",
    "linker_version": "2.37",
    "linker_driver_flags": []
  },
  "hosted": { "link_flags": ["-lm"] },
  "freestanding": {
    "link_flags": ["-nostdlib", "-static", "-Wl,--gc-sections,--build-id=none"],
    "embed_bearssl_from": "packaging/prebuilt/linux-riscv64/libbearssl.a"
  },
  "freestanding_core": {
    "link_flags": ["-nostdlib", "-static", "-Wl,--gc-sections,--build-id=none"],
    "embed_bearssl_from": "packaging/prebuilt/linux-riscv64/libbearssl.a"
  }
}
```

Also add `"linux-aarch64"` and `"linux-riscv64"` to the `freestanding` and
`freestanding_core` modes' `supported_targets` arrays (currently
`["linux-x86_64", "windows-x86_64"]`).

### 11.8 `EMBED_ASSET_SPECS` extension

Add two entries to `EMBED_ASSET_SPECS` in `scripts/release_tools.py`, mirroring
`"linux-x86_64"`'s shape:

```python
"linux-aarch64": {
    "linker": {
        "role": "linker",
        "name": "aarch64-linux-musl-ld",
        "source": "bin/aarch64-linux-musl-ld",
        "install_subpath": "linker/aarch64-linux-musl-ld",
        "flavor": "elf",
        "emulation": "aarch64linux",
    },
    "linker_runtime": [],
    "import_libs": [],
},
"linux-riscv64": {
    "linker": {
        "role": "linker",
        "name": "riscv64-linux-musl-ld",
        "source": "bin/riscv64-linux-musl-ld",
        "install_subpath": "linker/riscv64-linux-musl-ld",
        "flavor": "elf",
        "emulation": "elf64lriscv",
    },
    "linker_runtime": [],
    "import_libs": [],
},
```

These are single-target manifests, consistent with §11.1's single-target-per-build
decision. Each produces its own `native-link-assets.json` when
`prepare-embed-assets --target linux-aarch64` (or `linux-riscv64`) is run.

### 11.9 BearSSL cross-compilation

`.github/workflows/build-bearssl.yml` gains a matrix (or sequential steps) that
also cross-compile BearSSL for aarch64 and riscv64, using the pinned musl-cross
toolchains' own compilers:

**Approach:** add a `strategy.matrix` with `include`:

```yaml
strategy:
  matrix:
    include:
      - target: linux-x86_64
        cc: gcc
        toolchain_manifest: ""
        toolchain_dir: ""
      - target: linux-aarch64
        cc: bin/aarch64-linux-musl-gcc
        toolchain_manifest: packaging/toolchains/linux-aarch64.json
        toolchain_dir: build/toolchain-linux-aarch64
      - target: linux-riscv64
        cc: bin/riscv64-linux-musl-gcc
        toolchain_manifest: packaging/toolchains/linux-riscv64.json
        toolchain_dir: build/toolchain-linux-riscv64
```

For the aarch64/riscv64 matrix entries, add a "Fetch cross toolchain" step
before the build step that fetches the pinned toolchain tarball. Then the
compile step uses `${{ matrix.toolchain_dir }}/${{ matrix.cc }}` instead of
the bare `gcc`. Compile flags are identical:
`-c -I$BEARSSL_INC -I$BEARSSL_SRC -Os -ffreestanding -fno-builtin
-U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=0 -ffunction-sections -fdata-sections -w`.

Output paths:
- `packaging/prebuilt/linux-x86_64/libbearssl.a` (existing)
- `packaging/prebuilt/linux-aarch64/libbearssl.a` (new)
- `packaging/prebuilt/linux-riscv64/libbearssl.a` (new)

The `embed_bearssl_from` fields in the runtime-archive-contract (§11.7) already
point at these paths.

### 11.10 Exact argv — `render_elf_direct()` for aarch64/riscv64

The existing `render_elf_direct()` in `plan.rs` already renders the `emulation`
field from `LinkPlan.emulation` dynamically (not hardcoded). With the §11.5
change to `build_elf_plan`, the rendered argv for each target is:

**aarch64:**
```
-s -m aarch64linux -static --gc-sections --build-id=none
-o <output>
<program.o>
<runtime archive absolute path>
```

**riscv64:**
```
-s -m elf64lriscv -static --gc-sections --build-id=none
-o <output>
<program.o>
<runtime archive absolute path>
```

Same structure as x86_64 — no system_libs, no builtins, no `-nostdlib`, no `-L`.

### 11.11 `--run` constraint for cross-built binaries

When `--native-target` selects a non-host target and the link succeeds, `--run`
must NOT attempt to execute the binary (it is a foreign-architecture ELF).
The existing `--run` gate in `main.rs`'s `run_native_backend` already checks
`native_target.is_host()` and refuses to `--run` a non-host binary with a clear
error. No change needed.

---

## 12. `--extra-obj` / `--extra-lib` CLI

Two new repeatable CLI options for passing precompiled inputs directly to the
link step, complementing the existing `--extra-c` (which compiles from source).

### 12.1 Parser additions (`src/main.rs`)

Mirror the `--extra-c` block (~line 383–390). In the locals section (~line 312):

```rust
let mut extra_obj_files: Vec<String> = Vec::new();
let mut extra_lib_files: Vec<String> = Vec::new();
```

In the arg-match loop, after the `--extra-cflags` arm:

```rust
"--extra-obj" => {
    i += 1;
    if i < args.len() {
        extra_obj_files.push(args[i].clone());
    } else {
        eprintln!("--extra-obj requires an object file path (.o/.obj)");
        process::exit(1);
    }
}
"--extra-lib" => {
    i += 1;
    if i < args.len() {
        extra_lib_files.push(args[i].clone());
    } else {
        eprintln!("--extra-lib requires a static library path (.a/.lib)");
        process::exit(1);
    }
}
```

### 12.2 Validation

Apply plausible-extension checks at the point of use (not at parse time — be
consistent with `--extra-c`, which does no extension validation at parse time):

- `--extra-obj`: warn (not hard-reject) if the file does not end with `.o` or
  `.obj`. Rationale: GCC/ld accept any filename as a positional object; rejecting
  would be more restrictive than the tools themselves.
- `--extra-lib`: warn if the file does not end with `.a` or `.lib`. Same
  rationale.

Both hard-error if the file does not exist at link time (the linker would fail
anyway, but an early check gives a better diagnostic).

### 12.3 Threading through the call chain

`extra_obj_files` and `extra_lib_files` must flow through the same ~10 call
sites that `extra_c_files`/`extra_cflags` already flow through, as additional
parameters. The key touchpoints:

1. `run_native_backend()` gains `extra_obj_files: &[String]`,
   `extra_lib_files: &[String]` parameters.
2. `NativeLinkOptions` (`src/backend/link/mod.rs`) gains:
   ```rust
   pub extra_objects: &'a [String],
   pub extra_libs: &'a [String],
   ```
3. `compile_to_executable()` and `invoke_c_compiler()` (the C backend / `--emit-c`
   paths in main.rs) gain the same parameters, threaded through as positional
   args to the system `cc`/`gcc` invocation.

### 12.4 `LinkPlan` field usage

**For `--extra-obj`:** resolve each path to `PathBuf`, append to
`LinkPlan.extra_objects`. This field already exists and is already rendered
by all three renderers (`render_mingw_direct`, `render_elf_direct`,
`render_compiler_driver`) — today it holds user `.c` compiled outputs from the
`CompilerDriver` path. Adding precompiled `.o`/`.obj` to the same field is
semantically correct: they go into the same position (after program objects,
before archives) in every renderer.

**For `--extra-lib`:** do NOT join with `LinkPlan.archives` (which holds the
runtime archive — ordering matters for static resolution: program objects →
user archives → runtime archive). Instead, add a new field:

```rust
/// User-supplied precompiled static libraries (`--extra-lib`). Rendered
/// after the runtime archive(s) in the link order, so they can reference
/// runtime symbols but the runtime archive's member selection is not
/// affected by their undefined symbols.
pub extra_libs: Vec<PathBuf>,
```

Each renderer appends these after `archives` (and `system_libs` for
MingwDirect/ElfDirect, or after `-l` libs for CompilerDriver). This ordering
means user libs can call runtime functions (resolved leftward from the runtime
archive) but do not pull spurious archive members from the runtime. If the user
needs the opposite ordering (their lib before the runtime archive), they can use
`--extra-obj` with the individual `.o` files instead.

### 12.5 Renderer changes

**`render_mingw_direct` / `render_elf_direct`:** after the `builtins` block,
append:
```rust
for lib in &self.extra_libs {
    args.push(lib.clone().into_os_string());
}
```

**`render_compiler_driver`:** after the `builtins` (passthrough_cflags) block,
before the trailing passthrough flags, append:
```rust
for lib in &self.extra_libs {
    args.push(lib.clone().into_os_string());
}
```

### 12.6 C-backend / `--emit-c` paths

In `invoke_c_compiler` / `compile_cross_riscv64` / `compile_cross_wasi` (the
`--emit-c` paths in main.rs ~2016–2290 which today only handle
`extra_c_files`/`extra_cflags`):

- `--extra-obj` files are appended as positional arguments to the `cc` invocation
  (same position as `extra_c_files` — the compiler driver accepts `.o`/`.obj`
  as link inputs alongside `.c` sources).
- `--extra-lib` files are appended as positional arguments after all other
  inputs (the compiler driver forwards them to the linker). They must NOT be
  recompiled — they are precompiled static archives.

### 12.7 Eligibility interaction

`--extra-obj` and `--extra-lib` do NOT affect `is_mingw_eligible` or
`is_elf_eligible` (those only gate on `extra_c_files`, which require an external
C compiler). Precompiled objects/libs need no compilation step, so they are
compatible with the direct-link flavors.

This is a deliberate design choice: `--extra-c` forces `CompilerDriver` because
it needs a compiler; `--extra-obj`/`--extra-lib` do not, because their inputs
are already compiled. This lets users link precompiled FFI objects with the
embedded linker.

### 12.8 Usage/help text update

Add to `print_usage` (~line 263):
```
  --extra-obj <file>  Precompiled object file to link (.o/.obj, repeatable)
  --extra-lib <file>  Precompiled static library to link (.a/.lib, repeatable)
```

Update `tests/cli_help.rs` to assert `--extra-obj` and `--extra-lib` appear
in the help output.

---

## 13. CI coverage audit for AArch64 / RISC-V64

### 13.1 Required new/changed CI jobs

| Job | Workflow | Description | Feasibility |
|---|---|---|---|
| `native-link-embedding-smoke-linux-aarch64` | `ci.yml` | Mirrors `native-link-embedding-smoke-linux` (§10's existing x86-64 job): fetch aarch64 musl-cross toolchain, build runtime archives for `linux-aarch64`, stage embedded assets, build oscan with `OSCAN_EMBED_ASSETS_DIR` for aarch64, PATH-scrub compilers. Produces an aarch64 ELF. | ✅ Feasible. `ubuntu-latest` runners are x86-64 but can build oscan for x86-64 host while embedding aarch64 linker assets (single-target-per-build means the oscan binary itself is still an x86-64 host binary, just with aarch64's linker embedded). |
| `native-link-embedding-smoke-linux-riscv64` | `ci.yml` | Same as above but for `linux-riscv64`. | ✅ Feasible. |
| QEMU smoke test step (aarch64) | Added as a step within `native-link-embedding-smoke-linux-aarch64` | After producing the aarch64 ELF: `sudo apt-get install -y qemu-user-static`, then `qemu-aarch64-static ./hello_aarch64`. The binary is statically linked with no interpreter, so `-L` sysroot is unnecessary. | ✅ Feasible. `qemu-user-static` is available as an apt package on `ubuntu-latest`. Statically linked, no-interpreter ELFs require no sysroot. Mark `continue-on-error: true` consistent with the existing x86-64 smoke job pattern in case GH runner kernel config changes break `binfmt_misc`. |
| QEMU smoke test step (riscv64) | Added as a step within `native-link-embedding-smoke-linux-riscv64` | Same: `qemu-riscv64-static ./hello_riscv64`. | ✅ Feasible. Same `qemu-user-static` package provides `qemu-riscv64-static`. Same `continue-on-error: true`. |
| `readelf` shape checks (aarch64/riscv64) | Steps within the above jobs | `readelf -h <binary>` to confirm `Machine: AArch64` / `RISC-V`, `readelf -l` for no `PT_INTERP`, `readelf -d` for no `NEEDED`. Must use cross-capable `readelf` — the system `readelf` (GNU binutils) on `ubuntu-latest` handles any ELF architecture. | ✅ Feasible. |

### 13.2 Existing jobs — `--backend c` / `--backend native` coverage check

| Check | Status |
|---|---|
| `--backend c` runs in CI (all hosts) | ✅ Already covered by the main `cargo test` job (examples + tests run with C backend by default). |
| `--backend native` runs in CI (Windows x86-64) | ✅ Covered by `native-link-embedding-smoke` (Windows job). |
| `--backend native` runs in CI (Linux x86-64) | ✅ Covered by `native-link-embedding-smoke-linux`. |
| `--backend native` cross-codegen (aarch64/riscv64 object emission) | ⚠️ Not explicitly covered. Add a CI step that confirms `oscan hello.osc --backend native --native-target linux-aarch64 -o hello.o` produces a valid ELF relocatable (and similarly for riscv64). This is cheap and can go in the existing Linux test job. |

### 13.3 QEMU invocation pattern

```bash
sudo apt-get update && sudo apt-get install -y qemu-user-static

# aarch64
qemu-aarch64-static ./hello_aarch64
# Expected: prints "Hello, Oscan!" and exits 0

# riscv64
qemu-riscv64-static ./hello_riscv64
# Expected: prints "Hello, Oscan!" and exits 0
```

No `-L` sysroot flag needed: the binaries are fully static (`-static`,
no `PT_INTERP`, no `NEEDED`). `qemu-user-static` invokes the user-mode
emulator that runs the foreign-arch binary natively on the x86-64 host kernel.

### 13.4 Infeasibility flags

- **Kernel IA32 compat for the x86-64 musl-ld linker:** The x86-64 musl-ld is
  a 32-bit ELF binary (§10.2). GH `ubuntu-latest` runners have IA32
  compatibility enabled by default (verified empirically in the existing x86-64
  smoke job). If a future runner disables it, the linker will fail to launch.
  The existing `native-link-embedding-smoke-linux` job already has
  `continue-on-error: true` as a safety net. The aarch64/riscv64 jobs' own
  linker binaries are native x86-64 (32-bit static-pie on x86, same as the
  x86-64 linker) — same IA32 dependency, same mitigation.

- **QEMU `binfmt_misc` kernel module:** Required for `qemu-user-static` to
  intercept foreign-arch ELF execution transparently. All current GH-hosted
  `ubuntu-latest` runners have it enabled. Mark QEMU smoke steps
  `continue-on-error: true` as a belt-and-suspenders measure.

### 13.5 Release workflow changes

In `.github/workflows/release.yml`, the Linux matrix entry must:

1. Fetch all 3 toolchains (x86_64, aarch64, riscv64).
2. Build runtime archives for all 3 targets.
3. Stage embedded assets for the host target (x86_64) into
   `OSCAN_EMBED_ASSETS_DIR` for the main `cargo build --release`.
4. Package the aarch64 and riscv64 linker binaries as sidecar files in the
   release archive (under e.g. `toolchain/cross-linkers/`), so users can point
   `OSCAN_NATIVE_LINKER` at them.
5. Package the aarch64 and riscv64 runtime archives alongside the x86_64 ones.

---

## 14. Updated deferral / honesty table (replaces §1.2)

| Item | Status after this pass |
|---|---|
| Linux AArch64 / RISC-V64 **direct/cross** link | **In scope.** Runtime archives, BearSSL, toolchain manifests, linker embedding, and CI coverage all specified. Single-target-per-build model: the standard linux-x86_64 release binary embeds only its own linker; cross-linker binaries ship as sidecars in the release archive. `OSCAN_NATIVE_LINKER` + `OSCAN_NATIVE_LINKER_FLAVOR=elf` enables cross-linking. |
| `--extra-obj` / `--extra-lib` CLI | **In scope.** Specified in §12. |
| macOS native target | No `NativeTarget` variant exists; out of scope entirely. |
| Hosted `--libc` mode direct-link | Keeps the diagnosed external C-toolchain driver path. |
| Multi-target asset bundling (one binary embeds linkers for N targets) | Explicitly deferred (§11.1). Acceptable follow-up if user demand warrants. |
