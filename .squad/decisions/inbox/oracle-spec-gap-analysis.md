# Oracle — Spec-to-Test Gap Analysis

**Date:** 2025-07-15  
**Author:** Oracle (Language Spec Specialist)  
**Scope:** Map every spec section (§1–§10) to existing test coverage, identify gaps, propose exact new test cases grouped into ≤10 new files.

---

## Executive Summary

The existing test suite (32 positive, 16 negative) covers the "happy path" for most major features but has significant gaps in **edge cases**, **boundary conditions**, and **error rejection**. The spec defines many precise constraints (e.g., comparison chaining is an error, `Result` is reserved, assignment is not an expression, match arms must have matching types) — many of these are only partially tested or not tested at all.

**Key gap themes:**
1. Token/syntax edge cases (escape sequences, comment boundaries, identifier rules) — mostly untested
2. Type cast edge cases and cast-in-expression contexts — partially tested
3. Declaration edge cases (empty structs, trailing commas, zero-arg functions) — mostly untested
4. Expression edge cases (operator precedence combos, unary minus, modulo, for-loop boundaries) — partially tested
5. Scoping subtleties (match arm bindings, for loop variable) and error handling (try constraints) — partially tested
6. Micro-lib functions — several untested (abs_i32, abs_f64, mod_i32, str_to_cstr, arena_reset in small program)

---

## §1 — Keywords & Tokens

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| `fn` / `fn!` keywords | `purity.osc`, `ffi_impure_wrapper.osc` |
| `let` / `let mut` | `mutability.osc` |
| `struct` / `enum` | `structs_enums.osc` |
| `if` / `else` | `control_flow.osc` |
| `while` / `for` / `in` | `control_flow.osc`, `nested_control.osc` |
| `match` | `match_exhaustive.osc` |
| `return` | `error_handling.osc` (implicit in try) |
| `try` | `error_handling.osc` |
| `extern` | `ffi.osc`, `ffi_advanced.osc` |
| `as` | `type_casts.osc` |
| `and` / `or` / `not` | `logical.osc` |
| `true` / `false` | `logical.osc`, `comparison.osc` |
| Comparison operators | `comparison.osc` |
| Comparison chaining forbidden | **neg:** `comparison_chain.osc` |
| `..` range operator | `control_flow.osc` |
| `_` wildcard in match | `match_exhaustive.osc` |
| String literals + escapes `\t`, `\n` | `strings.osc` |
| Integer literals | `arithmetic.osc` |
| Float literals | `arithmetic.osc` (f64 ops) |
| Line comments `//` | Used in many files |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| Escape sequences `\r`, `\\`, `\"`, `\0` | §1.5 — only `\t` and `\n` are tested in `strings.osc` |
| Identifiers starting with `_` (e.g., `_unused`) | §1.4 — permitted but never tested |
| Top-level `let` with computed constant expression (e.g., `let X: i32 = 3 + 4 * 2;`) | §4.2 — `top_level_const.osc` has `COMPUTED = 6 * 7` but not a multi-operator expression |
| Block comments `/* ... */` | §1.6 — no test uses block comments |
| `fn!` as single token (not `fn` + `!`) | §1.1/§2.1 — implicitly tested but no edge-case test |
| Boolean literals as values in let bindings | §1.5 — `top_level_const.osc` has `IS_ENABLED` but no local bool literal test |
| Explicit `return` statement | §2 grammar — no test explicitly uses `return expr;` mid-function |

### Proposed Tests → `spec_tokens_syntax.osc`
1. **All escape sequences in strings:** Print strings containing `\r`, `\\`, `\"`, `\0`, `\t`, `\n` — verify output
2. **Underscore-prefixed identifier:** `let _unused: i32 = 99;` then use it — verify it works
3. **Block comment:** Code with `/* block comment */` before and after expressions — verify it's stripped
4. **Explicit return statement:** Function with early `return` mid-body
5. **Trailing commas everywhere:** Function params, struct fields, enum variants, array literals, match arms — all with trailing commas
6. **Empty array literal:** `let empty: [i32] = [];` then `len(empty)` → 0
7. **Negative integer literal in expression:** `-42` as argument, in let binding
8. **Float with leading zero:** `0.5`, `0.0` as literals

