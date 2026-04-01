# Oscan Test Suite

## Summary
- **Total Tests**: 85
- **Positive Tests**: 65 (compile and run successfully)
- **Negative Tests**: 20 (correctly rejected by compiler)

## Positive Tests

| Test Name | Description | Features Tested |
|-----------|-------------|-----------------|
| arithmetic | Basic arithmetic operations | +, -, *, /, % operators |
| arrays | Array operations | Array literals, indexing, dynamic arrays |
| block_expr | Block expressions | Block as expression, last expression as value |
| comparison | Comparison operators | ==, !=, <, >, <=, >= |
| control_flow | Control flow | if/else, while, for loops |
| error_handling | Error handling | Result type, try operator (?) |
| **ffi** | **FFI basic** | **extern block, calling C function (abs)** |
| **ffi_advanced** | **FFI multiple functions** | **Multiple extern blocks, math functions** |
| **ffi_impure_wrapper** | **FFI from fn!** | **fn! calling extern functions** |
| fibonacci | Fibonacci sequence | Recursion, order independence |
| hello_world | Hello world | Basic I/O, print functions |
| logical | Logical operators | &&, \|\|, ! operators |
| match_exhaustive | Match expressions | Pattern matching, exhaustiveness checking |
| mutability | Mutable variables | let mut, reassignment |
| nested_control | Nested control flow | Nested if/while/for |
| order_independence | Order independence | Forward references, circular dependencies |
| purity | Function purity | fn vs fn!, purity checking |
| scope | Variable scoping | Block scope, shadowing prevention |
| strings | String operations | str type, string literals |
| structs_enums | Structs and enums | Struct/enum declarations, pattern matching |
| top_level_const | Top-level constants | Global constants with let |
| type_casts | Type casting | as operator, explicit casts |

## Negative Tests

| Test Name | Description | Error Message |
|-----------|-------------|---------------|
| comparison_chain | Chained comparisons | Comparison chaining not allowed |
| **extern_duplicate** | **Duplicate extern** | **duplicate extern function** |
| **extern_in_pure** | **Pure fn calling extern** | **pure function cannot call impure function** |
| global_mut | Mutable global | Top-level let cannot be mut |
| immutable_assign | Reassigning immutable | Cannot reassign to immutable variable |
| implicit_coercion | Implicit type coercion | Type mismatch (no implicit coercion) |
| missing_type_annotation | Missing type annotation | Type annotation required |
| mixed_arithmetic | Mixed-type arithmetic | Type mismatch (must cast explicitly) |
| non_bool_condition | Non-boolean condition | Condition must be bool |
| non_exhaustive_match | Non-exhaustive match | Match must be exhaustive |
| purity_violation | Pure fn calling impure | pure function cannot call impure function |
| shadowing | Variable shadowing | Variable shadowing not allowed |
| type_mismatch | Type mismatch | Type mismatch error |
| undeclared_var | Undeclared variable | Undefined variable |
| unhandled_result | Unhandled Result | Result must be handled with try |

## FFI Tests (Phase 6)

### Positive FFI Tests
1. **ffi.osc** - Basic FFI
   - Tests: `extern { fn! abs(n: i32) -> i32; }`
   - Calls: `abs(-42)`, `abs(10)`
   - Expected output:
     ```
     abs(-42) = 42
     abs(10) = 10
     ```

2. **ffi_advanced.osc** - Multiple extern functions
   - Tests: Multiple extern blocks with various signatures
   - Functions: sqrt, pow, fabs, floor, ceil, abs
   - Type mappings: i32→i32, f64→f64, (f64,f64)→f64
   - Nested calls: `sqrt(pow(3.0, 2.0) + pow(4.0, 2.0))`
   - Expected output:
     ```
     sqrt(16.0) = 4
     pow(2.0, 3.0) = 8
     fabs(-3.14) = 3.14
     floor(3.7) = 3
     ceil(3.2) = 4
     abs(-42) = 42
     sqrt(3^2 + 4^2) = 5
     ```

3. **ffi_impure_wrapper.osc** - fn! calling extern
   - Tests: Impure function can wrap extern function
   - Pattern: `fn! impure_abs(x: i32) -> i32 { abs(x) }`
   - Expected output:
     ```
     abs(-10) = 10
     ```

### Negative FFI Tests
1. **extern_in_pure.osc** - Pure function calling extern
   - Tests: Purity violation detection
   - Code: `fn pure_abs(x: i32) -> i32 { abs(x) }`
   - Error: "pure function cannot call impure function 'abs'"

2. **extern_duplicate.osc** - Duplicate extern declaration
   - Tests: Duplicate function name detection
   - Code: Two extern blocks both declaring `abs`
   - Error: "duplicate extern function 'abs'"

## Running Tests

### Build the compiler:
```powershell
cargo build --release
```

### Run unit tests:
```powershell
cargo test
```

### Run integration tests (requires WSL with gcc):
```powershell
cd tests
wsl bash -c "cd /mnt/c/Users/lucabol/dev/Shaggot/Squad && <commands>"
```

### Test individual file:
```powershell
# Compile Oscan to C
.\target\release\oscan.exe tests\positive\ffi.osc -o tests\build\ffi.c

# Compile C to binary (via WSL)
wsl bash -c "cd /mnt/c/Users/lucabol/dev/Shaggot/Squad && gcc tests/build/ffi.c runtime/osc_runtime.c -Iruntime -o tests/build/ffi -std=c99 -lm"

# Run
wsl bash -c "cd /mnt/c/Users/lucabol/dev/Shaggot/Squad && tests/build/ffi"
```

## Test Results (Current)

All 85 tests pass:
- 53 unit tests pass (src/lib.rs)
- 65 positive integration tests pass (tests/positive/*.osc)
- 20 negative integration tests pass (tests/negative/*.osc)

**Full test listing:**
```bash
# Positive tests: 65 files in tests/positive/
ls tests/positive/*.osc

# Negative tests: 20 files in tests/negative/
ls tests/negative/*.osc
```

---
*Last updated: Phase 6 (C-FFI Implementation)*
