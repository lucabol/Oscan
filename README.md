# Oscan

[![CI](https://github.com/lucabol/Oscan/actions/workflows/ci.yml/badge.svg)](https://github.com/lucabol/Oscan/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/lucabol/Oscan?include_prereleases&sort=semver)](https://github.com/lucabol/Oscan/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20Linux%20%7C%20macOS-blue)

**A small, statically typed language for writing clear standalone programs.**

Oscan favors explicit, predictable code: types are written out, effects are
visible in function signatures, and errors are values rather than exceptions.
It is designed to be approachable for people and reliable for AI coding tools.

## Why Oscan?

- **Small and explicit:** no implicit type coercions, hidden exceptions, or
  surprise control flow.
- **Safe by design:** bounds checks, overflow checks, no null pointers, and no
  manual memory management. See the [safety guide](docs/safety.md).
- **Batteries included:** 328 built-in functions for strings, files, math,
  collections, networking, graphics, TLS, and more.
- **Standalone programs:** produce native executables, including freestanding
  programs that do not depend on libc on supported targets.
- **Clear effects:** `fn` declares pure functions and `fn!` declares functions
  that may perform I/O or other side effects.
- **Errors as values:** `Result<T, E>`, exhaustive `match`, and `try` make
  failures explicit.

## Install

Download the latest release from
[GitHub Releases](https://github.com/lucabol/Oscan/releases). Choose the
**LLVM slim package** for the smallest recommended install, or choose **full**
to use LLVM, Cranelift, and C from one compiler with `--backend`.

| Platform | Recommended download | Notes |
|---|---|---|
| Windows x86_64 | `oscan-vX.Y.Z-windows-x86_64-llvm.msi` or `oscan-vX.Y.Z-windows-x86_64-llvm.zip` | LLVM remains the transition default; `-full.zip` includes all backends |
| Linux x86_64 | `oscan-vX.Y.Z-linux-x86_64-llvm.tar.xz` | `-full.tar.xz` includes all backends; LLVM requires glibc 2.34+ |
| macOS x86_64 | `oscan-vX.Y.Z-macos-x86_64-c.tar.gz` | Requires Xcode Command Line Tools |

Windows and Linux provide `full`, `llvm`, `cranelift`, and `c` profiles. Full
contains one backend-neutral compiler; the others are smaller slim packages.
Each release
includes `SHA256SUMS`; keep the downloaded file's original name and verify it
before installing. The [installation guide](docs/guide.md#installation) has
step-by-step verification, upgrade, and uninstall instructions.

### Windows

Install the latest recommended package:

```powershell
iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 | iex
```

Install full, or select a slim package explicitly:

```powershell
iwr -useb https://raw.githubusercontent.com/lucabol/Oscan/master/scripts/install-latest.ps1 -OutFile install-latest.ps1
.\install-latest.ps1 -Profile full -SetDefault
.\install-latest.ps1 -Backend llvm
.\install-latest.ps1 -Backend cranelift
.\install-latest.ps1 -Backend c
```

Archive profiles coexist as `oscan-full`, `oscan-llvm`, `oscan-cranelift`, and
`oscan-c`. The first archive install safely creates `oscan`; later installs
preserve that selection unless `-SetDefault` is passed. Open a new terminal,
then check:

```powershell
oscan --version
```

### Linux

Download an archive and `SHA256SUMS`, verify it, extract it, and run the
included installer. This example installs full and selects it as the default:

```bash
tar xf oscan-vX.Y.Z-linux-x86_64-full.tar.xz
./oscan-vX.Y.Z-linux-x86_64-full/install.sh --set-default
oscan --version
oscan app.osc --backend cranelift
```

On Debian or Ubuntu, the LLVM and full packages also need:

```bash
sudo apt-get install libedit2 libffi8 libxml2 libz3-4 libzstd1 zlib1g
```

### macOS

Install Apple's command-line tools, then download and extract the macOS
package:

```bash
xcode-select --install
tar xf oscan-vX.Y.Z-macos-x86_64-c.tar.gz
```

Run the included `install.sh` to create `oscan-c` without disturbing another
profile, then run `oscan-c --version`. Pass `--set-default` only when the plain
`oscan` command should select this profile.

### Build from source

Building the compiler requires Rust:

```bash
git clone https://github.com/lucabol/Oscan.git
cd Oscan
cargo build --release
```

The binary is `target/release/oscan` (`oscan.exe` on Windows). Prebuilt releases
are easier for most users because they include the backend support files they
need.

## Your first program

Create `hello.osc`:

```rust
fn! main() {
    println("Hello, Oscan!");
}
```

Compile and run it:

```bash
oscan hello.osc --run
```

Or build an executable to run later:

```bash
oscan hello.osc
./hello
```

On Windows, run `.\hello.exe`.

## The language at a glance

This example shows explicit types, pure and impure functions, recursion,
ranges, and string interpolation:

```rust
fn fib(n: i32) -> i32 {
    if n <= 1 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn! main() {
    let name: str = "Oscan";

    for i in 0..10 {
        println("{name} fib({i}) = {fib(i)}");
    };
}
```

| Feature | Oscan syntax |
|---|---|
| Explicit types | `let count: i32 = 10;` |
| Mutable values | `let mut count: i32 = 10;` |
| Pure function | `fn add(a: i32, b: i32) -> i32 { a + b }` |
| Side-effecting function | `fn! save() { ... }` |
| Error propagation | `let value: i32 = try operation();` |
| Exhaustive matching | `match result { Result::Ok(v) => ..., Result::Err(e) => ... }` |
| Dynamic array | `let values: [i32] = [1, 2, 3];` |
| Import | `use "math.osc" as math;` |

Continue with the [introductory tutorial](docs/tutorial.md), which builds a
small `wc`-style command-line program step by step.

## Everyday commands

| Command | Result |
|---|---|
| `oscan app.osc --run` | Compile and run |
| `oscan app.osc` | Build a native executable |
| `oscan app.osc -o program` | Choose the executable name |
| `oscan app.osc -o program.c` | Emit C source |
| `oscan app.osc -o program.ll` | Emit LLVM IR |
| `oscan app.osc --backend llvm` | Select LLVM explicitly |
| `oscan app.osc --backend cranelift` | Select Cranelift explicitly |
| `oscan app.osc --opt-level speed` | Favor generated-code speed over the default size profile |
| `oscan app.osc --backend c` | Select the C backend explicitly |
| `oscan app.osc --debuginfo line-tables` | Keep Oscan source locations for debugging |
| `oscan --help` | Show every option |

Full contains every backend; slim packages contain one and identify the full or
matching slim package when another is requested. `--opt-level size|speed`
applies to LLVM and Cranelift generated code; the default is `size`.

Debug information is opt-in: `--debuginfo none` is the default, while
`--debuginfo line-tables` enables source breakpoints, stepping, stack
symbolization, and imported-file locations without changing the selected
optimization level. Oscan does not generate backend-independent local-variable
or type descriptions at this level, although a C toolchain may include
additional records. See
[compiler technical details](docs/technical-details.md#debug-information) for
backend and debugger-format notes.

## Examples

Run any example with `oscan <path> --run`.

- **Command-line tools:** [Hello World](examples/hello.osc),
  [word count](examples/wc.osc), [grep](examples/grep.osc),
  [sorting](examples/sort.osc), and
  [word frequencies](examples/word_freq.osc).
- **Files and data:** [file I/O](examples/file_io.osc),
  [Base64 encoding](examples/base64.osc), and
  [SHA-256 checksums](examples/file_checksum.osc).
- **Networking:** [HTTP client](examples/http_client.osc) and
  [web server](examples/web_server.osc).
- **Graphics and games:** [graphics demo](examples/gfx/gfx_demo.osc),
  [Conway's Game of Life](examples/gfx/life.osc), and
  [spirograph](examples/gfx/spirograph.osc).

Browse the full [examples directory](examples).

## Built-in functions

<!-- BEGIN BUILTIN TABLE -->

**328 built-in functions** across 21 categories: I/O, String, Conversion, Character, Math, Bitwise, File I/O, Filesystem, Path, Socket, HashMap, Array, Date/Time, System, Environment, Terminal, Process, Graphics, TrueType, Image, TLS.

See the [full built-in function reference](docs/builtins.md) for signatures and descriptions.

<!-- END BUILTIN TABLE -->

## Technical details

- **Backends:** LLVM is recommended, Cranelift is an independent native
  alternative, and C is the portability and source-emission backend.
- **Runtime:** programs can use Oscan's freestanding runtime or opt into hosted
  libc mode with `--libc`.
- **Memory:** arena allocation provides deterministic cleanup without manual
  `free` calls or a garbage collector.
- **Targets:** release and cross-compilation support varies by platform and
  backend.
- **Distribution:** full contains all backends; coexistence-safe slim profiles
  contain one backend and its required support files.

See [Compiler Technical Details](docs/technical-details.md) for backend
selection, package layouts, runtime and linker behavior, supported targets,
toolchain discovery, source builds, and validation.

## Documentation

- [Introductory tutorial](docs/tutorial.md)
- [Language guide](docs/guide.md)
- [Built-in function reference](docs/builtins.md)
- [Safety guide](docs/safety.md)
- [Language specification](docs/spec/oscan-spec.md)
- [Compiler technical details](docs/technical-details.md)

When using an AI coding agent, include
[the Oscan language reference](.github/instructions/oscan.instructions.md) in
its context. GitHub Copilot loads it automatically for `.osc` files.

## Contributing

Contributions that fit Oscan's small, explicit language design are welcome.
See the [test suite guide](docs/test_suite.md) and
[release guide](docs/releasing.md) for development workflows.

## License

MIT