**Expected output (approximate):**
```
backslash: \
quote: "
tab:	done
null_byte: (empty or impl-defined)
_unused = 99
block_comment_ok
early_return = 10
trailing_commas_ok
empty_len = 0
neg = -42
float = 0.5
```

---

## §2 — Grammar (EBNF)

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| `fn_decl` with params and return type | `fibonacci.osc`, `arithmetic.osc` |
| `struct_decl` with fields | `structs_enums.osc` |
| `enum_decl` with payloads | `structs_enums.osc` |
| `top-level let_decl` | `top_level_const.osc` |
| `extern_block` | `ffi.osc` |
| `block` as expression | `block_expr.osc` |
| `let_stmt` / `assign_stmt` | `mutability.osc` |
| `while_stmt` / `for_stmt` | `control_flow.osc` |
| `if_expr` / `match_expr` | `control_flow.osc`, `match_exhaustive.osc` |
| `try_expr` | `error_handling.osc` |
| `array_literal` | `arrays.osc` |
| `struct_literal` | `structs_enums.osc` |
| `enum_constructor` | `structs_enums.osc` |
| `pattern` (wildcard, enum, literal) | `match_exhaustive.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| Negative literal patterns in match (`-1 => ...`) | §2 `literal_pattern` — GAP-4 was added to spec but no test exists |
| Unit-returning function (no `-> type` annotation) | §4.1 — tested implicitly in `main` but no explicit test of `fn helper() { ... }` |
| Zero-parameter function call | §5.10 — `fn foo() -> i32 { 42 }` then `foo()` |
| `if` without `else` (statement form) | §5.6 — tested in various files but never as explicit statement-produces-unit test |
| `else if` chain (multi-branch) | §5.6 — `control_flow.osc` has this but edge cases untested |
| Match on string literals | §5.9 — not explicitly tested |
| Match on bool | §5.9 — `match_exhaustive.osc` has this |
| Enum variant with zero payload (no parens) | §5.14 — `Shape::Empty` tested in `structs_enums.osc` |

### Proposed Tests → included in `spec_tokens_syntax.osc` (grouped with §1)
9. **Negative literal pattern in match:** `match x { -1 => "neg one", 0 => "zero", _ => "other" }`
10. **Match on string literal:** `match s { "hello" => 1, "world" => 2, _ => 0 }`
11. **Zero-parameter pure function:** `fn answer() -> i32 { 42 }` then call it

---

## §3 — Type System

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| `i32` type | `arithmetic.osc` |
| `i64` type | `arithmetic.osc` (1B * 2) |
| `f64` type | `arithmetic.osc` (10.0/3.0) |
| `bool` type | `logical.osc`, `comparison.osc` |
| `str` type | `strings.osc` |
| `unit` type | Implicit in many `fn!` functions |
| Struct (nominal) | `structs_enums.osc` |
| Enum (tagged union) | `structs_enums.osc` |
| `Result<T, E>` | `error_handling.osc` |
| Fixed-size arrays `[T; N]` | `arrays.osc` |
| Dynamic arrays `[T]` | `arrays.osc`, arena tests |
| All 6 explicit casts | `type_casts.osc` |
| No implicit coercion | **neg:** `implicit_coercion.osc` |
| No mixed arithmetic | **neg:** `mixed_arithmetic.osc` |
| Missing type annotation | **neg:** `missing_type_annotation.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| Cast in complex expressions: `(a as i64) + (b as i64)` | §3.7 — only simple casts tested |
| Chained casts: `42 as i64 as f64` | §3.7 — not tested (should this work?) |
| Cast precedence: `-5 as i64` (unary minus vs. cast) | §1.2 — `as` at prec 9 vs unary `-` at prec 8 |
| `Result` as reserved type name (struct/enum) | §3.3 — **no negative test exists** |
| Enum equality compares tag only | §5.2 — not tested |
| Struct cannot be compared with `==` | §5.2 — no negative test |
| `Result::Ok` / `Result::Err` construction in various positions | §3.3 — only in function return position |
| Array of arrays `[[i32]]` | §3.4 — not tested |
| Array of enums | §3.4 — `mixed_alloc_enum_dynamic.osc` has this |
| Empty struct construction `Void {}` | §4.3 — documented as valid, never tested |
| Recursive enum with dynamic array (Tree example from spec) | §3.5 — never tested |

