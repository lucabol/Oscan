# Oscan Test Suite

## Overview

This directory contains the conformance test suite for the Oscan compiler. Tests verify that the compiler correctly accepts valid programs, rejects invalid programs, and produces correct output.

## Directory Structure

```
tests/
├── README.md               # This file
├── run_tests.sh            # Test runner (bash/Linux/macOS)
├── run_tests.ps1           # Test runner (PowerShell/Windows)
├── positive/               # Valid programs that must compile and run correctly
│   ├── hello_world.osc      # Basic hello world
│   ├── fibonacci.osc        # Recursion, if/else
│   ├── structs_enums.osc    # Struct/enum declarations and usage
│   ├── match_exhaustive.osc # Match expressions, exhaustive patterns
│   ├── error_handling.osc   # Result type, try, match on Result
│   ├── mutability.osc       # let vs let mut, assignment
│   ├── control_flow.osc     # if/else, while, for loops
│   ├── arithmetic.osc       # All arithmetic ops, precedence
│   ├── logical.osc          # and, or, not operators
│   ├── comparison.osc       # All comparison operators
│   ├── type_casts.osc       # Explicit casts (as keyword)
│   ├── arrays.osc           # Fixed and dynamic arrays, indexing, push, len
│   ├── strings.osc          # String literals, string functions
│   ├── block_expr.osc       # Block expressions returning values
│   ├── scope.osc            # Lexical scoping, variable lifetime
│   ├── purity.osc           # fn vs fn! purity enforcement
│   ├── top_level_const.osc  # Top-level let bindings
│   ├── ffi.osc              # Extern declarations
│   ├── order_independence.osc # Functions/types used before declaration
│   ├── nested_control.osc   # Nested if/while/for/match
│   └── socket_hostnames.osc # Loopback hostname regression for socket_connect/socket_sendto
├── negative/               # Invalid programs that must be REJECTED
│   ├── shadowing.osc        # Variable shadowing → compile error
│   ├── non_exhaustive_match.osc # Missing enum variant → error
│   ├── unhandled_result.osc # Using Result without match/try → error
│   ├── implicit_coercion.osc # Type mismatch without cast → error
│   ├── immutable_assign.osc # Assigning to immutable binding → error
│   ├── undeclared_var.osc   # Using undeclared variable → error
│   ├── type_mismatch.osc    # Wrong types in expressions → error
│   ├── purity_violation.osc # Pure fn calling fn! → error
│   ├── missing_type_annotation.osc # Binding without type → error
│   ├── mixed_arithmetic.osc # i32 + f64 without cast → error
│   ├── non_bool_condition.osc # if/while with non-bool → error
│   ├── global_mut.osc       # let mut at top level → error
│   ├── comparison_chain.osc # a < b < c → error
│   └── compound_assign.osc  # += operator → error
├── expected/               # Expected stdout for positive tests
│   ├── hello_world.expected
│   ├── fibonacci.expected
│   └── ... (one per positive test)
└── build/                  # Build artifacts (gitignored except .gitkeep)
    └── .gitkeep
```

## Running Tests

### Prerequisites
- The `oscan` compiler binary
- A C compiler (gcc or clang)
- The Oscan runtime library (`../runtime/osc_runtime.c` and `../runtime/osc_runtime.h`)

### Linux / macOS (bash)
```bash
chmod +x run_tests.sh
./run_tests.sh ../target/release/oscan gcc
# or with clang:
./run_tests.sh ../target/release/oscan clang
```

### Windows (PowerShell)
```powershell
.\run_tests.ps1 -Oscan ..\target\release\oscan.exe
```

### Differential backend oracle

The PowerShell runner can compare any opt-in native backend with the C backend:

```powershell
.\run_tests.ps1 -Oscan ..\target\release\oscan.exe -Backend native
# Full repository runner:
.\test.ps1 -Backend native
# GNU-style spelling is also accepted:
.\test.ps1 --backend native
# Focused native runtime/link-mode regression:
.\tests\native_hosted.tests.ps1 -Oscan .\target\release\oscan.exe
```

