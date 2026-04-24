# Oscan

[![CI](https://github.com/lucabol/Oscan/actions/workflows/ci.yml/badge.svg)](https://github.com/lucabol/Oscan/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lucabol/Oscan?include_prereleases&sort=semver)](https://github.com/lucabol/Oscan/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS%20%7C%20ARM64%20%7C%20RISC--V%20%7C%20WASI-blue)

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile to C99 and run anywhere. Oscan is designed so that LLMs *understand what they are writing* — a small, explicit grammar with readable C output you can inspect or embed directly.

## Contents

- [Language Highlights](#language-highlights)
- [For AI Coding Agents](#for-ai-coding-agents)
- [A Quick Look](#a-quick-look)
- [Installation](#installation)
- [Getting Started](#getting-started)
- [Examples](#examples)
- [Built-in Functions](#built-in-functions)
- [Learn More](#learn-more)
- [Testing](#testing)
- [Contributing](#contributing)
- [Project Structure](#project-structure)
- [License](#license)

## Language Highlights

- **Runs without a C library.** Compiles to freestanding C99 via direct syscalls on x86_64, ARM64, and RISC-V. Also targets WebAssembly (via WASI, which uses libc). (A `--libc` mode is available for hosted builds when you want it.)
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
- **162 tests, 37 examples.** Tested on Windows, Linux, macOS, and ARM64 via CI.

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
3. Run `./install.sh` (or manually add the extracted directory to your PATH)
4. Verify: `oscan --help`

The Linux release includes a bundled C toolchain for the tested distribution(s).

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
- C compiler (GCC, Clang, or MSVC) for compiling Oscan source code

**Build the compiler:**

```bash
git clone https://github.com/lucabol/Oscan.git
cd Oscan
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows). The compiler is self-contained — it embeds the runtime.

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
```

**CLI options:**
```
oscan [OPTIONS] <file.osc>
  -o <path>       Output path (exe by default; .c extension for C output)
  --run           Compile and execute immediately
  --emit-c        Emit generated C to stdout
  --libc          Use hosted libc mode instead of freestanding mode
  --target <arch> Cross-compile for target architecture (riscv64, wasi)
  --extra-c <file>  Extra C source file to compile and link (repeatable)
  --extra-cflags <flag>  Extra flag passed to the C compiler (repeatable)
  --dump-ast      Print AST (debug)
  --dump-tokens   Print tokens (debug)
```

**Windows/Linux toolchain lookup:**

For host-native builds on Windows and Linux, Oscan resolves the C compiler in this order:

1. `OSCAN_CC` — explicit compiler path/command override
2. `OSCAN_TOOLCHAIN_DIR` — bundled toolchain root override
3. sibling `toolchain/` directory next to the `oscan` binary
4. `toolchain/` directory in the current working directory
5. normal host compiler detection/fallback

When a bundled toolchain directory is used (`OSCAN_TOOLCHAIN_DIR`, sibling `toolchain/`, or `toolchain/` in the current working directory), Oscan checks platform-specific and generic `bin/` directories:

- Windows: `toolchain/windows/bin/`, then `toolchain/bin/`
- Linux: `toolchain/linux/bin/`, then `toolchain/bin/`

If your Windows/Linux Oscan distribution includes that bundled `toolchain/` directory, you do not always need to install a separate system compiler. If it does not, host compiler fallback still works as before. Cross-compilation targets such as `--target riscv64` and `--target wasi` still require their own target-specific toolchains.

**Supported targets:**

| Target | Mode | Compiler | Notes |
|--------|------|----------|-------|
| x86_64 Linux | Freestanding | bundled or host gcc / clang | Default on Linux |
| x86_64 Windows | Freestanding | bundled or host clang / MSVC | Default on Windows |
| ARM64 Linux | Freestanding | aarch64-linux-gnu-gcc | CI via QEMU |
| RISC-V 64 Linux | Freestanding | `--target riscv64` | CI via QEMU |
| WebAssembly | Libc (WASI) | `--target wasi` | Runs in wasmtime/wasmer |
| macOS | Libc | gcc / clang | No freestanding (Apple policy) |

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

- **[Language Guide](docs/guide.md)** — Concise walkthrough of syntax, types, and patterns
- **[Safety Guide](docs/safety.md)** — How Oscan prevents 11 of 11 major bug categories
- **[Language Specification](docs/spec/oscan-spec.md)** — Full formal semantics, grammar, and standard library reference
- **[Runtime Primitives](docs/spec/oscan-spec.md#appendix-a-available-runtime-primitives-future-builtins)** — Inventory of available freestanding OS primitives (Appendix A)

## Testing

```bash
cargo test                      # Rust unit tests
```

On Windows, you can also run the full validation suite:

```bash
.\test.ps1                      # full validation suite
.\tests\run_tests.ps1 -Oscan .\target\debug\oscan.exe   # integration tests
```

The repository currently includes **162 tests**:
- **62 unit tests** — lexer, parser, typechecker, codegen
- **74 positive integration tests** — programs that compile and run
- **26 negative integration tests** — programs that must be rejected by the compiler

Windows (MSVC), Linux (GCC), macOS (Clang), and ARM64 (QEMU) are tested in CI.

## Contributing

Oscan is a research project. The codebase is intentionally small and focused — contributions that align with the minimalist philosophy are welcome.

For architectural decisions and design rationale, see [.squad/decisions.md](.squad/decisions.md).

Release packaging (maintainer-only: musl mirroring, BearSSL builds, tagging) lives in [docs/releasing.md](docs/releasing.md).

## Project Structure

```
src/            Compiler (Rust): lexer, parser, typechecker, C codegen
runtime/        C runtime: arena, standard library, OS primitives
tests/          Positive and negative integration tests
examples/       27 CLI/network programs
examples/gfx/   10 graphics & game demos
docs/           Language guide, full specification, built-in reference
deps/           laststanding (freestanding OS library)
```

## License

MIT
