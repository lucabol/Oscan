# Release Packaging

Release builds are handled by GitHub Actions workflows. Two manual workflows must be run **once** (and again whenever their upstream dependencies change) before creating a release.

## What a release publishes (contract schema 2)

`packaging/toolchains/release-contract.json` (`schema_version: 2`) is the
single source of truth, and every published artifact is one **(target,
backend) pair**. A release therefore publishes **seven archives, one MSI and
one `SHA256SUMS`** — and nothing else:

| Target | Backend | Archive | Components staged |
|---|---|---|---|
| `windows-x86_64` | `llvm` | `oscan-v{version}-windows-x86_64-llvm.zip` | compiler, native-link sidecar (linker + the verified `libLLVM-22.dll` it shares with the LLVM provider), freestanding runtime archives |
| `windows-x86_64` | `cranelift` | `oscan-v{version}-windows-x86_64-cranelift.zip` | compiler, native-link sidecar, freestanding runtime archives |
| `windows-x86_64` | `c` | `oscan-v{version}-windows-x86_64-c.zip` | compiler, pinned C toolchain |
| `linux-x86_64` | `llvm` | `oscan-v{version}-linux-x86_64-llvm.tar.xz` | compiler, native-link sidecar, freestanding runtime archives, LLVM provider from the pinned provider archive |
| `linux-x86_64` | `cranelift` | `oscan-v{version}-linux-x86_64-cranelift.tar.xz` | compiler, native-link sidecar, freestanding runtime archives |
| `linux-x86_64` | `c` | `oscan-v{version}-linux-x86_64-c.tar.xz` | compiler, pinned C toolchain |
| `macos-x86_64` | `c` | `oscan-v{version}-macos-x86_64-c.tar.gz` | compiler only (uses the host's Apple Command Line Tools) |

Plus exactly **one installer**: `oscan-v{version}-windows-x86_64-llvm.msi`,
cut from the *already staged and smoked* Windows LLVM bundle directory. The
MSI backend is declared by the contract (`ci-matrix`'s `msi_backend`), so
adding a second installer is a contract change, not a workflow tweak. There is
no combined/`-full` archive of any kind, and `native` is never an artifact
label — it survives only as the compiler's deprecated `--backend` alias for
`cranelift`.

Invariants the tooling enforces (see `scripts/test_release_workflow.py`,
`scripts/test_release_packaging.py`, `scripts/test_smoke_release.py`):

* **One backend per package.** Each binary is built with
  `cargo build --release --no-default-features --features backend-<backend>`
  and `OSCAN_DISTRIBUTION_BACKEND=<backend>`, so the other backends' code is
  absent, and the package defaults deterministically to its own backend.
* **Object packages are toolchain-free.** An `llvm`/`cranelift` package may not
  contain a C compiler executable, a C header outside `native-link/`, a
  sysroot/`include/` tree, the hosted runtime archive, `native-runtime/`, or
  `cross-linkers/`. The Oscan runtime is precompiled from C *by the release
  factory* with the pinned toolchain and shipped as freestanding archives, so
  downstream users need no C tools at all.
* **C packages carry no object payload.** A `c` package may not ship
  `native-link/`, `build/`, `native-runtime/`, or `cross-linkers/`.
* **No cross-target sidecars.** Minimum packages ship only their own target's
  assets; the `cross-linkers/<target>/` staging that earlier full bundles used
  is refused outright now (see "Cross-target linking" below).
* **Only authenticated archive inputs.** Staging accepts pinned *source
  archives* (`--toolchain-archive`, `--llvm-provider-archive`, resolved and
  digest-verified by `release_tools.py resolve-archive`) plus the repo-derived,
  per-file-hashed native-link asset directory. The former `--toolchain-dir`
  and `--llvm-provider-dir` inputs are hard errors: a prepared directory
  cannot be checked against the digest its manifest pins.
* **Every archive is smoked before it is uploaded**, with its own backend, and
  the produced set is compared against the contract's expected archive names in
  both directions (nothing missing, nothing extra).

### Why the package job is per target, not per (target, backend)

All of a target's variants reuse the same pinned toolchain download, the same
freestanding runtime archives and the same native-link sidecar. The workflow
therefore fans out over **targets**, prepares those expensive inputs once, and
then builds the target's backends **sequentially** in one job — three
`cargo build`s into `$RUNNER_TEMP/oscan-release-binaries/<backend>/`, each
copied out before the next overwrites `target/release`. `target/` is
deliberately not cached: the same crate is built three times with mutually
exclusive feature sets.

### Release job order

1. `prepare` — resolve version/tag and publish mode, run
   `release_tools.py validate-contract`, and derive the package matrix with
   `release_tools.py ci-matrix --version <version>`.
2. `package` (per target) — resolve and verify the pinned archives, extract the
   toolchain, build runtime archives, prepare the native-link sidecar, build
   one compiler per backend, then for each backend: assemble → smoke → stage
   for upload. Windows additionally cuts the recommended LLVM MSI from the
   staged bundle.
3. `checksums` — collect every `.zip`/`.msi`/`.tar.gz`/`.tar.xz`, refuse
   duplicate asset names, and write one `SHA256SUMS` over all of them.
4. `publish` — only when publishing is enabled; uploads the archives, the MSI
   and `SHA256SUMS` to the GitHub release.

### Dry run without publishing

Run the workflow manually (Actions → "Oscan Release" → Run workflow) with a
version and **`publish` cleared**. `prepare` then reports
`should_publish=false`, the `publish` job is skipped, and the run still builds,
smokes and checksums all seven archives and the MSI — the supported way to
rehearse a release. A tag push (`v*`) always publishes.

### Assembling and smoking one variant by hand

Each variant can be produced locally with the same scripts CI uses, in the same
order. `-Backend` is mandatory everywhere, and only the canonical names are
accepted. This walkthrough produces the Windows LLVM package end to end; run it
from the repository root on the target's own platform:

```powershell
$target  = 'windows-x86_64'
$backend = 'llvm'
$version = '1.2.3'                       # archive/bundle version, no leading 'v'
$tag     = "v$version"                   # what --version reports
$manifest = "packaging/toolchains/$target.json"

# 1. Resolve the pinned source archives (digest-verified; downloads on demand).
$downloadDir = 'build/archive-cache'
$toolchainArchive = ./scripts/resolve-archive.ps1 `
  -ManifestPath $manifest -Component toolchain `
  -DownloadDir $downloadDir -Download | Select-Object -Last 1
# Linux llvm only: its provider comes from the manifest, not the sidecar.
# $providerArchive = ./scripts/resolve-archive.ps1 `
#   -ManifestPath $manifest -Component llvm-provider `
#   -DownloadDir $downloadDir -Download | Select-Object -Last 1

# 2. Extract that same verified archive locally. It is only used to *build*
#    inputs below; packages are always staged from the archive itself.
./scripts/fetch-toolchain.ps1 `
  -ManifestPath $manifest `
  -Destination "build/toolchain-$target" `
  -DownloadDir $downloadDir

# 3. Build this target's freestanding runtime archives with the pinned
#    compiler/archiver the manifest names (one archive per declared profile).
$runtimeTools = (Get-Content $manifest -Raw | ConvertFrom-Json -AsHashtable)['toolchain']['runtime']
foreach ($profileName in @('freestanding', 'freestanding_gfx', 'freestanding_core')) {
  ./scripts/build-runtime-archive.ps1 `
    -Target $target -Mode $profileName `
    -CC (Join-Path "build/toolchain-$target" ([string]$runtimeTools['compiler']['path'])) `
    -AR (Join-Path "build/toolchain-$target" ([string]$runtimeTools['archiver']['path'])) `
    -ToolchainManifest $manifest `
    -OutDir "build/runtime-archives/$target"
}

# 4. Prepare the verified native-link asset set this package ships as its
#    sidecar (linker, its runtime libraries, import libraries, builtins).
./scripts/prepare-embed-assets.ps1 `
  -Target $target `
  -ToolchainDir "build/toolchain-$target" `
  -ToolchainManifest $manifest `
  -OutputDir "build/native-link/$target"

# 5. Build exactly one backend into the compiler, stamped as that
#    distribution. A release binary must not embed assets, so clear the
#    embedding variables the dev/standalone build mode uses.
Remove-Item Env:OSCAN_EMBED_ASSETS_DIR, Env:OSCAN_REQUIRE_EMBEDDED_ASSETS -ErrorAction SilentlyContinue
$env:OSCAN_DISTRIBUTION_BACKEND = $backend
$env:OSCAN_VERSION = $tag
cargo build --release --no-default-features --features "backend-$backend"

# 6. Assemble that variant, passing only the inputs its component list declares.
$archive = ./scripts/assemble-release.ps1 `
  -Target $target -Backend $backend -Version $version `
  -BinaryPath target/release/oscan.exe `
  -NativeLinkDir (Resolve-Path "build/native-link/$target").Path `
  -PrebuiltRuntimeArchiveDir (Resolve-Path "build/runtime-archives/$target").Path `
  -OutputDir build/release-artifacts | Select-Object -Last 1

# 7. Smoke exactly that archive, as that backend.
./scripts/smoke-release.ps1 `
  -Target $target -Backend $backend -Version $version `
  -ArchivePath $archive `
  -ScratchDir "build/release-smoke/$target-$backend"
```

Variant differences:

* **`cranelift`** — identical, with `-Backend cranelift` and
  `--features backend-cranelift`; steps 1–4 are unchanged and can be reused
  from the LLVM run.
* **`c`** — skip steps 3 and 4 and pass `-ToolchainArchive $toolchainArchive`
  to `assemble-release.ps1` instead of `-NativeLinkDir`/
  `-PrebuiltRuntimeArchiveDir`; its `runtime_profiles` list is empty.
* **Linux `llvm`** — additionally pass
  `-LlvmProviderArchive $providerArchive` (uncomment step 1's second
  resolve), because its provider comes from the toolchain manifest rather
  than the native-link sidecar.
* **macOS `c`** — only step 5 plus `assemble-release.sh`/`smoke-release.sh`
  with `-Backend c`; there is nothing pinned to fetch.

Passing an input a variant's component list does not declare is an error, as is
omitting one it does. `scripts/assemble-release.sh` / `smoke-release.sh` and the
matching `.sh` wrappers for the steps above mirror the same options on
Linux/macOS.

Finally, checksum the assets exactly as the workflow does:

```powershell
./scripts/write-checksums.ps1 -OutputPath build/release-artifacts/SHA256SUMS `
  (Get-ChildItem build/release-artifacts -File |
    Where-Object { $_.Name -match '\.(zip|msi|tar\.gz|tar\.xz)$' } |
    Sort-Object Name).FullName
```

`release_tools.py write-checksums` is what both wrappers call, and the release
workflow refuses two assets with the same file name before writing them.

## Mirror musl toolchain (one-time setup)

The Linux release bundle ships a musl cross-compiler so users can compile freestanding programs without installing gcc. The toolchain comes from [musl.cc](https://musl.cc/) but that site blocks GitHub Actions, so we self-host it as a GitHub release asset.

**Run once from your local machine** (musl.cc blocks GitHub Actions, so the workflow won't work):

```bash
curl -fSL -o x86_64-linux-musl-cross.tgz https://musl.cc/x86_64-linux-musl-cross.tgz
gh release create toolchains --title "Toolchains" --notes "Pre-downloaded musl cross-compilation toolchains" x86_64-linux-musl-cross.tgz
```

Re-run if the musl.cc toolchain is updated. Whenever the asset changes, also
update its pinned `sha256`/size and the `toolchain.runtime` tool
versions/target in `packaging/toolchains/linux-x86_64.json`, and the matching
`targets.linux-x86_64.release_toolchain` block in
`packaging/toolchains/runtime-archive-contract.json` — staging deliberately
fails a Linux runtime archive whose recorded provenance no longer matches
either file (see "Runtime archives for native-codegen backends" below).

## Build BearSSL (when BearSSL submodule changes)

TLS support on Linux uses [BearSSL](https://www.bearssl.org/), compiled as a static library. Rather than rebuilding all 293 source files on every release, the library is pre-built and committed.

**Run** from Actions → "Build BearSSL" → Run workflow. This compiles BearSSL with system gcc (freestanding flags) and commits `packaging/prebuilt/linux-x86_64/libbearssl.a`.

Re-run whenever `deps/laststanding/bearssl/` is updated.

## Runtime archives for native-codegen backends

The transpile-to-C compiler embeds `runtime/osc_runtime.c` as source text and
compiles it together with the generated program in a single translation unit
(see `emit_includes()` in `src/codegen.rs`). A native (non-C) codegen backend
instead needs the Oscan runtime **precompiled** into a static archive it can
link against object files it emits directly.

`scripts/build-runtime-archive.ps1` / `.sh` build exactly that: per-target
`.a` archives of the runtime, in four modes:

- **hosted** — `libosc_runtime_hosted.a`, compiled from `runtime/osc_runtime.c`
  against the platform libc (`requires_libc: true`). For normal (non-freestanding)
  executables.
- **freestanding** — `libosc_runtime_freestanding.a`, compiled from
  `runtime/osc_runtime_freestanding.c` (a wrapper that reproduces
  `emit_includes()`'s exact macro/`#include` preamble: arena/strings/panic plus
  the full gfx/img/svg/tt/tls feature-library chain) with `-ffreestanding
  -fno-builtin`, no libc at all (`requires_libc: false`). Only `linux-x86_64`
  and `windows-x86_64` are supported (the same targets `emit_includes()`
  supports); RISC-V/WASI freestanding use a separate, narrower compile path
  in `main.rs` and are out of scope for this archive tool.
- **freestanding_core** — `libosc_runtime_freestanding_core.a`, the same
  freestanding runtime and sockets/TLS, compiled from
  `runtime/osc_runtime_freestanding_core.c` instead — the exact same preamble
  minus the gfx/img/svg/tt feature-library `#include`s/defines. `src/backend/
  link.rs` links a program against this smaller sibling instead of the full
  freestanding archive whenever the program's own compiled object has no
  undefined `osc_gfx_*`/`osc_canvas_*`/`osc_clipboard_*`/`osc_img_*`/
  `osc_svg_*`/`osc_tt_*` symbol (see that module's "Freestanding runtime
  profiles" docs) — `--gc-sections` cannot fully remove the graphics feature
  libraries' own floating-point constant pool from the full archive, so
  programs that never touch graphics link against this one instead to avoid
  carrying that dead weight.
- **freestanding_gfx** — `libosc_runtime_freestanding_gfx.a`, core plus
  `l_gfx` canvas/clipboard and built-in-font support, but without `l_img`,
  `l_svg`, or `l_tt`. It is selected for `osc_gfx_*`, `osc_canvas_*`, or
  `osc_clipboard_*` references unless image/SVG/TrueType symbols require the
  full archive. Unknown or unscanned inputs conservatively select full.

Usage:

```powershell
scripts\fetch-toolchain.ps1 `
  -ManifestPath packaging\toolchains\windows-x86_64.json `
  -Destination build\toolchain-windows-x86_64
scripts\build-runtime-archive.ps1 `
  -Target windows-x86_64 -Mode all `
  -Cc build\toolchain-windows-x86_64\bin\clang.exe `
  -Ar build\toolchain-windows-x86_64\bin\llvm-ar.exe `
  -ToolchainManifest packaging\toolchains\windows-x86_64.json
```

```bash
scripts/build-runtime-archive.sh --target linux-x86_64 --cc x86_64-linux-musl-gcc --ar x86_64-linux-musl-ar --mode all
```

Both wrappers delegate to `python scripts/release_tools.py build-runtime-archive`,
which is the canonical, reproducible entry point an object backend or CI job
should invoke directly. Pass `--cc`/`--ar` pointing at the appropriate
per-target toolchain (see `packaging/toolchains/*.json`; use
`scripts/fetch-toolchain.ps1|.sh` to fetch the musl cross-compiler for
`linux-x86_64`). Omitting `--target` detects the host target.

Omitting `--cc`/`--ar` auto-detects a working compiler/archiver on `PATH`
instead of assuming a fixed name:

- For the **host's own target**, it probes host-native names in priority
  order (`gcc`/`clang` on Windows; `cc`/`gcc`/`clang` elsewhere) — it never
  assumes a plain `cc` exists, since that's typically missing on stock
  Windows/MinGW installs.
- For a **cross target**, it probes the triple-prefixed binary names produced
  by the bundled toolchains (e.g. `x86_64-linux-musl-gcc`,
  `x86_64-w64-mingw32-gcc`) or a purpose-built bare `clang`
  (llvm-mingw's convention).
- `--ar` is then derived from whichever `--cc` was selected (matching its
  triple prefix, or `llvm-ar` for clang, falling back to `ar`).

Every selected compiler is probed with `-dumpmachine`; its reported triple
must match `--target`. A host compiler can therefore never produce objects
that are mislabeled as a cross-target archive. A general-purpose bare Clang
whose default triple does not match the requested cross target is rejected
with an actionable error. To configure one intentionally, pass both
`--target-triple <triple>` and `--sysroot <target-sysroot>` (the wrapper
scripts expose the same options), or use a target-specific compiler.

If nothing suitable is found on `PATH`, the tool fails immediately with a
clear message listing what it tried and how to fix it (pass `--cc`/`--ar`,
set `$OSCAN_ARCHIVE_CC`/`$OSCAN_ARCHIVE_AR`, or fetch the matching toolchain
via `scripts/fetch-toolchain.ps1|.sh`) — it never lets a missing-tool error
surface as a raw Python traceback.

On `linux-x86_64`, the freestanding archive additionally merges in the
prebuilt `packaging/prebuilt/linux-x86_64/libbearssl.a` object members (via
`ar x` + `ar rcs`) so TLS support is self-contained in one archive; the
manifest's `embedded_bearssl` field reports whether this happened. Windows
freestanding TLS uses Schannel (`-lsecur32 -lcrypt32`) instead of BearSSL, so
no embedding is needed there.

Each archive is written next to a `<archive>.json` sidecar manifest recording
`target`, `mode`, `cc`/`ar` used, `requires_libc`, the `link_flags` a
downstream linker must still supply (e.g. `-lm` for hosted, or
`-nostdlib -static -Wl,--gc-sections` / the Win32 import libs for
freestanding), `embedded_bearssl`, the `oscan_version` (via `git describe`),
and a `sha256` digest of the archive. It also records `cc_args`, `cc_target`,
and `sysroot`, making the compiler-target assertion auditable. Missing `git`
produces the explicit version value `unknown` rather than a traceback.

Windows and Linux release assembly do not use that local auto-detection. Both
fetch the digest-pinned toolchain from `packaging/toolchains/<target>.json`
(`bin/clang.exe`/`bin/llvm-ar.exe` on Windows, `bin/x86_64-linux-musl-gcc`/
`bin/x86_64-linux-musl-ar` on Linux) and pass the manifest to the archive
builder for version/target/linker validation. Staging rejects either
platform's runtime archive without matching pinned provenance (see
`validate_runtime_archive_release_toolchain`/`targets.<target>.release_toolchain`
in `packaging/toolchains/runtime-archive-contract.json`, which is generic over
`target` and simply does nothing for a target with no `release_toolchain`
entry — this is exactly the gap that let Linux runtime archives silently get
built with the host's own `cc` while the release packaged an unrelated musl
cross-compiler, so an installed bundle's native linking rejected the archive's
recorded compiler target, or fell back to requiring a host compiler that
defeated the point of bundling one). The generated archive sidecar records the
source-manifest name and digest, ABI/CRT (GNU/UCRT on Windows, musl/musl on
Linux), compiler/archiver/linker commands and versions, target triple, size
flag, and (Windows only) `-fuse-ld=lld`. Installed bundles can therefore find
their relocatable bundled compiler even though the sidecar's original
build-machine compiler path no longer exists (`assemble-release.ps1` deletes
the ephemeral toolchain it fetched to build the archives once staging
completes).

The bundled musl-cross-make GCC is itself a fully static (no host libc
dependency at all — every tool under `toolchain/bin` and
`toolchain/libexec/gcc/...` is statically linked, verified with `file`/`ldd`)
and relocatable cross-compiler: `gcc`/`cc1`/`collect2`/`as`/`ld` all resolve
their own support files relative to their own executable path, not a
hardcoded install prefix, so the same fetched tree works unmodified from
whatever directory it is extracted or moved to. The one genuine relocatability
defect found while fixing this — `x86_64-linux-musl/lib/ld-musl-x86_64.so.1`,
a symlink meaningful only relative to the toolchain's own embedded sysroot,
shipping as an absolute `/lib/libc.so` target that silently escaped
`fix_absolute_symlinks`'s tree-root-only search — is fixed in
`scripts/release_tools.py` by trying every ancestor of the symlink itself,
innermost first, as a candidate root. Cranelift's own object emission needed
one more fix to link cleanly against this toolchain: it emits non-PIC objects
(see `src/backend/target.rs`), and this GCC is configured with
`--enable-default-pie --enable-static-pie`, so plain `-static` alone is not
sufficient to avoid a PIE link (unlike many host toolchains, where `-static`
alone already disables PIE) — `src/backend/link.rs`'s freestanding link now
passes `-no-pie` explicitly on non-Windows targets, mirroring hosted mode's
existing Linux handling, for exactly this reason.

Each archive and manifest are built under a clean private object directory.
Publication moves any previous pair aside, publishes the complete manifest,
then atomically renames the matching archive as the final visibility point;
failures roll the old pair back. This prevents stale `ar` members and prevents
consumers from seeing a new archive without its matching manifest.

Archives are build output: `build/` is gitignored, and this tool never
commits its own artifacts (the only pre-committed binary remains
`packaging/prebuilt/linux-x86_64/libbearssl.a`, per the exception above). Run
`runtime/Makefile`'s `make archives` target for a local Unix-dev convenience
wrapper. The Makefile delegates to the same Python builder, uses a concrete
target tag, writes the same manifests, and embeds BearSSL under the same rules.

Release assembly stages each archive/manifest pair at
`build/runtime-archives/<target>/` inside an object package's bundle. Runtime
*sources* are not staged: schema 2 packages ship no `native-runtime/`
directory and no runtime builder, because the shim is already compiled into
every runtime archive (see below). The paths mirror the shared object-backend
linker's executable-relative lookup contract and are copied intact by the
installers. Release smoke tests assert the assets survived packaging and
installation, then compile and run the sample with the package's own backend —
freestanding LLVM for an `llvm` package, freestanding Cranelift for a
`cranelift` package — on Linux and Windows, and check that the package refuses
by name everything it does not contain. The macOS target ships only the C
package because no LLVM/Cranelift Darwin object target exists yet.

GitHub-hosted Windows release runners may run the packaging/smoke process with
an elevated Administrator token. Normal interactive native final links still
refuse elevated processes by default; trusted release smoke tests that build
only repository-controlled inputs must pass `--allow-elevated-native-link` when
they need native final linking or `--run` under that elevated token. This flag
does not relax path validation, cache verification, canonicalization, or
native-link sandboxing.

`scripts/smoke-release.ps1` takes the package's backend explicitly and checks
that variant's promises. A `c` package must report `bundled` compiler-source
for its `--backend c` compile; an object package must compile, link and run
with **no C toolchain reachable at all**, and must refuse `--backend c`,
`--emit-c`, `-o *.c`, `--libc`, `--extra-c`/`--extra-cflags` and generated C
ABI shims by name. Every compile additionally runs with a "no host compiler"
PATH prefix: a scratch directory containing `cc`/`gcc`/`clang` (and `cl` on
Windows) stubs that fail immediately, prepended to the real `PATH` so every
other tool (`sh`, `dirname`, `tar`, ...) still resolves normally.
Bundled-compiler discovery never consults `PATH` (it walks the package's own
`toolchain/` directory), so this only shadows PATH-based host-compiler
fallback — proving each package is genuinely self-contained rather than merely
preferring its own toolchain when a host one also happens to be present. The
object-package smoke also exercises the deprecated `--backend native` alias
once, asserting it still compiles and warns exactly once. This replaced an
earlier `OSCAN_CC=gcc`/`--libc` override that was added for Linux specifically
because the bundled musl GCC was believed to be non-relocatable; investigating
that belief while fixing the archive/compiler mismatch above found the
toolchain itself to be relocatable and fully functional (see above), so the
override was hiding a real bug rather than working around an unfixable one.

### LLVM backend release contract

The LLVM backend has no LLVM Cargo/build dependency. At run time it dynamically
loads exact-major LLVM 22 through the C API and performs parse, verify,
`default<Oz>` (the default size profile) or `default<O3>` (the speed profile),
and TargetMachine object emission in-process. It does not generate C or invoke
Clang, `llvm-as`, `opt`, or `llc`.

Both the Windows and the Linux `llvm` package must ship a compatible provider:

- Windows: `libLLVM-22.dll` from pinned llvm-mingw 22.1.2, staged as part of
  the verified `native-link/` sidecar (it is `ld.lld`'s own runtime dependency,
  so it is shared rather than duplicated); x86 and AArch64 target initializers
  are present, RISC-V is absent.
- Linux: pinned LLVM 22.1.8 `libLLVM.so`, staged under `toolchain/` from the
  separately pinned provider archive declared by the target manifest's
  `toolchain.llvm_code_generator` block — the code generator only, with no
  clang, GCC, headers or sysroot beside it. x86, AArch64, and RISC-V
  initializers are present. It is the apt.llvm.org Ubuntu 22.04 build and
  intentionally uses host runtime libraries (`glibc >= 2.34` plus the
  manifest's `debian_packages`), rather than bundling a second Linux userspace.

The Windows `cranelift` package also contains `libLLVM` for the same
LLD-dependency reason, and *only* for it: that package has no LLVM backend
compiled in, and `--backend llvm` there reports which package to install
instead. The Linux `cranelift` package has neither a provider nor a C
toolchain.

Provider lookup is limited to absolute `OSCAN_LLVM_LIB`, absolute
`OSCAN_LLVM_DIR`, absolute `OSCAN_TOOLCHAIN_DIR`, and executable-relative
package locations. Do not reintroduce CWD, `PATH`, or bare-loader lookup. In a
multi-backend development build, implicit selection falls back to Cranelift/C
when no compatible provider is available; explicit `--backend llvm` remains a
hard failure. A published `llvm` package defaults to LLVM by stamp, not by
probe.

Release/CI gates for LLVM are:

1. `cargo build --release` with no LLVM development libraries installed.
2. C-vs-LLVM differential runs on Windows and Linux.
3. Packaged Windows and Linux implicit-default compiles proving
   executable-relative provider discovery.
4. The shared embedded-link smoke, proving the resulting LLVM object uses the
   existing runtime archive and direct linker.
5. Explicit C-vs-Cranelift runs, preserving the previous backend as an option.
6. The pinned-Windows-toolchain size gate in
   `scripts/compare-backend-size.ps1` (run from
   `tests/windows_native_ci.tests.ps1`), which fails when the LLVM `hello.osc`
   executable is larger than the equivalent C-backend executable.
7. `tests/llvm_toolchain_isolation.tests.ps1` on Windows and Linux, proving a
   packaged freestanding LLVM build needs no C/Clang/LLVM tool executable
   (empty `PATH`, unusable `OSCAN_CC`, absolute LLVM library, strict
   `OSCAN_NO_TOOLCHAIN=1`, embedded linker).
8. `scripts/sample-backend-matrix.ps1`, compiling every recursive example with
   LLVM, Cranelift, and C and reporting per-sample plus aggregate sizes.

The release workflow installs the Linux manifest's `debian_packages` before
packaged smoke. This is a provider-load gate, not a toolchain dependency:
normal freestanding LLVM code generation still invokes no compiler or LLVM
command-line tool. `README-install.txt` records the same end-user packages.

The current pinned Windows matrix compiles all 37 examples with all three
backends (111 executables):

| Backend | Aggregate size |
|---|---:|
| LLVM | 811,008 bytes |
| Cranelift | 861,696 bytes |
| C | 875,520 bytes |

LLVM is 50,688 bytes (5.88%) smaller than Cranelift and 64,512 bytes
(7.37%) smaller than C.

### Windows native size-toolchain benchmark

The Windows GNU-ABI archive/link path was benchmarked on 2026-07-13 before
adopting llvm-mingw. The baseline was MinGW-w64 GCC 15.2.0 (`-Os`), GNU
binutils 2.45.1 (`ar`/`ld`); the candidate was the reproducibly packaged
llvm-mingw `20260324` UCRT x86-64 asset (Clang/LLVM ar/LLD 22.1.2,
`x86_64-w64-windows-gnu`, `-Oz`, `-fuse-ld=lld`). Both used function/data
sections, `--gc-sections`, and the native linker's existing `-s` flag. The
candidate archive was built from the same runtime source and the Cranelift
program objects were identical.

The llvm-mingw release asset is 187,042,907 bytes compressed and is pinned by
SHA-256
`e6d3195ab6ee67f66651ae263b91e395cef3ef3af95d20f1004f84e9fe988116`.
Fetching through `release_tools.py` verified that digest, pruned/extracted it
successfully, ran all three required tools, and confirmed Clang's default target
triple. The freestanding runtime archive fell from 1,482,828 to 317,614 bytes
(78.6% smaller).

| Native fixture | GCC + GNU ld | Clang + LLD | Reduction | Final DLL imports |
|---|---:|---:|---:|---|
| `hello_world` | 17,920 B | 8,192 B | 54.3% | `KERNEL32.dll` |
| `builtin_socket` | 19,456 B | 9,216 B | 52.6% | `KERNEL32.dll`, `WS2_32.dll` |
| `tls_fetch` | 1,076,736 B | 16,896 B | 98.4% | `KERNEL32.dll`, `WS2_32.dll`, `Secur32.dll` |
| `gfx_text_width` | 30,208 B | 21,504 B | 28.8% | `KERNEL32.dll` |
| `builtin_canvas_clipboard` | 26,624 B | 17,408 B | 34.6% | `KERNEL32.dll`, `USER32.dll`, `GDI32.dll` |

The stripped `hello_world` PE section comparison explains its total:

| Toolchain | `.text` | `.data` | `.rdata` | `.idata` | `.reloc` | Raw section bytes | File bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| GCC/GNU ld | 2,544 | 4,256 | 6,336 | 2,248 | 36 | 15,420 | 17,920 |
| Clang/LLD | 2,580 | 0 | 3,541 | merged into `.rdata` | 36 | 6,157 | 8,192 |

LLD requires optional Win32 import libraries to be available while it resolves
undefined names in the runtime archive, even when their calling sections will
later be discarded. The native linker therefore supplies all five optional
libraries for LLD; LLD's section GC still removes unused import thunks, as the
dependency scan above confirms. GNU ld retains the existing per-program
feature-library selection. Deterministic hello/gfx runs, the native
C-vs-Cranelift differential corpus, hosted-mode coverage, archive/release unit
tests, and packaged release smoke/dependency checks are the adoption gates.

### Freestanding runtime profiles (native-size-profiles)

Even with the llvm-mingw/LLD adoption above, `hello_world` was still 8,192 B
native vs 6,656 B for the C backend (+23.1%, above the 10% budget). Inspecting
the stripped LLD binary showed `.text` was already comparable to the C
backend's (2,580 vs 2,622 B) but `.rdata` was not (3,541 vs 1,230 B): a single,
unnamed, non-COMDAT `.rdata` input section of 2,668 B survived from
`osc_runtime_freestanding.c`'s single translation unit even though `hello.osc`
calls no graphics/image/SVG/TrueType builtin and no live function had any
relocation into it. That input section is the Clang/LLVM x86-64 backend's
shared floating-point constant pool for the whole translation unit (curve-
flattening/trig tables the gfx/img/svg/tt feature libraries need) — it isn't
split per function/global the way `-ffunction-sections -fdata-sections`
splits ordinary code and data, so `--gc-sections` can only keep or discard it
as one atomic unit, and something elsewhere in the file keeps it live.

Rather than a heuristic aimed at that one pool, `runtime/
osc_runtime_freestanding_core.c` is a second, sibling translation unit — the
same preamble as `osc_runtime_freestanding.c` minus the `l_gfx.h`/`l_img.h`/
`l_svg.h`/`l_tt.h` block and its `OSC_HAS_GFX`/`OSC_HAS_IMG`/`OSC_HAS_SVG`/
`OSC_HAS_TT` defines — built into a wholly separate archive,
`libosc_runtime_freestanding_core.a` (see the `freestanding_core` mode above).
`src/backend/link.rs`'s `program_needs_graphics_runtime` scans each compiled
program's own undefined symbols for the graphics-only `osc_gfx_*`/
`osc_canvas_*`/`osc_clipboard_*`/`osc_img_*`/`osc_svg_*`/`osc_tt_*` prefixes
(the same technique already used to pick optional Win32 import libraries) and
links against the core archive only when none are present and there are no
unscanned `extra_c_files`; core/sockets/TLS are unaffected and identical in
both archives (verified: no cross-references either way), and any
unparseable object or extra C source conservatively falls back to the full
archive, so this can never omit a symbol a program actually needs — including
one reached only indirectly through another runtime function — and never
requires end-user C compilation (both archives ship prebuilt, exactly like
the existing hosted/freestanding pair).

Measured on the pinned llvm-mingw 20260324 (Clang/LLD 22.1.2) toolchain,
reproduced with `scripts/size-matrix.ps1`:

| Fixture | C backend | Native, before | Native, after | Ratio, before | Ratio, after | Archive selected |
|---|---:|---:|---:|---:|---:|---|
| `hello_world` (core) | 6,656 B | 8,192 B | 6,656 B | 1.231 | 1.000 | `..._core.a` |
| `builtin_socket` | 6,656 B | — | 7,168 B | — | 1.077 | `..._core.a` |
| `tls_fetch` | 13,312 B | — | 15,360 B | — | 1.154 | `..._core.a` |
| `gfx_text_width` | 19,456 B | 21,504 B | 21,504 B | 1.105 | 1.105 | `libosc_runtime_freestanding.a` |

`hello_world` reaches exact byte parity with the C backend (and is comfortably
under the 10% budget); `gfx_text_width` is unchanged, as expected, since it
still needs and correctly selects the full archive. `builtin_socket`/
`tls_fetch`'s remaining ~8-15% gap is ordinary Cranelift-vs-Clang code-density
(tracked by `native-size-codegen`), not unreachable dead weight, so it was not
chased further here. `scripts/size-matrix.ps1` enforces a ratio threshold
(1.10 for core, looser for the feature families) instead of exact byte counts
as a standing regression gate for this split.

The later `freestanding_gfx` profile extends this split for graphics-only
programs: it includes `l_gfx` but excludes the image, SVG, and TrueType
translation-unit pools. Selection lives in `src/backend/link/capability.rs`;
unscanned extra C/object/library inputs still force the full archive.

## Native-link assets for self-contained Windows object builds

On Windows x86-64, freestanding object-backend builds (`--backend llvm` or
`--backend cranelift`) need no external C compiler or linker: `oscan.exe` uses
a linker (`ld.lld`) plus the minimal MinGW support files it needs. In a
schema-v2 release package those files ship as the verified `native-link/`
sidecar beside the executable and are used in place (nothing is embedded and
nothing is copied into a cache); an optional embedded CI/dev build carries the
same set inside the binary and extracts it to a local cache at first use (see
`docs/design/native-link-embedding.md` for the full design). This section
covers the release-time steps that make both possible; it does not
change anything described above about `build-runtime-archive` on its own.

The runtime shim (`runtime/osc_native_shim.c`) is now precompiled directly
into every runtime archive's `sources` list (`runtime-archive-contract.json`,
`schema_version: 2`) — `build-runtime-archive` compiles it like any other
runtime source, and its manifest records `contains_native_shim`/
`native_shim_member` so no C compilation is needed at native-build time
downstream. A freestanding archive built without the shim is a hard,
actionable error; a legacy hosted archive without it falls back to a
diagnosed local compile.

`scripts/release_tools.py`'s **`prepare-embed-assets`** subcommand stages the
embedded linker asset set — for `windows-x86_64` today, exactly 13 files
(`ld.lld.exe`, its 5 required runtime DLLs, 6 Win32 import libraries, and
`libclang_rt.builtins-x86_64.a`), totaling ≈85.4 MB — from an already-fetched
pinned toolchain directory into `packaging/prebuilt/<target>/`, alongside a
`native-link-assets.json` manifest with a per-file sha256. `ld.lld.exe` is not
a static binary; its runtime DLLs (`libLLVM-22.dll`, `libc++.dll`,
`libwinpthread-1.dll`, `libunwind.dll`, `libffi-8.dll`) are staged into the
same `bin/` subdirectory so Windows' default DLL search resolves them with no
`PATH` changes. Thin wrappers `scripts/prepare-embed-assets.ps1`/`.sh` mirror
the existing `build-runtime-archive.ps1`/`.sh` style:

```powershell
scripts\prepare-embed-assets.ps1 `
  -Target windows-x86_64 `
  -ToolchainDir build\toolchain-windows-x86_64 `
  -ToolchainManifest packaging\toolchains\windows-x86_64.json `
  -OutputDir packaging\prebuilt\windows-x86_64
```

`cargo build` then embeds those staged assets via two `build.rs` env vars:
`OSCAN_EMBED_ASSETS_DIR` (path to the staged directory) and
`OSCAN_REQUIRE_EMBEDDED_ASSETS=1` (fail the build if any asset is
missing/incomplete/digest-mismatched, rather than silently skipping
embedding). Neither is required for an ordinary dev `cargo build`; without
`OSCAN_EMBED_ASSETS_DIR` the build still succeeds with nothing embedded, and
the resulting `oscan.exe` falls back to the external/bundled C-toolchain
linker driver at native-link time (printing a one-line `note:` the first time
that happens).

**Release binaries deliberately embed nothing.** A published object package
ships the same asset set as a verified `native-link/` sidecar beside the
compiler instead, so the workflow explicitly clears
`OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS` before every release
`cargo build`. The embedding path above is what CI's dedicated smoke jobs
exercise, and what a local build can opt into.

**The release `package` job's step order** (one job per target; every expensive
input is prepared once and reused by that target's backends):

1. Checkout / Rust / Python setup.
2. **Resolve the pinned archives** (`resolve-archive.ps1`, once per target:
   the C toolchain archive and, where the manifest declares one, the LLVM
   provider archive), then **extract the pinned toolchain**
   (`fetch-toolchain.ps1`/`.sh`) for local use only.
3. **Build runtime archives with the shim baked in** (`build-runtime-archive.ps1`/`.sh`).
4. **`prepare-embed-assets.ps1`/`.sh`** — stages the native-link asset set
   (13 files/≈85.4 MB on Windows, 1 file/≈2.78 MB on Linux) that becomes each
   object package's sidecar.
5. **One `cargo build --release --no-default-features --features backend-<b>`
   per backend**, stamped with `OSCAN_DISTRIBUTION_BACKEND=<b>` and copied to a
   per-backend path before the next build overwrites `target/release`.
6. **For each backend: assemble → smoke → stage for upload.**
   `assemble-release.ps1` takes `-Backend` plus exactly the prepared inputs
   that variant's component list declares (`-NativeLinkDir`,
   `-PrebuiltRuntimeArchiveDir`, `-ToolchainArchive`, `-LlvmProviderArchive`),
   so a `c` package never receives object assets and an object package never
   receives a C toolchain. Only the C package's `toolchain/` carries a
   compiler; LLVM and Cranelift freestanding final links need no compiler
   executable at all.
7. **Cut the recommended MSI** from the staged Windows LLVM bundle directory
   (Windows only), so the installer contains exactly what the smoked archive
   contains.

CI (`ci.yml`) keeps its main `linux` and `windows` jobs building *without*
`OSCAN_EMBED_ASSETS_DIR`, so the dev/external-toolchain path stays covered.
The embedded path is covered by dedicated smoke jobs that stage
`prepare-embed-assets` and rebuild with `OSCAN_EMBED_ASSETS_DIR` plus
`OSCAN_REQUIRE_EMBEDDED_ASSETS=1`. These jobs are required, not optional —
none of them is `continue-on-error`, so a failure blocks merging:

- `native-link-embedding-smoke` (Windows) runs
  `tests/windows_native_ci.tests.ps1`, which chains the implicit-default check
  (`default_backend.tests.ps1`, expecting `llvm`),
  `llvm_toolchain_isolation.tests.ps1`, the `scripts/compare-backend-size.ps1`
  size gate (LLVM `hello.osc` no larger than the C build), the native-link
  isolation suite, and the full C-vs-Cranelift and C-vs-LLVM differential
  suites.
- `native-link-embedding-smoke-linux` runs the same implicit-LLVM-default and
  `llvm_toolchain_isolation.tests.ps1` gates, a freestanding hello smoke with
  `cc`/`gcc`/`clang`/`ld`/musl tool names stubbed out on a restricted `PATH`,
  and both differential suites. The size gate is Windows-only today, so it is
  not part of this job.
- `native-link-embedding-smoke-linux-aarch64` and
  `native-link-embedding-smoke-linux-riscv64` cover embedded cross-linking and
  QEMU execution for both Cranelift and LLVM objects. The RISC-V gate also
  asserts RVC plus the `lp64d` double-float ABI before execution.

## Native-link assets for self-contained Linux object builds

On Linux x86-64, freestanding LLVM and Cranelift object generation/final
linking need no external compiler driver. LLVM object emission loads packaged
LLVM 22 in-process, and the final link uses a fully static
`x86_64-linux-musl-ld` binary from the pinned musl-cross toolchain. In a
schema-v2 release package that linker arrives as the verified
`native-link/` sidecar and is used in place; in an optional embedded
(dev/standalone) build it lives inside the binary and is extracted to a local
cache at first use (see `docs/design/native-link-embedding.md` §10 for the
Linux-specific details). The asset-set size contrast is notable: **Linux needs
exactly 1 file (~2.78 MB)** vs Windows' 13 files (~85.4 MB) — the Linux linker
is a fully static binary with zero shared-library dependencies, while Windows'
`ld.lld.exe` requires 5 sibling DLLs.

The same `scripts/release_tools.py` **`prepare-embed-assets`** subcommand handles
both platforms. For Linux:

```bash
scripts/prepare-embed-assets.sh \
  --target linux-x86_64 \
  --toolchain-dir build/toolchain-linux-x86_64 \
  --toolchain-manifest packaging/toolchains/linux-x86_64.json \
  --output-dir packaging/prebuilt/linux-x86_64
```

The asset set (1 linker binary, `native-link-assets.json` manifest with sha256)
is staged into `packaging/prebuilt/linux-x86_64/linker/`. A dev/CI build can
embed it through `OSCAN_EMBED_ASSETS_DIR`/`OSCAN_REQUIRE_EMBEDDED_ASSETS=1`;
the Linux release job instead stages the same set as each object package's
verified `native-link/` sidecar, exactly as Windows does, and its step order
mirrors the Windows list above.

CI includes a `native-link-embedding-smoke-linux` job parallel to the Windows
one, validating the Linux embedded path without requiring every dev `cargo
build` to have staged assets.

## Cross-target linking for aarch64/riscv64 (Linux x86-64 host)

A single Linux `oscan` binary only carries the linker for its own host target
(`linux-x86_64`) — see `docs/design/native-link-embedding.md` §11.1 for why
multi-target embedding is deliberately out of scope.

**Schema 2 packages ship no cross-target sidecars.** Each published package
contains only its own target's assets, and staging *refuses* a package that
contains a `cross-linkers/` directory: the earlier full-bundle model that
folded `cross-linkers/<target>/` into the `linux-x86_64` archive is gone along
with the full bundle itself.

The override mechanism the compiler exposes is unchanged, so cross-linking
still works from user-supplied inputs. It needs a linker
(`OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR=elf`) *and* a
matching-target runtime archive to link against (`OSCAN_RUNTIME_ARCHIVE_DIR`;
see `src/backend/link/archive.rs`) — both, together:

```bash
OSCAN_NATIVE_LINKER=/path/to/aarch64-linux-musl-ld \
OSCAN_NATIVE_LINKER_FLAVOR=elf \
OSCAN_RUNTIME_ARCHIVE_DIR=/path/to/linux-aarch64-runtime-archives \
oscan prog.osc --backend cranelift --native-target linux-aarch64 -o prog
```

Build the target's runtime archives with `build-runtime-archive.ps1|.sh
--target linux-aarch64|linux-riscv64` and take the `ld` binary from that
target's pinned musl-cross toolchain (`fetch-toolchain.ps1|.sh`).

CI validates that override mechanism via a "Cross-link via OSCAN_NATIVE_LINKER
sidecar override" step in each of the
`native-link-embedding-smoke-linux-{aarch64,riscv64}` jobs, which builds a
plain (non-embedding) `oscan` binary and cross-links using only the fetched
toolchain's `ld` and the built runtime archive.

## Creating a release

After both prerequisites are in place, tag a version and push:

```bash
git tag v0.0.12
git push origin v0.0.12
```

The Release workflow then builds one compiler per backend for each target,
assembles and smokes all seven archives, cuts the recommended Windows LLVM
MSI, writes `SHA256SUMS` over everything, and publishes the result to GitHub
Releases. To rehearse without publishing, use the manual workflow with
`publish` cleared (see "Dry run without publishing" above).
