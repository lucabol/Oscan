# Oscan Compiler Technical Details

This document collects compiler, backend, runtime, linker, packaging, and
validation details that are intentionally omitted from the user-focused
[README](../README.md). For language syntax, use the
[language guide](guide.md) or [formal specification](spec/oscan-spec.md).

## Backend model

Oscan has three production backends:

| Backend | Role | Compilation path | Packaged requirement |
|---|---|---|---|
| `llvm` | Recommended optimized backend | Typed Oscan IR -> shared LIR -> LLVM IR -> in-process LLVM 22 -> object | Packaged `libLLVM` and Oscan's direct linker |
| `cranelift` | Independent native-code alternative | Typed Oscan IR -> shared LIR -> Cranelift object | Oscan's direct linker |
| `c` | Portability, readable source output, and differential oracle | Typed Oscan program -> C99 -> C compiler | Bundled or host C toolchain |

The LLVM and Cranelift backends share the same semantic lowering and runtime
ABI, but independently emit object code. The LLVM path loads LLVM as a library
inside the compiler; it does not invoke `clang`, `llvm-as`, `opt`, or `llc`.

`--backend native` is a deprecated compatibility alias for
`--backend cranelift`.

### Backend selection

Every release package contains exactly one backend and defaults to it. Asking a
release build for another backend produces an error that identifies the package
to install.

A default development build contains all three backends. Without an explicit
selector it chooses:

1. LLVM on a supported host when a compatible provider is available.
2. Cranelift when native object generation is available.
3. C as the portability fallback.

An explicit `--backend llvm|cranelift|c` always wins. An explicit LLVM failure
never falls back to another backend.

Some output requests imply a backend:

- `--emit-c`, `-o file.c`, and `--target riscv64|wasi` select C.
- `--emit-llvm-ir` and `-o file.ll` select LLVM.
- An explicit `--native-target` without `--backend` selects Cranelift for
  compatibility.
- `--libc`, `--extra-c`, and `--extra-cflags` do not by themselves select C,
  though they may require a C toolchain during the final build.

## Debug information

`--debuginfo none` is the release-compatible default. Opting into
`--debuginfo line-tables` preserves Oscan source locations for source
breakpoints, stepping, stack symbolization, and imported-file mappings. It does
not change optimization. Oscan's native emitters do not describe local
variables or source types at this level; a C toolchain may include additional
records.

| Backend | Line-table implementation |
|---|---|
| C | The generated translation unit uses `#line` directives that name the original `.osc` files. Clang uses `-gline-tables-only`, GCC uses `-g1`, and MSVC uses embedded `/Z7` records plus linker debug output (which can contain more than line tables). |
| LLVM | Oscan emits `DICompileUnit`, `DIFile`, `DISubprogram`, and `DILocation` metadata directly in its LLVM IR with `LineTablesOnly` emission. |
| Cranelift | Oscan interns Cranelift source locations, converts final machine-code ranges into DWARF line rows, and writes the required DWARF sections into the object. |

Debug final links retain symbols and line sections instead of passing strip
flags. Debug-mode C and native code also preserve frame pointers; unwind
metadata is retained or generated where the backend supports it.

On Windows, the C backend can produce a PDB through an MSVC-compatible
toolchain. The LLVM and Cranelift object backends currently emit DWARF in COFF,
not CodeView/PDB, so they require a debugger that understands DWARF-in-COFF.
Native CodeView/PDB generation, optimized locals, and full type information are
separate future tiers.

## Release packages

Release contract schema 2 publishes one archive per platform/backend pair. It
does not publish a combined package.

| Platform | Backend | Artifact | Additional host requirement |
|---|---|---|---|
| Windows x86_64 | `llvm` | `oscan-vX.Y.Z-windows-x86_64-llvm.msi` or `oscan-vX.Y.Z-windows-x86_64-llvm.zip` | None |
| Windows x86_64 | `cranelift` | `oscan-vX.Y.Z-windows-x86_64-cranelift.zip` | None |
| Windows x86_64 | `c` | `oscan-vX.Y.Z-windows-x86_64-c.zip` | None; the C toolchain is bundled |
| Linux x86_64 | `llvm` | `oscan-vX.Y.Z-linux-x86_64-llvm.tar.xz` | glibc 2.34+ and the provider's host libraries |
| Linux x86_64 | `cranelift` | `oscan-vX.Y.Z-linux-x86_64-cranelift.tar.xz` | None |
| Linux x86_64 | `c` | `oscan-vX.Y.Z-linux-x86_64-c.tar.xz` | None; the C toolchain is bundled |
| macOS x86_64 | `c` | `oscan-vX.Y.Z-macos-x86_64-c.tar.gz` | Xcode Command Line Tools |

