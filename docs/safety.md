# Why Oscan Is Safe

Oscan prevents **10 of the 12 major categories of bugs** that plague C programs — by design, not by convention. Every prevention mechanism is built into the language itself and enforced at compile time or runtime. A pure Oscan program (no `extern` blocks) **cannot produce any memory corruption bug whatsoever**.

---

## Scorecard

| Bug Category | C | Oscan | Rust | Go | Java |
|---|---|---|---|---|---|
| Buffer overflow | ❌ | ✅ | ✅ | ✅ | ✅ |
| Use-after-free | ❌ | ✅ | ✅ | ✅ | ✅ |
| Double free | ❌ | ✅ | ✅ | ✅ | ✅ |
| Null pointer deref | ❌ | ✅ | ✅ | ❌ | ❌ |
| Uninitialized memory | ❌ | ✅ | ✅ | ✅ | ✅ |
| Integer overflow | ❌ | ✅ | ⚠️ | ⚠️ | ✅ |
| Division by zero | ❌ | ✅ | ✅ | ✅ | ✅ |
| Type confusion | ❌ | ✅ | ✅ | ✅ | ✅ |
| Format string attack | ❌ | ✅ | ✅ | ✅ | ✅ |
| Data races | ❌ | ✅ | ✅ | ❌ | ❌ |
| Resource leaks | ❌ | ✅ | ✅ | ⚠️ | ⚠️ |
| **Prevented:** | **0/11** | **11/11** | **11/11** | **7/11** | **8/11** |

**Legend:** ✅ = prevented by design, ⚠️ = partially prevented, ❌ = not prevented

---

## Buffer Overflow

### The C Bug

```c
int arr[10];
arr[15] = 42;  // writes past end of array — silent corruption
```

Array bounds are not checked in C. Writing to `arr[15]` silently overwrites adjacent memory, often corrupting data or crashing the program unpredictably.

### How Oscan Prevents It

```oscan
let arr: [i32] = [1, 2, 3];
let x: i32 = arr[10];  // RUNTIME PANIC: "array index out of bounds"
```

Every array access goes through a bounds check. Accessing an index less than 0 or greater than or equal to the array length causes an immediate panic with a clear error message. There is no silent corruption.

---

## Use-After-Free

### The C Bug

```c
int *p = malloc(sizeof(int));
free(p);
*p = 42;  // dangling pointer — silent corruption or crash
```

After freeing a pointer, dereferencing it is undefined behavior. The memory may have been reused, or the pointer may be stale, leading to subtle corruption.

### How Oscan Prevents It

```oscan
// There is literally no way to write use-after-free in Oscan.
// There is no free(), no delete, no arena_reset().
// All allocations live until the program exits.
```

Oscan uses a single arena allocator that lives for the entire program. Memory is freed in bulk only at program exit. There is no way for user code to invalidate pointers. Use-after-free is structurally impossible.

---

## Double Free

### The C Bug

```c
free(p);
free(p);  // undefined behavior — heap corruption
```

Calling `free()` twice on the same pointer corrupts the heap and causes a crash or silent data corruption.

### How Oscan Prevents It

No per-object deallocation exists in the language. The arena frees everything at program exit. You cannot double-free something you cannot free at all.

---

## Null Pointer Dereference

### The C Bug

```c
int *p = NULL;
*p = 42;  // segfault
```

NULL pointers are the classic bug source. Forgetting to check for NULL before dereferencing causes a crash or (in some cases) silent memory corruption.

### How Oscan Prevents It

```oscan
let x: i32 = 5;       // must initialize
// let y: i32;         // COMPILE ERROR — no uninitialized variables
```

There is no null value in Oscan. All variables must be initialized at declaration. There is no `null`, `nil`, `None`, or `undefined`. The type system has no nullable types. Errors are handled via `Result<T, E>`, not via null returns. Every variable you declare has a real value.

---

## Uninitialized Memory

### The C Bug

```c
int x;
printf("%d\n", x);  // reads garbage — undefined behavior
```

Variables declared without an initializer contain garbage from the stack or heap. Reading from them is undefined behavior.

### How Oscan Prevents It

```oscan
let x: i32 = 5;       // must initialize at declaration
// let y: i32;         // COMPILE ERROR
```

Every `let` binding requires an explicit initializer. The parser rejects declarations without initialization. The compiler ensures every variable has a value before use.

---

## Integer Overflow

### The C Bug

```c
int x = INT_MAX;
x = x + 1;  // undefined behavior in C — may silently wrap or be optimized away
```

Integer arithmetic in C is undefined when values overflow. Optimizers may assume overflow never happens and remove safety checks, leading to subtle bugs.

### How Oscan Prevents It

```oscan
let x: i32 = 2147483647;  // INT32_MAX
let y: i32 = x + 1;       // RUNTIME PANIC: "i32 addition overflow"
```

