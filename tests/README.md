# Babel-C Test Suite

## Overview

This directory contains the conformance test suite for the Babel-C compiler. Tests verify that the compiler correctly accepts valid programs, rejects invalid programs, and produces correct output.

## Directory Structure

```
tests/
├── README.md               # This file
├── run_tests.sh            # Test runner (bash/Linux/macOS)
├── run_tests.ps1           # Test runner (PowerShell/Windows)
├── positive/               # Valid programs that must compile and run correctly
│   ├── hello_world.bc      # Basic hello world
│   ├── fibonacci.bc        # Recursion, if/else
│   ├── structs_enums.bc    # Struct/enum declarations and usage
│   ├── match_exhaustive.bc # Match expressions, exhaustive patterns
│   ├── error_handling.bc   # Result type, try, match on Result
│   ├── mutability.bc       # let vs let mut, assignment
│   ├── control_flow.bc     # if/else, while, for loops
│   ├── arithmetic.bc       # All arithmetic ops, precedence
│   ├── logical.bc          # and, or, not operators
│   ├── comparison.bc       # All comparison operators
│   ├── type_casts.bc       # Explicit casts (as keyword)
│   ├── arrays.bc           # Fixed and dynamic arrays, indexing, push, len
│   ├── strings.bc          # String literals, string functions
│   ├── block_expr.bc       # Block expressions returning values
│   ├── scope.bc            # Lexical scoping, variable lifetime
│   ├── purity.bc           # fn vs fn! purity enforcement
│   ├── top_level_const.bc  # Top-level let bindings
│   ├── ffi.bc              # Extern declarations
│   ├── order_independence.bc # Functions/types used before declaration
│   └── nested_control.bc   # Nested if/while/for/match
├── negative/               # Invalid programs that must be REJECTED
│   ├── shadowing.bc        # Variable shadowing → compile error
│   ├── non_exhaustive_match.bc # Missing enum variant → error
│   ├── unhandled_result.bc # Using Result without match/try → error
│   ├── implicit_coercion.bc # Type mismatch without cast → error
│   ├── immutable_assign.bc # Assigning to immutable binding → error
│   ├── undeclared_var.bc   # Using undeclared variable → error
│   ├── type_mismatch.bc    # Wrong types in expressions → error
│   ├── purity_violation.bc # Pure fn calling fn! → error
│   ├── missing_type_annotation.bc # Binding without type → error
│   ├── mixed_arithmetic.bc # i32 + f64 without cast → error
│   ├── non_bool_condition.bc # if/while with non-bool → error
│   ├── global_mut.bc       # let mut at top level → error
│   ├── comparison_chain.bc # a < b < c → error
│   └── compound_assign.bc  # += operator → error
├── expected/               # Expected stdout for positive tests
│   ├── hello_world.expected
│   ├── fibonacci.expected
│   └── ... (one per positive test)
└── build/                  # Build artifacts (gitignored except .gitkeep)
    └── .gitkeep
```

## Running Tests

### Prerequisites
- The `babelc` compiler binary
- A C compiler (gcc or clang)
- The Babel-C runtime library (`../runtime/bc_runtime.c` and `../runtime/bc_runtime.h`)

### Linux / macOS (bash)
```bash
chmod +x run_tests.sh
./run_tests.sh ../target/release/babelc gcc
# or with clang:
./run_tests.sh ../target/release/babelc clang
```

### Windows (PowerShell)
```powershell
.\run_tests.ps1 -BabelC ..\target\release\babelc.exe -CC gcc
```

## How It Works

### Positive Tests
1. **Transpile:** `babelc input.bc -o build/input.c`
2. **Compile C:** `gcc build/input.c ../runtime/bc_runtime.c -I../runtime -o build/input -std=c99 -lm`
3. **Run & compare:** Execute `build/input`, capture stdout, compare against `expected/input.expected`

### Negative Tests
1. **Transpile:** `babelc input.bc -o build/input.c`
2. **Expect failure:** The compiler must return a non-zero exit code

## Writing New Tests

### Positive Test
1. Create `positive/my_test.bc` with valid Babel-C code
2. Create `expected/my_test.expected` with exact expected stdout
3. Include comments explaining what language features are being tested

### Negative Test
1. Create `negative/my_test.bc` with intentionally invalid code
2. Include `// EXPECT ERROR: <description>` as the first comment
3. Each test should trigger exactly ONE specific error

## Test Coverage Matrix

| Spec Section | Feature                | Positive Test(s)          | Negative Test(s)           |
|-------------|------------------------|---------------------------|----------------------------|
| §1          | Keywords, operators     | arithmetic, logical       | compound_assign            |
| §3          | Type system            | type_casts, structs_enums | implicit_coercion, mixed_arithmetic, type_mismatch |
| §4.1        | fn / fn!               | purity                    | purity_violation           |
| §4.2        | let / let mut          | mutability                | immutable_assign, missing_type_annotation |
| §4.3-4.4    | Structs / Enums        | structs_enums             | —                          |
| §4.5        | Extern (FFI)           | ffi                       | —                          |
| §5.1        | Arithmetic             | arithmetic                | mixed_arithmetic           |
| §5.2        | Comparisons            | comparison                | comparison_chain           |
| §5.3        | Logical ops            | logical                   | non_bool_condition         |
| §5.4        | Assignment             | mutability                | immutable_assign, compound_assign |
| §5.5        | Block expressions      | block_expr                | —                          |
| §5.6-5.8    | Control flow           | control_flow              | non_bool_condition         |
| §5.9        | Match                  | match_exhaustive          | non_exhaustive_match       |
| §6.1-6.2    | Scoping / anti-shadow  | scope                     | shadowing                  |
| §6.4        | No global mut          | top_level_const           | global_mut                 |
| §6.5        | Order independence     | order_independence        | —                          |
| §7          | Error handling         | error_handling            | unhandled_result           |
| §8          | Arrays                 | arrays                    | —                          |
| §9          | FFI                    | ffi                       | —                          |
| §10         | Micro-lib              | strings, hello_world      | —                          |
| various     | Nested constructs      | nested_control            | undeclared_var             |
