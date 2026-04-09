# Oscan — Copilot Project Instructions

## What is Oscan?

Oscan is a statically-typed programming language that **transpiles to C**. It produces
standalone executables with no libc dependency — the runtime is a freestanding C
implementation embedded by the compiler at build time.

## Building the Compiler

The compiler is written in Rust.

```sh
cargo build --release      # builds target/release/oscan(.exe)
```

## Testing

```sh
cargo test --quiet         # Rust unit tests (parser, semantic, codegen)
.\test.ps1                 # full integration test suite (Windows)
```

## Compiling Oscan Programs

```sh
oscan hello.osc --run          # compile and run immediately
oscan hello.osc -o hello       # transpile to C then compile to executable
oscan hello.osc -o hello.c     # transpile to C only
```

## Repository Structure

| Directory | Contents |
|-----------|----------|
| `src/` | Rust compiler source (lexer, parser, semantic analysis, C codegen) |
| `runtime/` | Freestanding C runtime — embedded into every compiled program |
| `examples/` | Example `.osc` programs demonstrating language features |
| `docs/` | Language specification and user guide |
| `tests/` | Integration test `.osc` files and expected outputs |
| `scripts/` | Build, release, and code-generation scripts |
| `libs/` | Oscan library files included via `use` |

## Key Conventions

- **Rebuild after runtime changes**: the compiler embeds `runtime/*.c` and
  `runtime/*.h` files at compile time. If you modify the runtime, rebuild the
  compiler with `cargo build`.
- **Auto-generated content**: `scripts/gen-builtin-table.py` generates the
  builtin function table in `README.md`. `scripts/gen-copilot-instructions.py`
  generates the language reference in `.github/instructions/oscan.instructions.md`.
  Run them after changing builtins or examples.
- **Oscan language reference**: see `.github/instructions/oscan.instructions.md`
  for the full language reference that Copilot uses when editing `.osc` files.
