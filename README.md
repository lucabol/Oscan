# Oscan

[![CI](https://github.com/lucabol/Oscan/actions/workflows/ci.yml/badge.svg)](https://github.com/lucabol/Oscan/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lucabol/Oscan?include_prereleases&sort=semver)](https://github.com/lucabol/Oscan/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20ARM64%20%7C%20RISC--V%20%7C%20WASI-blue)

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile through LLVM, direct Cranelift object code, or portable C99. Oscan is designed so that LLMs *understand what they are writing* — a small, explicit grammar with readable C and LLVM IR output you can inspect.

## Three Backends

Oscan ships three production backends, and every release package contains
exactly one of them. **LLVM is the recommended backend and the default
choice**; `cranelift` is the independent direct-codegen alternative, and `c`
is the portability/reference path.

| Backend | Role | Compilation path | Packaged requirement |
|---|---|---|---|
| **`llvm`** | **Recommended default** | Typed Oscan IR -> shared LIR -> LLVM IR -> in-process LLVM 22 -> object | Packaged `libLLVM` and Oscan's direct linker; no C compiler, no C source |
| **`cranelift`** | Independent direct-codegen alternative | Typed Oscan IR -> shared LIR -> Cranelift object | Oscan's direct linker; no C compiler, no C source |
| **`c`** | Portability, readable source output, and differential oracle | Typed Oscan program -> C99 -> C compiler | Bundled pinned C toolchain (Windows/Linux) or Apple Command Line Tools (macOS) |

`--backend native` is a deprecated compatibility alias for `--backend
cranelift`: it still works and warns once, but `cranelift` is the canonical
spelling used by every command, package name, and diagnostic below.


## Contents

- [Three Backends](#three-backends)
- [Language Highlights](#language-highlights)
- [For AI Coding Agents](#for-ai-coding-agents)
- [A Quick Look](#a-quick-look)
- [Installation](#installation)
- [Getting Started](#getting-started)
- [Tutorial](docs/tutorial.md)
- [Examples](#examples)
- [Built-in Functions](#built-in-functions)
- [Learn More](#learn-more)
- [Testing](#testing)
- [Contributing](#contributing)
- [Project Structure](#project-structure)
- [License](#license)

## Language Highlights

- **Three deliberate backend roles.** LLVM is the recommended default; Cranelift is the independent direct object-code implementation, and C remains the portability/reference/source-emission path.
- **Runs without a C library.** LLVM, Cranelift, and C support freestanding direct-syscall builds on their validated targets. (A `--libc` mode is available for hosted builds when you want it.)
- **[Safe by design.](docs/safety.md)** No buffer overflows, no use-after-free, no null pointers, no integer overflow UB — [11 of 11 major bug categories](docs/safety.md) eliminated.
- **Built-in graphics.** Canvas, drawing primitives, and input handling — write games and visualizations with zero external dependencies.
- **Socket networking.** TCP and UDP builtins with hostname resolution — build HTTP clients and web servers out of the box.
- **238 standard functions.** String interpolation, hash maps, math, file I/O, SHA-256, sorting, graphics, networking, and more — batteries included. See the [full reference](docs/builtins.md).
- **Purity visible in signatures.** `fn` for pure functions, `fn!` for side effects — the type system tracks who can do I/O.
- **Errors as values.** `Result<T, E>` with `try` propagation. No exceptions, no hidden control flow.
- **Guarded C output.** Generated C systematically avoids undefined behavior with bounds checks and overflow guards.
- **One allocation model.** Arena-based memory — no manual alloc/free, no GC, deterministic cleanup.
- **Immutable by default.** `let` is immutable; `let mut` opts in to mutation. Anti-shadowing enforced.
- **[26 reserved words.](docs/spec/oscan-spec.md#11-reserved-words-26-total)** Explicit types, no inference, no implicit coercions — minimal surface for LLMs to hallucinate on.
- **Order-independent definitions.** Use functions, types, and constants before they are declared.
- **Namespaced imports.** `use "math.osc" as math` — access imported symbols via `math.add(...)` to avoid name collisions in larger programs.
- **99 positive tests, 35 negative tests, 37 examples.** Tested across LLVM, C, and Cranelift backends on Windows and Linux, with ARM64/RISC-V64/QEMU, macOS, and WASI coverage where supported.

## For AI Coding Agents

Oscan is a new language — LLMs are not pre-trained on it. If you're using an AI coding agent (GitHub Copilot, Claude, Cursor, etc.), point it at the **language reference** before writing `.osc` code:

📄 [`.github/instructions/oscan.instructions.md`](.github/instructions/oscan.instructions.md) — critical syntax differences, common anti-patterns, annotated examples, and the full built-in function table.

GitHub Copilot picks this up automatically via `applyTo: "**/*.osc"`. For other tools, include it in your context or system prompt.

This file is **auto-generated** from the compiler source and example programs — run `python scripts/gen-copilot-instructions.py --inject` to update it, or let CI do it on push.

## A Quick Look

```rust
fn fib(n: i32) -> i32 {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}

fn! main() {
    let name: str = "Oscan";
    for i in 0..10 {
        println("{name} fib({i}) = {fib(i)}");
    };
}
```

## Installation

### Option 1: GitHub Releases (recommended for most users)

Every release publishes **one archive per (platform, backend) pair** plus a
single Windows installer for the recommended LLVM package. Pick the backend
first, then the archive for your platform.

**Release artifacts**

| Platform | Backend | Artifact | Contents | Toolchain you must install | Recommendation |
|---|---|---|---|---|---|
| Windows x86_64 | `llvm` | `oscan-vX.Y.Z-windows-x86_64-llvm.msi` (installer) or `oscan-vX.Y.Z-windows-x86_64-llvm.zip` | `oscan.exe`, `native-link/` (linker + verified `libLLVM-22.dll`), precompiled freestanding runtime archives | none | **Recommended** |
| Windows x86_64 | `cranelift` | `oscan-vX.Y.Z-windows-x86_64-cranelift.zip` | `oscan.exe`, `native-link/` (linker), precompiled freestanding runtime archives | none | Alternative direct-codegen package |
| Windows x86_64 | `c` | `oscan-vX.Y.Z-windows-x86_64-c.zip` | `oscan.exe`, pinned C toolchain under `toolchain/` | none (toolchain is bundled) | Use when you need C output or C interop |
| Linux x86_64 | `llvm` | `oscan-vX.Y.Z-linux-x86_64-llvm.tar.xz` | `oscan`, `native-link/` (linker), `toolchain/` with the packaged LLVM code generator only, precompiled freestanding runtime archives | glibc ≥ 2.34 plus the provider's host runtime packages (see below) | **Recommended** |
| Linux x86_64 | `cranelift` | `oscan-vX.Y.Z-linux-x86_64-cranelift.tar.xz` | `oscan`, `native-link/` (linker), precompiled freestanding runtime archives | none | Alternative direct-codegen package |
| Linux x86_64 | `c` | `oscan-vX.Y.Z-linux-x86_64-c.tar.xz` | `oscan`, pinned C toolchain under `toolchain/` | none (toolchain is bundled) | Use when you need C output or C interop |
| macOS x86_64 | `c` | `oscan-vX.Y.Z-macos-x86_64-c.tar.gz` | `oscan` plus its install script and phase-1 note; no toolchain | Xcode Command Line Tools (`xcode-select --install`) | The only macOS package today |

There is no combined package: each archive contains exactly one backend, and
the compiler inside it reports which package to install if you ask for a
different one. `SHA256SUMS` covers every published archive and the MSI.

**No C toolchain is required to run the LLVM and Cranelift packages.** The
release factory precompiles the Oscan runtime from C once, with the pinned
toolchain, and ships the resulting freestanding archives inside the package;
downstream users never compile C. Consequently the `llvm` and `cranelift`
packages contain no C compiler, no C headers, and no sysroot, and they refuse
the operations that would need one: `--backend c`, `--emit-c`, `-o file.c`,
`--libc`, `--extra-c`, `--extra-cflags`, rebuilding the runtime from source,
and `extern` signatures that would require a generated C ABI shim. macOS is
the exception: it has no object-backend target yet, so its only package is the
`c` package, which uses your installed Apple Command Line Tools.

> **Windows `cranelift` package:** it ships `libLLVM` only because the dynamic
> `ld.lld` linker it uses depends on that library at load time. The LLVM
> *backend* is not compiled into that package — `--backend llvm` there tells
> you to install the `llvm` package instead.

#### Windows x86_64

*Quick install (downloads, verifies and installs the latest release):*

```powershell
# Recommended: LLVM package (zip, per-user, no elevation)
iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex

# Or run the script explicitly, choosing the backend:
.\install-latest.ps1 -Backend llvm                 # recommended (default)
.\install-latest.ps1 -Backend llvm -Mode msi       # the one published MSI
.\install-latest.ps1 -Backend cranelift            # cranelift zip
.\install-latest.ps1 -Backend c                    # C package with bundled toolchain
```

`-Backend` defaults to `llvm`. The script resolves the *exact* asset name the
release contract derives from the resolved tag and that backend, verifies its
SHA-256 against the release's `SHA256SUMS`, and fails with an actionable
message if that exact asset is not published — it never falls back to a
differently versioned or differently named archive, and never silently
installs another backend's package. A release that publishes no `SHA256SUMS`
is a hard error unless you pass `-SkipChecksum` explicitly. Only the
recommended LLVM package is published as an MSI; `-Mode msi` with any other
backend is refused, and if that exact MSI is missing the script falls back
only to the exact `-llvm.zip`. `-Version vX.Y.Z` installs a specific release
instead of the latest.

*Manual install:*

1. Download `oscan-vX.Y.Z-windows-x86_64-<backend>.zip` (or
   `oscan-vX.Y.Z-windows-x86_64-llvm.msi` for the recommended installer) and
   the release's `SHA256SUMS`, keeping the downloaded file's original name
2. Verify exactly that asset:

   ```powershell
   $asset = 'oscan-vX.Y.Z-windows-x86_64-llvm.zip'
   $expected = (Select-String -Path SHA256SUMS -Pattern "\s\*?$([regex]::Escape($asset))$").Line.Split(' ')[0]
   $actual = (Get-FileHash -Path $asset -Algorithm SHA256).Hash.ToLowerInvariant()
   if ($actual -ne $expected.ToLowerInvariant()) { throw "checksum mismatch for $asset" }
   ```

3. Extract the archive and run `install.ps1` (or add the extracted directory to
   your PATH); for the MSI, double-click it or run
   `msiexec /i oscan-vX.Y.Z-windows-x86_64-llvm.msi /quiet`
4. Open a **new** terminal and verify: `oscan --version`

To remove an MSI install, use **Settings → Apps → Installed apps → Oscan →
Uninstall**, or `msiexec /x oscan-vX.Y.Z-windows-x86_64-llvm.msi /quiet`.
Deleting the installed directory by hand is only appropriate for archive
(zip/tar) installs — it leaves an MSI installation registered.

#### Linux x86_64

*Quick install (recommended LLVM package):*

```bash
set -eu
# The packaged LLVM code generator loads against the host's runtime libraries.
sudo apt-get install libedit2 libffi8 libxml2 libz3-4 libzstd1 zlib1g
ASSET_NAME=oscan-vX.Y.Z-linux-x86_64-llvm.tar.xz   # the exact release asset
BASE=https://github.com/lucabol/Oscan/releases/download/vX.Y.Z
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
# Keep the canonical file name so it matches the SHA256SUMS entry verbatim.
curl -fsSL -L "$BASE/$ASSET_NAME" -o "$TMPDIR/$ASSET_NAME"
curl -fsSL -L "$BASE/SHA256SUMS" -o "$TMPDIR/SHA256SUMS"
cd "$TMPDIR"
# Verify exactly this asset, not whatever else the file happens to list.
grep -E "[[:space:]]\*?${ASSET_NAME}$" SHA256SUMS > "${ASSET_NAME}.sha256"
sha256sum -c "${ASSET_NAME}.sha256"
tar -xJf "$ASSET_NAME"
./oscan-*/install.sh
```

Swap `llvm` in `ASSET_NAME` for `cranelift` or `c` to install another backend.
The Cranelift package has neither an LLVM provider nor a C toolchain and needs
no extra host packages; the C package carries its own pinned C toolchain.

*Manual install:*

1. Download `oscan-vX.Y.Z-linux-x86_64-<backend>.tar.xz` and `SHA256SUMS`,
   keeping the archive's original file name
2. Verify exactly that asset, then extract:
   ```bash
   grep -E "[[:space:]]\*?oscan-vX.Y.Z-linux-x86_64-llvm\.tar\.xz$" SHA256SUMS | sha256sum -c -
   tar xf oscan-vX.Y.Z-linux-x86_64-llvm.tar.xz
   ```
3. For the `llvm` package on Debian/Ubuntu:
   `sudo apt-get install libedit2 libffi8 libxml2 libz3-4 libzstd1 zlib1g`
   (glibc 2.34 or newer is also required; the package's `README-install.txt`
   records the same prerequisite)
4. Run `./install.sh` (or add the extracted directory to your PATH)
5. Verify: `oscan --version`

To uninstall an archive install, delete the directory you extracted/installed
into and remove it from your PATH.

#### macOS x86_64

1. Download `oscan-vX.Y.Z-macos-x86_64-c.tar.gz` and `SHA256SUMS`, keeping the
   archive's original file name
2. Verify exactly that asset, then extract:
   ```bash
   grep -E "[[:space:]]\*?oscan-vX.Y.Z-macos-x86_64-c\.tar\.gz$" SHA256SUMS | shasum -a 256 -c -
   tar xf oscan-vX.Y.Z-macos-x86_64-c.tar.gz
   ```
3. Copy `oscan` to `/usr/local/bin/` or another directory in your PATH
4. Verify: `oscan --version`

**macOS requires Xcode Command Line Tools** (or an equivalent C compiler),
because the macOS package is a C-backend package:

```bash
xcode-select --install
```

### Option 2: Build from Source

**Requirements:**
- Rust toolchain (for building the compiler)
- LLVM 22 shared library only when running the LLVM backend from a source build;
  release packages ship it, and no Clang executable or LLVM SDK is used
- C compiler (GCC, Clang, or MSVC) for the C backend, hosted object-backend
  builds, `--extra-c`, and local final links without packaged direct-link assets

**Build the compiler:**

```bash
git clone https://github.com/lucabol/Oscan.git
cd Oscan
cargo build --release
```

A default source build contains all three backends, which is exactly what a
released package does not: release packages are built with a single backend
feature (`--no-default-features --features backend-llvm|backend-cranelift|backend-c`)
and are stamped so they default deterministically to their own backend.

The binary is `target/release/oscan` (or `oscan.exe` on Windows). Building
Oscan itself needs only Rust: there is no LLVM Cargo/build dependency. To run
the LLVM backend from an ordinary source build, point `OSCAN_LLVM_LIB` at an
absolute LLVM 22 shared library path (or use `OSCAN_LLVM_DIR`). A plain local
build also omits packaged direct-link assets, so executable final linking may
use an external C-toolchain driver. Release builds package both the LLVM
provider and direct linker; their ordinary freestanding LLVM path generates no
C and invokes no C/Clang/LLVM tool executable.

<details>
<summary><strong>What lives inside a release package</strong></summary>

An object-backend package (`llvm` or `cranelift`) ships the compiler, the
verified native-link sidecar, and the precompiled freestanding runtime
archives — no C compiler, no C headers, no sysroot:

```text
oscan-vX.Y.Z-windows-x86_64-llvm/
  oscan.exe
  native-link/
    native-link-assets.json     # SHA-256 for every asset, verified before use
    bin/ld.lld.exe, libLLVM-22.dll, ...
  build/runtime-archives/windows-x86_64/
    libosc_runtime_freestanding*.a (+ .json manifests)
  oscan-package.json
  install.ps1
  README-install.txt
```

A `c` package instead ships the pinned C toolchain the C backend needs, and
none of the object-backend assets:

```text
oscan-vX.Y.Z-windows-x86_64-c/
  oscan.exe
  toolchain/
    bin/clang.exe, llvm-ar.exe, ...
  oscan-package.json
  install.ps1
  README-install.txt
```

Toolchains and runtime archives are **not** in the Git repository: they are
large binary artifacts produced by release builds, not source. The `oscan`
compiler discovers whichever of these its own package contains relative to its
executable — never from `PATH` or the current directory.

</details>

---

## Getting Started

### Compile Your First Oscan Program

Create `hello.osc`:

```rust
fn! main() {
    println("Hello, Oscan!");
}
```

Run it:

```bash
oscan hello.osc --run       # compile and execute
oscan hello.osc              # compile to hello.exe (Windows) / hello (Linux)
oscan hello.osc -o out.c     # transpile to C only
oscan hello.osc -o out.ll    # emit textual LLVM IR
oscan hello.osc --backend llvm --run       # require LLVM; never falls back
oscan hello.osc --backend cranelift --run  # force direct Cranelift code generation
oscan hello.osc --backend c --run          # force the portability/reference backend
```

A released package contains one backend, so `--backend` there either names
that backend or reports which package to install instead. A development build
contains all three.

**Continue with the [introductory tutorial](docs/tutorial.md):** build a small
`wc`-style command-line tool while learning types, loops, pure and impure
functions, structs, and `Result` error handling.

**CLI options:**
```
oscan [OPTIONS] <file.osc>
  -o <path>       Output path (exe by default; .c for C, .ll for LLVM IR)
  --run           Compile and execute immediately
  --emit-c        Emit C-backend source to stdout
  --emit-llvm-ir  Emit textual LLVM IR to stdout
  --libc          Use hosted libc mode (including LLVM/Cranelift)
  --backend <name>  llvm|cranelift|c, limited to the backends this build contains
                    ('native' is a deprecated alias for cranelift)
  --native-target <tag>  LLVM/Cranelift object target (default: host)
  --target <arch> Cross-compile for target architecture (riscv64, wasi)
  --allow-elevated-native-link  Trusted CI/release only: allow native final link/--run from an elevated Windows process
  --extra-c <file>  Extra C source file to compile and link (repeatable)
  --extra-obj <file>  Precompiled object file to link (.o/.obj, repeatable)
  --extra-lib <lib>  Static library path (.a/.lib) or system library name (repeatable)
  --extra-cflags <flag>  Extra flag passed to the C compiler (repeatable)
  --dump-ast      Print AST (debug)
  --dump-tokens   Print tokens (debug)
```

**Backend roles:**

- **`llvm` (recommended):** lowers typed Oscan IR through the same
  backend-neutral LIR used by Cranelift, emits LLVM IR directly, and asks the
  packaged LLVM 22 library in-process to parse, verify, optimize, and emit a
  relocatable object. No generated C or code-generation subprocess is involved.
- **`cranelift`:** direct Cranelift object-code backend for Windows
  x86-64 and Linux x86-64/AArch64/RISC-V64 — an independent implementation of
  the same object path, and the capability fallback in a build that also
  contains LLVM. `--backend native` is its deprecated alias.
- **`c`:** portability/reference backend. It is selected implicitly by
  `--emit-c`, `-o file.c`, `--target riscv64|wasi`, and hosts with no object
  backend such as macOS. It remains the differential correctness oracle.

An explicit `--backend llvm|cranelift|c` always wins, and an explicit LLVM
failure never falls back. A released package defaults deterministically to its
own single backend. In a multi-backend (development) build without a selector,
supported Windows/Linux hosts choose LLVM when a compatible packaged provider
is available, then Cranelift, then C. For compatibility, an explicit
`--native-target` without `--backend` selects Cranelift. C source and
`--target riscv64|wasi` requests select C; LLVM IR requests select LLVM.
`--libc`, `--extra-c`, and `--extra-cflags` do not force C.

See [the LLVM backend design](docs/design/llvm-backend.md) for the architecture,
no-toolchain boundary, targets, and tradeoffs.

**Windows/Linux provider and toolchain lookup:**

LLVM provider lookup order is:

1. `OSCAN_LLVM_LIB` — absolute shared-library override
2. `OSCAN_LLVM_DIR` — absolute provider-directory override
3. `OSCAN_TOOLCHAIN_DIR` — absolute packaged-toolchain root
4. executable-relative roots, in order: `<exe-dir>/toolchain`,
   `<exe-dir>/native-link` (the verified package sidecar), `<exe-dir>`

LLVM is never loaded from the current directory, `PATH`, or the bare platform
loader search path. The Windows and Linux `llvm` packages ship the required
LLVM 22 provider and therefore default to LLVM. Windows resolves it from the
verified `native-link/` sidecar it shares with the linker; Linux resolves it
from the package's `toolchain/` directory, which contains the code generator
only — no clang, no GCC, no headers, no sysroot. macOS remains on C.

For builds that need a C compiler, Oscan resolves it in this order:

1. `OSCAN_CC` — explicit compiler path/command override
2. `OSCAN_TOOLCHAIN_DIR` — bundled toolchain root override
3. sibling `toolchain/` directory next to the `oscan` binary
4. `toolchain/` directory in the current working directory
5. normal host compiler detection/fallback

When a bundled toolchain directory is used (`OSCAN_TOOLCHAIN_DIR`, sibling `toolchain/`, or `toolchain/` in the current working directory), Oscan checks platform-specific and generic `bin/` directories:

- Windows: `toolchain/windows/bin/`, then `toolchain/bin/`
- Linux: `toolchain/linux/bin/`, then `toolchain/bin/`

If your Oscan `c` package includes that bundled `toolchain/` directory, you do not need to install a separate system compiler. The `llvm` and `cranelift` packages ship no C compiler at all and refuse the builds that would need one. Cross-compilation targets such as `--target riscv64` and `--target wasi` still require their own target-specific toolchains.

### Self-contained object-backend final links (Windows & Linux)

**On Windows x86-64 and Linux x86-64, freestanding LLVM and Cranelift objects
use the same self-contained final-link path.** LLVM IR and object generation
also run in-process through packaged `libLLVM`. No installed C compiler, Clang
executable, LLVM SDK, `llc`, `opt`, or `llvm-as` is involved.

Where those linker assets come from depends on how the compiler was produced:

- **Release packages (schema v2, the normal case):** the assets ship *beside*
  the executable as a verified sidecar,
  `<exe-dir>/native-link/native-link-assets.json` plus its files. Nothing is
  embedded in the binary, and nothing is copied into a cache: every file is
  SHA-256-verified against that manifest and then **used in place**. The
  manifest is only ever read from that one executable-relative path — never
  the current directory, never `PATH`, never an ancestor search — and any
  verification failure is a hard, named error with no fallback. Windows also
  resolves the LLVM code generator from this directory, because
  `libLLVM-22.dll` is simultaneously `ld.lld.exe`'s runtime dependency.
- **Optional embedded (dev/standalone) builds:** a build that sets
  `OSCAN_EMBED_ASSETS_DIR` (with `OSCAN_REQUIRE_EMBEDDED_ASSETS=1`) carries the
  same asset set *inside* the binary and extracts it on first use to a local
  cache (`%LOCALAPPDATA%\oscan\native-assets\` on Windows;
  `$XDG_CACHE_HOME/oscan/native-assets` or `$HOME/.cache/oscan/native-assets`
  on Linux). This mode exists for CI smoke jobs and standalone single-file
  builds; release binaries deliberately do not use it.
- **Plain `cargo build`:** neither mechanism is present, so final linking falls
  back to an external/bundled C-toolchain driver.

The pinned Windows release configuration gates `hello.osc` size in CI. The
current recursive matrix compiled all 37 examples with all three backends (111
executables):

| Backend | `hello.osc` | 37-example total |
|---|---:|---:|
| LLVM | 6,144 bytes | 814,080 bytes |
| Cranelift | 6,656 bytes | 863,232 bytes |
| C | 8,704 bytes | 875,520 bytes |

LLVM is 49,152 bytes (5.69%) smaller than Cranelift and 61,440 bytes
(7.02%) smaller than C in aggregate.
`scripts/compare-backend-size.ps1` enforces the focused `hello.osc` invariant;
`scripts/sample-backend-matrix.ps1` reports the complete matrix and totals.

The asset set (sidecar or embedded — it is the same set, staged by
`scripts/prepare-embed-assets.ps1|.sh`) differs by platform:
- **Windows:** 13 files (~85.4 MB) — `ld.lld.exe` plus 5 required runtime DLLs (`libLLVM-22.dll`, `libc++.dll`, `libwinpthread-1.dll`, `libunwind.dll`, `libffi-8.dll`), 6 MinGW import libraries, and compiler-builtins.
- **Linux:** 1 file (~2.78 MB) — a fully static `x86_64-linux-musl-ld` binary from the pinned musl-cross toolchain with zero shared-library dependencies.

This self-contained story is scoped narrowly — it does **not** extend to:

- **Hosted `--libc` mode** on any platform (still uses the external/bundled C-toolchain driver path).
- **Explicit `--extra-c <file>` sources** (compiling user-supplied C always goes through the external/bundled C-toolchain driver, even on Windows and Linux).
- **User `extern` signatures containing `str`**, when a generated C ABI shim is
  required. `OSCAN_NO_TOOLCHAIN=1` rejects that path rather than silently
  invoking a compiler.

**Linux AArch64/RISC-V64 object-backend support:** Cranelift can cross-link
both targets through target-matched linker/runtime sidecars. LLVM uses the same
link path when its packaged provider exposes the target: the Linux provider
includes AArch64 and RISC-V, while the Windows provider includes AArch64 but
not RISC-V. The C backend's existing ARM64/RISC-V paths remain separate.

Advanced overrides (rarely needed): `OSCAN_NATIVE_LINKER`/`OSCAN_NATIVE_LINKER_FLAVOR` select a different linker or force the legacy compiler-driver path (`OSCAN_NATIVE_LINKER_FLAVOR=mingw` for Windows, `=elf` for Linux); `OSCAN_NATIVE_ASSET_CACHE_DIR` relocates the extraction cache. See `oscan --help` and `docs/design/native-link-embedding.md` for details.

On Windows, native final linking and `--run` refuse to run from an elevated
Administrator process by default. Trusted CI/release jobs that build only
trusted inputs may pass `--allow-elevated-native-link`; this only bypasses the
elevated-process refusal and does not disable path validation, cache
verification, canonicalization, or native-link sandboxing.

**Supported targets:**

| Target | Backends | Tooling | Notes |
|--------|----------|---------|-------|
| x86_64 Linux | LLVM, Cranelift, C | packaged LLVM provider and embedded final linker | LLVM is the recommended package |
| x86_64 Windows | LLVM, Cranelift, C | packaged LLVM provider and embedded final linker | LLVM is the recommended package |
| ARM64 Linux | LLVM, Cranelift, C | target-capable provider, C cross-compiler, or sidecar linker | Object link/execution is QEMU-gated in CI |
| RISC-V 64 Linux | LLVM, Cranelift, C | Linux LLVM provider, C cross-compiler, or sidecar linker | Windows LLVM provider lacks RISC-V |
| WebAssembly | C backend (WASI) | `--target wasi` | Runs in wasmtime/wasmer |
| macOS | C backend (libc) | Apple Clang | LLVM/Cranelift object targets not yet available |

## Examples

You can write **CLI utilities** (text processing, file handling, sorting, grepping), **network programs** (HTTP clients, web servers, UDP tools), **graphics** (games, animations, data visualization), and **data tools** (CSV processing, log analysis, word frequency counters).

### Command-Line Utilities

- [hello.osc](examples/hello.osc) — Hello World
- [fibonacci.osc](examples/fibonacci.osc) — Recursive fibonacci
- [error_handling.osc](examples/error_handling.osc) — Result type and pattern matching
- [file_io.osc](examples/file_io.osc) — Reading and writing files
- [countlines.osc](examples/countlines.osc) — Count lines in files
- [upper.osc](examples/upper.osc) — Convert text to uppercase
- [wc.osc](examples/wc.osc) — Word count utility
- [grep.osc](examples/grep.osc) — Pattern matching in files
- [sort.osc](examples/sort.osc) — Sort lines from files
- [hexdump.osc](examples/hexdump.osc) — Hex dump utility
- [base64.osc](examples/base64.osc) — Base64 encode/decode
- [checksum.osc](examples/checksum.osc) — File checksums
- [env_info.osc](examples/env_info.osc) — System info (datetime, environment, glob matching)
- [file_checksum.osc](examples/file_checksum.osc) — SHA-256 file hasher
- [word_freq.osc](examples/word_freq.osc) — Word frequency counter (using hash maps)
- [string_interpolation.osc](examples/string_interpolation.osc) — String interpolation showcase

### Network Programs

- [http_client.osc](examples/http_client.osc) — HTTP GET client (TCP with hostname support)
- [web_server.osc](examples/web_server.osc) — TCP web server

### Graphics & Games

- [gfx_demo.osc](examples/gfx/gfx_demo.osc) — Shape and text rendering
- [bounce.osc](examples/gfx/bounce.osc) — Bouncing ball animation
- [starfield.osc](examples/gfx/starfield.osc) — 3D starfield effect
- [plasma.osc](examples/gfx/plasma.osc) — Sine wave plasma animation
- [life.osc](examples/gfx/life.osc) — Conway's Game of Life
- [spirograph.osc](examples/gfx/spirograph.osc) — Animated spirograph
- [ui_demo.osc](examples/gfx/ui_demo.osc) — UI widget library showcase

## Built-in Functions

<!-- BEGIN BUILTIN TABLE -->

**238 built-in functions** across 21 categories: I/O, String, Conversion, Character, Math, Bitwise, File I/O, Filesystem, Path, Socket, HashMap, Array, Date/Time, System, Environment, Terminal, Process, Graphics, TrueType, Image, TLS.

See the [full built-in function reference](docs/builtins.md) for signatures and descriptions.

<!-- END BUILTIN TABLE -->

## Learn More

- **[Introductory Tutorial](docs/tutorial.md)** — Build a useful command-line program step by step
- **[Language Guide](docs/guide.md)** — Concise walkthrough of syntax, types, and patterns
- **[Safety Guide](docs/safety.md)** — How Oscan prevents 11 of 11 major bug categories
- **[Language Specification](docs/spec/oscan-spec.md)** — Full formal semantics, grammar, and standard library reference
- **[Runtime Primitives](docs/spec/oscan-spec.md#appendix-a-available-runtime-primitives-future-builtins)** — Inventory of available freestanding OS primitives (Appendix A)
- **[LLVM Backend Design](docs/design/llvm-backend.md)** — Architecture, default policy, and tradeoffs

## Testing

```bash
cargo test                      # Rust unit tests
```

On Windows, you can also run the full validation suite:

```bash
.\test.ps1                      # full validation suite
.\test.ps1 -Backend llvm        # C-vs-LLVM differential and cross-target suite
.\test.ps1 -Backend cranelift   # C-vs-Cranelift regression suite
.\tests\run_tests.ps1 -Oscan .\target\debug\oscan.exe   # integration tests
```

`scripts/sample-backend-matrix.ps1` (PowerShell, Windows or Linux) is a local
cross-backend build check over the sample programs:

```bash
pwsh ./scripts/sample-backend-matrix.ps1
```

It recursively collects every `.osc` file under
`examples/` (override with `-SourceDirectory`), probes `llvm`, `cranelift`, and
`c`, and skips — with a printed reason — any backend that cannot produce a host
executable on the current machine. Each remaining backend gets its own
subdirectory under the output root (`tests\build\sample-backend-matrix` by
default, override with `-OutputDirectory`); the absolute output root is printed
first and wiped before the run. Nested and case-colliding sample names are
flattened to unique artifact names. It reports the sample/backend artifact
count, then finishes with a deterministic, sorted size table (bytes per sample
per backend), and exits non-zero if any available backend fails to compile a
sample or does not produce a non-empty host executable. Pass `-Oscan <path>` to pick a compiler; otherwise
`target/release` then `target/debug` are used.

The repository currently includes:
- **99 positive integration tests** — programs that compile and run
- **35 negative integration tests** — programs that must be rejected
- Rust unit/integration tests for the compiler, object linker, packaging, and CLI

Windows, Linux, macOS, ARM64 and RISC-V64 (QEMU), and WASI are tested in CI.
Windows and Linux x86-64 run the full C-vs-LLVM and C-vs-Cranelift differential
validation; ARM64/RISC-V64 gates link and QEMU-run both object backends,
including arithmetic/float ABI coverage for LLVM.

## Contributing

Oscan is a research project. The codebase is intentionally small and focused — contributions that align with the minimalist philosophy are welcome.

For architectural decisions and design rationale, see [.squad/decisions.md](.squad/decisions.md).

Release packaging (maintainer-only: musl mirroring, BearSSL builds, tagging) lives in [docs/releasing.md](docs/releasing.md).

## Project Structure

```
src/            Compiler (Rust): frontend, C/LLVM/Cranelift backends, linker
runtime/        C runtime: arena, standard library, OS primitives
tests/          Positive and negative integration tests
examples/       27 CLI/network programs
examples/gfx/   10 graphics & game demos
docs/           Language guide, full specification, built-in reference
deps/           laststanding (freestanding OS library)
```

## License

MIT
