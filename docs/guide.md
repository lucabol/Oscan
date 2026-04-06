# Oscan Language Guide

A concise guide to writing correct Oscan programs. For the full formal specification, see [Oscan-spec.md](spec/Oscan-spec.md).

**Interested in safety?** See [Safety Guide](safety.md) to learn how Oscan prevents 10 of 12 major bug categories that plague C.

---

## Basics

Every Oscan program needs a `fn! main()` entry point. Files use the `.osc` extension.

```
fn! main() {
    println("Hello, world!");
}
```

**Key rules that apply everywhere:**

- All bindings require explicit type annotations — no type inference.
- Semicolons terminate statements. The last expression in a block (without `;`) is the block's return value.
- Trailing commas are allowed everywhere.
- Comments: `// line` and `/* block */` (block comments do not nest).

---

## Types

### Primitives

| Type   | Description                  | Literal examples        |
|--------|------------------------------|-------------------------|
| `i32`  | 32-bit signed integer        | `0`, `42`, `-7`         |
| `i64`  | 64-bit signed integer        | `42 as i64`             |
| `f64`  | 64-bit float                 | `3.14`, `0.0`           |
| `bool` | Boolean                      | `true`, `false`         |
| `str`  | Immutable string (UTF-8)     | `"hello"`, `"line\n"`   |
| `unit` | Void equivalent (0 bits)     | `()`                    |

- Integer literals are `i32` by default and must fit the `i32` range. Build larger `i64` values from in-range `i32` values with explicit casts.
- Float literals must have digits on both sides: `0.5` not `.5`.
- String escapes: `\n`, `\t`, `\r`, `\\`, `\"`, `\0`.

### Type Casts

Only 6 explicit casts are allowed, using the `as` keyword:

```
let x: i32 = 42;
let y: i64 = x as i64;    // i32 → i64 (widening)
let z: f64 = x as f64;    // i32 → f64
let w: i32 = 3.9 as i32;  // f64 → i32 (truncates to 3)
```

Allowed pairs: `i32↔i64`, `i32↔f64`, `i64↔f64`. No implicit coercions ever.

---

## Variables

Bindings are immutable by default. Use `mut` to allow mutation.

```
let x: i32 = 10;           // immutable
let mut count: i32 = 0;    // mutable
count = count + 1;         // OK
// x = 20;                 // compile error!
```

### Compound Assignment

Compound assignment operators (`+=`, `-=`, `*=`, `/=`, `%=`) provide shorthand for common mutations:

```
let mut x: i32 = 10;
x += 5;         // equivalent to x = x + 5
x -= 2;         // equivalent to x = x - 2
x *= 3;         // equivalent to x = x * 3
```

Compound assignment requires the binding to be mutable and follows the same type rules as the underlying operator.

### Top-Level Constants

Top-level `let` bindings are constants visible throughout the file:

```
let MAX: i32 = 100;
let PI: f64 = 3.14159265358979;

fn! main() {
    print_i32(MAX);
}
```

---

## Functions

### Pure (`fn`) vs Impure (`fn!`)

```
// Pure — no I/O, no extern calls, no calling fn!
fn add(a: i32, b: i32) -> i32 {
    a + b
}

// Impure — may perform I/O and side effects
fn! greet(name: str) {
    print("Hello, ");
    println(name);
}
```

**Rules:**
- `fn` cannot call `fn!` or `extern` functions — compile error if it tries.
- `fn!` can call anything.
- `main` must be `fn!`. It can return `unit` (implicitly) or `Result<unit, str>` (explicit error handling).
- Functions without `->` return unit (no value).
- The last expression in the body (without `;`) is the return value.
- `return` is available for early exit.

### Order Independence

Functions, structs, enums, and constants can be used before they are declared. No forward declarations needed.

```
fn! main() {
    print_i32(compute(3));  // compute is defined below
}

fn compute(x: i32) -> i32 { x * 2 }
```

### Parameters and Passing Semantics

All parameters are passed by value and are immutable inside the function. There is no `mut` parameter syntax. If you need to modify a parameter or pass a mutable reference, bind it to a local `let mut`:

