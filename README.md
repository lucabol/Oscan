# Oscan

A minimalist programming language designed for LLM code generation, transpiling to C.

## Why Oscan?

LLMs hallucinate less when the target language is small and unambiguous. Oscan gives them:

- **21 reserved words.** One way to do everything — no syntax to argue about.
- **`fn` / `fn!` purity split.** Side effects are visible in the signature.
- **`Result<T, E>` errors as values.** No exceptions, no hidden control flow.
- **Immutable by default.** `let` is immutable; `let mut` opts in to mutation.
- **Anti-shadowing.** Re-declaring a name in a nested scope is a compile error.
- **Arena memory.** One allocation model, no manual alloc/free, zero UB.
- **Explicit everything.** No type inference, no implicit coercions, no operator overloading.

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

Oscan provides **81 builtin functions** across 8 categories (36 core + 45 new OS-level primitives):

| Category | Functions | Count |
|----------|-----------|-------|
| **I/O** | `print_i32`, `print_str`, `println`, `read_line` | 7 |
| **String (Core)** | `str_len`, `str_eq`, `str_concat`, `str_find`, `str_slice`, `s[i]` indexing | 7 |
| **String (New)** | `str_contains`, `str_starts_with`, `str_ends_with`, `str_trim`, `str_split`, `str_to_upper`, `str_to_lower`, `str_replace`, `str_compare` | 9 |
| **Math & Bitwise** | `abs_i32`, `abs_f64`, `abs_i64`, `min`, `max`, `band`, `bor`, `bxor`, `bshl`, `bshr`, `bnot` | 11 |
| **Character Ops** | `char_is_alpha`, `char_is_digit`, `char_is_alnum`, `char_is_space`, `char_is_upper`, `char_is_lower`, `char_is_print`, `char_is_xdigit`, `char_to_upper`, `char_to_lower` | 10 |
| **Parsing & Conversion** | `parse_i32`, `parse_i64`, `str_from_i64`, `str_from_f64`, `str_from_bool`, `i32_to_str` | 6 |
| **Array & Memory** | `len`, `push`, `arena_reset` | 3 |
| **Args** | `arg_count`, `arg_get` | 2 |
| **File I/O (Core)** | `file_open_read`, `file_open_write`, `read_byte`, `write_byte`, `write_str`, `file_close`, `file_delete` | 7 |
| **File I/O (New)** | `file_rename`, `file_exists`, `file_size`, `file_open_append`, `dir_create`, `dir_remove`, `dir_current`, `dir_change`, `dir_list` | 8 |
| **System** | `rand_seed`, `rand_i32`, `time_now`, `sleep_ms`, `exit`, `errno_get`, `errno_str`, `env_get` | 8 |
| **Terminal** | `term_width`, `term_height` | 2 |
| **Process** | `proc_run` | 1 |

For a detailed reference with descriptions and type signatures, see **[§10 of the spec](docs/spec/oscan-spec.md#10-standard-library-micro-lib)**.

## Building & Testing

```bash
cargo build                     # debug build
cargo build --release           # optimized build
cargo test                      # 53 unit tests + 63 integration tests
cargo test --lib                # unit tests only
cargo test --test '*'           # integration tests only
```

Tests run on Windows (Clang), Linux (GCC), macOS (Clang), and ARM64 (QEMU) via CI.

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
│   ├── positive/        # 42 programs that must compile & produce expected output
│   ├── negative/        # 21 programs that must be rejected by the compiler
│   └── integration.rs   # Test harness
├── examples/            # 12 programs: hello, fibonacci, error_handling, countlines, upper, wc, grep, checksum, hexdump, base64, sort, file_io
├── docs/
│   ├── guide.md         # Concise language guide
│   └── spec/
│       └── Oscan-spec.md  # Full language specification
└── Cargo.toml
```

## Status

Oscan v0.1 is feature-complete for its initial scope: the full language compiles to C, all 116 tests pass across four platforms, and the CLI supports compile-to-exe, transpile-to-C, and run modes. The compiler is ~4,500 lines of Rust with zero dependencies. Recent additions include file I/O, string operations, bitwise operators, and command-line argument access.

## Freestanding Runtime (`deps/laststanding`)

Oscan's generated C code uses **laststanding** (`deps/laststanding/l_os.h`), a freestanding C library that provides OS primitives via direct syscalls — no libc required. This enables Oscan binaries to be truly self-contained.

The laststanding library has been updated with many new primitives beyond what Oscan v0.1 currently exposes, including: sorting/searching (`l_qsort`, `l_bsearch`), random number generation (`l_rand`, `l_srand`), time (`l_time`), error reporting (`l_errno`, `l_strerror`), POSIX option parsing (`l_getopt`), symlink operations, formatted fd output (`l_dprintf`), process spawning, terminal control, and more. See **[Appendix A of the spec](docs/spec/oscan-spec.md#appendix-a-available-runtime-primitives-future-builtins)** for the full inventory of available primitives and their potential Oscan builtin mappings.

## Contributing

This is a research project. The codebase is intentionally small and focused — contributions that align with the minimalist philosophy are welcome.

## License

[Specify your license here]
