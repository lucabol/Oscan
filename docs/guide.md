# Oscan Language Guide

A concise guide to writing correct Oscan programs. For the full formal specification, see [Oscan-spec.md](spec/Oscan-spec.md).

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