```
fn process(arr: [i32]) {
    // arr is immutable; you cannot call push(arr, val)
    // To work with a mutable copy, create a local binding:
    let mut local_arr: [i32] = arr;
    push(local_arr, 99);   // OK
}
```

### Function Pointers

Functions can be passed as values using function pointer types. The syntax for a function pointer type is `fn(param_types) -> return_type`.

```
fn ascending(a: i32, b: i32) -> bool { a < b }
fn descending(a: i32, b: i32) -> bool { a > b }

fn! sort_with(arr: [i32], cmp: fn(i32, i32) -> bool) {
    let n: i32 = len(arr);
    let mut i: i32 = 0;
    while i < n - 1 {
        let mut j: i32 = 0;
        while j < n - i - 1 {
            if cmp(arr[j + 1], arr[j]) {
                let tmp: i32 = arr[j];
                arr[j] = arr[j + 1];
                arr[j + 1] = tmp;
            };
            j = j + 1;
        };
        i = i + 1;
    };
}

fn! main() {
    let mut nums: [i32] = [5, 3, 8, 1, 9];
    sort_with(nums, ascending);   // 1 3 5 8 9
    sort_with(nums, descending);  // 9 8 5 3 1
}
```

You can also store function pointers in local variables:

```
fn add(a: i32, b: i32) -> i32 { a + b }

fn! main() {
    let op: fn(i32, i32) -> i32 = add;
    print_i32(op(3, 4));  // prints 7
}
```

**Notes:**
- Only user-defined functions (`fn` and `fn!`) can be used as function pointer values. Builtin and `extern` functions cannot.
- Functions are not closures — they capture no variables.
- Type checking is structural: any function matching the signature is accepted.

---

## Structs

Product types with named fields.

```
struct Point {
    x: f64,
    y: f64,
}

fn! main() {
    let p: Point = Point { x: 1.0, y: 2.0 };
    print_f64(p.x);            // field access
    println("");

    let mut q: Point = Point { x: 0.0, y: 0.0 };
    q.x = 5.0;                 // field mutation (binding must be mut)
}
```

**Field order in struct literals does not need to match declaration order.** You can write `Point { y: 2.0, x: 1.0 }` and it will work.

---

## Enums

Tagged unions (sum types). Variants can carry payloads.

```
enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Dot(Point),
    Empty,
}

fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.14159 * r * r,
        Shape::Rectangle(w, h) => w * h,
        Shape::Dot(p) => 0.0,
        Shape::Empty => 0.0,
    }
}
```

Enum variants are always qualified: `Shape::Circle(5.0)`, never just `Circle(5.0)`.

### Recursive Data Structures

To define recursive structures (e.g., tree nodes), use dynamic arrays for indirection:

```
enum Tree {
    Leaf(i32),
    Node([Tree]),     // array of Tree nodes
}
```

This works because array elements are allocated indirectly through the array's backing buffer, avoiding infinite type sizes.

---

## Pattern Matching

`match` expressions must be **exhaustive** — cover every variant or use `_` as wildcard.

```
// Match on enum
match shape {
    Shape::Circle(r) => print_f64(r),
    _ => println("not a circle"),
};

// Match on i32
match n {
    0 => "zero",
    1 => "one",
    _ => "other",
}

// Match on bool
match flag {
    true => "yes",
    false => "no",
}
```

`match` is an expression — it returns a value. All arms must have the same type.

---

## Error Handling

No exceptions. Use `Result<T, E>` for operations that can fail.

### Creating Results

```
fn divide(a: i32, b: i32) -> Result<i32, str> {
    if b == 0 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}
```

### Consuming Results

You **must** handle a `Result` — you cannot assign it to a non-Result variable.

**Option 1: `match`**
```
match divide(10, 3) {
    Result::Ok(val) => print_i32(val),
    Result::Err(msg) => println(msg),
};
```

**Option 2: `try` (propagation)**

`try` unwraps `Ok` or returns the `Err` to the caller. The calling function must also return a compatible `Result`.

```
fn compute(x: i32, y: i32) -> Result<i32, str> {
    let v: i32 = try divide(x, y);   // early-return Err if division fails
    Result::Ok(v + 1)
}
```