### Proposed Tests → `spec_types_casts.osc`
1. **Cast in arithmetic expression:** `let r: i64 = (10 as i64) + (20 as i64);` → print 30
2. **Cast precedence:** `let x: i64 = -5 as i64;` — verify it's -5 (not error)
3. **f64→i32 truncation:** `let x: i32 = 3.7 as i32;` → 3
4. **f64→i64 truncation:** `let x: i64 = 99.9 as i64;` → 99
5. **i32→f64 then arithmetic:** `let x: f64 = (7 as f64) * 2.0;` → 14.0
6. **Enum tag equality:** Two enums with same tag but different payloads: `==` returns true
7. **Empty struct:** `struct Unit {}` → construct `Unit {}`, pass to function, print confirmation
8. **Result in let binding:** `let r: Result<i32, str> = Result::Ok(42);` then match
9. **Array of arrays:** `let grid: [[i32]] = [[1,2],[3,4]];` access `grid[1][0]` → 3

**Expected output:**
```
cast_arith = 30
cast_neg = -5
trunc_f64_i32 = 3
trunc_f64_i64 = 99
i32_f64_arith = 14.000000
enum_tag_eq = true
empty_struct_ok
result_in_let = 42
grid_access = 3
```

---

## §4 — Declarations

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| Pure function `fn` | `fibonacci.osc`, `purity.osc` |
| Impure function `fn!` | `hello_world.osc`, many others |
| Pure calling pure | `purity.osc` |
| Impure calling pure | `purity.osc` |
| Recursion | `fibonacci.osc` |
| `let` immutable binding | `mutability.osc` |
| `let mut` mutable binding | `mutability.osc` |
| Top-level immutable const | `top_level_const.osc` |
| Struct declaration + field access | `structs_enums.osc` |
| Enum declaration + construction | `structs_enums.osc` |
| Extern block | `ffi.osc`, `ffi_advanced.osc` |
| Order independence | `order_independence.osc` |
| Purity violation | **neg:** `purity_violation.osc` |
| Extern in pure | **neg:** `extern_in_pure.osc` |
| Global mut | **neg:** `global_mut.osc` |
| Immutable assign | **neg:** `immutable_assign.osc` |
| Duplicate extern | **neg:** `extern_duplicate.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| `main` with `Result<unit, str>` return type | §4.1 — documented as valid signature, never tested |
| Function with no parameters and no return type | §4.1 — `fn! helper() { ... }` only tested via `main` |
| Struct with single field | §4.3 — always multi-field in tests |
| Enum with single variant | §4.4 — always multi-variant |
| Enum variant with multiple payloads (3+) | §4.4 — only up to 2 payloads tested |
| Field order independence in struct literal | §5.13 — documented ("field order does not matter") but never tested |
| Parameters as local scope (not shadowing function-level) | §6.2 — never tested that parameter names don't shadow |
| Multiple extern blocks in same program | §9.1 — `ffi_advanced.osc` has this |

### Proposed Tests → `spec_declarations.osc`
1. **main returning Result<unit, str>:** `fn! main() -> Result<unit, str> { Result::Ok(()) }` — but since we need output, have it print first
2. **Zero-arg pure function:** `fn get_magic() -> i32 { 42 }`
3. **Struct with single field:** `struct Wrapper { value: i32 }` → construct, access
4. **Enum with single variant:** `enum Single { Only(i32) }` → construct, match
5. **Enum variant with 3 payloads:** `enum Triple { Val(i32, i32, i32) }` → construct, match, print
6. **Field order independence:** `struct Pair { a: i32, b: i32 }` → construct as `Pair { b: 2, a: 1 }` → verify `p.a == 1`
7. **Function calling function defined later in file (forward ref):** Already tested in `order_independence.osc` but add a struct-forward-ref test
8. **Trailing comma in all list positions:** function params, struct fields, enum variants, array literal

**Expected output:**
```
magic = 42
wrapper = 10
single = 99
triple = 1 2 3
field_order: a=1 b=2
forward_struct_ok
trailing_commas_ok
```

---

## §5 — Expressions & Statements

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| `+`, `-`, `*`, `/` on i32 | `arithmetic.osc` |
| `%` modulo | `arithmetic.osc` |
| Unary `-` | `arithmetic.osc` |
| i64 arithmetic | `arithmetic.osc` |
| f64 arithmetic | `arithmetic.osc` |
| Operator precedence (basic) | `arithmetic.osc` |
| All comparison ops on i32 | `comparison.osc` |
| f64 comparisons | `comparison.osc` |
| `and`, `or`, `not` | `logical.osc` |
| Short-circuit (logical) | `logical.osc` |
| Assignment to mut | `mutability.osc` |
| Block expression | `block_expr.osc` |
| If/else as expression | `control_flow.osc` |
| While loop | `control_flow.osc` |
| For-in loop | `control_flow.osc` |
| Match on enum | `match_exhaustive.osc` |
| Match with wildcard | `match_exhaustive.osc` |
| Match on i32 literals | `match_exhaustive.osc` |
| Function calls | Everywhere |
| Field access | `structs_enums.osc` |
| Array indexing | `arrays.osc` |
| Struct literal | `structs_enums.osc` |
| Enum construction | `structs_enums.osc` |
| No compound assign | **neg:** `compound_assign.osc` |
| Non-bool condition | **neg:** `non_bool_condition.osc` |
| Non-exhaustive match | **neg:** `non_exhaustive_match.osc` |
| Type mismatch in ops | **neg:** `type_mismatch.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| `%` modulo on i64 | §5.1 — only i32 tested |
| Unary minus on i64 and f64 | §5.1 — only i32 unary minus tested |
| Operator precedence: `not` vs `and` vs `or` combined | §1.2 — `logical.osc` tests precedence partially |
| `as` cast precedence in expressions | §1.2 — not tested in complex expression |
| Deeply nested if-else-if chains | §5.6 — only simple chain tested |
| For-loop with computed start/end: `for i in a..b` where a, b are variables | §5.8 — only literal ranges tested |
| For-loop with range where start == end (zero iterations) | §5.8 — never tested |
| For-loop with start > end (zero iterations, half-open range) | §5.8 — never tested |
| Match: all arms must produce same type | §5.9 rule 4 — **no negative test** |
| Match on f64 literal | §5.9 — not tested |
| Match on bool literal | §5.9 — tested in `match_exhaustive.osc` |
| Assignment is statement, not expression | §5.4 — **no negative test for `x = y = 5`** |
| Block ending with `;` evaluates to unit | §5.5 — partially tested in `block_expr.osc` |
| Struct field mutation via `let mut` binding | §3.2 — tested in `mutability.osc` |
| Array element mutation `arr[i] = val` | §5.4 — tested in some mixed_alloc tests |
| Nested field access `a.b.c` | §5.11 — tested in `mixed_alloc_nested_dynamic.osc` |