All integer arithmetic (`+`, `-`, `*`, `/`, `%` on `i32` and `i64`) is checked. Before each operation, the runtime tests for overflow and panics if it would occur. There is no silent wrapping.

---

## Division by Zero

### The C Bug

```c
int x = 10 / 0;  // undefined behavior
```

Dividing by zero is undefined in C. The program may segfault, produce garbage, or continue silently depending on the platform.

### How Oscan Prevents It

```oscan
let x: i32 = 10 / 0;  // RUNTIME PANIC: "i32 division by zero"
let y: i32 = 10 % 0;  // RUNTIME PANIC: "i32 modulo by zero"
```

Division and modulo operations check for zero divisors before performing the operation. A zero divisor causes an immediate panic.

---

## Type Confusion

### The C Bug

```c
float f = 3.14;
int *p = (int *)&f;  // type punning — reinterprets memory
```

Casts in C can reinterpret memory arbitrarily. You can cast a float to an int pointer and read its bit pattern as an integer — a source of subtle and dangerous bugs.

### How Oscan Prevents It

```oscan
let big: i64 = 9999999999 as i64;
let small: i32 = big as i32;  // RUNTIME PANIC: "i64 to i32 narrowing overflow"

let nan: f64 = 0.0 / 0.0;
let bad: i32 = nan as i32;    // RUNTIME PANIC: "f64 to i32: NaN"
```

Oscan has strict nominal typing with no implicit coercions. There is no `void*`, no raw pointer types, no type punning. Only six explicit casts are allowed, all type-safe. Narrowing casts (e.g., `i64` to `i32`) are checked at runtime and panic if the value doesn't fit.

---

## Format String Attack

### The C Bug

```c
char *user_input = get_input();
printf(user_input);  // if input contains %s, reads stack memory
```

If untrusted input is used as a printf format string, an attacker can read stack memory or write to arbitrary addresses.

### How Oscan Prevents It

There is no `printf`-style format string mechanism in Oscan. All I/O goes through typed builtins (`print`, `println`, `print_i32`, etc.). String interpolation (e.g., `"hello {name}"`) is resolved at compile time with type-checked expressions — no runtime format parsing. Untrusted input cannot be used as a format specifier.

---

## Data Races

### The C Bug

```c
// Thread 1:        // Thread 2:
x = 1;              x = 2;    // data race — undefined behavior
```

Concurrent access to shared mutable state without synchronization is a data race. In C, races cause unpredictable behavior, corruption, and crashes.

### How Oscan Prevents It

Oscan is single-threaded by design. There are no threads, no concurrency primitives, and no shared mutable state. Data races are structurally impossible.

---

## Unhandled Errors / Silent Failures

### The C Bug

```c
FILE *f = fopen("data.txt", "r");
// forgot to check f == NULL
fread(buf, 1, 100, f);  // crash if file doesn't exist
```

C functions return error codes or NULL, but checking them is optional. Forgetting a check causes silent failures or crashes.

### How Oscan Prevents It

```oscan
let result: Result<str, str> = read_file("data.txt");
match result {
    Result::Ok(text) => { println(text); },
    Result::Err(e) => { println("Error: {e}"); },
};
```

Functions that can fail return `Result<T, E>`. The `match` expression forces the programmer to handle both `Ok` and `Err` branches. `try` propagates errors explicitly. There are no exceptions and no hidden control flow. Errors cannot be silently ignored.

---

## Immutability Violations

### The Bug

```oscan
let config: i32 = 5;  // intended to be constant
// ... 100 lines later ...
config = 10;  // oops! unintended mutation
```

Without explicit intent markers, it is easy to accidentally mutate a variable intended to be constant.

### How Oscan Prevents It

```oscan
let x: i32 = 5;
x = 10;             // COMPILE ERROR: "cannot assign to immutable variable 'x'"

let mut y: i32 = 5;
y = 10;             // OK — explicitly opted in
```

`let` bindings are immutable by default. `let mut` is required for mutation. The compiler rejects assignment to immutable variables. Anti-shadowing prevents the subtle bug of accidentally reusing a variable name in a nested scope.

---

## Resource Leaks

### The C Bug

```c
FILE *file = fopen("data.txt", "r");
if (some_error_condition) {
    return;     // oops! forgot to close file
}
fclose(file);
```

Forgetting to pair resource acquisition with cleanup leads to leaks. In C, `malloc` without `free`, `open` without `close`, and other resource pairs are easy to forget, especially on early `return` paths.

### How Oscan Prevents It

```oscan
fn! process_file(path: str) {
    let fd: i32 = file_open_read(path);
    defer file_close(fd);    // cleanup registered immediately
    
    if some_error_condition {
        return;              // file_close(fd) still runs!
    };
    
    // ... process file ...
    
}   // file_close(fd) runs automatically
```

The `defer` statement registers cleanup to run automatically when the function exits, whether via normal return or early exit. Cleanup always runs, eliminating the leak. Multiple defers execute in reverse order (LIFO), ensuring correct teardown of nested resources.