---

## Cleanup with Defer

The `defer` statement registers a function call to execute when the enclosing function returns. This is useful for pairing resource acquisition with cleanup, ensuring cleanup happens even if the function returns early.

```
fn! process_file(path: str) {
    let fd: i32 = file_open_read(path);
    defer file_close(fd);    // will close the file when function exits
    
    // ... use fd to read and process ...
    
}   // file_close(fd) runs automatically here
```

**Key points:**

- `defer` is only allowed in `fn!` (side-effecting) functions.
- The deferred call executes when the function returns, whether via normal exit or early `return`.
- Multiple defers execute in reverse order (LIFO): the last defer registered runs first.
- Arguments are evaluated immediately (at defer-time), but the function call runs at exit.

**Example with multiple defers:**

```oscan
fn! setup_resources() {
    let db: i32 = open_database();
    defer close_database(db);
    
    let file: i32 = open_file("log.txt");
    defer close_file(file);
    
    // ... work with db and file ...
    
}   // close_file(file) runs first, then close_database(db)
```

This pattern eliminates resource leaks: you register the cleanup immediately after acquiring the resource, and the compiler ensures it runs.

---

## Memory Management: Arena Blocks

Long-running programs (web servers, game loops) can accumulate memory without reclamation. While Oscan's arena allocator handles memory for the entire program, **arena blocks** provide a way to reclaim per-request or per-frame allocations safely.

### What Are Arena Blocks?

An **arena block** is a scope (`arena { ... };`) where all allocations are made to a child arena. When the block exits, the child arena is freed and memory is reclaimed.

```oscan
fn! handle_request(client: i32) {
    arena {
        let request: str = socket_recv(client, 4096);
        let response: str = format_response(request);
        socket_send(client, response);
    };  // child arena freed — memory reclaimed
}
```

### When to Use Arena Blocks

Use arena blocks for:
- **Per-request cleanup** in servers: wrap the request handler body
- **Per-frame cleanup** in game loops: wrap the frame update/render loop
- **Temporary allocations** in long-running code: wrap code that creates temporary strings or arrays

### Safety: What Can Escape?

Arena blocks prevent use-after-free by enforcing that **arena-allocated types cannot escape the block**:

- ❌ **Forbidden:** `str`, `[T]` (dynamic arrays), `map`, and structs containing these
- ✅ **Allowed:** primitives (`i32`, `i64`, `f64`, `bool`, `unit`)

This is checked at compile time.

**Example:**

```oscan
fn! process() -> i32 {
    let count: i32;
    arena {
        let text: str = read_file("data.txt");
        count = str_len(text);   // ✓ OK: i32 escapes
    };  // text is freed; cannot be used after this
    count   // ✓ OK: returns primitive
}
```

### How It Works

- `defer` statements inside arena blocks still work correctly — they run before the arena is freed.
- Early `return` from a function containing an arena block triggers cleanup normally.
- Arena blocks are only allowed in `fn!` (side-effecting) functions.
- Arena blocks can be nested.

---

## Control Flow

### If / Else

Braces are mandatory. `if` is an expression.

```
// Statement form
if x > 0 {
    println("positive");
} else if x == 0 {
    println("zero");
} else {
    println("negative");
};

// Expression form
let label: str = if x > 10 { "big" } else { "small" };
```

### While Loop

```
let mut i: i32 = 0;
while i < 10 {
    print_i32(i);
    i += 1;
};
```

### Loop Control: Break and Continue

Exit or skip iterations with `break` and `continue`:

```
let mut i: i32 = 0;
while i < 10 {
    if i == 5 {
        break;     // exit the loop
    };
    if i == 2 {
        i += 1;
        continue;  // skip to next iteration
    };
    print_i32(i);
    i += 1;
};
```

### For Loop

Iterates over an integer range (`start..end`, exclusive upper bound).

```
for i in 0..5 {
    print_i32(i);   // prints 0 1 2 3 4
};
```

The range bound can be an expression: `for i in 0..len(arr) { ... }`.

### Block Expressions

Any `{ ... }` block is an expression. The last expression (without `;`) is its value.