`c` remains the default, so existing commands are unchanged. Selecting another
backend requires the compiler to advertise `--backend`; otherwise the runner
stops with a clear gating message. Use `-BackendOption` if a development
compiler exposes the selector under another option name.

For each positive test, the differential run invokes both `--backend c` and
the selected backend in isolated working directories. It compares compile and
runtime exit status, normalized stdout, stable normalized stderr, and the
resulting fixture files. Both outputs are also checked against
`expected/<name>.expected`, which remains an independent third oracle.

- Put per-test input fixtures in `fixtures/<name>/`.
- Put the complete expected final fixture tree in `expected_files/<name>/`.
- Optional `expected_stderr/<name>.expected` and
  `expected_exit/<name>.expected` files add explicit expected diagnostics and
  exit status (the default exit status is zero).
- Pass `-UnstableStderrTests name1,name2` only for tests whose stderr is
  intentionally platform- or backend-dependent.

Negative tests are compiled once without a backend selector: they exercise the
shared frontend and are not duplicated per backend.

The focused hosted-mode regression verifies that plain `--backend native`
remains libc-free, while explicit `--libc --backend native` differentially
runs all FFI fixtures (including libm symbols), preserves object-only output,
and passes `--extra-c`/`--extra-cflags` through the hosted linker.

### Cross-platform runs (WSL Linux x64, WSL native cross-link, ARM64)

`test.ps1`'s WSL and ARM64 (QEMU) phases cross-compile and run every positive
test outside Windows. They honor the same `expected_exit/<name>.expected`
convention as the differential oracle above:

- The WSL native cross-link phase (`--backend native` cross-emitted to
  `linux-x86_64`, linked and run under WSL) reads
  `expected_exit/<name>.expected` for each attempted test and fails a program
  whose actual exit code doesn't match its declared expectation (default
  zero) — see `Resolve-WslNativeBatchRecords` in `test.ps1` and its focused
  tests in `wsl_native_batch.tests.ps1`.
- A handful of builtins are legitimately unavailable or behave differently
  outside the primary Windows freestanding environment: canvas/clipboard
  "alive before open" defaults differ between Windows and POSIX's
  backend-selection logic, `img_load`/`svg_load`/`tt_load` decoding and
  `tls_connect` are only wired up for `__x86_64__`/`_WIN32` targets (so ARM64
  falls back to the same "not supported" stub used by hosted/libc mode), and
  `tls_fetch` needs real outbound network access a sandboxed runner may not
  have. All three WSL/ARM phases accept either `expected/<name>.expected` or
  `expected_libc/<name>.expected` (when present) so those constrained
  contexts don't fail, while any *other* divergence (a crash, a wrong error,
  partial output, ...) still fails — see `Test-ExpectedOutputMatch` in
  `backend_oracle.ps1` and its tests in `backend_oracle.tests.ps1`.

## How It Works

### Positive Tests
1. **Transpile:** `oscan input.osc -o build/input.c`
2. **Compile C:** `gcc build/input.c ../runtime/osc_runtime.c -I../runtime -o build/input -std=c99 -lm`
3. **Run & compare:** Execute `build/input`, capture stdout, compare against `expected/input.expected`

### Negative Tests
1. **Transpile:** `oscan input.osc -o build/input.c`
2. **Expect failure:** The compiler must return a non-zero exit code

## Writing New Tests

### Positive Test
1. Create `positive/my_test.osc` with valid Oscan code
2. Create `expected/my_test.expected` with exact expected stdout
3. Include comments explaining what language features are being tested

### Negative Test
1. Create `negative/my_test.osc` with intentionally invalid code
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
| appendix A  | Socket hostname adaption | builtin_socket, builtin_udp, socket_hostnames | —                      |
| various     | Nested constructs      | nested_control            | undeclared_var             |
