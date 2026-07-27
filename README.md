# Oscan

[![CI](https://github.com/lucabol/Oscan/actions/workflows/ci.yml/badge.svg)](https://github.com/lucabol/Oscan/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lucabol/Oscan?include_prereleases&sort=semver)](https://github.com/lucabol/Oscan/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20ARM64%20%7C%20RISC--V%20%7C%20WASI-blue)

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile through LLVM, portable C99, or direct Cranelift object code. Oscan is designed so that LLMs *understand what they are writing* — a small, explicit grammar with readable C and LLVM IR output you can inspect.

## Contents

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

- **Three deliberate backend roles.** LLVM is preferred when Oscan's packaged LLVM provider is available; Cranelift remains the direct object-code option, and C remains the portability/reference/source-emission path.
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

### Option 1: GitHub Releases (Recommended for most users)

Download the prebuilt binary for your platform from [GitHub Releases](https://github.com/lucabol/Oscan/releases).

**Windows x86_64:**

*Quick install (downloads and installs the latest release):*

```powershell
iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex
```

Pass `-Mode msi` to use the MSI installer instead of the zip bundle. The script verifies the asset's SHA-256 against the release's `SHA256SUMS` before installing.

*Option A — MSI installer (simplest):*

1. Download `oscan-vX.Y.Z-windows-x86_64.msi`
2. Double-click to install (or run `msiexec /i oscan-*.msi /quiet` for silent install)
3. Open a **new** terminal and verify: `oscan --help`

*Option B — Zip archive:*

1. Download `oscan-vX.Y.Z-windows-x86_64-full.zip`
2. Extract the archive
3. Run `install.ps1` (or manually add the extracted directory to your PATH)
4. Verify: `oscan --help`

Both options include a bundled C toolchain, so you do **not** need Visual Studio or MinGW installed.

**Linux x86_64:**

*Quick install (downloads and installs the latest release):*

```bash
set -eu
sudo apt-get install libedit2 libffi8 libxml2 libz3-4 libzstd1 zlib1g
ASSET=$(curl -fsSL https://api.github.com/repos/lucabol/Oscan/releases/latest \
  | grep -o '"browser_download_url": *"[^"]*linux-x86_64-full.tar.xz"' \
  | head -1 | cut -d'"' -f4)
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT
curl -fsSL -L "$ASSET" -o "$TMPDIR/oscan.tar.xz"
tar -xJf "$TMPDIR/oscan.tar.xz" -C "$TMPDIR"
"$TMPDIR"/oscan-*/install.sh
```

> **SHA-256 verification:** the one-liner above does not verify the asset
> checksum. For stricter installs, also download `SHA256SUMS` from the same
> release and run `sha256sum -c SHA256SUMS` inside `$TMPDIR` before invoking
> `install.sh`.

*Manual install:*

1. Download `oscan-vX.Y.Z-linux-x86_64-full.tar.xz`
2. Extract: `tar xf oscan-*.tar.xz`
3. On Debian/Ubuntu: `sudo apt-get install libedit2 libffi8 libxml2 libz3-4 libzstd1 zlib1g`
4. Run `./install.sh` (or manually add the extracted directory to your PATH)
5. Verify: `oscan --help`

The Linux release includes a bundled C toolchain and pinned `libLLVM`. The
provider requires glibc 2.34 or newer plus the listed host runtime libraries,
but no installed C/Clang/LLVM toolchain. The generated `README-install.txt`
records the same prerequisite.

**macOS (Intel x86_64 or Apple Silicon arm64):**

1. Download the macOS release archive that matches your CPU (`x86_64` for Intel, `arm64` for Apple Silicon)
2. Extract: `tar xf oscan-*.tar.gz`
3. Copy `oscan` to `/usr/local/bin/` or another directory in your PATH
4. Verify: `oscan --help`

For stricter installs, also download `SHA256SUMS` from the same release and verify with `shasum -a 256 -c SHA256SUMS` before copying the binary.

**macOS requires Xcode Command Line Tools** (or an equivalent C compiler). Install it with:

```bash
xcode-select --install
```

### Option 2: Build from Source

**Requirements:**
- Rust toolchain (for building the compiler)
- LLVM 22 shared library only when running the LLVM backend from a source build;
  release bundles package it, and no Clang executable or LLVM SDK is used
- C compiler (GCC, Clang, or MSVC) for the C backend, hosted object-backend
  builds, `--extra-c`, and local final links without packaged direct-link assets

**Build the compiler:**

```bash
git clone https://github.com/lucabol/Oscan.git
cd Oscan
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows). Building
Oscan itself needs only Rust: there is no LLVM Cargo/build dependency. To run
the LLVM backend from an ordinary source build, point `OSCAN_LLVM_LIB` at an
absolute LLVM 22 shared library path (or use `OSCAN_LLVM_DIR`). A plain local
build also omits packaged direct-link assets, so executable final linking may
use an external C-toolchain driver. Release builds package both the LLVM
provider and direct linker; their ordinary freestanding LLVM path generates no
C and invokes no C/Clang/LLVM tool executable.

<details>
<summary><strong>Why bundles include <code>toolchain/</code> on Windows and Linux</strong></summary>

On Windows and Linux, the bundled release archives include a `toolchain/` directory that sits alongside the `oscan` binary. This directory contains a pre-configured C compiler and related tools so you don't have to install a separate system toolchain.

**This is not in the Git repository** because:

- Toolchains are large binary artifacts (not source code)
- They are generated during release builds, not part of development
- Bundling them in Git would bloat the repository with platform-specific binaries

When you unpack the release, the directory layout looks like:

```text
oscan-vX.Y.Z-windows-x86_64-full/
  oscan.exe
  toolchain/
    bin/
      libLLVM-22.dll
      clang.exe
      ...
  install.ps1
  README-install.txt
```

The `oscan` compiler discovers this bundled `toolchain/` automatically, so your first Oscan programs will compile without any additional setup.

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
oscan hello.osc --backend llvm --run    # require LLVM; never falls back
oscan hello.osc --backend native --run  # force direct Cranelift code generation
oscan hello.osc --backend c --run       # force the portability/reference backend
```

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
  --backend <name>  llvm, c, or native (cranelift is an alias for native)
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

- **`llvm` (preferred default):** lowers typed Oscan IR through the same
  backend-neutral LIR used by Cranelift, emits LLVM IR directly, and asks the
  packaged LLVM 22 library in-process to parse, verify, optimize, and emit a
  relocatable object. No generated C or code-generation subprocess is involved.
- **`native` / `cranelift`:** direct Cranelift object-code backend for Windows
  x86-64 and Linux x86-64/AArch64/RISC-V64. It remains available explicitly
  and is the capability fallback when LLVM is unavailable.
- **`c`:** portability/reference backend. It is selected implicitly by
  `--emit-c`, `-o file.c`, `--target riscv64|wasi`, and unsupported native
  hosts such as macOS. It remains the differential correctness oracle.

An explicit `--backend llvm|native|c` always wins, and an explicit LLVM failure
never falls back. Without a selector, supported Windows/Linux hosts choose LLVM
when a compatible packaged provider is available, then Cranelift, then C. For compatibility,
an explicit `--native-target` without `--backend` selects Cranelift. C source
and `--target riscv64|wasi` requests select C; LLVM IR requests select LLVM.
`--libc`, `--extra-c`, and `--extra-cflags` do not force C.

See [the LLVM backend design](docs/design/llvm-backend.md) for the architecture,
no-toolchain boundary, targets, and tradeoffs.

**Windows/Linux provider and toolchain lookup:**

LLVM provider lookup order is:

1. `OSCAN_LLVM_LIB` — absolute shared-library override
2. `OSCAN_LLVM_DIR` — absolute provider-directory override
3. `OSCAN_TOOLCHAIN_DIR` — absolute packaged-toolchain root
4. executable-relative `toolchain/` and executable directory

LLVM is never loaded from the current directory, `PATH`, or the bare platform
loader search path. Windows and Linux full releases both package the required
LLVM 22 provider and therefore default to LLVM. The macOS target remains on C.

For builds that need a C compiler, Oscan resolves it in this order:

1. `OSCAN_CC` — explicit compiler path/command override
2. `OSCAN_TOOLCHAIN_DIR` — bundled toolchain root override
3. sibling `toolchain/` directory next to the `oscan` binary
4. `toolchain/` directory in the current working directory
5. normal host compiler detection/fallback

When a bundled toolchain directory is used (`OSCAN_TOOLCHAIN_DIR`, sibling `toolchain/`, or `toolchain/` in the current working directory), Oscan checks platform-specific and generic `bin/` directories:

- Windows: `toolchain/windows/bin/`, then `toolchain/bin/`
- Linux: `toolchain/linux/bin/`, then `toolchain/bin/`

If your Windows/Linux Oscan distribution includes that bundled `toolchain/` directory, you do not always need to install a separate system compiler. If it does not, host compiler fallback still works as before. Cross-compilation targets such as `--target riscv64` and `--target wasi` still require their own target-specific toolchains.

### Self-contained object-backend final links (Windows & Linux)

**On Windows x86-64 and Linux x86-64, freestanding LLVM and Cranelift objects
use the same self-contained final-link path.** LLVM IR and object generation
also run in-process through packaged `libLLVM`. `oscan` embeds a linker plus the
minimal support files it needs and extracts them on first use to a local cache
(`%LOCALAPPDATA%\oscan\native-assets\` on Windows;
`$XDG_CACHE_HOME/oscan/native-assets` or `$HOME/.cache/oscan/native-assets` on
Linux). No installed C compiler, Clang executable, LLVM SDK, `llc`, `opt`, or
`llvm-as` is involved.

The pinned Windows release configuration gates `hello.osc` size in CI. The
current recursive 37-example matrix totals 814,080 bytes for LLVM versus
875,520 bytes for C, so LLVM is 61,440 bytes (7.02%) smaller in aggregate.
`scripts/compare-backend-size.ps1` enforces the focused `hello.osc` invariant;
`scripts/sample-backend-matrix.ps1` reports the complete matrix and totals.

The embedded payload size differs by platform:
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
| x86_64 Linux | LLVM, Cranelift, C | packaged LLVM provider and embedded final linker | LLVM is the full-bundle default |
| x86_64 Windows | LLVM, Cranelift, C | packaged LLVM provider and embedded final linker | LLVM is the full-bundle default |
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
.\test.ps1 -Backend native      # C-vs-Cranelift regression suite
.\tests\run_tests.ps1 -Oscan .\target\debug\oscan.exe   # integration tests
```

`scripts/sample-backend-matrix.ps1` (PowerShell, Windows or Linux) is a local
cross-backend build check over the sample programs:

```bash
pwsh ./scripts/sample-backend-matrix.ps1
```

It recursively collects every `.osc` file under
`examples/` (override with `-SourceDirectory`), probes `llvm`, `native`, and
`c`, and skips — with a printed reason — any backend that cannot produce a host
executable on the current machine. Each remaining backend gets its own
subdirectory under the output root (`tests\build\sample-backend-matrix` by
default, override with `-OutputDirectory`); the absolute output root is printed
first and wiped before the run. Nested and case-colliding sample names are
flattened to unique artifact names. It finishes with a deterministic,
sorted size table (bytes per sample per backend), and exits non-zero if any
available backend fails to compile a sample or does not produce a non-empty
host executable. Pass `-Oscan <path>` to pick a compiler; otherwise
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
