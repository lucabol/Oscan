# Oscan

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile to C99 and run anywhere. Oscan is designed so that LLMs *understand what they are writing* — a small, explicit grammar with readable C output you can inspect or embed directly.

## Language Highlights

- **Runs without a C library.** Compiles to freestanding C99 via direct syscalls — no libc, no linker surprises. (A `--libc` mode is available when you want it.)
- **Built-in graphics.** Canvas, drawing primitives, and input handling — write games and visualizations with zero external dependencies.
- **Socket networking.** TCP and UDP builtins with hostname resolution — build HTTP clients and web servers out of the box.
- **~150 standard functions.** String interpolation, hash maps, math, file I/O, SHA-256, sorting, graphics, networking, and more — batteries included.
- **Purity visible in signatures.** `fn` for pure functions, `fn!` for side effects — the type system tracks who can do I/O.
- **Errors as values.** `Result<T, E>` with `try` propagation. No exceptions, no hidden control flow.
- **Guarded C output.** Generated C systematically avoids undefined behavior with bounds checks and overflow guards.
- **One allocation model.** Arena-based memory — no manual alloc/free, no GC, deterministic cleanup.
- **Immutable by default.** `let` is immutable; `let mut` opts in to mutation. Anti-shadowing enforced.
- **25 reserved words.** Explicit types, no inference, no implicit coercions — minimal surface for LLMs to hallucinate on.
- **Order-independent definitions.** Use functions, types, and constants before they are declared.
- **162 tests, 25 examples.** Tested on Windows, Linux, macOS, and ARM64 via CI.

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

## Getting Started

**Requires:** Rust toolchain (to build the compiler) and a C compiler (GCC, Clang, or MSVC).

**Build the compiler:**

```bash
git clone <repository-url>
cd Oscan
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows). The compiler is self-contained — it embeds the runtime; you only need the binary and a C compiler to run Oscan programs.

**Your first program:**

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
  --dump-ast      Print AST (debug)
  --dump-tokens   Print tokens (debug)
```

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