Object-backend packages contain the compiler, precompiled freestanding runtime
archives, and the files needed for final linking. They do not contain a C
compiler, C headers, or a sysroot. They therefore reject operations that need
one, including C emission, hosted libc builds, explicit C sources, rebuilding
the runtime, and generated C ABI shims.

The C packages contain the compiler and a pinned C toolchain on Windows and
Linux. The macOS package uses Apple Clang from Xcode Command Line Tools.

### Package layout

A Windows object-backend package has this shape:

```text
oscan-vX.Y.Z-windows-x86_64-llvm/
  oscan.exe
  native-link/
    native-link-assets.json
    bin/ld.lld.exe
    libLLVM-22.dll
    ...
  build/runtime-archives/windows-x86_64/
    libosc_runtime_freestanding*.a
    libosc_runtime_freestanding*.json
  oscan-package.json
  install.ps1
  README-install.txt
```

A Windows C-backend package instead carries its toolchain:

```text
oscan-vX.Y.Z-windows-x86_64-c/
  oscan.exe
  toolchain/
    bin/clang.exe
    bin/llvm-ar.exe
    ...
  oscan-package.json
  install.ps1
  README-install.txt
```

Toolchains and runtime archives are release artifacts rather than source files,
so they are not committed to the Git repository.

## Runtime and final linking

Oscan supports two runtime modes:

- **Freestanding:** uses Oscan's runtime and direct OS interfaces. This is the
  normal mode for standalone programs on validated targets.
- **Hosted:** `--libc` uses the platform C library and a compiler-driver link.

On Windows x86_64 and Linux x86_64, packaged LLVM and Cranelift builds share a
self-contained freestanding final-link path. Users do not need an installed C
compiler for that path.

### Release sidecars and embedded assets

Release packages keep linker assets beside the compiler under
`<exe-dir>/native-link/`. The manifest is
`<exe-dir>/native-link/native-link-assets.json`; every file is SHA-256 verified
before use. Verified files are **used in place**, not copied into a cache. A
missing or invalid file is a hard error with no silent fallback.

Development and standalone builds may instead set
`OSCAN_EMBED_ASSETS_DIR` together with `OSCAN_REQUIRE_EMBEDDED_ASSETS=1`. Those
builds embed the same assets in the compiler and extract them on first use to:

- `%LOCALAPPDATA%\oscan\native-assets\` on Windows.
- `$XDG_CACHE_HOME/oscan/native-assets` or
  `$HOME/.cache/oscan/native-assets` on Linux.

A plain `cargo build` has neither a release sidecar nor embedded assets, so
final linking can use an external or bundled compiler driver.

The direct-link boundary does not include:

- Hosted `--libc` mode.
- Explicit `--extra-c <file>` sources.
- User `extern` signatures containing `str` when a generated C ABI shim is
  required.

`OSCAN_NO_TOOLCHAIN=1` makes a development build reject paths that would escape
to a C toolchain.

On Windows, object-backend final linking and `--run` refuse an elevated
Administrator process by default. Trusted CI and release jobs may pass
`--allow-elevated-native-link`; other path, manifest, and sandbox checks remain
enabled.

## Provider and toolchain discovery

### LLVM provider

The LLVM provider lookup order is:

1. `OSCAN_LLVM_LIB` - absolute shared-library override.
2. `OSCAN_LLVM_DIR` - absolute provider-directory override.
3. `OSCAN_TOOLCHAIN_DIR` - absolute packaged-toolchain root.
4. Executable-relative roots, in order: `<exe-dir>/toolchain`,
   `<exe-dir>/native-link`, and `<exe-dir>`.

LLVM is not loaded from the current directory, `PATH`, or a bare platform
loader search. Windows resolves the provider from the verified native-link
sidecar. Linux resolves it from the package's provider-only `toolchain/`
directory.

### C compiler

When a build needs a C compiler, discovery uses:

1. `OSCAN_CC` - explicit compiler path or command.
2. `OSCAN_TOOLCHAIN_DIR` - bundled toolchain root override.
3. A sibling `toolchain/` directory beside the Oscan executable.
4. A `toolchain/` directory in the current working directory.
5. Normal host compiler detection.

Bundled toolchains search `toolchain/windows/bin/` then `toolchain/bin/` on
Windows, and `toolchain/linux/bin/` then `toolchain/bin/` on Linux.
Target-specific cross-compilation still needs the matching target tools.

## Building from source

Build the development compiler with:

```bash
cargo build --release
```

The compiler itself has no LLVM SDK or LLVM executable build dependency. A
default source build contains all three backends. Single-backend builds use:

```bash
cargo build --release --no-default-features --features backend-llvm
cargo build --release --no-default-features --features backend-cranelift
cargo build --release --no-default-features --features backend-c
```

Running LLVM from an ordinary source build requires an LLVM 22 shared library,
selected with `OSCAN_LLVM_LIB` or `OSCAN_LLVM_DIR`. Source builds also need a C
compiler for the C backend, hosted builds, extra C sources, and final links when
no direct-link assets are available.

Release builds stage provider, linker, runtime, and toolchain components
according to [the release contract](../packaging/toolchains/release-contract.json).
See the [release guide](releasing.md) for the reproducible packaging workflow.

## Supported targets

| Target | Backends | Notes |
|---|---|---|
| Windows x86_64 | LLVM, Cranelift, C | All three release packages are published |
| Linux x86_64 | LLVM, Cranelift, C | All three release packages are published |
| Linux AArch64 | LLVM, Cranelift, C | Cross-link and QEMU validation depend on target-matched assets |
| Linux RISC-V 64 | LLVM, Cranelift, C | Cross-link and QEMU validation depend on target-matched assets |
| WebAssembly/WASI | C | Selected with `--target wasi` |
| macOS x86_64 | C | Uses hosted Apple Clang |

Object generation, final linking, release packaging, and local execution are
separate capabilities. A backend may emit a target object even when the current
package cannot link or run that target locally.

## Size and validation

The pinned Windows release-toolchain matrix compiled all 37 examples with all
three backends:

| Backend | `hello.osc` | 37-example total |
|---|---:|---:|
| LLVM | 5,632 bytes | 811,008 bytes |
| Cranelift | 6,144 bytes | 861,696 bytes |
| C | 8,704 bytes | 875,520 bytes |

These measurements are toolchain- and revision-specific, not general language
guarantees. `scripts/compare-backend-size.ps1` enforces the focused
`hello.osc` release invariant, and `scripts/sample-backend-matrix.ps1` produces
the recursive example matrix.

Compiler validation includes:

```bash
cargo test
```

On Windows:

```powershell
.\test.ps1
.\test.ps1 -Backend llvm
.\test.ps1 -Backend cranelift
.\tests\run_tests.ps1 -Oscan .\target\release\oscan.exe
```

See the [test suite guide](test_suite.md) for focused backend, isolation,
cross-target, and packaging checks.

## Repository layout

| Directory | Contents |
|---|---|
| `src/` | Rust compiler frontend, semantic analysis, backends, and linker |
| `runtime/` | Freestanding C runtime embedded or precompiled for Oscan programs |
| `tests/` | Positive, negative, backend, and packaging tests |
| `examples/` | Runnable Oscan programs |
| `docs/` | User guides, specification, and design documents |
| `scripts/` | Release, packaging, generation, and validation tooling |
| `libs/` | Oscan source libraries imported with `use` |

## Design documents

- [Direct LLVM backend](design/llvm-backend.md)
- [Native link embedding](design/native-link-embedding.md)
- [Scoped arenas](design/scoped-arenas.md)
- [Release packaging](releasing.md)
- [Language safety](safety.md)
