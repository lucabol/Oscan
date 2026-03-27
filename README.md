# Babel-C

**A minimalist, LLM-optimized programming language that transpiles to C99**

Babel-C is designed to be the ideal target language for Large Language Models generating systems code. It combines modern language features (algebraic data types, pattern matching, explicit error handling) with the simplicity and portability of C.

## Features

- **LLM-Friendly Syntax**: Clear, unambiguous grammar that's easy for AI to generate correctly
- **Modern Type System**: Sum types (enums), product types (structs), Result<T,E> for errors
- **Memory Safety Hints**: Explicit mutability (`let mut`), pure vs side-effecting functions (`fn` vs `fn!`)
- **Zero Dependencies**: Compiles to pure C99 with a minimal runtime library
- **Full Toolchain**: Lexer, parser, semantic analyzer, and C code generator

## Quick Start

### Prerequisites

- Rust toolchain (for building the compiler)
- GCC (for compiling the generated C code)
  - On Windows: Use WSL or install MinGW/TDM-GCC
  - On Linux/macOS: Usually pre-installed

### Building the Compiler

```bash
git clone <repository-url>
cd Squad
cargo build --release
```

The compiler binary will be at `target/release/babelc` (or `babelc.exe` on Windows).

### Your First Program

Create `hello.bc`:

```babel-c
fn! main() {
    println("Hello, Babel-C!");
}
```

Compile and run:

```bash
# Single-step compile and run
babelc hello.bc --run

# Or transpile to C and compile manually
babelc hello.bc -o hello.c
gcc hello.c runtime/bc_runtime.c -Iruntime -o hello
./hello
```

## Language Overview

### Functions

Babel-C distinguishes between pure and side-effecting functions:

```babel-c
// Pure function - no I/O, no mutation of globals
fn add(a: i32, b: i32) -> i32 {
    a + b
}

// Side-effecting function - can perform I/O
fn! greet(name: str) {
    print("Hello, ");
    println(name);
}
```

### Variables

```babel-c
let x: i32 = 42;           // Immutable binding
let mut count: i32 = 0;    // Mutable binding
count = count + 1;
```

### Types

**Primitive Types:**
- `i32`, `i64` - Signed integers
- `f64` - Double precision float
- `bool` - Boolean (`true` or `false`)
- `str` - String (immutable, heap-allocated)

**Compound Types:**
```babel-c
// Struct (product type)
struct Point {
    x: i32,
    y: i32
}

// Enum (sum type / tagged union)
enum Option {
    Some(i32),
    None
}

// Array
let numbers: [i32; 5] = [1, 2, 3, 4, 5];
```

### Error Handling

Babel-C uses `Result<T, E>` instead of exceptions:

```babel-c
fn divide(a: i32, b: i32) -> Result<i32, str> {
    if b == 0 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}

fn! main() {
    match divide(10, 2) {
        Result::Ok(val) => print_i32(val),
        Result::Err(msg) => println(msg)
    }
}
```

The `try` operator propagates errors automatically:

```babel-c
fn compute() -> Result<i32, str> {
    let x: i32 = try divide(10, 2);  // Auto-propagates errors
    Result::Ok(x * 2)
}
```

### Pattern Matching

```babel-c
match value {
    Result::Ok(x) => {
        print_i32(x);
    },
    Result::Err(msg) => {
        println(msg);
    }
}
```

### Control Flow

```babel-c
// If-else (expression form)
let max: i32 = if a > b { a } else { b };

// While loop
while count < 10 {
    count = count + 1;
}

// For-in loop (range iteration)
for i in 0..10 {
    print_i32(i);
}
```

### Built-in Functions

**I/O:**
- `print(s: str)`, `println(s: str)`
- `print_i32(n: i32)`, `print_i64(n: i64)`, `print_f64(n: f64)`, `print_bool(b: bool)`
- `read_line() -> Result<str, str>`

**String:**
- `str_len(s: str) -> i32`
- `str_eq(a: str, b: str) -> bool`
- `str_concat(a: str, b: str) -> str`

**Math:**
- `abs_i32(n: i32) -> i32`, `abs_f64(n: f64) -> f64`
- `mod_i32(a: i32, b: i32) -> i32`

**Conversion:**
- `i32_to_str(n: i32) -> str`, `i64_to_str(n: i64) -> str`, `f64_to_str(n: f64) -> str`
- `str_to_i32(s: str) -> Result<i32, str>`

## Examples

See the `examples/` directory for complete programs:

- **`hello.bc`** - Classic hello world
- **`fibonacci.bc`** - Recursive Fibonacci sequence
- **`error_handling.bc`** - Result types and error propagation

## Compiler CLI

```
Usage: babelc [OPTIONS] <file.bc>

Options:
  --run              Transpile, compile with gcc, and run immediately
  -o <output.c>      Transpile to C and write to file
  --dump-tokens      Print lexer tokens (debug)
  --dump-ast         Print AST (debug)
```

## Building from Source

### Development Build

```bash
cargo build
./target/debug/babelc examples/hello.bc --run
```

### Release Build (Optimized)

```bash
cargo build --release
./target/release/babelc examples/hello.bc --run
```

### Running Tests

The test suite includes comprehensive unit tests for all compiler phases and integration tests for end-to-end compilation:

```bash
# Run all tests (53 unit + 34 integration = 87 tests)
cargo test

# Run only unit tests
cargo test --lib

# Run only integration tests
cargo test --test '*'

# Run tests with output
cargo test -- --nocapture
```

#### Runtime Tests

The C runtime library has its own test suite:

```bash
cd runtime
make test

# Or compile and run manually
gcc test_runtime.c bc_runtime.c -Iruntime -o test_runtime
./test_runtime
```

## Project Structure

```
Squad/
├── src/
│   ├── main.rs          # CLI driver
│   ├── lexer.rs         # Tokenizer
│   ├── parser.rs        # Parser (tokens → AST)
│   ├── semantic.rs      # Type checker & semantic analysis
│   ├── codegen.rs       # Code generator (AST → C)
│   ├── ast.rs           # Abstract Syntax Tree definitions
│   ├── token.rs         # Token types and span info
│   ├── types.rs         # Type system definitions
│   └── error.rs         # Error types
├── runtime/
│   ├── bc_runtime.c     # Runtime library implementation
│   ├── bc_runtime.h     # Runtime library header
│   └── test_runtime.c   # Runtime tests
├── tests/
│   └── integration.rs   # End-to-end compiler tests
├── examples/
│   ├── hello.bc         # Hello world
│   ├── fibonacci.bc     # Recursive fibonacci
│   └── error_handling.bc # Result type demo
├── docs/
│   └── spec/
│       └── babel-c-spec.md  # Complete language specification
└── Cargo.toml           # Rust project metadata
```

## Language Specification

For the complete, definitive language specification including:
- Full grammar in EBNF
- Type system rules
- Memory model
- C-FFI interface
- All standard library functions

See: [`docs/spec/babel-c-spec.md`](docs/spec/babel-c-spec.md)

## Design Philosophy

Babel-C is built on three core principles:

1. **LLM-First Design**: Every syntax decision optimized for correctness when generated by AI
   - No ambiguous constructs
   - Explicit over implicit
   - Consistent patterns

2. **Minimalism**: Only essential features
   - 21 keywords total
   - No macros, no metaprogramming
   - Small, auditable codebase

3. **C Interop**: First-class C integration
   - Compiles to readable, portable C99
   - Direct C-FFI support
   - Can call any C library

## Why Babel-C?

**For LLM Code Generation:**
- Unambiguous syntax reduces hallucination
- Explicit error handling catches mistakes at compile time
- Type system prevents common errors

**For Systems Programming:**
- Compiles to fast, portable C code
- Zero runtime overhead
- Works anywhere C works

**For Learning:**
- Small, understandable compiler (< 5000 LOC)
- Clear separation of concerns
- Well-documented codebase

## Roadmap

Current status: **Phase 8 - Integration & Developer Experience** ✅

Completed phases:
- ✅ Phase 1: Core type system & IR
- ✅ Phase 2: Lexer & parser
- ✅ Phase 3: Semantic analyzer
- ✅ Phase 4: Code generator
- ✅ Phase 5: Runtime library (76 C tests)
- ✅ Phase 6: Standard library integration
- ✅ Phase 7: Testing & validation (87 tests)
- ✅ Phase 8: Integration & developer experience

Future possibilities:
- LSP server for editor support
- Debugger integration
- Optimization passes
- Additional backends (WASM, LLVM)

## Contributing

This is a research/educational project. The codebase is intentionally kept small and focused.

## License

[Specify your license here]

## Credits

**Lead Architect**: Neo  
**Design Philosophy**: LLM-optimized systems programming

---

**"What is real? How do you define 'real'? If you're talking about what you can compile, what you can execute, what you can run — then 'real' is simply C code that your processor can understand."** — Morpheus (probably)
