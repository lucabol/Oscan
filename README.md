# Oscan

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile to C99 and run anywhere. Oscan is designed so that LLMs *understand what they are writing* — a small, explicit grammar with readable C output you can inspect or embed directly.

## Language Highlights

- **Runs without a C library.** Compiles to freestanding C99 via direct syscalls on x86_64, ARM64, and RISC-V. Also targets WebAssembly via WASI. (A `--libc` mode is available when you want it.)
- **[Safe by design.](docs/safety.md)** No buffer overflows, no use-after-free, no null pointers, no integer overflow UB — [10 of 12 major bug categories](docs/safety.md) eliminated.
- **Built-in graphics.** Canvas, drawing primitives, and input handling — write games and visualizations with zero external dependencies.
- **Socket networking.** TCP and UDP builtins with hostname resolution — build HTTP clients and web servers out of the box.
- **173 standard functions.** String interpolation, hash maps, math, file I/O, SHA-256, sorting, graphics, networking, and more — batteries included. See the [full table](#built-in-functions) below.
- **Purity visible in signatures.** `fn` for pure functions, `fn!` for side effects — the type system tracks who can do I/O.
- **Errors as values.** `Result<T, E>` with `try` propagation. No exceptions, no hidden control flow.
- **Guarded C output.** Generated C systematically avoids undefined behavior with bounds checks and overflow guards.
- **One allocation model.** Arena-based memory — no manual alloc/free, no GC, deterministic cleanup.
- **Immutable by default.** `let` is immutable; `let mut` opts in to mutation. Anti-shadowing enforced.
- **26 reserved words.** Explicit types, no inference, no implicit coercions — minimal surface for LLMs to hallucinate on.
- **Order-independent definitions.** Use functions, types, and constants before they are declared.
- **Namespaced imports.** `use "math.osc" as math` — access imported symbols via `math.add(...)` to avoid name collisions in larger programs.
- **162 tests, 25 examples.** Tested on Windows, Linux, macOS, and ARM64 via CI.

## For AI Coding Agents

Oscan is a new language — LLMs are not pre-trained on it. If you're using an AI coding agent (GitHub Copilot, Claude, Cursor, etc.), point it at the **language reference** before writing `.osc` code:

📄 [`.github/instructions/oscan.instructions.md`](.github/instructions/oscan.instructions.md) — critical syntax differences, common anti-patterns, annotated examples, and the full built-in function table.

GitHub Copilot picks this up automatically via `applyTo: "**/*.osc"`. For other tools, include it in your context or system prompt.

This file is **auto-generated** from the compiler source and example programs — run `python scripts/gen-copilot-instructions.py --inject` to update it, or let CI do it on push.

## A Quick Look

```oscan
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

1. Download `oscan-vX.Y.Z-linux-x86_64-full.tar.gz`
2. Extract: `tar xf oscan-*.tar.gz`
3. Run `install.sh` (or manually add the extracted directory to your PATH)
4. Verify: `oscan --help`

The Linux release includes a bundled C toolchain for the tested distribution(s).

**macOS:**

1. Download the macOS release archive
2. Extract: `tar xf oscan-*.tar.gz`
3. Copy `oscan` to `/usr/local/bin/` or another directory in your PATH
4. Verify: `oscan --help`

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

### Why Bundles Include `toolchain/` on Windows and Linux

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

---

## Getting Started

### Compile Your First Oscan Program

Create `hello.osc`:

```oscan
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

**214 built-in functions** across 20 categories.

### I/O (7 functions)

| Function | Description |
|----------|-------------|
| `fn! print(s: str)` | Print string to stdout |
| `fn! println(s: str)` | Print string with newline |
| `fn! print_i32(n: i32)` | Print i32 to stdout |
| `fn! print_i64(n: i64)` | Print i64 to stdout |
| `fn! print_f64(n: f64)` | Print f64 to stdout |
| `fn! print_bool(b: bool)` | Print bool to stdout |
| `fn! read_line() -> Result<str, str>` | Read a line from stdin |

### String (19 functions)

| Function | Description |
|----------|-------------|
| `fn str_len(s: str) -> i32` | Length of string in bytes |
| `fn str_eq(a: str, b: str) -> bool` | String equality check |
| `fn! str_concat(a: str, b: str) -> str` | Concatenate two strings |
| `fn! str_to_cstr(s: str) -> str` | Convert to null-terminated C string |
| `fn str_find(haystack: str, needle: str) -> i32` | Find substring index or -1 |
| `fn! str_from_i32(n: i32) -> str` | Convert i32 to string |
| `fn! str_slice(s: str, start: i32, end: i32) -> str` | Extract substring by index range |
| `fn str_contains(s: str, sub: str) -> bool` | Check if string contains substring |
| `fn str_starts_with(s: str, prefix: str) -> bool` | Check if string starts with prefix |
| `fn str_ends_with(s: str, suffix: str) -> bool` | Check if string ends with suffix |
| `fn! str_trim(s: str) -> str` | Remove leading and trailing whitespace |
| `fn! str_split(s: str, delim: str) -> [str]` | Split string by delimiter |
| `fn! str_to_upper(s: str) -> str` | Convert string to uppercase |
| `fn! str_to_lower(s: str) -> str` | Convert string to lowercase |
| `fn! str_replace(s: str, old: str, new_s: str) -> str` | Replace all occurrences of substring |
| `fn str_compare(a: str, b: str) -> i32` | Lexicographic comparison (-1, 0, 1) |
| `fn! str_from_chars(arr: [i32]) -> str` | Build string from char code array |
| `fn! str_to_chars(s: str) -> [i32]` | Convert string to char code array |
| `fn! str_join(arr: [str], sep: str) -> str` | Join string array with separator |

### Conversion (8 functions)

| Function | Description |
|----------|-------------|
| `fn! i32_to_str(n: i32) -> str` | Convert i32 to string |
| `fn parse_i32(s: str) -> Result<i32, str>` | Parse string to i32 |
| `fn parse_i64(s: str) -> Result<i64, str>` | Parse string to i64 |
| `fn! str_from_i64(n: i64) -> str` | Convert i64 to string |
| `fn! str_from_f64(n: f64) -> str` | Convert f64 to string |
| `fn str_from_bool(b: bool) -> str` | Convert bool to string |
| `fn! str_from_i32_hex(n: i32) -> str` | Convert i32 to hex string |
| `fn! str_from_i64_hex(n: i64) -> str` | Convert i64 to hex string |

### Character (10 functions)

| Function | Description |
|----------|-------------|
| `fn char_is_alpha(c: i32) -> bool` | Check if character is alphabetic |
| `fn char_is_digit(c: i32) -> bool` | Check if character is a digit |
| `fn char_is_alnum(c: i32) -> bool` | Check if character is alphanumeric |
| `fn char_is_space(c: i32) -> bool` | Check if character is whitespace |
| `fn char_is_upper(c: i32) -> bool` | Check if character is uppercase |
| `fn char_is_lower(c: i32) -> bool` | Check if character is lowercase |
| `fn char_is_print(c: i32) -> bool` | Check if character is printable |
| `fn char_is_xdigit(c: i32) -> bool` | Check if character is hex digit |
| `fn char_to_upper(c: i32) -> i32` | Convert character to uppercase |
| `fn char_to_lower(c: i32) -> i32` | Convert character to lowercase |

### Math (28 functions)

| Function | Description |
|----------|-------------|
| `fn abs_i32(n: i32) -> i32` | Absolute value of i32 |
| `fn abs_f64(n: f64) -> f64` | Absolute value of f64 |
| `fn mod_i32(a: i32, b: i32) -> i32` | Integer modulus |
| `fn math_sin(x: f64) -> f64` | Sine |
| `fn math_cos(x: f64) -> f64` | Cosine |
| `fn math_sqrt(x: f64) -> f64` | Square root |
| `fn math_pow(base: f64, exp: f64) -> f64` | Power |
| `fn math_exp(x: f64) -> f64` | Exponential (e^x) |
| `fn math_log(x: f64) -> f64` | Natural logarithm |
| `fn math_atan2(y: f64, x: f64) -> f64` | Two-argument arctangent |
| `fn math_floor(x: f64) -> f64` | Floor |
| `fn math_ceil(x: f64) -> f64` | Ceiling |
| `fn math_fmod(x: f64, y: f64) -> f64` | Floating-point modulus |
| `fn math_abs(x: f64) -> f64` | Absolute value of f64 |
| `fn math_pi() -> f64` | Constant pi |
| `fn math_e() -> f64` | Constant e |
| `fn math_ln2() -> f64` | Constant ln(2) |
| `fn math_sqrt2() -> f64` | Constant sqrt(2) |
| `fn abs_i64(n: i64) -> i64` | Absolute value of i64 |
| `fn min_i32(a: i32, b: i32) -> i32` | Minimum of two i32 |
| `fn max_i32(a: i32, b: i32) -> i32` | Maximum of two i32 |
| `fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32` | Clamp i32 to range |
| `fn min_i64(a: i64, b: i64) -> i64` | Minimum of two i64 |
| `fn max_i64(a: i64, b: i64) -> i64` | Maximum of two i64 |
| `fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64` | Clamp i64 to range |
| `fn min_f64(a: f64, b: f64) -> f64` | Minimum of two f64 |
| `fn max_f64(a: f64, b: f64) -> f64` | Maximum of two f64 |
| `fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64` | Clamp f64 to range |

### Bitwise (6 functions)

| Function | Description |
|----------|-------------|
| `fn band(a: i32, b: i32) -> i32` | Bitwise AND |
| `fn bor(a: i32, b: i32) -> i32` | Bitwise OR |
| `fn bxor(a: i32, b: i32) -> i32` | Bitwise XOR |
| `fn bshl(a: i32, n: i32) -> i32` | Bitwise shift left |
| `fn bshr(a: i32, n: i32) -> i32` | Bitwise shift right |
| `fn bnot(a: i32) -> i32` | Bitwise NOT |

### File I/O (13 functions)

| Function | Description |
|----------|-------------|
| `fn! file_open_read(path: str) -> Result<i32, str>` | Open file for reading, returns fd |
| `fn! file_open_write(path: str) -> Result<i32, str>` | Open file for writing, returns fd |
| `fn! read_byte(fd: i32) -> i32` | Read one byte from fd |
| `fn! write_byte(fd: i32, b: i32)` | Write one byte to fd |
| `fn! write_str(fd: i32, s: str)` | Write string to fd |
| `fn! file_close(fd: i32)` | Close file descriptor |
| `fn! file_delete(path: str) -> Result<str, str>` | Delete a file |
| `fn! file_rename(old: str, new_path: str) -> Result<str, str>` | Rename a file |
| `fn! file_exists(path: str) -> bool` | Check if file exists |
| `fn! file_open_append(path: str) -> Result<i32, str>` | Open file for appending, returns fd |
| `fn! file_size(path: str) -> i64` | Get file size in bytes |
| `fn! read_file(path: str) -> Result<str, str>` | Read entire file as string |
| `fn! write_file(path: str, data: str) -> Result<str, str>` | Write string to file |

### Filesystem (5 functions)

| Function | Description |
|----------|-------------|
| `fn! dir_create(path: str) -> Result<str, str>` | Create a directory |
| `fn! dir_remove(path: str) -> Result<str, str>` | Remove a directory |
| `fn! dir_current() -> str` | Get current working directory |
| `fn! dir_change(path: str) -> Result<str, str>` | Change working directory |
| `fn! dir_list(path: str) -> [str]` | List directory contents |

### Path (6 functions)

| Function | Description |
|----------|-------------|
| `fn! path_join(dir: str, file: str) -> str` | Join directory and filename |
| `fn path_ext(path: str) -> str` | Get file extension |
| `fn! path_exists(path: str) -> bool` | Check if path exists |
| `fn! path_is_dir(path: str) -> bool` | Check if path is a directory |
| `fn path_basename(path: str) -> str` | Get filename from path |
| `fn! path_dirname(path: str) -> str` | Get directory from path |

### Socket (12 functions)

| Function | Description |
|----------|-------------|
| `fn! socket_tcp() -> Result<i32, str>` | Create TCP socket |
| `fn! socket_connect(sock: i32, addr: str, port: i32) -> Result<str, str>` | Connect to address and port |
| `fn! socket_bind(sock: i32, port: i32) -> Result<str, str>` | Bind socket to port |
| `fn! socket_listen(sock: i32, backlog: i32) -> Result<str, str>` | Listen for connections |
| `fn! socket_accept(sock: i32) -> Result<i32, str>` | Accept incoming connection |
| `fn! socket_send(sock: i32, data: str) -> Result<i32, str>` | Send data on socket |
| `fn! socket_recv(sock: i32, max_len: i32) -> str` | Receive data from socket |
| `fn! socket_close(sock: i32)` | Close socket |
| `fn! socket_udp() -> Result<i32, str>` | Create UDP socket |
| `fn! socket_sendto(sock: i32, data: str, addr: str, port: i32) -> i32` | Send UDP data to address |
| `fn! socket_recvfrom(sock: i32, max_len: i32) -> str` | Receive UDP data |
| `fn! socket_unix_connect(path: str) -> Result<i32, str>` | Connect to Unix domain socket |

### HashMap (36 functions)

| Function | Description |
|----------|-------------|
| `fn! map_new() -> map` | Create empty hash map |
| `fn! map_set(m: map, key: str, value: str)` | Set key-value pair |
| `fn! map_get(m: map, key: str) -> str` | Get value by key |
| `fn map_has(m: map, key: str) -> bool` | Check if key exists |
| `fn! map_delete(m: map, key: str)` | Delete key from map |
| `fn map_len(m: map) -> i32` | Number of entries in map |
| `fn! map_str_i32_new() -> map_str_i32` | Create empty str→i32 map |
| `fn! map_str_i32_set(m: map_str_i32, key: str, value: i32)` | Set key-value pair |
| `fn! map_str_i32_get(m: map_str_i32, key: str) -> i32` | Get value by key (0 if missing) |
| `fn map_str_i32_has(m: map_str_i32, key: str) -> bool` | Check if key exists |
| `fn! map_str_i32_delete(m: map_str_i32, key: str)` | Delete key from map |
| `fn map_str_i32_len(m: map_str_i32) -> i32` | Number of entries |
| `fn! map_str_i64_new() -> map_str_i64` | Create empty str→i64 map |
| `fn! map_str_i64_set(m: map_str_i64, key: str, value: i64)` | Set key-value pair |
| `fn! map_str_i64_get(m: map_str_i64, key: str) -> i64` | Get value by key (0 if missing) |
| `fn map_str_i64_has(m: map_str_i64, key: str) -> bool` | Check if key exists |
| `fn! map_str_i64_delete(m: map_str_i64, key: str)` | Delete key from map |
| `fn map_str_i64_len(m: map_str_i64) -> i32` | Number of entries |
| `fn! map_str_f64_new() -> map_str_f64` | Create empty str→f64 map |
| `fn! map_str_f64_set(m: map_str_f64, key: str, value: f64)` | Set key-value pair |
| `fn! map_str_f64_get(m: map_str_f64, key: str) -> f64` | Get value by key (0.0 if missing) |
| `fn map_str_f64_has(m: map_str_f64, key: str) -> bool` | Check if key exists |
| `fn! map_str_f64_delete(m: map_str_f64, key: str)` | Delete key from map |
| `fn map_str_f64_len(m: map_str_f64) -> i32` | Number of entries |
| `fn! map_i32_str_new() -> map_i32_str` | Create empty i32→str map |
| `fn! map_i32_str_set(m: map_i32_str, key: i32, value: str)` | Set key-value pair |
| `fn! map_i32_str_get(m: map_i32_str, key: i32) -> str` | Get value by key (empty string if missing) |
| `fn! map_i32_str_has(m: map_i32_str, key: i32) -> bool` | Check if key exists |
| `fn! map_i32_str_delete(m: map_i32_str, key: i32)` | Delete key from map |
| `fn map_i32_str_len(m: map_i32_str) -> i32` | Number of entries |
| `fn! map_i32_i32_new() -> map_i32_i32` | Create empty i32→i32 map |
| `fn! map_i32_i32_set(m: map_i32_i32, key: i32, value: i32)` | Set key-value pair |
| `fn! map_i32_i32_get(m: map_i32_i32, key: i32) -> i32` | Get value by key (0 if missing) |
| `fn! map_i32_i32_has(m: map_i32_i32, key: i32) -> bool` | Check if key exists |
| `fn! map_i32_i32_delete(m: map_i32_i32, key: i32)` | Delete key from map |
| `fn map_i32_i32_len(m: map_i32_i32) -> i32` | Number of entries |

### Array (4 functions)

| Function | Description |
|----------|-------------|
| `fn! sort_i32(arr: [i32])` | Sort i32 array in place |
| `fn! sort_i64(arr: [i64])` | Sort i64 array in place |
| `fn! sort_str(arr: [str])` | Sort string array in place |
| `fn! sort_f64(arr: [f64])` | Sort f64 array in place |

### Date/Time (7 functions)

| Function | Description |
|----------|-------------|
| `fn! time_format(timestamp: i64, fmt: str) -> str` | Format timestamp as string |
| `fn! time_utc_year(timestamp: i64) -> i32` | Get UTC year from timestamp |
| `fn! time_utc_month(timestamp: i64) -> i32` | Get UTC month from timestamp |
| `fn! time_utc_day(timestamp: i64) -> i32` | Get UTC day from timestamp |
| `fn! time_utc_hour(timestamp: i64) -> i32` | Get UTC hour from timestamp |
| `fn! time_utc_min(timestamp: i64) -> i32` | Get UTC minute from timestamp |
| `fn! time_utc_sec(timestamp: i64) -> i32` | Get UTC second from timestamp |

### System (10 functions)

| Function | Description |
|----------|-------------|
| `fn! arg_count() -> i32` | Number of command-line arguments |
| `fn! arg_get(i: i32) -> str` | Get command-line argument by index |
| `fn! rand_seed(seed: i32)` | Seed the random number generator |
| `fn! rand_i32() -> i32` | Generate random i32 |
| `fn! time_now() -> i64` | Current time as Unix timestamp |
| `fn! sleep_ms(ms: i32)` | Sleep for milliseconds |
| `fn! exit(code: i32)` | Exit with status code |
| `fn glob_match(pattern: str, text: str) -> bool` | Match text against glob pattern |
| `fn! sha256(data: str) -> str` | Compute SHA-256 hash |
| `fn is_tty() -> bool` | Check if stdout is a terminal |

### Environment (8 functions)

| Function | Description |
|----------|-------------|
| `fn! env_get(name: str) -> Result<str, str>` | Get environment variable |
| `fn! errno_get() -> i32` | Get last error code |
| `fn! errno_str(code: i32) -> str` | Convert error code to message |
| `fn! env_count() -> i32` | Number of environment variables |
| `fn! env_key(i: i32) -> str` | Get env variable name by index |
| `fn! env_value(i: i32) -> str` | Get env variable value by index |
| `fn! env_set(name: str, value: str) -> Result<str, str>` | Set environment variable |
| `fn! env_delete(name: str) -> Result<str, str>` | Delete environment variable |

### Terminal (5 functions)

| Function | Description |
|----------|-------------|
| `fn! term_width() -> i32` | Get terminal width in columns |
| `fn! term_height() -> i32` | Get terminal height in rows |
| `fn! term_raw() -> Result<str, str>` | Enter raw terminal mode |
| `fn! term_restore() -> Result<str, str>` | Restore normal terminal mode |
| `fn! read_nonblock() -> i32` | Non-blocking read from stdin |

### Process (1 functions)

| Function | Description |
|----------|-------------|
| `fn! proc_run(cmd: str, args: [str]) -> i32` | Run external process |

### Graphics (22 functions)

| Function | Description |
|----------|-------------|
| `fn! canvas_open(width: i32, height: i32, title: str) -> Result<str, str>` | Open graphics canvas |
| `fn! canvas_close()` | Close graphics canvas |
| `fn! canvas_alive() -> bool` | Check if canvas is still open |
| `fn! canvas_flush()` | Flush canvas to screen |
| `fn! canvas_clear(color: i32)` | Clear canvas with color |
| `fn! gfx_pixel(x: i32, y: i32, color: i32)` | Draw a pixel |
| `fn! gfx_get_pixel(x: i32, y: i32) -> i32` | Get pixel color at position |
| `fn! gfx_line(x0: i32, y0: i32, x1: i32, y1: i32, color: i32)` | Draw a line |
| `fn! gfx_rect(x: i32, y: i32, w: i32, h: i32, color: i32)` | Draw rectangle outline |
| `fn! gfx_fill_rect(x: i32, y: i32, w: i32, h: i32, color: i32)` | Draw filled rectangle |
| `fn! gfx_circle(cx: i32, cy: i32, r: i32, color: i32)` | Draw circle outline |
| `fn! gfx_fill_circle(cx: i32, cy: i32, r: i32, color: i32)` | Draw filled circle |
| `fn! gfx_draw_text(x: i32, y: i32, text: str, color: i32)` | Draw text on canvas |
| `fn! gfx_draw_text_scaled(x: i32, y: i32, text: str, color: i32, sx: i32, sy: i32)` | Draw scaled text on canvas |
| `fn! gfx_blit(dx: i32, dy: i32, w: i32, h: i32, pixels: [i32])` | Blit pixel buffer to canvas |
| `fn! gfx_blit_alpha(dx: i32, dy: i32, w: i32, h: i32, pixels: [i32])` | Alpha-blended blit to canvas |
| `fn! canvas_key() -> i32` | Get last key press |
| `fn! canvas_mouse_x() -> i32` | Get mouse X position |
| `fn! canvas_mouse_y() -> i32` | Get mouse Y position |
| `fn! canvas_mouse_btn() -> i32` | Get mouse button state |
| `fn rgb(r: i32, g: i32, b: i32) -> i32` | Create RGB color value |
| `fn rgba(r: i32, g: i32, b: i32, a: i32) -> i32` | Create RGBA color value |

### Image (1 functions)

| Function | Description |
|----------|-------------|
| `fn! img_load(data: str) -> Result<[i32], str>` | Decode PNG/JPEG/BMP/GIF image from memory |

### TLS (6 functions)

| Function | Description |
|----------|-------------|
| `fn! tls_connect(host: str, port: i32) -> Result<i32, str>` | Connect to host over TLS |
| `fn! tls_send(handle: i32, data: str) -> Result<i32, str>` | Send data over TLS |
| `fn! tls_recv(handle: i32, max_len: i32) -> str` | Receive data over TLS |
| `fn! tls_recv_byte(handle: i32) -> i32` | Receive single byte over TLS (-1 on close) |
| `fn! tls_close(handle: i32)` | Close TLS connection |
| `fn! tls_cleanup()` | Clean up TLS subsystem |

<!-- END BUILTIN TABLE -->

## Learn More

- **[Language Guide](docs/guide.md)** — Concise walkthrough of syntax, types, and patterns
- **[Safety Guide](docs/safety.md)** — How Oscan prevents 10 of 12 major bug categories
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

## Release Packaging

Release builds are handled by GitHub Actions workflows. Two manual workflows must be run **once** (and again whenever their upstream dependencies change) before creating a release:

### Mirror musl toolchain (one-time setup)

The Linux release bundle ships a musl cross-compiler so users can compile freestanding programs without installing gcc. The toolchain comes from [musl.cc](https://musl.cc/) but that site blocks GitHub Actions, so we self-host it.

**Run once** (from Actions → "Mirror musl toolchain" → Run workflow), or manually:

```bash
curl -fSL -o x86_64-linux-musl-cross.tgz https://musl.cc/x86_64-linux-musl-cross.tgz
gh release create toolchains --title "Toolchains" --notes "Pre-downloaded musl cross-compilation toolchains" x86_64-linux-musl-cross.tgz
```

Re-run if the musl.cc toolchain is updated.

### Build BearSSL (when BearSSL submodule changes)

TLS support on Linux uses [BearSSL](https://www.bearssl.org/), compiled as a static library. Rather than rebuilding all 293 source files on every release, the library is pre-built and committed.

**Run** from Actions → "Build BearSSL" → Run workflow. This compiles BearSSL with system gcc (freestanding flags) and commits `packaging/prebuilt/linux-x86_64/libbearssl.a`.

Re-run whenever `deps/laststanding/bearssl/` is updated.

### Creating a release

After both prerequisites are in place, tag a version and push:

```bash
git tag v0.0.12
git push origin v0.0.12
```

The Release workflow automatically builds oscan for Windows and Linux, assembles bundles with the toolchain and libbearssl.a, runs smoke tests, and publishes to GitHub Releases.

## Project Structure

```
src/            Compiler (Rust): lexer, parser, typechecker, C codegen
runtime/        C runtime: arena, standard library, OS primitives
tests/          Positive and negative integration tests
examples/       18 non-graphics programs + 7 graphics programs
docs/           Language guide and full specification
deps/           laststanding (freestanding OS library)
```

## License

MIT