---

## Unbounded Memory Growth (Long-Running Programs)

### The Problem

Oscan's arena allocator collects all allocations into a single arena that persists for the entire program. For short-lived programs, this is efficient and leak-free. However, long-running programs (servers handling thousands of requests, game loops running for hours) can accumulate unbounded memory.

```oscan
fn! handle_request(client: i32) {
    let request: str = socket_recv(client, 4096);      // allocated
    let response: str = format_response(request);      // allocated
    socket_send(client, response);
}   // function exits, but memory is NOT reclaimed
```

Each request adds memory to the global arena. If the server runs for days, memory grows without bound.

### How Oscan Addresses It: Arena Blocks

Oscan provides **scoped arenas** — child arenas created for a specific scope that are freed when the scope exits:

```oscan
fn! handle_request(client: i32) {
    arena {
        let request: str = socket_recv(client, 4096);
        let response: str = format_response(request);
        socket_send(client, response);
    };  // child arena freed — per-request memory reclaimed
}
```

**Benefits:**

- ✅ Per-request or per-frame memory is reclaimed automatically
- ✅ No use-after-free: the type system prevents arena-allocated values from escaping the block
- ✅ Simple to use: just wrap the code in `arena { ... };`
- ✅ Works with `defer` and early returns

**Scope:** Arena blocks are only allowed in `fn!` (side-effecting) functions and cannot return `str` or `[T]` types (arrays/maps/strings). Primitives like `i32` can escape freely, making it safe to return counts, flags, or decision values computed from arena-allocated data.

See [docs/guide.md](guide.md) for examples and [docs/spec/oscan-spec.md](spec/oscan-spec.md) section 8.3.1 for formal semantics.

---

## What Oscan Does NOT Prevent

### Stack Overflow from Deep Recursion

Oscan does not have tail-call optimization or a recursion depth limit. Deep recursion overflows the C stack, causing a segfault.

```oscan
fn fib(n: i32) -> i32 {
    if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
}
// fib(100000) will segfault
```

**Severity:** Low — same behavior as Rust, Go, Java, and Python. This is not a memory safety issue (no corruption), just a crash.

**Workaround:** Convert deep recursion to iteration, or use an explicit stack data structure.

### Unsafe FFI (`extern` blocks)

The `extern` block allows calling arbitrary C functions. The compiler cannot verify that declared signatures match actual C functions. Mismatched declarations produce undefined behavior.

```oscan
extern {
    fn! bad_function(x: i32) -> i32;  // if real C function takes a pointer, UB
}
```

**Severity:** Moderate — but mitigated by forcing all `extern` functions to be `fn!`, so pure code cannot call them. The unsafety is isolated to impure code paths.

**Why this exists:** Every language with C interop faces this trade-off (Rust has `unsafe`, Go has `cgo`). The mitigation (purity boundary) is the right design.

---

## Comparison: Oscan vs Other Languages

| Safety Goal | Oscan | Rust | Go | Java |
|---|---|---|---|---|
| **No memory corruption** | ✅ Pure programs | ✅ | ✅ | ✅ |
| **Bounds checking** | ✅ All accesses | ✅ | ✅ | ✅ |
| **No null pointers** | ✅ No null value | ❌ Nullable pointers | ❌ Nil pointers | ⚠️ Null references |
| **Checked arithmetic** | ✅ All ops | ⚠️ Debug mode only | ⚠️ No checks | ✅ Checked exceptions |
| **No data races** | ✅ Single-threaded | ✅ Type system | ⚠️ Runtime + convention | ⚠️ Convention only |
| **Resource safety (RAII)** | ❌ No RAII | ✅ | ⚠️ GC | ⚠️ GC |
| **Simplicity (for LLMs)** | ✅ 26 keywords | ❌ Complex | ⚠️ Medium | ❌ Large |

**Oscan's unique position:** Matches Rust's memory safety guarantees with a simpler grammar. Achieves Go's straightforwardness while preventing null pointer dereferences. Provides deterministic resource cleanup (arena) without RAII boilerplate.

---

## Summary

**For a pure Oscan program (no `extern`), memory corruption is impossible.** The language eliminates 11 of the 12 major bug categories that plague C:

- ✅ Buffer overflow
- ✅ Use-after-free
- ✅ Double free
- ✅ Null pointer dereference
- ✅ Uninitialized memory
- ✅ Integer overflow
- ✅ Division by zero
- ✅ Type confusion
- ✅ Format string attacks
- ✅ Data races
- ✅ Resource leaks (via `defer` and arena blocks)

The one gap (stack overflow) is structural, affects only edge cases, and matches the behavior of other modern languages like Rust, Go, and Java.

This safety is the foundation for **confident LLM code generation**: the compiler catches mistakes before they become bugs, and the generated C can be audited for correctness.