```
let result: i32 = {
    let a: i32 = 10;
    let b: i32 = 20;
    a + b          // block evaluates to 30
};
```

---

## Arrays

### Fixed-Size Arrays

```
let nums: [i32; 5] = [10, 20, 30, 40, 50];
print_i32(nums[0]);       // indexing (zero-based)
print_i32(len(nums));     // 5
```

### Dynamic Arrays

```
let mut arr: [i32] = [1, 2, 3];
push(arr, 4);              // append element
print_i32(len(arr));       // 4
print_i32(arr[3]);         // 4

let mut empty: [i32] = [];
push(empty, 99);
```

Iterate with a `for` loop over indices:

```
for i in 0..len(arr) {
    print_i32(arr[i]);
};
```

---

## Strings

Strings (`str`) are immutable. Use built-in functions to work with them:

```
let a: str = "Hello, ";
let b: str = "World!";
let c: str = str_concat(a, b);   // "Hello, World!"
print_i32(str_len(c));           // 13
print_bool(str_eq(a, b));        // false
```

String escape sequences: `\n` (newline), `\t` (tab), `\r` (carriage return), `\\`, `\"`, `\0`.

### String Interpolation

Interpolated string literals use `"text {expr} text"` syntax:

```
fn! main() {
    let name: str = "Neo";
    let count: i32 = 42;
    let ok: bool = true;

    println("hello {name}");
    println("count={count} ok={ok}");
    println("literal braces: {{ready}}");
}
```

Rules:

- Embedded expression types are limited to `str`, `i32`, `i64`, `f64`, and `bool`.
- Embedded expressions must be pure: they can call `fn`, but not `fn!` or `extern`.
- Write literal braces as `{{` and `}}`. A lone `}` is a lexical error.
- Interpolation lowers to the existing string builders plus the numeric/bool conversion helpers.
- Mixed-part or numeric interpolation allocates, so it is typically used in `fn!` code.

See `examples/string_interpolation.osc` for a small end-to-end showcase.

### String Functions (Pure)

| Function                          | Description              |
|-----------------------------------|--------------------------|
| `str_len(s: str) -> i32`         | String length            |
| `str_eq(a: str, b: str) -> bool` | String equality          |

### String Functions (Impure)

| Function                          | Description                              |
|-----------------------------------|------------------------------------------|
| `str_concat(a: str, b: str) -> str` | Concatenate strings (allocates)      |
| `str_to_cstr(s: str) -> str`      | Convert to C-compatible null-terminated string (FFI) |

### String Conversion Helpers

| Function                          | Description                              |
|-----------------------------------|------------------------------------------|
| `i32_to_str(n: i32) -> str`       | Convert `i32` to string (allocates)      |
| `str_from_i64(n: i64) -> str`     | Convert `i64` to string (allocates)      |
| `str_from_f64(n: f64) -> str`     | Convert `f64` to string (allocates)      |
| `str_from_bool(b: bool) -> str`   | Convert `bool` to `"true"` / `"false"` |

These helpers are the runtime surface interpolation lowers to.

## Operators

### Arithmetic
`+`, `-`, `*`, `/`, `%` (remainder, integers only), unary `-`

### Comparison
`==`, `!=`, `<`, `>`, `<=`, `>=`

**Comparison chaining is forbidden.** `a < b < c` is a compile error — write `a < b and b < c`.

### Logical
`and`, `or`, `not` (keywords, not symbols). Short-circuit evaluation.

```
if x > 0 and x < 100 {
    println("in range");
};
```

---

## C FFI

Declare external C functions in `extern` blocks. All externs are implicitly `fn!`.

```
extern {
    fn! sqrt(x: f64) -> f64;
    fn! abs(n: i32) -> i32;
}

fn! main() {
    print_f64(sqrt(16.0));    // 4.0
    print_i32(abs(-42));      // 42
}
```

Only primitive types (`i32`, `i64`, `f64`, `bool`, `str`) are supported in FFI signatures.

---

## Built-in Functions

### I/O (all `fn!`)

