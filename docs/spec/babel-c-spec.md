# Babel-C Language Specification v0.1

**Status:** DEFINITIVE — This is the sole reference for compiler, runtime, and test implementation.

**Authors:** Neo (Lead Architect)

**Date:** 2025-07-14

---

## Table of Contents

1. [Keywords & Tokens](#1-keywords--tokens)
2. [Grammar (EBNF)](#2-grammar-ebnf)
3. [Type System](#3-type-system)
4. [Declarations](#4-declarations)
5. [Expressions & Statements](#5-expressions--statements)
6. [Scoping Rules](#6-scoping-rules)
7. [Error Handling](#7-error-handling)
8. [Memory Model](#8-memory-model)
9. [C-FFI](#9-c-ffi)
10. [Standard Library (Micro-Lib)](#10-standard-library-micro-lib)
11. [Example Programs](#11-example-programs)
12. [Compiler Architecture Overview](#12-compiler-architecture-overview)

---

## 1. Keywords & Tokens

### 1.1 Reserved Words (21 total)

| Keyword   | Purpose                                      |
|-----------|----------------------------------------------|
| `fn`      | Pure function declaration                    |
| `fn!`     | Side-effecting function declaration          |
| `let`     | Immutable binding                            |
| `mut`     | Mutable modifier (used with `let`)           |
| `struct`  | Nominal struct type declaration              |
| `enum`    | Tagged union declaration                     |
| `if`      | Conditional expression                       |
| `else`    | Else branch of conditional                   |
| `while`   | While loop                                   |
| `for`     | For-in loop                                  |
| `in`      | Range iteration binding                      |
| `match`   | Pattern match expression                     |
| `return`  | Early return from function                   |
| `try`     | Error propagation                            |
| `extern`  | C-FFI declaration block                      |
| `as`      | Explicit type cast                           |
| `and`     | Logical AND                                  |
| `or`      | Logical OR                                   |
| `not`     | Logical NOT                                  |
| `true`    | Boolean literal                              |
| `false`   | Boolean literal                              |

**Note:** The table above lists 21 reserved words. `fn!` is lexed as a single token (`FN_BANG`), not `fn` followed by `!`. `true` and `false` are reserved keywords, not identifiers. `_` is additionally reserved as a wildcard in `match` patterns (see §1.3). `mut` is only valid immediately after `let`; it is not a standalone keyword elsewhere. `Result` is a reserved type name and cannot be used for user-defined struct or enum names (see §3.3).

### 1.2 Operators and Precedence

Precedence from **highest** (tightest binding) to **lowest**:

| Prec | Operator(s)            | Associativity | Description                    |
|------|------------------------|---------------|--------------------------------|
| 11   | `(` `)` (grouping)    | —             | Parenthesized expression       |
| 10   | `f(args)` (call)      | Left          | Function call                  |
| 10   | `a[i]` (index)        | Left          | Array indexing                 |
| 10   | `a.field` (access)    | Left          | Field access                   |
| 9    | `as`                  | Left          | Type cast (e.g., `x as i64`)  |
| 8    | `not`                 | Prefix        | Logical negation               |
| 8    | `-` (unary)           | Prefix        | Arithmetic negation            |
| 7    | `*` `/` `%`           | Left          | Multiplication, division, remainder |
| 6    | `+` `-`               | Left          | Addition, subtraction          |
| 5    | `<` `>` `<=` `>=`     | Left (non-chaining) | Relational comparison   |
| 4    | `==` `!=`             | Left (non-chaining) | Equality                |
| 3    | `and`                 | Left          | Logical AND (short-circuit)    |
| 2    | `or`                  | Left          | Logical OR (short-circuit)     |
| 1    | `=`                   | Right         | Assignment (statement only)    |

**Comparison chaining is forbidden.** `a < b < c` is a compile error. Write `a < b and b < c`.

### 1.3 Punctuation & Delimiters

| Symbol  | Purpose                                          |
|---------|--------------------------------------------------|
| `{` `}` | Block delimiters                                |
| `(` `)` | Grouping, function parameters, tuple-like syntax |
| `[` `]` | Array type, array literal, array indexing        |
| `,`     | Separator for parameters, arguments, fields      |
| `:`     | Type annotation separator                        |
| `;`     | Statement terminator                             |
| `.`     | Field access                                     |
| `..`    | Range operator (in `for` loops only)             |
| `->`    | Return type annotation                           |
| `<` `>` | Generic type parameters (only for `Result`)      |
| `_`     | Wildcard in match patterns                       |

### 1.4 Identifiers

```
identifier     = letter (letter | digit | '_')* ;
letter         = 'a'..'z' | 'A'..'Z' | '_' ;
digit          = '0'..'9' ;
```

Rules:
- Identifiers are case-sensitive.
- Identifiers must not collide with any keyword.
- Identifiers beginning with `_` followed by at least one letter/digit are permitted (for unused bindings).
- The bare `_` is reserved as a wildcard pattern in `match`.
- Convention: `snake_case` for functions and variables, `PascalCase` for types. The compiler does NOT enforce convention, but the standard library follows it.

### 1.5 Literals

#### Integer Literals

```
int_literal    = digit+ ;
```

- No leading zeros except `0` itself.
- No hex, octal, or binary prefixes. Decimal only.
- No underscores in numeric literals.
- Integer literals default to `i32`. Use an explicit cast for other integer types: `42 as i64`.

#### Float Literals

```
float_literal  = digit+ '.' digit+ ;
```

- Must have digits on both sides of the decimal point. `.5` and `5.` are invalid.
- No scientific notation.
- Float literals are always `f64`.

#### String Literals

```
string_literal = '"' string_char* '"' ;
string_char    = any_unicode_except_backslash_or_quote | escape_seq ;
escape_seq     = '\\' ('n' | 't' | 'r' | '\\' | '"' | '0') ;
```

- Strings are UTF-8 encoded.
- No raw strings, no multi-line strings, no string interpolation.
- String literals have type `str`.

#### Boolean Literals

```
bool_literal   = 'true' | 'false' ;
```

### 1.6 Comments

```
line_comment   = '//' any_char_except_newline* newline ;
block_comment  = '/*' any_char* '*/' ;
```

- Block comments do NOT nest. `/* /* */ */` is a compile error (the first `*/` ends the comment, then `*/` is stray tokens).
- Comments are stripped during lexing and do not appear in the AST.

### 1.7 Whitespace

- Spaces, tabs, newlines, and carriage returns are whitespace.
- Whitespace is insignificant except as token separator.
- No indentation sensitivity.

---

## 2. Grammar (EBNF)

The following is the complete formal grammar. The notation uses:
- `|` for alternatives
- `*` for zero or more repetitions
- `+` for one or more repetitions
- `?` for optional
- `'keyword'` for terminal keywords/symbols
- `UPPER_CASE` for tokens from the lexer

```ebnf
(* === Top Level === *)

program          = top_decl* ;

top_decl         = fn_decl
                 | struct_decl
                 | enum_decl
                 | let_decl
                 | extern_block ;

(* === Function Declaration === *)

fn_decl          = fn_keyword IDENT '(' param_list? ')' return_type? block ;
fn_keyword       = 'fn' | 'fn!' ;
param_list       = param (',' param)* ','? ;
param            = IDENT ':' type ;
return_type      = '->' type ;

(* === Struct Declaration === *)

struct_decl      = 'struct' IDENT '{' field_list? '}' ;  (* empty structs are permitted; see §4.3 *)
field_list       = field (',' field)* ','? ;
field            = IDENT ':' type ;

(* === Enum Declaration === *)

enum_decl        = 'enum' IDENT '{' variant_list '}' ;
variant_list     = variant (',' variant)* ','? ;
variant          = IDENT ( '(' type_list ')' )? ;
type_list        = type (',' type)* ','? ;

(* === Top-level Let (const only) === *)

let_decl         = 'let' IDENT ':' type '=' expr ';' ;

(* === Extern Block === *)

extern_block     = 'extern' '{' extern_fn_decl* '}' ;
extern_fn_decl   = 'fn!' IDENT '(' param_list? ')' return_type? ';' ;

(* === Types === *)

type             = primitive_type
                 | array_type
                 | result_type
                 | named_type ;

primitive_type   = 'i32' | 'i64' | 'f64' | 'bool' | 'str' | 'unit' ;
array_type       = '[' type ';' INT_LIT ']'     (* fixed-size *)
                 | '[' type ']' ;                 (* dynamic    *)
named_type       = IDENT ;
result_type      = 'Result' '<' type ',' type '>' ;

(* === Statements === *)

block            = '{' stmt* expr? '}' ;

stmt             = let_stmt
                 | assign_stmt
                 | expr_stmt
                 | while_stmt
                 | for_stmt
                 | return_stmt ;

let_stmt         = 'let' 'mut'? IDENT ':' type '=' expr ';' ;
assign_stmt      = place '=' expr ';' ;
expr_stmt        = expr ';' ;
while_stmt       = 'while' expr block ';'? ;
for_stmt         = 'for' IDENT 'in' expr '..' expr block ';'? ;
return_stmt      = 'return' expr? ';' ;

place            = IDENT ( '.' IDENT | '[' expr ']' )* ;

(* === Expressions === *)

expr             = or_expr ;

or_expr          = and_expr ('or' and_expr)* ;
and_expr         = equality_expr ('and' equality_expr)* ;
equality_expr    = relational_expr (('==' | '!=') relational_expr)? ;
relational_expr  = additive_expr (('<' | '>' | '<=' | '>=') additive_expr)? ;
additive_expr    = mult_expr (('+' | '-') mult_expr)* ;
mult_expr        = unary_expr (('*' | '/' | '%') unary_expr)* ;
unary_expr       = ('not' | '-') unary_expr | cast_expr ;
cast_expr        = postfix_expr ('as' type)? ;
postfix_expr     = primary_expr (call_suffix | index_suffix | field_suffix)* ;
call_suffix      = '(' arg_list? ')' ;
index_suffix     = '[' expr ']' ;
field_suffix     = '.' IDENT ;
arg_list         = expr (',' expr)* ','? ;

primary_expr     = INT_LIT
                 | FLOAT_LIT
                 | STRING_LIT
                 | 'true'
                 | 'false'
                 | IDENT
                 | block_expr
                 | if_expr
                 | match_expr
                 | try_expr
                 | array_literal
                 | struct_literal
                 | enum_constructor
                 | '(' expr ')' ;

block_expr       = block ;
if_expr          = 'if' expr block ('else' (if_expr | block))? ;
match_expr       = 'match' expr '{' match_arm+ '}' ;
match_arm        = pattern '=>' expr ','  ;
try_expr         = 'try' IDENT ('.' IDENT)* '(' arg_list? ')' ;

array_literal    = '[' (expr (',' expr)* ','?)? ']' ;

struct_literal   = IDENT '{' (field_init (',' field_init)* ','?)? '}' ;
field_init       = IDENT ':' expr ;

enum_constructor = IDENT '::' IDENT ('(' arg_list ')')? ;

(* === Patterns === *)

pattern          = '_'
                 | IDENT
                 | literal_pattern
                 | enum_pattern ;

literal_pattern  = '-'? (INT_LIT | FLOAT_LIT) | STRING_LIT | 'true' | 'false' ;
enum_pattern     = IDENT '::' IDENT ('(' pattern_list ')')? ;
pattern_list     = pattern (',' pattern)* ','? ;
```

### 2.1 Grammar Notes

1. **Single-pass parseable:** The grammar is LL(2) — at most 2 tokens of lookahead are needed. The parser can distinguish between an expression statement and a let statement by the leading keyword. Struct literals vs. blocks are resolved by checking if the identifier before `{` is a known type name (resolved via the two-pass name collection).

2. **No ambiguity:** `fn!` is a single token (not `fn` followed by `!`). The lexer handles this.

3. **Trailing commas:** Allowed everywhere lists appear (parameters, arguments, fields, variants, match arms). This improves LLM generation consistency.

4. **Semicolons:** Required on all statements. The last expression in a block (the "tail expression") does NOT have a semicolon — it becomes the block's value. If the block ends with a semicolon, the block has type `unit`. **Exception:** Trailing semicolons are optional for `while`, `for`, `if`, and `match` when used as statements (since these constructs end with a closing `}`). The parser accepts the semicolon if present but does not require it.

5. **`try` syntax:** `try` is a prefix on a function call. The grammar restricts `try` to a dotted name path followed by arguments: `try foo(x)` or `try obj.method(x)`. This avoids ambiguity with `postfix_expr` greedily consuming the call suffix. `try foo(x)` calls `foo(x)` which must return `Result<T, E>`, and either unwraps to `T` or causes an early return of the error. The enclosing function must also return `Result<_, E>` with a compatible error type.

6. **Negative literal patterns:** Match patterns allow an optional leading `-` on integer and float literals (e.g., `match x { -1 => "neg", _ => "other" }`). The `-` is part of the pattern syntax, not a unary operator.

---

## 3. Type System

### 3.1 Primitive Types

| Type   | Size    | Description                    | C Mapping       |
|--------|---------|--------------------------------|-----------------|
| `i32`  | 32 bits | Signed integer                 | `int32_t`       |
| `i64`  | 64 bits | Signed integer                 | `int64_t`       |
| `f64`  | 64 bits | IEEE 754 double                | `double`        |
| `bool` | 8 bits  | Boolean (true/false only)      | `uint8_t` (0/1) |
| `str`  | ptr+len | UTF-8 string (immutable view)  | `bc_str`        |
| `unit` | 0 bits  | No value (void equivalent)     | `void`          |

### 3.2 Composite Types

#### Structs (Nominal)

Structs are nominal — two structs with identical fields but different names are distinct types.

```
struct Point {
    x: f64,
    y: f64,
}
```

- All fields are immutable by default.
- Mutation of a struct field requires the binding holding the struct to be declared `let mut`.
- Structs cannot be recursive (no self-referential fields). Use enums with boxed variants for recursive structures (see §3.5).
- No methods. All operations on structs are free functions.
- No inheritance, no traits, no interfaces.

#### Enums (Tagged Unions)

Enums are nominal tagged unions. Each variant can carry zero or more payload values.

```
enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Empty,
}
```

- Enums are the ONLY way to represent sum types.
- Every `match` on an enum must be exhaustive (all variants covered) or include a `_` wildcard.
- Enum variants are constructed via `EnumName::VariantName(args)`.

### 3.3 Result Type

`Result<T, E>` is a built-in generic type. It is semantically equivalent to:

```
enum Result {
    Ok(T),
    Err(E),
}
```

But it is special-cased in the compiler for `try` support.

- `Result` is a **reserved type name**. Users cannot define `struct Result { ... }` or `enum Result { ... }` — doing so is a compile error.
- `Result::Ok(value)` constructs the success variant.
- `Result::Err(error)` constructs the error variant.
- You cannot access the inner value without `match` or `try`. There is no `.ok` field, `.err` field, or `.unwrap()` method.

### 3.4 Array Types

#### Fixed-size Arrays

```
let xs: [i32; 5] = [1, 2, 3, 4, 5];
```

- Size `N` must be a compile-time integer literal.
- All elements must be the same type.
- Stored inline (on stack or in enclosing struct).

#### Dynamic Arrays

```
let xs: [i32] = [1, 2, 3];
```

- Heap-allocated, growable.
- Managed by the arena allocator (see §8).
- Created via array literals or standard library functions.

#### Array Operations

- `a[i]` — indexing. Bounds-checked at runtime; out-of-bounds is a runtime panic (program terminates with error message).
- `len(a)` — returns `i32`, the number of elements.
- `push(a, val)` — appends to dynamic array (mutates, requires `mut` binding). Available via micro-lib.
- Fixed-size arrays do NOT support `push`.

### 3.5 Boxed Types for Recursion

For recursive data structures, dynamic arrays serve as the indirection mechanism:

```
enum Tree {
    Leaf(i32),
    Node([Tree]),
}
```

The dynamic array `[Tree]` provides the heap indirection needed for recursion. There is no separate `Box` type — dynamic arrays handle all heap allocation needs.

### 3.6 No Type Aliases

There are no type aliases. Every type must be referred to by its full name. This maximizes explicitness.

### 3.7 Explicit Casts

The `as` keyword performs explicit type conversion. Only the following casts are permitted:

| From  | To    | Semantics                                      |
|-------|-------|-------------------------------------------------|
| `i32` | `i64` | Widening (lossless)                             |
| `i64` | `i32` | Narrowing (runtime checked — panics on overflow)|
| `i32` | `f64` | Integer to float (lossless for i32 range)       |
| `i64` | `f64` | Integer to float (may lose precision)           |
| `f64` | `i32` | Truncation (runtime checked — panics if NaN, Inf, or out of range) |
| `f64` | `i64` | Truncation (runtime checked — panics if NaN, Inf, or out of range) |

All other casts are compile errors. There are NO implicit coercions whatsoever.

### 3.8 Type Checking Rules

1. **All bindings must have explicit type annotations.** No type inference. This is a deliberate choice for LLM clarity — the type of every binding is visible at its declaration site.

2. **Function return types are mandatory** unless the function returns `unit`, in which case the `-> type` annotation is omitted.

3. **No subtyping.** `i32` is not a subtype of `i64`. You must cast explicitly.

4. **No generics** except the built-in `Result<T, E>`. User-defined generic types and functions are not supported. This keeps the type system simple and eliminates monomorphization complexity.

5. **Structural equality for type checking** of primitives. **Nominal equality** for structs and enums.

---

## 4. Declarations

### 4.1 Function Declarations

#### Pure Functions (`fn`)

```
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

- May not call `fn!` functions.
- May not perform I/O.
- May not mutate anything outside their parameters.
- The compiler enforces purity by verifying: no calls to `fn!`, no calls to `extern` functions, no mutation of variables captured from outer scope (there is no closure capture — functions are not closures).

#### Side-Effecting Functions (`fn!`)

```
fn! greet(name: str) {
    print(name);
}
```

- May call both `fn` and `fn!` functions.
- May perform I/O, mutate mutable bindings.
- All micro-lib I/O functions are `fn!`.

#### Rules Applying to Both

- Functions are NOT closures. They cannot capture variables from enclosing scopes.
- Functions are NOT first-class values. You cannot pass a function as an argument or return one.
- All parameters are passed by value. Structs and dynamic arrays are passed by reference under the hood in the C output for efficiency, but semantically they behave as immutable copies (the callee cannot mutate the caller's data unless the parameter is explicitly `mut`). **Clarification:** parameters are immutable within the function body by default. A `mut` parameter annotation is not supported — instead, the function should take the value and the caller binds the return.
- Recursion is permitted.
- The `main` function is the program entry point. Its signature must be `fn! main() { ... }` (no parameters, returns unit). For programs that use error handling at the top level, `fn! main() -> Result<unit, str> { ... }` is also accepted.

### 4.2 Variable Bindings

#### Immutable Binding

```
let x: i32 = 42;
```

- `x` cannot be reassigned.
- The type annotation is mandatory.
- The initializer expression is mandatory. There are no uninitialized variables.

#### Mutable Binding

```
let mut counter: i32 = 0;
counter = counter + 1;
```

- `counter` can be reassigned.
- Only `let mut` bindings can appear on the left side of `=` assignment.

#### Top-Level Bindings

```
let PI: f64 = 3.14159265358979;
```

- Top-level `let` bindings are always immutable (no `mut` at top level).
- The initializer must be a compile-time constant expression (literal, or arithmetic on literals).
- These become `static const` in the generated C.

### 4.3 Struct Declarations

```
struct Color {
    r: i32,
    g: i32,
    b: i32,
}
```

- Declared at top level only.
- Fields are accessed with dot notation: `color.r`.
- Constructed with struct literal syntax: `Color { r: 255, g: 0, b: 0 }`.
- All fields must be specified in the literal (no defaults).
- Field order in the literal does not need to match declaration order.
- **Empty structs** (`struct Void {}`) are grammatically valid. An empty struct has a distinct nominal type and is constructed with `Void {}`. Its runtime size is implementation-defined (typically 0 or 1 byte in the generated C).

### 4.4 Enum Declarations

```
enum Option {
    Some(i32),
    None,
}
```

- Declared at top level only.
- Variants can have zero or more payload fields.
- Constructed via `Option::Some(42)` or `Option::None`.
- Destructured via `match`.

### 4.5 Extern Declarations

```
extern {
    fn! malloc(size: i64) -> i64;
    fn! free(ptr: i64);
    fn! printf(fmt: str) -> i32;
}
```

- All extern functions are implicitly `fn!` (side-effecting).
- Extern blocks declare C functions available for linking.
- Type mappings must use Babel-C types (see §9 for the mapping table).
- The compiler generates appropriate C `#include` and function declarations.

---

## 5. Expressions & Statements

### 5.1 Arithmetic Expressions

All arithmetic is **checked for overflow** at runtime.

| Operator | Types          | Result Type | Behavior on Overflow          |
|----------|----------------|-------------|-------------------------------|
| `+`      | i32, i64, f64  | Same        | Runtime panic (integer); IEEE 754 (float) |
| `-`      | i32, i64, f64  | Same        | Runtime panic (integer); IEEE 754 (float) |
| `*`      | i32, i64, f64  | Same        | Runtime panic (integer); IEEE 754 (float) |
| `/`      | i32, i64, f64  | Same        | Panic on division by zero (integer); IEEE 754 (float) |
| `%`      | i32, i64       | Same        | Panic on division by zero     |
| `-` (unary) | i32, i64, f64 | Same      | Panic on `MIN_VALUE` negate (integer) |

- Mixed-type arithmetic is a compile error. `1 + 1.0` does not compile. Write `(1 as f64) + 1.0`.

### 5.2 Comparison Expressions

| Operator | Types         | Result Type |
|----------|---------------|-------------|
| `==`     | All primitives, enums (tag only) | `bool` |
| `!=`     | All primitives, enums (tag only) | `bool` |
| `<`      | i32, i64, f64 | `bool`     |
| `>`      | i32, i64, f64 | `bool`     |
| `<=`     | i32, i64, f64 | `bool`     |
| `>=`     | i32, i64, f64 | `bool`     |

- Comparing different types is a compile error.
- Structs cannot be compared with `==`; write a comparison function.
- Enum equality (`==`, `!=`) compares only the variant tag, not payloads. For deep comparison, use `match`.

### 5.3 Logical Expressions

| Operator | Operand Type | Result Type | Behavior           |
|----------|-------------|-------------|---------------------|
| `and`    | `bool`      | `bool`      | Short-circuit AND   |
| `or`     | `bool`      | `bool`      | Short-circuit OR    |
| `not`    | `bool`      | `bool`      | Logical negation    |

- `and` and `or` are keywords, not symbols. This is deliberate for LLM tokenizer clarity.
- Operands must be `bool`. `0 and 1` is a compile error.

### 5.4 Assignment

```
counter = counter + 1;
```

- Only valid for `let mut` bindings.
- Assigning to an immutable binding is a compile error.
- There is no compound assignment (`+=`, `-=`, etc.). Write it out explicitly.
- Assignment is a statement, not an expression. `x = y = 5` is a compile error.

### 5.5 Block Expressions

```
let result: i32 = {
    let a: i32 = 10;
    let b: i32 = 20;
    a + b
};
```

- A block `{ ... }` is an expression whose value is its last expression (without semicolon).
- If the last item in a block has a semicolon, the block evaluates to `unit`.
- Variables declared in a block are scoped to that block.

### 5.6 If/Else Expression

```
let max: i32 = if a > b { a } else { b };
```

- `if` is an expression, not a statement. It always produces a value.
- When used as an expression, both branches must have the same type.
- When used as a statement (value discarded), `else` is optional and both branches must have type `unit`.
- The condition must be type `bool`. No truthy/falsy conversions.
- Braces are always required, even for single expressions.

```
if condition {
    do_something();
};

if condition {
    value_a
} else if other_condition {
    value_b
} else {
    value_c
}
```

### 5.7 While Loop

```
while condition {
    body;
}
```

- The condition must be `bool`.
- The body is a block.
- `while` is a statement that produces `unit`.
- There is no `break` or `continue`. Use a mutable boolean flag if needed.

### 5.8 For-In Loop

```
for i in 0..10 {
    body;
}
```

- `for` iterates over a half-open integer range: `start..end` where `start` is inclusive, `end` is exclusive.
- Both `start` and `end` must be `i32` expressions.
- The loop variable `i` is immutable within the body and has type `i32`.
- The loop variable is scoped to the loop body.
- `for` is a statement that produces `unit`.
- There is no iteration over arrays directly. Use `for i in 0..len(arr) { ... arr[i] ... }`.

### 5.9 Match Expression

```
let description: str = match shape {
    Shape::Circle(r) => "circle",
    Shape::Rectangle(w, h) => "rectangle",
    Shape::Empty => "empty",
};
```

Rules:
1. **Exhaustiveness:** Every enum variant must be covered, OR a `_` wildcard must be present.
2. **No fallthrough:** Each arm is independent.
3. **Pattern bindings:** Variables bound in patterns (like `r`, `w`, `h` above) are scoped to that arm's expression.
4. **Return type:** All arms must produce the same type.
5. **Match on Result:**
   ```
   match result {
       Result::Ok(val) => use_val(val),
       Result::Err(e) => handle_error(e),
   }
   ```
6. **Literal patterns:**
   ```
   match x {
       0 => "zero",
       1 => "one",
       _ => "other",
   }
   ```
   Literal patterns work on `i32`, `i64`, `f64`, `bool`, `str`.

7. **Each match arm's expression is followed by a comma.** This is mandatory.

### 5.10 Function Calls

```
let result: i32 = add(1, 2);
```

- No named arguments.
- No default arguments.
- No variadic arguments (except via C-FFI).
- Argument types must exactly match parameter types.

### 5.11 Field Access

```
let x: f64 = point.x;
```

- Dot notation only for struct fields.
- No method syntax. There is no `point.distance(other)` — write `distance(point, other)`.

### 5.12 Array Indexing

```
let first: i32 = arr[0];
```

- Index must be type `i32`.
- Bounds-checked at runtime. Out-of-bounds access is a runtime panic.

### 5.13 Struct Literals

```
let p: Point = Point { x: 1.0, y: 2.0 };
```

- All fields must be initialized.
- No shorthand (no `Point { x, y }` where x and y are variables with matching names).
- Field order does not matter.

### 5.14 Enum Construction

```
let s: Shape = Shape::Circle(5.0);
let e: Shape = Shape::Empty;
```

- Use `TypeName::VariantName(args)` syntax.
- Variant with no payload: `TypeName::VariantName` (no parens).

---

## 6. Scoping Rules

### 6.1 Strict Lexical Scoping

- Every block `{ }` creates a new scope.
- Variables are visible from their declaration point to the end of their enclosing block.
- Function parameters are scoped to the function body.
- Loop variables in `for` are scoped to the loop body.
- Pattern bindings in `match` arms are scoped to the arm's expression.

### 6.2 Anti-Shadowing Rule

**Declaring a variable with the same name as any variable visible in any enclosing scope is a compile error.**

```
let x: i32 = 1;
{
    let x: i32 = 2;   // COMPILE ERROR: 'x' already declared in outer scope
}
```

This applies across all scope boundaries:
- Function parameter names vs. local variables: OK (parameters ARE the local scope).
- Nested blocks: shadowing is forbidden.
- Match arm bindings vs. outer scope: shadowing is forbidden.
- Loop variable vs. outer scope: shadowing is forbidden.

The anti-shadowing rule does NOT apply across different functions. Two different functions can each have a local variable named `x`.

### 6.3 Variable Lifetime

- A variable's lifetime is exactly its enclosing block.
- When a block exits, all its local variables become inaccessible.
- For heap-allocated data (dynamic arrays, structs stored on the arena), the memory is freed when the arena is released (see §8), not when the variable goes out of scope.

### 6.4 No Global Mutable State

- All top-level `let` bindings are immutable constants.
- `let mut` at the top level is a compile error.
- Mutable state exists only within function bodies.

### 6.5 Order Independence

- Top-level declarations (functions, structs, enums, constants) can appear in any order.
- A function can call another function defined later in the file.
- A struct can reference an enum defined later in the file.
- The compiler achieves this via a two-pass approach (see §12).

---

## 7. Error Handling

### 7.1 The Result Type

```
enum Result {
    Ok(T),
    Err(E),
}
```

Any function that can fail returns `Result<T, E>` where:
- `T` is the success type
- `E` is the error type (typically `str` for simple programs, or a custom enum)

### 7.2 Constructing Results

```
fn divide(a: i32, b: i32) -> Result<i32, str> {
    if b == 0 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}
```

### 7.3 Handling Results with `match`

```
fn! print_result(r: Result<i32, str>) {
    match r {
        Result::Ok(val) => print_i32(val),
        Result::Err(msg) => print(msg),
    };
}
```

### 7.4 Propagating Errors with `try`

```
fn compute(a: i32, b: i32, c: i32) -> Result<i32, str> {
    let ab: i32 = try divide(a, b);
    let result: i32 = try divide(ab, c);
    Result::Ok(result)
}
```

`try` semantics:
1. Evaluate the function call.
2. If the result is `Result::Ok(val)`, unwrap and produce `val`.
3. If the result is `Result::Err(e)`, immediately return `Result::Err(e)` from the enclosing function.

Constraints:
- `try` can only appear inside a function that returns `Result<_, E>` where the error types are compatible.
- `try` applies only to function call expressions, not arbitrary expressions.
- The error type `E` of the called function must exactly match the error type of the enclosing function.

### 7.5 What You Cannot Do

```
let r: Result<i32, str> = divide(10, 0);
let val: i32 = r;              // COMPILE ERROR: cannot use Result as i32
// There is no .unwrap(), .ok, .err, or any way to extract without match/try
```

---

## 8. Memory Model

### 8.1 Decision: Arena-Based Allocation with Scope-Based Deallocation

**Chosen approach:** Every Babel-C program uses a **single implicit arena allocator**. All heap allocations go to this arena. Deallocation happens at arena granularity, not per-object.

**Justification:**
- **Deterministic:** No garbage collector pauses, no reference counting cycles.
- **Uniform:** One allocation mechanism, no choice paralysis for LLMs.
- **Simple for LLMs:** No `malloc`/`free` pairs to match. No lifetime annotations. No ownership system.
- **Zero UB:** No use-after-free (arena lives for the program duration by default), no double-free.

### 8.2 Arena Mechanics

1. **Program arena:** A single arena is created at program start (`main`). All heap allocations (dynamic arrays, large structs) are allocated from this arena.

2. **Allocation:** When the compiler emits code that needs heap memory (e.g., creating a `[T]` dynamic array or growing one via `push`), it calls the runtime's `bc_arena_alloc(arena, size)`.

3. **Deallocation:** The arena is freed in bulk when the program exits. For long-running programs, the programmer can use `arena_reset()` from the micro-lib to reclaim all arena memory (invalidating all dynamic arrays — the programmer must ensure no references are used after reset).

4. **Stack allocation:** Primitives, fixed-size arrays, and small structs are stack-allocated. The threshold for "small" is implementation-defined; the compiler determines placement based on size. This is an optimization detail that does not affect program semantics.

### 8.3 What the LLM Writes

- **Nothing regarding memory.** The LLM writes standard Babel-C code. All memory management is implicit.
- Dynamic arrays are created via literals or `push` and automatically use the arena.
- The LLM never writes `alloc`, `free`, `new`, or `delete`.
- The only memory-related function exposed is `arena_reset()` for advanced use.

### 8.4 Runtime Representation (for Morpheus)

The runtime library provides:

```c
typedef struct {
    uint8_t* data;
    size_t   used;
    size_t   capacity;
} bc_arena;

bc_arena* bc_arena_create(size_t initial_capacity);
void*     bc_arena_alloc(bc_arena* arena, size_t size);
void      bc_arena_reset(bc_arena* arena);
void      bc_arena_destroy(bc_arena* arena);
```

- Default initial capacity: 1 MB.
- Growth strategy: double when full.
- All generated C code receives the arena as a hidden first parameter to functions that allocate.

### 8.5 String Representation

Strings (`str`) are immutable UTF-8 byte sequences stored in the arena:

```c
typedef struct {
    const char* data;
    int32_t     len;
} bc_str;
```

- String literals are stored in static data (not the arena).
- String concatenation creates a new arena-allocated string.

### 8.6 Dynamic Array Representation

```c
typedef struct {
    void*   data;
    int32_t len;
    int32_t capacity;
    int32_t elem_size;
} bc_array;
```

- `push` grows the array within the arena (may relocate).
- Arrays carry their length for bounds checking.

### 8.7 Panic Behavior

When a runtime error occurs (overflow, bounds check, division by zero, failed cast):
1. Print an error message to stderr with source location (file:line).
2. Call `exit(1)`.

There is no recovery from panics. They are not errors-as-values — they indicate programming bugs (like assertions).

---

## 9. C-FFI

### 9.1 Extern Block Syntax

```
extern {
    fn! c_function_name(param1: type1, param2: type2) -> return_type;
}
```

- All extern functions are `fn!` (side-effecting).
- The Babel-C function name must exactly match the C function name.
- Multiple extern blocks are allowed.

### 9.2 Type Mapping Table

| Babel-C Type | C Type         | Notes                              |
|-------------|----------------|------------------------------------|
| `i32`       | `int32_t`      | From `<stdint.h>`                  |
| `i64`       | `int64_t`      | From `<stdint.h>`                  |
| `f64`       | `double`       |                                    |
| `bool`      | `uint8_t`      | 0 = false, 1 = true               |
| `str`       | `bc_str`       | Struct with `data` (const char*) and `len` (int32_t) |
| `unit`      | `void`         |                                    |
| `[T; N]`    | `T[N]`         | Passed as pointer in C             |
| `[T]`       | `bc_array*`    | Pointer to runtime array struct    |
| `struct Foo`| `Foo`          | Same name, matching fields         |
| `Result<T,E>` | `bc_result_T_E` | Tagged union in C               |

### 9.3 Safety Rules

1. All extern functions are implicitly side-effecting (`fn!`).
2. A `fn` (pure function) cannot call an extern function — compile error.
3. The programmer is responsible for correctness of extern declarations. The Babel-C compiler cannot verify that the declared signature matches the actual C function.
4. Passing Babel-C strings to C functions expecting `const char*` requires using the micro-lib function `str_to_cstr()` which null-terminates the string.

### 9.4 Linking with C Headers

The compiler generates `#include` directives based on a pragma or configuration file. For v0.1, the approach is:

- The compiler always generates `#include <stdint.h>`, `#include <stdio.h>`, `#include <stdlib.h>`, and `#include "bc_runtime.h"`.
- Additional C headers are specified via a build configuration file (not part of the language syntax).
- The generated C file is compiled with a standard C compiler (gcc/clang) and linked with the Babel-C runtime library and any user-specified C libraries.

---

## 10. Standard Library (Micro-Lib)

The micro-lib is deliberately tiny: 18 functions. All I/O functions are `fn!`.

### 10.1 I/O Functions (7)

```
fn! print(s: str)                          // Print string to stdout, no newline
fn! println(s: str)                        // Print string to stdout with newline
fn! print_i32(n: i32)                      // Print i32 to stdout
fn! print_i64(n: i64)                      // Print i64 to stdout
fn! print_f64(n: f64)                      // Print f64 to stdout
fn! print_bool(b: bool)                    // Print "true" or "false" to stdout
fn! read_line() -> Result<str, str>        // Read line from stdin, may fail
```

### 10.2 String Functions (4)

```
fn str_len(s: str) -> i32                  // Length in bytes (not chars)
fn str_eq(a: str, b: str) -> bool          // Byte-level equality
fn! str_concat(a: str, b: str) -> str      // Concatenate (allocates on arena, hence fn!)
fn! str_to_cstr(s: str) -> str             // Null-terminate for C interop (allocates)
```

**Note:** `str_concat` and `str_to_cstr` are `fn!` because they allocate on the arena, which is a side effect (mutation of the arena).

### 10.3 Math Functions (3)

```
fn abs_i32(n: i32) -> i32                  // Absolute value (panics on MIN_VALUE)
fn abs_f64(n: f64) -> f64                  // Absolute value
fn mod_i32(a: i32, b: i32) -> i32          // Modulo (same as %, included for clarity)
```

### 10.4 Array Functions (2)

```
fn len(arr: [i32]) -> i32                  // Array length (generic over element type in implementation)
fn! push(arr: [i32], val: i32)             // Append to dynamic array (generic in implementation)
```

**Note:** `len` and `push` are compiler-special-cased to work with any element type `T`. They appear as if monomorphic in documentation but the compiler handles them generically. The LLM writes `len(my_array)` and `push(my_array, value)` regardless of element type.

### 10.5 Conversion Functions (1)

```
fn! i32_to_str(n: i32) -> str              // Convert integer to string representation (allocates on arena)
```

**Note:** `i32_to_str` is `fn!` because it returns a `str`, which requires arena allocation. By the same logic as `str_concat`, any function that allocates on the arena is side-effecting.

### 10.6 Memory Functions (1)

```
fn! arena_reset()                          // Reset the program arena (frees all dynamic memory)
```

**Total: 18 functions.**

---

## 11. Example Programs

### 11.1 Hello World

```
fn! main() {
    println("Hello, World!");
}
```

### 11.2 Fibonacci (Recursion, If/Else)

```
fn fib(n: i32) -> i32 {
    if n <= 1 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn! main() {
    let result: i32 = fib(10);
    print("fib(10) = ");
    print_i32(result);
    println("");
}
```

### 11.3 Structs, Enums, and Match

```
struct Point {
    x: f64,
    y: f64,
}

enum Shape {
    Circle(f64),
    Rectangle(f64, f64),
    Dot(Point),
}

fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.14159265358979 * r * r,
        Shape::Rectangle(w, h) => w * h,
        Shape::Dot(p) => 0.0,
    }
}

fn describe(s: Shape) -> str {
    match s {
        Shape::Circle(r) => "circle",
        Shape::Rectangle(w, h) => "rectangle",
        Shape::Dot(p) => "dot",
    }
}

fn! main() {
    let c: Shape = Shape::Circle(5.0);
    let r: Shape = Shape::Rectangle(3.0, 4.0);
    let d: Shape = Shape::Dot(Point { x: 0.0, y: 0.0 });

    print("Shape: ");
    println(describe(c));
    print("Area: ");
    print_f64(area(c));
    println("");

    print("Shape: ");
    println(describe(r));
    print("Area: ");
    print_f64(area(r));
    println("");
}
```

### 11.4 Error Handling with Result

```
fn divide(a: i32, b: i32) -> Result<i32, str> {
    if b == 0 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}

fn compute(x: i32, y: i32, z: i32) -> Result<i32, str> {
    let first: i32 = try divide(x, y);
    let second: i32 = try divide(first, z);
    Result::Ok(second + 1)
}

fn! main() {
    let r1: Result<i32, str> = compute(100, 5, 2);
    match r1 {
        Result::Ok(val) => {
            print("Success: ");
            print_i32(val);
            println("");
        },
        Result::Err(msg) => {
            print("Error: ");
            println(msg);
        },
    };

    let r2: Result<i32, str> = compute(100, 0, 2);
    match r2 {
        Result::Ok(val) => {
            print("Success: ");
            print_i32(val);
            println("");
        },
        Result::Err(msg) => {
            print("Error: ");
            println(msg);
        },
    };
}
```

Expected output:
```
Success: 11
Error: division by zero
```

---

## 12. Compiler Architecture Overview

### 12.1 Pipeline

```
Source Code (.bc)
    │
    ▼
┌──────────┐
│  Lexer   │  Source → Token stream
└──────────┘
    │
    ▼
┌──────────┐
│  Parser  │  Token stream → Untyped AST
└──────────┘
    │
    ▼
┌───────────────────┐
│  Name Collection  │  Pass 1: Collect all top-level names (types, functions, constants)
└───────────────────┘
    │
    ▼
┌───────────────────┐
│  Name Resolution  │  Pass 2: Resolve all references, check anti-shadowing
│  & Type Checking  │  Annotate AST with types → Typed AST
└───────────────────┘
    │
    ▼
┌──────────────────┐
│  C Code Generator │  Typed AST → C source file
└──────────────────┘
    │
    ▼
┌──────────────────┐
│  C Compiler      │  C source → executable (gcc/clang)
│  (external)      │
└──────────────────┘
```

### 12.2 Lexer

- Input: UTF-8 source file.
- Output: Stream of tokens.
- Handles: keywords (including `fn!` as a single token), identifiers, literals, operators, punctuation, comments (discarded), whitespace (discarded).
- The lexer is context-free. No lexer modes or state.
- `fn!` is tokenized as a single `FN_BANG` token by checking if `fn` is immediately followed by `!`.

### 12.3 Parser

- Input: Token stream.
- Output: Untyped AST.
- Strategy: Recursive descent, LL(2).
- No backtracking needed.
- Struct literal ambiguity: When the parser sees `IDENT {`, it checks if the identifier is a known type name (from the name collection pass, or via a pre-scan of top-level declarations). If yes, parse as struct literal; otherwise, parse as block expression. **Implementation note:** Since we do two passes anyway, the parser can run after name collection. Alternatively, the parser can do a quick pre-scan of top-level `struct` declarations before full parsing.

### 12.4 Name Collection (Pass 1)

Walk the top-level declarations and build a symbol table of:
- All struct names and their fields
- All enum names and their variants
- All function names and their signatures
- All top-level constants

This enables order-independent references.

### 12.5 Name Resolution & Type Checking (Pass 2)

Walk the full AST:
1. **Resolve names:** Every identifier reference maps to a declaration. Unresolved names are compile errors.
2. **Check anti-shadowing:** Every local declaration is checked against all enclosing scopes. Duplicate names are compile errors.
3. **Type check:** Every expression is assigned a type. Type mismatches are compile errors.
4. **Purity check:** `fn` functions are verified to not call `fn!` functions or extern functions.
5. **Exhaustiveness check:** `match` expressions over enums must cover all variants.
6. **Result propagation check:** `try` expressions are verified to be inside functions returning compatible `Result` types.

Output: A typed AST where every expression node carries its resolved type.

### 12.6 C Code Generator

Transforms the typed AST into C99 source code:

1. **Header section:** `#include` directives, type definitions (structs, enums as tagged unions), function prototypes (for order independence).

2. **Runtime integration:** The generated C code links against `bc_runtime.h` / `bc_runtime.c` which provides:
   - Arena allocator
   - Overflow-checked arithmetic functions
   - Bounds-checked array access
   - Panic handler
   - String operations

3. **Translation patterns:**

| Babel-C Construct     | C Output                                    |
|----------------------|---------------------------------------------|
| `fn foo(x: i32) -> i32` | `int32_t foo(bc_arena* _arena, int32_t x)` |
| `fn! bar()`          | `void bar(bc_arena* _arena)`                |
| `let x: i32 = 5;`   | `const int32_t x = 5;`                      |
| `let mut x: i32 = 5;` | `int32_t x = 5;`                           |
| `a + b` (i32)        | `bc_add_i32(a, b)` (checked)                |
| `a[i]`               | `bc_array_get(a, i)` (bounds-checked)       |
| `if c { a } else { b }` | Ternary or temp variable + if/else        |
| `match expr { ... }` | `switch` on tag + cast payloads              |
| `try f(x)`           | `result = f(_arena, x); if (result.tag == BC_ERR) return result;` |
| `struct Foo { ... }` | `typedef struct { ... } Foo;`               |
| `enum Bar { A(i32), B }` | Tagged union with tag enum + payload union |
| `Result::Ok(v)`      | `(bc_result){ .tag = BC_OK, .ok = v }`      |
| `Result::Err(e)`     | `(bc_result){ .tag = BC_ERR, .err = e }`    |

4. **The `main` function:** The compiler generates a C `main()` that creates the arena, calls the Babel-C `main`, destroys the arena, and returns.

```c
int main(void) {
    bc_arena* _arena = bc_arena_create(1048576); // 1 MB
    babel_main(_arena);
    bc_arena_destroy(_arena);
    return 0;
}
```

### 12.7 File Extension

- Babel-C source files use the `.bc` extension.
- Generated C files use the `.c` extension with the same base name.

### 12.8 Compilation Command

```bash
babelc input.bc -o output.c    # Transpile to C
cc output.c bc_runtime.c -o program  # Compile with C compiler
```

---

## Appendix A: Reserved for Future Consideration

The following features are explicitly **NOT** in v0.1 but may be considered later:
- User-defined generics
- Modules / imports (multi-file programs)
- Closures / first-class functions
- Traits / interfaces
- Pattern matching on structs
- String interpolation
- Iterators

These are documented here to confirm they are intentional omissions, not oversights.

## Appendix B: Design Rationale Summary

| Decision                         | Rationale                                                |
|----------------------------------|----------------------------------------------------------|
| No type inference                | LLMs benefit from seeing every type explicitly            |
| No methods / no OOP             | One way to call functions — maximizes uniformity          |
| `and`/`or`/`not` as keywords    | Clearer token boundaries for LLM tokenizers              |
| No `break`/`continue`           | Minimalism; use boolean flags or restructure logic        |
| No compound assignment           | One way to assign; explicit is better for LLMs           |
| Arena allocation                 | Uniform, simple, no lifetime tracking needed             |
| No closures                      | Eliminates capture semantics, keeps functions simple      |
| Mandatory type annotations       | Every binding is self-documenting                        |
| `fn` vs `fn!`                    | Purity is visible at the declaration site                |
| No generics (except Result)     | Keeps type system trivially simple for v0.1              |
| Panics for programmer bugs       | Overflow, bounds — these are bugs, not handleable errors |
| Trailing commas everywhere       | Reduces LLM diff noise when adding/removing items        |