### Proposed Tests → `spec_expressions.osc`
1. **Modulo on i64:** `let r: i64 = 17 as i64 % 5 as i64;` → 2 (but need to check if % works on i64)
2. **Unary minus on f64:** `let x: f64 = -3.14;` → print
3. **Unary minus on i64:** `let x: i64 = -(100 as i64);` → -100
4. **For-loop with variable range:** `let a: i32 = 2; let b: i32 = 5; for i in a..b { ... }` → prints 2,3,4
5. **For-loop zero iterations (start == end):** `for i in 5..5 { print("FAIL"); }` → prints nothing
6. **For-loop zero iterations (start > end):** `for i in 10..5 { print("FAIL"); }` → prints nothing
7. **Match on f64 literal:** `match x { 0.0 => "zero", 1.0 => "one", _ => "other" }`
8. **Deeply nested if-else-if:** 4-level chain returning different values
9. **Operator precedence combo:** `not true or false and true` → verify correct eval
10. **Block returning unit (ends with `;`):** Assign to unit type and verify no error
11. **Parenthesized expression:** `(((1 + 2)) * (3))` → 9
12. **Complex precedence:** `2 + 3 * 4 - 1` → 13 (multiply first)

**Expected output:**
```
mod_i64 = 2
neg_f64 = -3.140000
neg_i64 = -100
var_range: 2 3 4
zero_iter_ok
reverse_range_ok
match_f64 = one
nested_if = D
precedence_logic = true
unit_block_ok
parens = 9
complex_prec = 13
```