| Function                        | Description                  |
|---------------------------------|------------------------------|
| `print(s: str)`                 | Print string, no newline     |
| `println(s: str)`               | Print string with newline    |
| `print_i32(n: i32)`            | Print i32                    |
| `print_i64(n: i64)`            | Print i64                    |
| `print_f64(n: f64)`            | Print f64                    |
| `print_bool(b: bool)`          | Print bool                   |
| `read_line() -> Result<str, str>` | Read a line from stdin    |

### String (split into Pure and Impure)

See the **Strings** section above for the complete string function tables.

### Math (all `fn`)

| Function                           | Description             |
|------------------------------------|-------------------------|
| `abs_i32(n: i32) -> i32`          | Absolute value          |
| `abs_f64(n: f64) -> f64`          | Absolute value          |
| `mod_i32(a: i32, b: i32) -> i32`  | Integer modulo          |

### Conversion

| Function                            | Description             |
|-------------------------------------|-------------------------|
| `i32_to_str(n: i32) -> str`        | Integer to string (allocates) |
| `str_from_i64(n: i64) -> str`      | `i64` to string (allocates) |
| `str_from_f64(n: f64) -> str`      | `f64` to string (allocates) |
| `str_from_bool(b: bool) -> str`    | Bool to string (pure) |

### Array Functions (Pure)

| Function                   | Description                |
|----------------------------|----------------------------|
| `len(arr) -> i32`          | Array length               |

### Array Functions (Impure)

| Function                   | Description                |
|----------------------------|----------------------------|
| `push(arr, elem)`          | Append to dynamic array    |

### Whole-File I/O (Impure)

| Function                             | Description                          |
|--------------------------------------|--------------------------------------|
| `read_file(path: str) -> Result<str, str>` | Read entire file; returns content or error |
| `write_file(path: str, data: str) -> Result<str, str>` | Write string to file; returns empty string or error |

### Min/Max/Clamp (Pure)

| Function                                    | Description                  |
|---------------------------------------------|------------------------------|
| `min_i32(a: i32, b: i32) -> i32`           | Minimum of two i32 values    |
| `max_i32(a: i32, b: i32) -> i32`           | Maximum of two i32 values    |
| `clamp_i32(v: i32, lo: i32, hi: i32) -> i32` | Clamp i32 to range [lo, hi] |
| `min_i64(a: i64, b: i64) -> i64`           | Minimum of two i64 values    |
| `max_i64(a: i64, b: i64) -> i64`           | Maximum of two i64 values    |
| `clamp_i64(v: i64, lo: i64, hi: i64) -> i64` | Clamp i64 to range [lo, hi] |
| `min_f64(a: f64, b: f64) -> f64`           | Minimum of two f64 values    |
| `max_f64(a: f64, b: f64) -> f64`           | Maximum of two f64 values    |
| `clamp_f64(v: f64, lo: f64, hi: f64) -> f64` | Clamp f64 to range [lo, hi] |

### String Join (Impure)

| Function                          | Description                              |
|-----------------------------------|------------------------------------------|
| `str_join(arr: [str], sep: str) -> str` | Join array of strings with separator (allocates) |

### Path Utilities (Mixed Purity)

| Function                          | Description                              |
|-----------------------------------|------------------------------------------|
| `path_basename(path: str) -> str` | Extract filename from path (pure)        |
| `path_dirname(path: str) -> str`  | Extract directory from path (allocates)  |

### Hash Maps

The basic `map` type stores string keys and string values:

```oscan
let mut m: map = map_new();
map_set(m, "name", "Alice");
println(map_get(m, "name"));       // Alice
print_bool(map_has(m, "name"));    // true
```

**Typed map variants** provide type-safe maps with different key/value types. Each variant uses a `map_TYPE_` prefix for all operations:

| Type | Keys | Values | Example |
|------|------|--------|---------|
| `map_str_i32` | `str` | `i32` | Word counts |
| `map_str_i64` | `str` | `i64` | Large counters |
| `map_str_f64` | `str` | `f64` | Named measurements |
| `map_i32_str` | `i32` | `str` | ID→name lookups |
| `map_i32_i32` | `i32` | `i32` | Sparse integer mappings |

