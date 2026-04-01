# Oscan

**A minimalist language for LLM code generation.** Write clear, unambiguous programs that compile to C99 and run anywhere.

Oscan is designed so that LLMs *understand what they are writing*. A small, explicit grammar. 24 reserved words. One allocation model. Clear error handling. No hidden control flow. The compiler outputs readable C that you can inspect or embed directly.

## Language Highlights

**Minimal and unambiguous:** 24 reserved words, explicit types, no type inference or implicit coercions.  
**Purity visible in signatures:** `fn` for pure functions, `fn!` for side effects (I/O, mutation, etc.).  
**Error handling as values:** `Result<T, E>` type; `try` for propagation. No exceptions.  
**Order-independent definitions:** Functions, types, and constants can be used before they are declared.  
**Guarded C output:** Generated C systematically avoids undefined behavior with bounds checks and overflow guards.  
**One allocation model:** Arena-based memory that deallocates on program exit.  
**Immutable by default:** `let x = ...` is immutable; `let mut x = ...` opts in to mutation.  
**Built-in batteries:** String interpolation, socket networking, graphics (freestanding), hash maps, math, file I/O, and ~130 other standard functions.

## A Quick Look

```
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

**Key patterns:**
- `fn` = pure function; `fn!` = can do I/O and side effects
- `for i in 0..10` loops over ranges
- `let name: str = ...` declares immutable bindings; `let mut x = ...` for mutation
- String interpolation: `"{expr}"` embeds values; escape literal braces as `{{` and `}}`
- Semicolons terminate statements; the last expression in a block is its return value

## Install & Build

**Requires:** Rust toolchain (to build the compiler) and a C compiler (GCC, Clang, or MSVC).

```bash
git clone <repository-url>
cd Oscan
cargo build --release
```

The binary is `target/release/oscan` (or `oscan.exe` on Windows). The compiler is self-contained — it embeds the runtime; you only need the binary and a C compiler to run Oscan programs.

## Getting Started

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

## What You Can Build

**CLI utilities:** Text processing, file handling, sorting, grepping, checksums.

**Network programs:** HTTP clients, web servers, UDP tools.

**Graphics:** Games, animations, data visualization (using built-in canvas + drawing).

**Data tools:** CSV processing, log analysis, word frequency counters.

See **[Examples](#examples)** below for concrete programs.

## Examples

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
- [word_freq.osc](examples/word_freq.osc) — Word frequency counter (using hash maps)
- [string_interpolation.osc](examples/string_interpolation.osc) — String interpolation showcase

### Network Programs

- [http_client.osc](examples/http_client.osc) — HTTP GET client (TCP with hostname support)
- [web_server.osc](examples/web_server.osc) — TCP web server
- [env_info.osc](examples/env_info.osc) — System info (datetime, environment, glob matching)
- [file_checksum.osc](examples/file_checksum.osc) — SHA-256 file hasher

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
- **[Language Specification](docs/spec/oscan-spec.md)** — Full formal semantics, grammar, and standard library reference
- **[Runtime Primitives](docs/spec/oscan-spec.md#appendix-a-available-runtime-primitives-future-builtins)** — Inventory of available freestanding OS primitives (Appendix A)

## Testing

```bash
cargo test                      # Rust unit tests
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

[Specify your license here]
