# Oscan

A minimalist programming language designed for LLM code generation, transpiling to freestanding C.

## Why Oscan?

LLMs hallucinate less when the target language is small and unambiguous. Oscan gives them:

- **25 reserved words.** One way to do everything — no syntax to argue about.
- **Module system.** `use "file.osc"` imports declarations from other files; circular imports are silently skipped.
- **`fn` / `fn!` purity split.** Side effects are visible in the signature.
- **`Result<T, E>` errors as values.** No exceptions, no hidden control flow.
- **Immutable by default.** `let` is immutable; `let mut` opts in to mutation.
- **Anti-shadowing.** Re-declaring a name in a nested scope is a compile error.
- **Arena memory.** One allocation model, no manual alloc/free, zero UB.
- **Explicit everything.** No type inference, no implicit coercions, no operator overloading.
- **No C Standard Library dependency.** Compiles down to direct syscalls (but you can use trusty old stdlibc).
- **Built-in graphics.** Canvas, drawing primitives, and input — write graphical demos with zero external dependencies.
- **Socket networking.** TCP client/server builtins for network programming.
- **Hash maps.** Built-in string→string hash maps with `map_new`, `map_set`, `map_get`, `map_has`, `map_delete`, `map_len`.
- **Math library.** Trig, logarithms, powers, and constants — all built in.

The output is readable C99 that compiles on any platform with a C compiler.

## Quick Start

### Prerequisites

- **Rust** toolchain (to build the compiler)
- **A C compiler** — GCC, Clang, or MSVC (the compiler auto-detects one). This is the only external dependency needed when running Oscan-generated binaries.

### Build

```bash
git clone <repository-url>
cd Squad
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows).

**Self-contained compiler:** The Oscan compiler embeds its runtime files (`osc_runtime.h`, `osc_runtime.c`, `l_os.h`) directly in the binary using Rust's `include_str!()`. This means the compiled binary is fully self-contained — you only need the `oscan` binary and a C compiler on the target machine. No need to distribute the `runtime/` or `deps/` directories.

**Development mode:** If a `runtime/` directory exists next to the binary or in the current working directory during compilation, it takes precedence over the embedded files. This is useful for local development and testing of runtime changes.

### Hello World

Create `hello.osc`:

```
fn! main() {
    println("Hello, Oscan!");
}
```

```bash
oscan hello.osc --run        # compile and execute immediately
oscan hello.osc              # compile to hello.exe (Windows) / hello (Linux/macOS)
oscan hello.osc -o out       # compile to out.exe / out
oscan hello.osc -o out.c     # transpile to C only (no compilation)
oscan hello.osc --emit-c     # emit generated C to stdout
```

## CLI Reference

```
oscan [OPTIONS] <file.osc>

Options:
  -o <path>        Output path. If extension is .c, transpile only.
                   Otherwise compile to executable (adds .exe on Windows).
  --run            Compile and immediately execute.
  --emit-c         Emit generated C code to stdout.
  --dump-tokens    Print lexer tokens (debug).
  --dump-ast       Print AST (debug).
```

**Compiler discovery order:** clang → gcc → cl.exe (PATH) → cl.exe (Visual Studio installation via vswhere).

## Language at a Glance

```
// Pure function — no I/O, no side effects
fn fib(n: i32) -> i32 {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}