```oscan
let mut counts: map_str_i32 = map_str_i32_new();
map_str_i32_set(counts, "apple", 3);
print_i32(map_str_i32_get(counts, "apple"));    // 3
print_i32(map_str_i32_get(counts, "missing"));   // 0 (default)
```

Each typed map has: `_new`, `_set`, `_get`, `_has`, `_delete`, `_len`. Unlike `map_get` (which panics on missing keys), typed `_get` functions return a default value: `0` for integers, `0.0` for f64, `""` for strings.

---

## Scoping Rules

- **Lexical scoping.** Variables are visible from declaration to end of their block.
- **Anti-shadowing.** You cannot declare a variable with the same name as one in any enclosing scope.
- **No globals.** Top-level `let` bindings are immutable constants, not mutable state.

```
let x: i32 = 1;
{
    // let x: i32 = 2;  // compile error: shadowing forbidden
    let y: i32 = 2;     // OK: different name
};
```

---

## Gotchas & Notes

1. **No `++`, `--`.** Write `x = x + 1` or use compound assignment `x += 1`.
2. **No `&&`, `||`, `!`.** Use `and`, `or`, `not`.
3. **No closures, no first-class functions.** Functions are not values.
4. **No methods.** All functions are free-standing. Use `area(shape)`, not `shape.area()`.
5. **No generics** except the built-in `Result<T, E>`.
6. **Interpolation is intentionally small.** Only `str`, `i32`, `i64`, `f64`, and `bool` can appear inside `{...}`.
7. **Semicolons after control flow (optional).** `if`/`while`/`for`/`match` used as statements may have a trailing `;` but it is not required.
8. **Integer literals are `i32`.** For `i64`, write `42 as i64`.
9. **Panics are for bugs.** Overflow, out-of-bounds, division by zero panic at runtime. Expected failures use `Result`.

---

## Native Toolchain Lookup (Windows/Linux)

For normal host builds on Windows and Linux, Oscan can use a bundled C toolchain shipped in a `toolchain/` directory instead of always requiring a separately installed system compiler.

Lookup order:

1. `OSCAN_CC` — explicit compiler path/command override
2. `OSCAN_TOOLCHAIN_DIR` — bundled toolchain root override
3. sibling `toolchain/` directory next to the `oscan` binary
4. `toolchain/` directory in the current working directory
5. normal host compiler detection/fallback

When a bundled toolchain directory is used, Oscan searches platform-specific and generic `bin/` directories:

- Windows: `toolchain/windows/bin/`, then `toolchain/bin/`
- Linux: `toolchain/linux/bin/`, then `toolchain/bin/`

`OSCAN_TOOLCHAIN_DIR` points at the root of the bundled toolchain. If no override or bundled toolchain is present, host compiler fallback still applies. Cross-compilation targets such as `riscv64` and `wasi` still require their own target-specific toolchains.

---

## Cross-Compilation

Oscan supports cross-compilation to RISC-V and WebAssembly (WASI) targets using the `--target` flag.

### RISC-V 64-bit

Compile for RISC-V 64-bit using freestanding mode:

```bash
oscan program.osc --target riscv64 -o program.c
riscv64-linux-gnu-gcc -std=gnu11 -ffreestanding -nostdlib -static program.c runtime/osc_runtime.c -Iruntime -o program
# Run under QEMU:
qemu-riscv64 ./program
```

**Requirements:**
- `riscv64-linux-gnu-gcc` — RISC-V cross-compiler
- `qemu-riscv64` — (optional) QEMU user-mode for local testing

### WebAssembly via WASI

Compile for WebAssembly System Interface (WASI):

```bash
oscan program.osc --target wasi -o program.c
clang --target=wasm32-wasi program.c runtime/osc_runtime.c -Iruntime -o program.wasm
# Run under wasmtime:
wasmtime ./program.wasm
```

**Requirements:**
- `clang` — Compiler with WASI support
- `wasi-sysroot` — WASI C library headers and runtime
- `wasmtime` — (optional) WASI runtime for local testing. Alternatives: Node.js (with WASI), wasmer, etc.

**Note:** RISC-V uses freestanding mode and requires manual runtime linking. WASI uses libc-compatible mode and provides standard POSIX APIs through the WASI interface.
