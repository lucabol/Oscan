# Release Packaging

Release builds are handled by GitHub Actions workflows. Two manual workflows must be run **once** (and again whenever their upstream dependencies change) before creating a release.

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
`.a` archives of the runtime, in three modes:

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
which is the canonical, reproducible entry point a native backend or CI job
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

Release assembly builds the target's configured runtime modes, then stages
each archive/manifest pair at
`build/runtime-archives/<target>/` inside the bundle. It also stages
`runtime/osc_native_shim.c` and `runtime/osc_runtime.h` under the bundle's
`native-runtime/` directory. Keeping that directory separate avoids making the
C backend mistake a native-only source subset for a complete on-disk C runtime.
The paths mirror the native backend's executable-relative lookup contract and
are copied intact by the installers. Release smoke tests assert the assets
survived packaging and installation, then compile and run the sample with freestanding
`--backend native` on Linux and Windows. The phase-1 macOS binary-only target
does not advertise or package the native backend because that backend has no
Darwin target yet.

`scripts/smoke-release.ps1` expects `bundled` compiler-source reporting for
both the regular `--backend c` compile and the packaged `--backend native`
compile on every bundle-kind-`full` target (Windows and Linux) — there is no
per-platform host-compiler override. Both compiles additionally run with a
"no host compiler" PATH prefix: a scratch directory containing `cc`/`gcc`/
`clang` (and `cl` on Windows) stubs that fail immediately, prepended to the
real `PATH` so every other tool (`sh`, `dirname`, `tar`, ...) still resolves
normally. Bundled-compiler discovery never consults `PATH` (it walks the
toolchain directory directly), so this only shadows PATH-based host-compiler
fallback paths — proving each bundle is genuinely self-contained rather than
merely preferring its own toolchain when a host one also happens to be
present. This replaced an earlier `OSCAN_CC=gcc`/`--libc` override that was
added for Linux specifically because the bundled musl GCC was believed to be
non-relocatable; investigating that belief while fixing the archive/compiler
mismatch above found the toolchain itself to be relocatable and fully
functional (see above), so the override was hiding a real bug rather than
working around an unfixable one.

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

## Creating a release

After both prerequisites are in place, tag a version and push:

```bash
git tag v0.0.12
git push origin v0.0.12
```

The Release workflow automatically builds oscan for Windows and Linux, assembles bundles with the toolchain and libbearssl.a, runs smoke tests, and publishes to GitHub Releases.