// Side-effecting function — can do I/O
fn! main() {
    for i in 0..10 {
        print_i32(fib(i));
        print(" ");
    };
    println("");
}
```

For a complete walkthrough, see the **[Language Guide](docs/guide.md)**.
For the full formal specification, see **[docs/spec/Oscan-spec.md](docs/spec/Oscan-spec.md)**.

## Built-in Functions

Oscan provides **~139 builtin functions** across these categories:

| Category | Functions | Count |
|----------|-----------|-------|
| **I/O** | `print`, `println`, `print_i32`, `print_i64`, `print_f64`, `print_bool`, `read_line` | 7 |
| **String (Core)** | `str_len`, `str_eq`, `str_concat`, `str_to_cstr`, `str_find`, `str_from_i32`, `str_slice`, `s[i]` indexing | 7 |
| **String (Extended)** | `str_contains`, `str_starts_with`, `str_ends_with`, `str_trim`, `str_split`, `str_to_upper`, `str_to_lower`, `str_replace`, `str_compare` | 9 |
| **String ↔ Chars** | `str_from_chars`, `str_to_chars` | 2 |
| **Math (Core)** | `abs_i32`, `abs_f64`, `abs_i64`, `mod_i32` | 4 |
| **Math Functions** | `math_sin`, `math_cos`, `math_sqrt`, `math_pow`, `math_exp`, `math_log`, `math_atan2`, `math_floor`, `math_ceil`, `math_fmod`, `math_abs` | 11 |
| **Math Constants** | `math_pi`, `math_e`, `math_ln2`, `math_sqrt2` | 4 |
| **Bitwise** | `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot` | 6 |
| **Character Ops** | `char_is_alpha`, `char_is_digit`, `char_is_alnum`, `char_is_space`, `char_is_upper`, `char_is_lower`, `char_is_print`, `char_is_xdigit`, `char_to_upper`, `char_to_lower` | 10 |
| **Parsing & Conversion** | `parse_i32`, `parse_i64`, `str_from_i64`, `str_from_f64`, `str_from_bool`, `i32_to_str`, `str_from_i32_hex`, `str_from_i64_hex` | 8 |
| **Array & Memory** | `len`, `push`, `pop`, `arena_reset` | 4 |
| **Sorting** | `sort_i32`, `sort_i64`, `sort_str`, `sort_f64` | 4 |
| **Hash Map** | `map_new`, `map_set`, `map_get`, `map_has`, `map_delete`, `map_len` | 6 |
| **Args** | `arg_count`, `arg_get` | 2 |
| **File I/O (Core)** | `file_open_read`, `file_open_write`, `file_open_append`, `read_byte`, `write_byte`, `write_str`, `file_close`, `file_delete` | 8 |
| **File I/O (Extended)** | `file_rename`, `file_exists`, `file_size`, `dir_create`, `dir_remove`, `dir_current`, `dir_change`, `dir_list` | 8 |
| **Path Utilities** | `path_join`, `path_ext`, `path_exists`, `path_is_dir` | 4 |
| **Socket Networking** | `socket_tcp`, `socket_connect`, `socket_bind`, `socket_listen`, `socket_accept`, `socket_send`, `socket_recv`, `socket_close`, `socket_udp`, `socket_sendto`, `socket_recvfrom` | 11 |
| **System** | `rand_seed`, `rand_i32`, `time_now`, `sleep_ms`, `exit`, `errno_get`, `errno_str`, `env_get` | 8 |
| **Environment** | `env_count`, `env_key`, `env_value` | 3 |
| **Terminal** | `term_width`, `term_height`, `term_raw`, `term_restore`, `read_nonblock` | 5 |
| **Process** | `proc_run` | 1 |
| **Graphics (Canvas)** | `canvas_open`, `canvas_close`, `canvas_alive`, `canvas_flush`, `canvas_clear` | 5 |
| **Graphics (Drawing)** | `gfx_pixel`, `gfx_get_pixel`, `gfx_line`, `gfx_rect`, `gfx_fill_rect`, `gfx_circle`, `gfx_fill_circle`, `gfx_draw_text` | 8 |
| **Graphics (Input)** | `canvas_key`, `canvas_mouse_x`, `canvas_mouse_y`, `canvas_mouse_btn` | 4 |
| **Graphics (Color)** | `rgb`, `rgba` | 2 |
| **Date/Time** | `time_format`, `time_utc_year`, `time_utc_month`, `time_utc_day`, `time_utc_hour`, `time_utc_min`, `time_utc_sec` | 7 |
| **Glob Matching** | `glob_match` | 1 |
| **SHA-256** | `sha256` | 1 |
| **Terminal Detection** | `is_tty` | 1 |
| **Environment Mod** | `env_set`, `env_delete` | 2 |

For a detailed reference with descriptions and type signatures, see **[§10 of the spec](docs/spec/oscan-spec.md#10-standard-library-micro-lib)** and **[§11 (Imports)](docs/spec/oscan-spec.md#11-imports)**.

## Graphics

Oscan supports creating graphical applications using a unified graphics API built on **laststanding**'s `l_gfx.h` library (GDI on Windows, framebuffer on Linux). Graphics are available in **freestanding mode only**.

### Key Features

- **Single global canvas** — no handle or resource management needed
- **Drawing primitives**: `gfx_pixel`, `gfx_get_pixel`, `gfx_line`, `gfx_rect`, `gfx_fill_rect`, `gfx_circle`, `gfx_fill_circle`, `gfx_draw_text`
- **Input handling**: `canvas_key`, `canvas_mouse_x`, `canvas_mouse_y`, `canvas_mouse_btn`
- **Color helpers**: `rgb(r, g, b)`, `rgba(r, g, b, a)` for color construction
- **Frame control**: `canvas_open`, `canvas_close`, `canvas_alive`, `canvas_clear`, `canvas_flush`
- **Pure-Oscan UI library:** `libs/ui.osc` provides reusable widgets (button, checkbox, slider, panel, label, separator) built entirely in Oscan using graphics builtins. Import with `use "libs/ui.osc"`.

### Examples

Seven example programs in `examples/gfx/` demonstrate graphics capabilities:

- **`bounce.osc`** — Bouncing ball animation with collision detection
- **`gfx_demo.osc`** — Shape and text rendering showcase
- **`starfield.osc`** — 3D perspective starfield scrolling effect
- **`plasma.osc`** — Sine wave plasma effect using procedural color blending
- **`life.osc`** — Conway's Game of Life cellular automaton
- **`ui_demo.osc`** — UI widget showcase using `libs/ui.osc`
- **`spirograph.osc`** — Animated spirograph using trigonometric parametric curves

## CLI Examples

Beyond the graphics examples, Oscan includes ~21 CLI utility programs in `examples/` demonstrating language features:

- **`hello.osc`** — Hello World
- **`fibonacci.osc`** — Recursive fibonacci
- **`error_handling.osc`** — Result type and pattern matching
- **`countlines.osc`** — Count lines in files
- **`upper.osc`** — Convert text to uppercase
- **`wc.osc`** — Word count (like Unix wc)
- **`grep.osc`** — Pattern matching in files
- **`checksum.osc`** — MD5 checksums (legacy)
- **`hexdump.osc`** — Hex dump utility
- **`base64.osc`** — Base64 encode/decode
- **`sort.osc`** — Sort lines from files
- **`file_io.osc`** — Basic file I/O operations
- **`word_freq.osc`** — Word frequency counter (map, str_split, for-in)
- **`http_client.osc`** — Simple HTTP GET client (TCP sockets)
- **`file_checksum.osc`** — SHA-256 file hasher (sha256, path_ext, file_size)
- **`env_info.osc`** — System info tool (datetime, is_tty, glob_match, env_get)
- **`web_server.osc`** — TCP socket web server (socket_bind, socket_listen, socket_accept)
- **`web_client.osc`** — TCP socket web client (socket_tcp, socket_connect)
- Plus 3 more utility programs showcasing additional features

## Building & Testing

```bash
cargo build                     # debug build
cargo build --release           # optimized build
cargo test                      # unit tests + 85 integration tests (65 positive + 20 negative)
cargo test --lib                # unit tests only
cargo test --test '*'           # integration tests only
```

Tests run on Windows (MSVC), Linux (GCC), macOS (Clang), and ARM64 (QEMU) via CI.

The C runtime has its own test suite:

```bash
cd runtime && make test
```

## Project Structure

```
├── src/
│   ├── main.rs          # CLI entry point & compiler discovery
│   ├── lexer.rs         # Tokenizer
│   ├── parser.rs        # Recursive-descent parser → AST
│   ├── semantic.rs      # Type checker & semantic analysis
│   ├── codegen.rs       # AST → C code generator
│   ├── ast.rs           # AST node definitions
│   ├── token.rs         # Token types
│   ├── types.rs         # Type system definitions
│   └── error.rs         # Compiler error types
├── runtime/
│   ├── osc_runtime.c     # Arena allocator + standard library (C)
│   ├── osc_runtime.h     # Runtime header
│   └── test_runtime.c   # Runtime unit tests
├── tests/
│   ├── positive/        # 61 programs that must compile & produce expected output
│   ├── negative/        # 20 programs that must be rejected by the compiler
│   └── integration.rs   # Test harness
├── examples/            # ~24 programs: hello, fibonacci, error_handling, countlines, upper, wc, grep, checksum, hexdump, base64, sort, file_io, word_freq, http_client, file_checksum, env_info, + 7 graphics demos
├── docs/
│   ├── guide.md         # Concise language guide
│   └── spec/
│       └── Oscan-spec.md  # Full language specification
└── Cargo.toml
```

## Status

Oscan v0.1+ now includes graphics capabilities alongside the original feature-complete implementation. The compiler supports the full language plus graphics, socket networking, math functions, and path utilities. All tests pass across four platforms, and the CLI supports compile-to-exe, transpile-to-C, and run modes. Recent additions include socket networking, math builtins, sorting, path utilities, and a pure-Oscan UI library.

## Freestanding Runtime (`deps/laststanding`)

Oscan's generated C code uses **laststanding** (`deps/laststanding/l_os.h`), a freestanding C library that provides OS primitives via direct syscalls — no libc required. This enables Oscan binaries to be truly self-contained.

The laststanding library has been updated with many new primitives beyond what Oscan v0.1 currently exposes, including: sorting/searching (`l_qsort`, `l_bsearch`), random number generation (`l_rand`, `l_srand`), time (`l_time`), error reporting (`l_errno`, `l_strerror`), POSIX option parsing (`l_getopt`), symlink operations, formatted fd output (`l_dprintf`), process spawning, terminal control, and more. See **[Appendix A of the spec](docs/spec/oscan-spec.md#appendix-a-available-runtime-primitives-future-builtins)** for the full inventory of available primitives and their potential Oscan builtin mappings.

## Contributing

This is a research project. The codebase is intentionally small and focused — contributions that align with the minimalist philosophy are welcome.

## License

[Specify your license here]