---

## §6 — Scoping Rules

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| Block scoping | `scope.osc` |
| For loop var scoped to body | `scope.osc` |
| Different functions reuse names | `scope.osc` |
| Match arm scoping | `scope.osc` |
| Anti-shadowing | **neg:** `shadowing.osc` |
| No global mutable state | **neg:** `global_mut.osc` |
| Order independence | `order_independence.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| Shadow in match arm vs outer | §6.2 — never tested (negative test) |
| Shadow in for-loop variable vs outer | §6.2 — never tested (negative test) |
| Variable declared after block not accessible inside | §6.1 — obvious but untested |
| Pattern binding scoped to match arm (positive: can reuse name in different arms) | §6.1/§6.2 — not tested |
| Variable lifetime: inner block var inaccessible after block | §6.3 — `scope.osc` tests this somewhat |

### Proposed Tests → `spec_scoping_errors.osc`
1. **Reuse name across different match arms:** Each arm binds `val` independently — verify this is legal
2. **Variable from inner block inaccessible outside:** Positive test confirming scoping
3. **For-loop variable not visible after loop:** Declare a different `i` after loop
4. **Nested blocks with different variable names:** Deep nesting (3 levels) with unique names
5. **Top-level const used inside function:** Verify access

**Expected output:**
```
arm_reuse: 10 20 30
inner_scope_ok
loop_var_gone_ok
nested_3_levels = 6
const_in_fn = 100
```

---

## §7 — Error Handling

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| `Result::Ok` / `Result::Err` construction | `error_handling.osc` |
| `try` propagation | `error_handling.osc` |
| `match` on Result | `error_handling.osc` |
| Cannot use Result as inner type directly | **neg:** `unhandled_result.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| `try` in non-Result-returning function | §7.4 — **no negative test** |
| `try` with dotted path: `try obj.method(x)` | §2.1 note 5 — never tested positively |
| Chained `try` (3+ in sequence) | §7.4 — only 2 chained in `error_handling.osc` |
| `Result` with non-str error type (custom enum) | §7.1 — documented but never tested |
| `main` returning `Result<unit, str>` | §4.1 — never tested |
| Error type mismatch in try (error types don't match) | §7.4 — no negative test |

### Proposed Tests → `spec_scoping_errors.osc` (grouped with §6)
6. **Chained try (3 operations):** Three `try` calls in sequence, all succeed
7. **Result with custom error enum:** `enum MyError { NotFound, Invalid(str) }` → `Result<i32, MyError>`
8. **try on dotted path (if supported):** May not be testable without methods; skip if not applicable

**Expected output additions:**
```
chained_try = 5
custom_error: NotFound
```

---

## §8 — Memory Model

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| Arena allocation (dynamic arrays) | `arena_growth.osc`, `arena_stress.osc`, `arena_stress_200k.osc` |
| Arena growth (multiple doublings) | `arena_growth.osc` |
| `push` on dynamic arrays | Many tests |
| String allocation on arena | `mixed_alloc_struct_with_string.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| `arena_reset()` in a small program | §8.2 — only large stress tests; no micro-lib test for `arena_reset()` |
| Panic on out-of-bounds array access | §8.7 — never tested (would require runtime panic test infrastructure) |

*Note: Runtime panic tests (overflow, bounds, division by zero) require infrastructure to test exit codes and stderr. These are out of scope for standard positive/negative tests but should be noted.*

---

## §9 — C-FFI

### Already Tested
| Spec Feature | Test File(s) |
|---|---|
| Extern block syntax | `ffi.osc` |
| Multiple extern blocks | `ffi_advanced.osc` |
| fn! wrapper around extern | `ffi_impure_wrapper.osc` |
| i32 and f64 extern types | `ffi.osc`, `ffi_advanced.osc` |
| Pure function cannot call extern | **neg:** `extern_in_pure.osc` |
| Duplicate extern declaration | **neg:** `extern_duplicate.osc` |

### NOT Tested — Gaps
| Gap | Spec Reference |
|---|---|
| Passing `str` to extern function | §9.2 — str→osc_str mapping never tested |
| `str_to_cstr` usage before C call | §9.3 — documented but never tested |
| Extern function returning void (unit) | §9.1 — never tested |

*FFI edge cases are hard to test without custom C code. The existing FFI tests using stdlib functions are adequate for the compiler. The `str_to_cstr` test is proposed below in §10.*

---

## §10 — Standard Library (Micro-Lib)

### Already Tested
| Function | Test File(s) |
|---|---|
| `print(s: str)` | `hello_world.osc`, many |
| `println(s: str)` | `hello_world.osc`, many |
| `print_i32(n: i32)` | `arithmetic.osc`, many |
| `print_i64(n: i64)` | `arithmetic.osc` |
| `print_f64(n: f64)` | `arithmetic.osc` |
| `print_bool(b: bool)` | `logical.osc` |
| `str_len(s: str)` | `strings.osc` |
| `str_eq(a, b)` | `strings.osc` |
| `str_concat(a, b)` | `strings.osc`, `mixed_alloc_struct_with_string.osc` |
| `i32_to_str(n)` | `arena_stress.osc` |
| `len(arr)` | `arrays.osc` |
| `push(arr, val)` | `arrays.osc`, arena tests |
| `arena_reset()` | `arena_stress.osc` (large-scale only) |

### NOT Tested — Gaps
| Function | Spec Reference |
|---|---|
| `read_line()` | §10.1 — interactive, hard to test in CI |
| `str_to_cstr(s)` | §10.2 — never tested |
| `abs_i32(n)` | §10.3 — never tested |
| `abs_f64(n)` | §10.3 — never tested |
| `mod_i32(a, b)` | §10.3 — never tested |
| `arena_reset()` in focused test | §10.6 — only in stress test, never isolated |

### Proposed Tests → `spec_microlib.osc`
1. **abs_i32:** `abs_i32(-42)` → 42, `abs_i32(0)` → 0, `abs_i32(7)` → 7
2. **abs_f64:** `abs_f64(-3.14)` → 3.14, `abs_f64(0.0)` → 0.0
3. **mod_i32:** `mod_i32(17, 5)` → 2, `mod_i32(10, 3)` → 1, `mod_i32(0, 7)` → 0
4. **str_to_cstr:** `str_to_cstr("hello")` — verify it returns a string (print it)
5. **str_len on empty string:** `str_len("")` → 0
6. **str_eq negative case:** `str_eq("abc", "def")` → false
7. **str_concat with empty:** `str_concat("", "hello")` → "hello", `str_concat("hello", "")` → "hello"
8. **i32_to_str:** `i32_to_str(0)` → "0", `i32_to_str(-99)` → "-99"
9. **arena_reset followed by new allocation:** Reset, then push to new array, verify it works
10. **len on fixed-size array:** `len([1,2,3,4,5])` — verify it returns 5
11. **push multiple elements:** Push 10 elements, verify len is 10

**Expected output:**
```
abs_i32: 42 0 7
abs_f64: 3.140000 0.000000
mod_i32: 2 1 0
str_to_cstr: hello
str_len_empty: 0
str_eq_false: false
concat_empty: hello hello
i32_to_str: 0 -99
arena_reset_ok
fixed_len: 5
push_10: 10
```

---

## Negative Test Files

### 7. `result_reserved_name.osc`
**Purpose:** Verify `Result` is a reserved type name (§3.3)

**Test cases:**
- Define `struct Result { value: i32 }` — should be compile error
- OR define `enum Result { A, B }` — should be compile error

**Why rejected:** The spec explicitly states "Users cannot define `struct Result { ... }` or `enum Result { ... }` — doing so is a compile error."

**Proposed code:**
```
struct Result {
    value: i32,
}

fn! main() {
    let r: Result = Result { value: 42 };
    print_i32(r.value);
}
```

---

### 8. `try_outside_result.osc`
**Purpose:** Verify `try` can only appear in a Result-returning function (§7.4)

**Test cases:**
- Function returning `i32` (not Result) uses `try divide(10, 2)` — should be compile error

**Why rejected:** "try can only appear inside a function that returns `Result<_, E>` where the error types are compatible."

**Proposed code:**
```
fn divide(a: i32, b: i32) -> Result<i32, str> {
    if b == 0 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}

fn compute(a: i32, b: i32) -> i32 {
    try divide(a, b)
}

fn! main() {
    let r: i32 = compute(10, 2);
    print_i32(r);
}
```

---

### 9. `assignment_expression.osc`
**Purpose:** Verify assignment is a statement, not an expression (§5.4)

**Test cases:**
- `x = y = 5;` — chained assignment should be compile error
- OR `let z: i32 = (x = 5);` — assignment as expression should be compile error

**Why rejected:** "Assignment is a statement, not an expression. `x = y = 5` is a compile error."

**Proposed code:**
```
fn! main() {
    let mut x: i32 = 0;
    let mut y: i32 = 0;
    x = y = 5;
}
```

---

### 10. `match_type_mismatch.osc`
**Purpose:** Verify match arms must all produce the same type (§5.9 rule 4)

**Test cases:**
- Match where one arm returns `i32` and another returns `str` — should be compile error

**Why rejected:** "All arms must produce the same type."

**Proposed code:**
```
fn! main() {
    let x: i32 = 5;
    let r: i32 = match x {
        0 => 0,
        1 => "one",
        _ => 2,
    };
    print_i32(r);
}
```

---

## Coverage Matrix Summary

| Spec Section | Existing Positive | Existing Negative | Proposed Positive | Proposed Negative |
|---|---|---|---|---|
| §1 Keywords & Tokens | 12 files | 1 (comparison_chain) | `spec_tokens_syntax.osc` | — |
| §2 Grammar | 10+ files | — | `spec_tokens_syntax.osc` | — |
| §3 Type System | 8 files | 3 (implicit, mixed, missing_type) | `spec_types_casts.osc` | `result_reserved_name.osc` |
| §4 Declarations | 10 files | 5 (purity, extern, global, immut, dup) | `spec_declarations.osc` | — |
| §5 Expressions | 10+ files | 4 (compound, non_bool, non_exhaust, type_mismatch) | `spec_expressions.osc` | `assignment_expression.osc`, `match_type_mismatch.osc` |
| §6 Scoping | 2 files | 2 (shadowing, global_mut) | `spec_scoping_errors.osc` | — |
| §7 Error Handling | 1 file | 1 (unhandled_result) | `spec_scoping_errors.osc` | `try_outside_result.osc` |
| §8 Memory | 6 files | — | — (arena_reset in microlib) | — |
| §9 C-FFI | 3 files | 2 (extern_dup, extern_pure) | — | — |
| §10 Micro-Lib | partial | — | `spec_microlib.osc` | — |

---

## Priority Ranking

**High Priority (spec rules with zero test coverage):**
1. `Result` as reserved name → `result_reserved_name.osc`
2. `try` outside Result function → `try_outside_result.osc`
3. Assignment as expression → `assignment_expression.osc`
4. Match arm type mismatch → `match_type_mismatch.osc`
5. `abs_i32`, `abs_f64`, `mod_i32`, `str_to_cstr` → `spec_microlib.osc`
6. Negative literal patterns in match → `spec_tokens_syntax.osc`
7. For-loop edge cases (empty range, variable range) → `spec_expressions.osc`

**Medium Priority (partially tested, edge cases missing):**
8. Cast in complex expressions → `spec_types_casts.osc`
9. Empty struct construction → `spec_types_casts.osc`
10. Enum tag equality → `spec_types_casts.osc`
11. Escape sequences beyond `\t` and `\n` → `spec_tokens_syntax.osc`
12. Explicit `return` statement → `spec_tokens_syntax.osc`

**Low Priority (nice-to-have, mostly covered by existing tests):**
13. Trailing commas in all positions → `spec_declarations.osc`
14. Block comments → `spec_tokens_syntax.osc`
15. Zero-parameter function → `spec_declarations.osc`
