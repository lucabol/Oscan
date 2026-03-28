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
- **A C compiler** — GCC, Clang, or MSVC (the compiler auto-detects one)

### Build

```bash
git clone <repository-url>
cd Squad
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows).

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

**Compiler discovery order:** gcc → clang → cl.exe (PATH) → cl.exe (Visual Studio installation via vswhere).

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

## Building & Testing

```bash
cargo build                     # debug build
cargo build --release           # optimized build
cargo test                      # 53 unit tests + 38 integration tests
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
│   ├── positive/        # 22 programs that must compile & produce expected output
│   ├── negative/        # 16 programs that must be rejected by the compiler
│   └── integration.rs   # Test harness
├── examples/            # hello.osc, fibonacci.osc, error_handling.osc
├── docs/
│   ├── guide.md         # Concise language guide
│   └── spec/
│       └── Oscan-spec.md  # Full language specification
└── Cargo.toml
```

## Status

Oscan v0.1 is feature-complete for its initial scope: the full language compiles to C, all 91 tests pass across four platforms, and the CLI supports compile-to-exe, transpile-to-C, and run modes. The compiler is ~4,500 lines of Rust with zero dependencies.

## Contributing

This is a research project. The codebase is intentionally small and focused — contributions that align with the minimalist philosophy are welcome.

## License

[Specify your license here]
