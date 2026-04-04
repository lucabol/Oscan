# Oscan Language Specification v0.1

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
11. [Imports](#11-imports)
12. [Example Programs](#12-example-programs)
13. [Compiler Architecture Overview](#13-compiler-architecture-overview)
- [Appendix A: Available Runtime Primitives (Future Builtins)](#appendix-a-available-runtime-primitives-future-builtins)
- [Appendix B: Reserved for Future Consideration](#appendix-b-reserved-for-future-consideration)
- [Appendix C: Design Rationale Summary](#appendix-c-design-rationale-summary)

---

## 1. Keywords & Tokens

### 1.1 Reserved Words (25 total)

| Keyword    | Purpose                                      |
|------------|----------------------------------------------|
| `fn`       | Pure function declaration                    |
| `fn!`      | Side-effecting function declaration          |
| `let`      | Immutable binding                            |
| `mut`      | Mutable modifier (used with `let`)           |
| `struct`   | Nominal struct type declaration              |
| `enum`     | Tagged union declaration                     |
| `if`       | Conditional expression                       |
| `else`     | Else branch of conditional                   |
| `while`    | While loop                                   |
| `for`      | For-in loop                                  |
| `in`       | Range/array iteration binding                |
| `match`    | Pattern match expression                     |
| `return`   | Early return from function                   |
| `break`    | Exit the nearest enclosing loop              |
| `continue` | Skip to next iteration of nearest loop       |
| `try`      | Error propagation                            |
| `defer`    | Deferred cleanup call                        |
| `use`      | Import declarations from another file        |
| `extern`   | C-FFI declaration block                      |
| `as`       | Explicit type cast                           |
| `and`      | Logical AND                                  |
| `or`       | Logical OR                                   |
| `not`      | Logical NOT                                  |
| `true`     | Boolean literal                              |
| `false`    | Boolean literal                              |

**Note:** The table above lists 25 reserved words. `fn!` is lexed as a single token (`FN_BANG`), not `fn` followed by `!`. `true` and `false` are reserved keywords, not identifiers. `_` is additionally reserved as a wildcard in `match` patterns (see §1.3). `mut` is only valid immediately after `let`; it is not a standalone keyword elsewhere. `defer` is only valid immediately after the keyword and followed by a function call. `Result` is a reserved type name and cannot be used for user-defined struct or enum names (see §3.3).

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
| 1    | `+=` `-=` `*=` `/=` `%=` | Right     | Compound assignment (statement only) |

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
- Integer literals default to `i32` and must fit the `i32` range. Build larger `i64` values from in-range `i32` values with explicit casts and arithmetic.

#### Float Literals

```
float_literal  = digit+ '.' digit+ ;
```

- Must have digits on both sides of the decimal point. `.5` and `5.` are invalid.
- No scientific notation.
- Float literals are always `f64`.

#### String Literals

```
string_literal = '"' string_item* '"' ;
string_item    = string_char | interpolation ;
string_char    = any_unicode_except_backslash_or_quote_or_left_brace
               | escape_seq
               | '{{'
               | '}}' ;
escape_seq     = '\\' ('n' | 't' | 'r' | '\\' | '"' | '0') ;
interpolation  = '{' expr '}' ;
```

- Strings are UTF-8 encoded.
- No raw strings and no multi-line strings.
- Interpolation is supported inside string literals with `"text {expr} text"` syntax.
- Embedded expressions must have type `str`, `i32`, `i64`, `f64`, or `bool`.
- Embedded expressions must be pure: they may call `fn`, but not `fn!` or `extern`.
- Literal braces inside interpolated strings are written as `{{` and `}}`.
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
                 | compound_assign_stmt
                 | expr_stmt
                 | while_stmt
                 | for_stmt
                 | return_stmt
                 | break_stmt
                 | continue_stmt
                 | defer_stmt ;

let_stmt         = 'let' 'mut'? IDENT ':' type '=' expr ';' ;
assign_stmt      = place '=' expr ';' ;
compound_assign_stmt = place ('+=' | '-=' | '*=' | '/=' | '%=') expr ';' ;
expr_stmt        = expr ';' ;
while_stmt       = 'while' expr block ';'? ;
for_stmt         = 'for' IDENT 'in' expr '..' expr block ';'?
                 | 'for' IDENT 'in' expr block ';'? ;
return_stmt      = 'return' expr? ';' ;
break_stmt       = 'break' ';' ;
continue_stmt    = 'continue' ';' ;
defer_stmt       = 'defer' call_expr ';' ;

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
                 | string_literal
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
7. **Interpolation tokenization:** The lexer keeps interpolation single-pass parseable by emitting segmented string tokens around `{expr}` holes. Nested braces inside the embedded expression are balanced before control returns to string scanning.
8. **Literal brace escaping:** Inside string literals, literal braces must be written as `{{` and `}}`. A lone `}` is always a lexical error.

---

## 3. Type System

### 3.1 Primitive Types

| Type   | Size    | Description                    | C Mapping       |
|--------|---------|--------------------------------|-----------------|
| `i32`  | 32 bits | Signed integer                 | `int32_t`       |
| `i64`  | 64 bits | Signed integer                 | `int64_t`       |
| `f64`  | 64 bits | IEEE 754 double                | `double`        |
| `bool` | 8 bits  | Boolean (true/false only)      | `uint8_t` (0/1) |
| `str`  | ptr+len | UTF-8 string (immutable view)  | `osc_str`        |
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

### 3.5 Map Type

The `map` type is a built-in hash map from `str` keys to `str` values. It is an opaque, arena-allocated handle.

```
let mut m: map = map_new();
map_set(m, "key", "value");
let v: str = map_get(m, "key");
```

#### Map Operations

| Function | Signature | Purity | Description |
|----------|-----------|--------|-------------|
| `map_new()` | `() -> map` | `fn!` | Create an empty map |
| `map_set(m, key, value)` | `(map, str, str) -> unit` | `fn!` | Set a key-value pair (insert or update) |
| `map_get(m, key)` | `(map, str) -> str` | `fn!` | Get value by key (panics if missing) |
| `map_has(m, key)` | `(map, str) -> bool` | `fn` | Check if key exists |
| `map_delete(m, key)` | `(map, str) -> unit` | `fn!` | Remove a key |
| `map_len(m)` | `(map) -> i32` | `fn` | Number of entries |

- Uses FNV-1a hashing with open addressing.
- Managed by the arena allocator.
- `map_get` panics at runtime if the key is not found; use `map_has` to check first.

### 3.6 Boxed Types for Recursion

For recursive data structures, dynamic arrays serve as the indirection mechanism:

```
enum Tree {
    Leaf(i32),
    Node([Tree]),
}
```

The dynamic array `[Tree]` provides the heap indirection needed for recursion. There is no separate `Box` type — dynamic arrays handle all heap allocation needs.

### 3.7 No Type Aliases

There are no type aliases. Every type must be referred to by its full name. This maximizes explicitness.

### 3.8 Explicit Casts

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
- Type mappings must use Oscan types (see §9 for the mapping table).
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
| `<`      | i32, i64, f64, str | `bool`     |
| `>`      | i32, i64, f64, str | `bool`     |
| `<=`     | i32, i64, f64, str | `bool`     |
| `>=`     | i32, i64, f64, str | `bool`     |

- Comparing different types is a compile error.
- Structs cannot be compared with `==`; write a comparison function.
- Enum equality (`==`, `!=`) compares only the variant tag, not payloads. For deep comparison, use `match`.
- **String comparison** (`<`, `>`, `<=`, `>=`): lexicographic byte-by-byte comparison. `==` and `!=` for strings use `str_eq` semantics (byte-level equality). Two strings are equal if they have the same length and identical bytes.

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
- Assignment is a statement, not an expression. `x = y = 5` is a compile error.

#### Compound Assignment

```
counter += 1;
counter -= 5;
counter *= 2;
counter /= 4;
counter %= 3;
```

The compound assignment operators `+=`, `-=`, `*=`, `/=`, and `%=` are syntactic sugar. `x += e` is equivalent to `x = x + e`. They follow the same rules as regular assignment:

- Only valid for `let mut` bindings (or mutable array elements).
- The left-hand side and right-hand side must have the same type.
- Works with `i32`, `i64`, and `f64` operands (integer overflow is checked at runtime).
- Works on array element targets: `arr[i] += 1;`.

```
let mut sum: i32 = 0;
for i in 1..11 {
    sum += i;
}
```

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
- Supports `break` and `continue` statements (see §5.7.1).

### 5.7.1 Break and Continue

`break` exits the nearest enclosing `while` or `for` loop immediately. `continue` skips the rest of the current iteration and proceeds to the next iteration of the nearest enclosing loop.

```
// break: find first value > 5
let mut i: i32 = 0;
while i < 100 {
    if i > 5 { break; };
    i += 1;
}

// continue: skip even numbers, print odd 1..9
let mut j: i32 = 0;
while j < 10 {
    j += 1;
    if j % 2 == 0 { continue; };
    print_i32(j);
}
```

Rules:
- `break` and `continue` must appear inside a `while` or `for` loop. Using them outside a loop is a compile error.
- In nested loops, `break` and `continue` apply only to the **innermost** enclosing loop.
- Both are statements terminated by a semicolon: `break;` and `continue;`.
- There are no labeled loops or labeled break/continue.

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
- Supports `break` and `continue` (see §5.7.1).

#### Array Iteration

`for` also supports direct iteration over arrays:

```
for item in array_expr {
    body;
}
```

- The iterable expression must be an array type (`[T]` or `[T; N]`).
- The loop variable has the element type `T` and is immutable within the body.
- Works with any element type: `[i32]`, `[str]`, `[MyStruct]`, etc.
- Supports `break` and `continue`.

```
let nums: [i32] = [10, 20, 30, 40];
for n in nums {
    print_i32(n);
    print(" ");
}

let words: [str] = ["hello", "world", "oscan"];
for w in words {
    println(w);
}

// for-in with break
let arr: [i32] = [1, 2, 3, 4, 5];
for v in arr {
    if v == 3 { break; };
    print_i32(v);
}
```

The parser distinguishes range iteration from array iteration by checking for the `..` token after the first expression. If `..` is present, it is a range loop; otherwise, it is an array iteration loop.

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

### 5.12.1 String Indexing

```
let byte: i32 = s[0];
```

- Array indexing syntax extends to `str` operands.
- Returns `i32` — the byte value at the given position (0-based).
- Index must be type `i32`.
- Bounds-checked at runtime. Out-of-bounds access is a runtime panic.
- **Read-only.** Strings are immutable; `s[0] = 65;` is a compile error.
- Strings are byte sequences (not Unicode codepoints). `s[i]` returns the raw byte.

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

### 5.15 Defer Statement

```oscan
fn! process_file(path: str) {
    let fd: i32 = file_open_read(path);
    defer file_close(fd);
    // ... use fd ...
}   // file_close(fd) runs here automatically
```

`defer <call>;` registers a function call to execute when the enclosing function returns. Multiple defers execute in LIFO (last-in, first-out) order.

**Rules:**

- Only allowed in `fn!` functions. Using `defer` in a `fn` (pure function) is a compile error.
- The deferred expression must be a function call (either a simple function name or a dotted qualified path).
- Defers run on both normal exit and early `return` from the function.
- Arguments are evaluated **at defer-time** (when the defer statement executes), but the function call itself happens at function-exit time.
- If multiple defers are registered in a function, they execute in reverse order: the last defer registered runs first.

**Example:**

```oscan
fn! setup_and_cleanup() {
    let x: i32 = init_resource();
    defer cleanup(x);
    
    let y: i32 = another_resource();
    defer another_cleanup(y);
    
    // ... code ...
    
}   // another_cleanup(y) runs, then cleanup(x)
```

This is commonly used for resource cleanup: file handles, network connections, temporary allocations, or any operation that must be paired with a corresponding teardown call.

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

**Chosen approach:** Every Oscan program uses a **single implicit arena allocator**. All heap allocations go to this arena. Deallocation happens at arena granularity, not per-object.

**Justification:**
- **Deterministic:** No garbage collector pauses, no reference counting cycles.
- **Uniform:** One allocation mechanism, no choice paralysis for LLMs.
- **Simple for LLMs:** No `malloc`/`free` pairs to match. No lifetime annotations. No ownership system.
- **Zero UB:** No use-after-free (arena lives for the program duration by default), no double-free.

### 8.2 Arena Mechanics

1. **Program arena:** A single arena is created at program start (`main`). All heap allocations (dynamic arrays, large structs) are allocated from this arena.

2. **Allocation:** When the compiler emits code that needs heap memory (e.g., creating a `[T]` dynamic array or growing one via `push`), it calls the runtime's `osc_arena_alloc(arena, size)`.

3. **Deallocation:** The arena is freed in bulk when the program exits.

4. **Stack allocation:** Primitives, fixed-size arrays, and small structs are stack-allocated. The threshold for "small" is implementation-defined; the compiler determines placement based on size. This is an optimization detail that does not affect program semantics.

### 8.3 What the LLM Writes

- **Nothing regarding memory.** The LLM writes standard Oscan code. All memory management is implicit.
- Dynamic arrays are created via literals or `push` and automatically use the arena.
- The LLM never writes `alloc`, `free`, `new`, or `delete`.
### 8.4 Runtime Representation (for Morpheus)

The runtime library provides:

```c
typedef struct {
    uint8_t* data;
    size_t   used;
    size_t   capacity;
} osc_arena;

osc_arena* osc_arena_create(size_t initial_capacity);
void*     osc_arena_alloc(osc_arena* arena, size_t size);
void      osc_arena_reset(osc_arena* arena);
void      osc_arena_destroy(osc_arena* arena);
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
} osc_str;
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
} osc_array;
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
- The Oscan function name must exactly match the C function name.
- Multiple extern blocks are allowed.

### 9.2 Type Mapping Table

| Oscan Type | C Type         | Notes                              |
|-------------|----------------|------------------------------------|
| `i32`       | `int32_t`      | From `<stdint.h>`                  |
| `i64`       | `int64_t`      | From `<stdint.h>`                  |
| `f64`       | `double`       |                                    |
| `bool`      | `uint8_t`      | 0 = false, 1 = true               |
| `str`       | `osc_str`       | Struct with `data` (const char*) and `len` (int32_t) |
| `unit`      | `void`         |                                    |
| `[T; N]`    | `T[N]`         | Passed as pointer in C             |
| `[T]`       | `osc_array*`    | Pointer to runtime array struct    |
| `struct Foo`| `Foo`          | Same name, matching fields         |
| `Result<T,E>` | `osc_result_T_E` | Tagged union in C               |

### 9.3 Safety Rules

1. All extern functions are implicitly side-effecting (`fn!`).
2. A `fn` (pure function) cannot call an extern function — compile error.
3. The programmer is responsible for correctness of extern declarations. The Oscan compiler cannot verify that the declared signature matches the actual C function.
4. Passing Oscan strings to C functions expecting `const char*` requires using the micro-lib function `str_to_cstr()` which null-terminates the string.

### 9.4 Linking with C Headers

The compiler generates `#include` directives based on a pragma or configuration file. For v0.1, the approach is:

- The compiler always generates `#include <stdint.h>`, `#include <stdio.h>`, `#include <stdlib.h>`, and `#include "osc_runtime.h"`.
- Additional C headers are specified via a build configuration file (not part of the language syntax).
- The generated C file is compiled with a standard C compiler (gcc/clang) and linked with the Oscan runtime library and any user-specified C libraries.

**Embedded runtime:** The Oscan compiler binary embeds the runtime files (`osc_runtime.h`, `osc_runtime.c`, and `l_os.h`) directly using Rust's `include_str!()`. This makes the compiler self-contained — no need to distribute `runtime/` or `deps/` directories. The only external dependency is a C compiler (clang preferred, but gcc and MSVC are also supported).

For development or customization, if a `runtime/` directory exists next to the oscan binary or in the current working directory, it takes precedence over the embedded files.

---

## 10. Standard Library (Micro-Lib)

The micro-lib provides built-in functions organized by category. All I/O functions are `fn!`.

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

### 10.2 String Functions (9)

```
fn str_len(s: str) -> i32                  // Length in bytes (not chars)
fn str_eq(a: str, b: str) -> bool          // Byte-level equality
fn! str_concat(a: str, b: str) -> str      // Concatenate (allocates on arena, hence fn!)
fn! str_to_cstr(s: str) -> str             // Null-terminate for C interop (allocates)
fn str_find(haystack: str, needle: str) -> i32  // Index of first occurrence, or -1
fn! str_from_i32(n: i32) -> str            // Integer to string representation (allocates)
fn! str_slice(s: str, start: i32, end: i32) -> str  // Substring [start, end) (allocates)
fn! str_from_chars(arr: [i32]) -> str      // Create string from array of character codes (allocates)
fn! str_to_chars(s: str) -> [i32]          // Convert string to array of byte values (allocates)
```

**Note:** `str_concat`, `str_to_cstr`, `str_from_i32`, and `str_slice` are `fn!` because they allocate on the arena, which is a side effect (mutation of the arena). `str_find` is pure — it only reads. Interpolated strings lower to these helpers plus the conversion functions in §10.4 (`i32_to_str`, `str_from_i64`, `str_from_f64`, `str_from_bool`).

### 10.3 Math & Bitwise Functions (9)

```
fn abs_i32(n: i32) -> i32                  // Absolute value (panics on MIN_VALUE)
fn abs_f64(n: f64) -> f64                  // Absolute value
fn mod_i32(a: i32, b: i32) -> i32          // Modulo (same as %, included for clarity)
fn band(a: i32, b: i32) -> i32             // Bitwise AND
fn bor(a: i32, b: i32) -> i32              // Bitwise OR
fn bxor(a: i32, b: i32) -> i32             // Bitwise XOR
fn bshl(a: i32, n: i32) -> i32             // Shift left (unsigned semantics)
fn bshr(a: i32, n: i32) -> i32             // Shift right (unsigned/logical)
fn bnot(a: i32) -> i32                     // Bitwise NOT (complement)
```

**Note:** Shift functions use unsigned semantics to avoid C undefined behavior. All bitwise functions take `i32` operands only.

### 10.4 Array Functions (3)

```
fn len(arr: [i32]) -> i32                  // Array length (generic over element type in implementation)
fn! push(arr: [i32], val: i32)             // Append to dynamic array (generic in implementation)
fn! pop(arr: [i32]) -> i32                 // Remove and return last element (generic in implementation; panics if empty)
```

**Note:** `len`, `push`, and `pop` are compiler-special-cased to work with any element type `T`. They appear as if monomorphic in documentation but the compiler handles them generically. The LLM writes `len(my_array)`, `push(my_array, value)`, and `pop(my_array)` regardless of element type.

### 10.5 Conversion Functions (3)

```
fn! i32_to_str(n: i32) -> str              // Convert integer to string representation (allocates on arena)
fn! str_from_i32_hex(n: i32) -> str        // Convert i32 to hexadecimal string representation (allocates)
fn! str_from_i64_hex(n: i64) -> str        // Convert i64 to hexadecimal string representation (allocates)
```

**Note:** `i32_to_str` is `fn!` because it returns a `str`, which requires arena allocation. By the same logic as `str_concat`, any function that allocates on the arena is side-effecting. `str_from_i32` is the equivalent helper in the `str_from_*` family and may be used interchangeably by compiler lowering.

### 10.6 Command-Line Arguments (2)

```
fn! arg_count() -> i32                     // Number of command-line arguments (including program name)
fn! arg_get(i: i32) -> str                 // Get argument at index i (0 = program name). Panics if out of bounds.
```

**Note:** These expose the underlying C `argc`/`argv`. `arg_get(0)` returns the program name.

### 10.7 Whole-File I/O (2) — `fn!`

```
fn! read_file(path: str) -> Result<str, str>      // Read entire file into a string. Returns Ok(content) or Err(message).
fn! write_file(path: str, data: str) -> Result<str, str>  // Write string to file (creates/overwrites). Returns Ok("") or Err(message).
```

**Note:** `read_file` uses a single bulk read (not byte-at-a-time) for performance. The entire file is loaded into arena memory. Both functions return `Result<str, str>` where the error is a human-readable message.

### 10.8 Minimum/Maximum/Clamp Functions (9) — Pure

```
fn min_i32(a: i32, b: i32) -> i32          // Returns the smaller of a and b
fn max_i32(a: i32, b: i32) -> i32          // Returns the larger of a and b
fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32  // Clamps v to [lo, hi]
fn min_i64(a: i64, b: i64) -> i64          // Returns the smaller of a and b
fn max_i64(a: i64, b: i64) -> i64          // Returns the larger of a and b
fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64  // Clamps v to [lo, hi]
fn min_f64(a: f64, b: f64) -> f64          // Returns the smaller of a and b
fn max_f64(a: f64, b: f64) -> f64          // Returns the larger of a and b
fn clamp_f64(v: f64, lo: f64, hi: f64) -> f64  // Clamps v to [lo, hi]
```

**Note:** These are pure functions (`fn`). No allocation. `min` and `max` compare values directly. `clamp` ensures `v` falls within the inclusive range `[lo, hi]`. If `v < lo`, returns `lo`; if `v > hi`, returns `hi`; otherwise returns `v`.

### 10.9 String Join (1) — `fn!`

```
fn! str_join(arr: [str], sep: str) -> str   // Join array of strings with separator (allocates)
```

**Note:** `str_join` allocates on the arena, so it is `fn!`. It concatenates all strings in `arr` with `sep` inserted between each pair. Empty array returns empty string.

### 10.10 Path Utilities (2)

```
fn path_basename(path: str) -> str          // Extract filename from path (pure — returns view)
fn! path_dirname(path: str) -> str          // Extract directory from path (allocates)
```

**Note:** `path_basename` is pure and returns a view into the input string (no allocation). `path_dirname` allocates because it may need to trim the trailing separator. Both work on POSIX-style (`/`) and Windows-style (`\`) path separators.

### 10.11 File I/O (7)

```
fn! file_open_read(path: str) -> i32       // Open file for reading, returns handle (-1 on error)
fn! file_open_write(path: str) -> i32      // Open/create file for writing, returns handle (-1 on error)
fn! read_byte(fd: i32) -> i32              // Read one byte, returns -1 on EOF
fn! write_byte(fd: i32, b: i32)            // Write one byte
fn! write_str(fd: i32, s: str)             // Write string to file handle
fn! file_close(fd: i32)                    // Close file handle
fn! file_delete(path: str) -> i32          // Delete file, returns 0 on success
```

**Constants** (module-level constants available without declaration):

| Name     | Type  | Value | Description           |
|----------|-------|-------|-----------------------|
| `STDIN`  | `i32` | `0`   | Standard input handle  |
| `STDOUT` | `i32` | `1`   | Standard output handle |
| `STDERR` | `i32` | `2`   | Standard error handle  |

**Note:** In freestanding mode, file I/O uses OS syscalls. In libc mode, it uses standard C stdio. Byte-at-a-time I/O avoids buffer management complexity.

### 10.12 Tier 1: Character Classification (11)

All pure functions (`fn`).

```
fn char_is_alpha(c: i32) -> bool             // True if c is alphabetic (a-z, A-Z)
fn char_is_digit(c: i32) -> bool             // True if c is digit (0-9)
fn char_is_alnum(c: i32) -> bool             // True if c is alphanumeric (a-z, A-Z, 0-9)
fn char_is_space(c: i32) -> bool             // True if c is whitespace (space, tab, newline, etc.)
fn char_is_upper(c: i32) -> bool             // True if c is uppercase (A-Z)
fn char_is_lower(c: i32) -> bool             // True if c is lowercase (a-z)
fn char_is_print(c: i32) -> bool             // True if c is printable (ASCII 32-126)
fn char_is_xdigit(c: i32) -> bool            // True if c is hexadecimal digit (0-9, a-f, A-F)
fn char_to_upper(c: i32) -> i32              // Convert character to uppercase (returns uppercase equivalent or unchanged)
fn char_to_lower(c: i32) -> i32              // Convert character to lowercase (returns lowercase equivalent or unchanged)
fn abs_i64(n: i64) -> i64                    // Absolute value of 64-bit integer
```

**Note:** Character classification functions treat the i32 as a Unicode code point or ASCII value. Input values should be in the range 0-127 for ASCII, or valid Unicode code points for extended use.

### 10.13 Tier 2: Number Parsing & Conversion (5)

```
fn parse_i32(s: str) -> Result<i32, str>     // Parse string to i32; returns error message on failure
fn parse_i64(s: str) -> Result<i64, str>     // Parse string to i64; returns error message on failure
fn! str_from_i64(n: i64) -> str              // Convert i64 to string (allocates on arena)
fn! str_from_f64(n: f64) -> str              // Convert f64 to string (allocates on arena)
fn str_from_bool(b: bool) -> str             // Convert bool to "true" or "false" (static strings, no allocation)
```

**Note:** Parsing functions use standard C number parsing rules. `str_from_i64` and `str_from_f64` allocate on the arena, so they are `fn!`. `str_from_bool` returns static string literals and is pure.

**Interpolation MVP runtime note:** no dedicated formatter is required. Compiler lowering can build interpolated strings entirely from `str_concat`, `i32_to_str` / `str_from_i32`, `str_from_i64`, `str_from_f64`, and `str_from_bool`.

### 10.14 Tier 3: System Functions (5) — `fn!`

```
fn! rand_seed(seed: i32)                     // Seed the random number generator
fn! rand_i32() -> i32                        // Generate a random 32-bit integer
fn! time_now() -> i64                        // Return current Unix time (seconds since epoch)
fn! sleep_ms(ms: i32)                        // Sleep for specified milliseconds
fn! exit(code: i32)                          // Exit the process with given exit code
```

**Note:** `rand_seed` must be called before using `rand_i32` (otherwise RNG behavior is undefined). `time_now` returns seconds since January 1, 1970 UTC.

### 10.15 Tier 4: Environment & Error (6) — `fn!`

```
fn! env_get(name: str) -> Result<str, str>   // Get environment variable; returns error if not found
fn! env_count() -> i32                       // Get the number of environment variables
fn! env_key(i: i32) -> str                   // Get the name of the i-th environment variable (0-based)
fn! env_value(i: i32) -> str                 // Get the value of the i-th environment variable (0-based)
fn! errno_get() -> i32                       // Get the value of errno from the last OS call
fn! errno_str(code: i32) -> str              // Convert errno code to human-readable error message
```

**Note:** `env_get` returns a string from the environment (must be copied to the arena if you need it to persist). `env_count`, `env_key`, and `env_value` allow iterating over all environment variables by index. `errno_get` and `errno_str` are useful for debugging OS-level errors.

### 10.16 Tier 5: Filesystem Operations (8) — `fn!`

```
fn! file_rename(old: str, new: str) -> i32   // Rename file; returns 0 on success, -1 on error
fn! file_exists(path: str) -> bool           // Check if file exists at path
fn! file_size(path: str) -> i64              // Get file size in bytes; returns -1 on error
fn! file_open_append(path: str) -> i32       // Open file for appending; returns handle (-1 on error)
fn! dir_create(path: str) -> i32             // Create directory; returns 0 on success, -1 on error
fn! dir_remove(path: str) -> i32             // Remove empty directory; returns 0 on success, -1 on error
fn! dir_current() -> str                     // Get current working directory as string (allocates on arena)
fn! dir_change(path: str) -> i32             // Change current working directory; returns 0 on success, -1 on error
```

**Note:** File operations use OS-level syscalls (or libc equivalents). File handles are the same format as those returned by `file_open_read`/`file_open_write`. `dir_current()` returns a newly allocated string on the arena with the CWD path.

### 10.17 Tier 6: String Operations (9)

Most are pure (`fn`); allocation functions are `fn!`.

```
fn str_contains(s: str, sub: str) -> bool    // Check if string contains substring
fn str_starts_with(s: str, prefix: str) -> bool  // Check if string starts with prefix
fn str_ends_with(s: str, suffix: str) -> bool    // Check if string ends with suffix
fn! str_trim(s: str) -> str                  // Return new string with leading/trailing whitespace removed (allocates)
fn! str_split(s: str, delim: str) -> [str]   // Split string by delimiter; returns array of substrings (allocates)
fn! str_to_upper(s: str) -> str              // Convert string to uppercase (allocates on arena)
fn! str_to_lower(s: str) -> str              // Convert string to lowercase (allocates on arena)
fn! str_replace(s: str, old: str, new: str) -> str  // Replace all occurrences of old with new (allocates)
fn str_compare(a: str, b: str) -> i32        // Lexicographic comparison: -1 (a<b), 0 (a==b), 1 (a>b)
```

**Note:** All string functions that create new strings (`str_trim`, `str_split`, `str_to_upper`, `str_to_lower`, `str_replace`) are `fn!` because they allocate on the arena.

### 10.18 Tier 7: Directory Listing, Process Control & Terminal (7) — `fn!`

```
fn! dir_list(path: str) -> [str]             // List directory entries (returns array of names, allocates)
fn! proc_run(cmd: str, args: [str]) -> i32   // Run external process; returns exit code
fn! term_width() -> i32                      // Get terminal width in columns (0 if not a terminal)
fn! term_height() -> i32                     // Get terminal height in rows (0 if not a terminal)
fn! term_raw() -> i32                        // Enable raw terminal mode (no line buffering, no echo); returns 0 on success
fn! term_restore() -> i32                    // Restore terminal to normal (cooked) mode; returns 0 on success
fn! read_nonblock() -> i32                   // Non-blocking read of a single byte from stdin; returns -1 if no data available
```

**Note:** `proc_run` executes an external command synchronously and returns its exit code. `term_width()` and `term_height()` return 0 if output is not connected to a terminal (useful for detecting interactive vs. piped execution). `term_raw()` and `term_restore()` are used in pairs for interactive TUI applications; always call `term_restore()` before program exit. `read_nonblock()` is useful in raw mode for polling keyboard input without blocking.

### 10.19 Tier 8: Math Functions (15) — `fn`

All math functions are pure (`fn`). They operate on `f64` values and delegate to the C math library (`<math.h>`).

#### Trigonometric & Exponential

```
fn math_sin(x: f64) -> f64                  // Sine (x in radians)
fn math_cos(x: f64) -> f64                  // Cosine (x in radians)
fn math_atan2(y: f64, x: f64) -> f64        // Two-argument arctangent (radians)
fn math_exp(x: f64) -> f64                  // e raised to the power x
fn math_log(x: f64) -> f64                  // Natural logarithm (base e)
fn math_pow(base: f64, exp: f64) -> f64     // base raised to the power exp
fn math_sqrt(x: f64) -> f64                 // Square root
```

#### Rounding & Remainder

```
fn math_floor(x: f64) -> f64                // Floor (largest integer ≤ x)
fn math_ceil(x: f64) -> f64                 // Ceiling (smallest integer ≥ x)
fn math_fmod(x: f64, y: f64) -> f64         // Floating-point remainder of x / y
fn math_abs(x: f64) -> f64                  // Absolute value
```

#### Constants

```
fn math_pi() -> f64                          // π ≈ 3.14159265358979
fn math_e() -> f64                           // Euler's number e ≈ 2.71828182845905
fn math_ln2() -> f64                         // ln(2) ≈ 0.693147180559945
fn math_sqrt2() -> f64                       // √2 ≈ 1.41421356237310
```

**Note:** Math constants are zero-argument pure functions, not global variables. Call them as `math_pi()`, not `math_pi`.

### 10.20 Tier 9: Socket Networking (11) — `fn!`

Socket builtins provide TCP and UDP networking. All are side-effecting (`fn!`).

```
fn! socket_tcp() -> i32                      // Create a TCP socket; returns socket descriptor (-1 on error)
fn! socket_connect(sock: i32, addr: str, port: i32) -> i32  // Connect to remote IPv4 address or hostname; returns 0 on success, -1 on error
fn! socket_bind(sock: i32, port: i32) -> i32               // Bind socket to local port; returns 0 on success, -1 on error
fn! socket_listen(sock: i32, backlog: i32) -> i32          // Listen for incoming connections; returns 0 on success, -1 on error
fn! socket_accept(sock: i32) -> i32          // Accept incoming connection; returns client socket descriptor (-1 on error)
fn! socket_send(sock: i32, data: str) -> i32 // Send data over socket; returns number of bytes sent (-1 on error)
fn! socket_recv(sock: i32, max_len: i32) -> str  // Receive data from socket; returns received data as string
fn! socket_close(sock: i32)                  // Close socket (works for both TCP and UDP)
fn! socket_udp() -> i32                      // Create a UDP socket; returns socket descriptor (-1 on error)
fn! socket_sendto(sock: i32, data: str, addr: str, port: i32) -> i32  // Send data to IPv4 address or hostname via UDP; returns bytes sent (-1 on error)
fn! socket_recvfrom(sock: i32, max_len: i32) -> str  // Receive UDP datagram; returns received data as string
```

**Example — Simple TCP client:**

```
fn! main() {
    let sock: i32 = socket_tcp();
    let status: i32 = socket_connect(sock, "localhost", 8080);
    if status == 0 {
        socket_send(sock, "Hello, server!");
        let response: str = socket_recv(sock, 1024);
        println(response);
    };
    socket_close(sock);
}
```

**Example — Simple UDP sender:**

```
fn! main() {
    let sock: i32 = socket_udp();
    socket_sendto(sock, "Hello, UDP!", "localhost", 9000);
    socket_close(sock);
}
```

**Note:** Socket operations use OS-level syscalls. Error handling follows the C convention of returning -1 on failure; use `errno_get()` for detailed error information.

### 10.21 Tier 10: Path Utilities (4) — mixed purity

```
fn path_ext(path: str) -> str                // Extract file extension (e.g., "foo.txt" → ".txt"); pure
fn! path_join(dir: str, file: str) -> str    // Join directory and filename with path separator (allocates)
fn! path_exists(path: str) -> bool           // Check if a path exists on the filesystem
fn! path_is_dir(path: str) -> bool           // Check if a path is a directory
```

**Note:** `path_ext` is pure — it only inspects the string. `path_join` allocates a new string on the arena. `path_exists` and `path_is_dir` perform filesystem checks and are side-effecting.

### 10.22 Tier 11: Array Sorting (4) — `fn!`

Sort builtins sort dynamic arrays in-place. All are `fn!` because they mutate the array.

```
fn! sort_i32(arr: [i32])                     // Sort array of i32 in ascending order (in-place)
fn! sort_i64(arr: [i64])                     // Sort array of i64 in ascending order (in-place)
fn! sort_f64(arr: [f64])                     // Sort array of f64 in ascending order (in-place)
fn! sort_str(arr: [str])                     // Sort array of strings lexicographically (in-place)
```

**Example:**

```
fn! main() {
    let mut nums: [i32] = [3, 1, 4, 1, 5, 9];
    sort_i32(nums);
    for n in nums {
        print_i32(n);
        print(" ");
    }
    println("");
}
```

**Note:** Sorting uses an in-place algorithm (Shell sort) with no additional heap allocation. There are separate functions per type because Oscan does not support user-defined generics.

### 10.23 Tier 12: Graphics Builtins (19) — `fn!` / `fn`

Graphics builtins are available in freestanding mode only and use `l_gfx.h` from the laststanding library.

#### Canvas Lifecycle (all `fn!`)

```
fn! canvas_open(width: i32, height: i32, title: str) -> i32   // Opens a graphics window. Returns 0 on success, -1 on error. Pass width/height ≤ 0 for fullscreen.
fn! canvas_close() -> unit                                     // Closes the graphics window and frees resources.
fn! canvas_alive() -> bool                                     // Returns true if the window is still open (not closed by user).
fn! canvas_flush() -> unit                                     // Updates the display, blitting the pixel buffer to screen.
fn! canvas_clear(color: i32) -> unit                           // Fills the entire canvas with a solid color.
```

#### Drawing Primitives (all `fn!`)

```
fn! gfx_pixel(x: i32, y: i32, color: i32) -> unit             // Sets a single pixel. No-op if out of bounds.
fn! gfx_get_pixel(x: i32, y: i32) -> i32                      // Returns the color at (x,y), or 0 if out of bounds.
fn! gfx_line(x0: i32, y0: i32, x1: i32, y1: i32, color: i32) -> unit  // Draws a line using Bresenham's algorithm.
fn! gfx_rect(x: i32, y: i32, w: i32, h: i32, color: i32) -> unit     // Draws an outline rectangle.
fn! gfx_fill_rect(x: i32, y: i32, w: i32, h: i32, color: i32) -> unit // Draws a filled rectangle.
fn! gfx_circle(cx: i32, cy: i32, r: i32, color: i32) -> unit   // Draws an outline circle.
fn! gfx_fill_circle(cx: i32, cy: i32, r: i32, color: i32) -> unit // Draws a filled circle.
fn! gfx_draw_text(x: i32, y: i32, text: str, color: i32) -> unit  // Draws text using built-in 8x8 bitmap font.
```

#### Input (all `fn!`)

```
fn! canvas_key() -> i32                                        // Returns next key press (non-blocking), or 0 if none. Key codes: 32-126 (ASCII), 27 (Esc), 1001-1004 (arrows).
fn! canvas_mouse_x() -> i32                                    // Returns current mouse X position.
fn! canvas_mouse_y() -> i32                                    // Returns current mouse Y position.
fn! canvas_mouse_btn() -> i32                                  // Returns mouse button bitmask (1=left, 2=right, 4=middle).
```

#### Color (pure `fn`)

```
fn rgb(r: i32, g: i32, b: i32) -> i32                         // Creates an ARGB color with full opacity. Parameters 0-255.
fn rgba(r: i32, g: i32, b: i32, a: i32) -> i32                // Creates an ARGB color with custom alpha.
```

**Note:** All graphics functions operate on a single global canvas (no handle parameter needed). Color format is 32-bit ARGB (alpha in high byte, then red, green, blue). The built-in font is 8x8 pixels per character, covering ASCII 32-126.

---

### 10.24 Tier 13: Date/Time, Glob, SHA-256, Terminal & Environment Modification (14)

Extended system builtins for date/time manipulation, pattern matching, hashing, terminal detection, and environment modification.

#### Date/Time (`fn!`)

```
fn! time_format(timestamp: i64, fmt: str) -> str    // Format UTC timestamp using strftime-style format string. Allocates result on arena.
fn! time_utc_year(timestamp: i64) -> i32             // Extract UTC year (e.g. 2024) from Unix timestamp.
fn! time_utc_month(timestamp: i64) -> i32            // Extract UTC month (1-12) from Unix timestamp.
fn! time_utc_day(timestamp: i64) -> i32              // Extract UTC day of month (1-31) from Unix timestamp.
fn! time_utc_hour(timestamp: i64) -> i32             // Extract UTC hour (0-23) from Unix timestamp.
fn! time_utc_min(timestamp: i64) -> i32              // Extract UTC minute (0-59) from Unix timestamp.
fn! time_utc_sec(timestamp: i64) -> i32              // Extract UTC second (0-59) from Unix timestamp.
```

**Note:** All time functions operate on UTC. Use `time_now()` to get the current Unix timestamp, then pass it to these functions. `time_format` uses standard C strftime format specifiers (e.g. `"%Y-%m-%d"`, `"%H:%M:%S"`).

#### Glob Matching (pure `fn`)

```
fn glob_match(pattern: str, text: str) -> bool       // Returns true if text matches the glob pattern. Supports *, ?, and character ranges.
```

**Note:** `glob_match` is a pure function — no side effects. Equivalent to POSIX `fnmatch()`.

#### SHA-256 (`fn!`)

```
fn! sha256(data: str) -> str                         // Returns the SHA-256 hash of data as a 64-character lowercase hex string. Allocates on arena.
```

**Note:** The result is always exactly 64 hex characters (representing 32 bytes). Example: `sha256("hello")` returns `"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"`.

#### Terminal Detection (pure `fn`)

```
fn is_tty() -> bool                                  // Returns true if stdout is connected to a terminal (not redirected/piped).
```

#### Environment Modification (`fn!`)

```
fn! env_set(name: str, value: str) -> i32            // Set environment variable. Returns 0 on success, -1 on error.
fn! env_delete(name: str) -> i32                     // Delete (unset) environment variable. Returns 0 on success, -1 on error.
```

**Note:** `env_set` and `env_delete` modify the process environment. Use `env_get` (from Tier 4) to read variables.

---

**Extended Library Summary:**

The library is organized into 13 tiers with core functions plus specialized tiers:

- **Core:** I/O (7), String (9), Math & Bitwise (9), Array (3), Conversion (3), CLI args (2), Whole-File I/O (2), File I/O (7), Min/Max/Clamp (9), String Join (1), Path Utilities (2)
- **Tier 1:** Character classification (11 functions)
- **Tier 2:** Number parsing & conversion (5 functions)
- **Tier 3:** System functions (5 functions)
- **Tier 4:** Environment & error (6 functions)
- **Tier 5:** Filesystem operations (8 functions)
- **Tier 6:** String operations (9 functions)
- **Tier 7:** Directory, process control & terminal (7 functions)
- **Tier 8:** Math functions (15 functions)
- **Tier 9:** Socket networking (11 functions)
- **Tier 10:** Path utilities (4 functions)
- **Tier 11:** Array sorting (4 functions)
- **Tier 12:** Graphics builtins (19 functions)
- **Tier 13:** Date/Time, glob, SHA-256, terminal & environment modification (14 functions)

**Total builtin functions:** ~168 (54 core + 11 Tier 1 + extended tiers)

---

## 11. Imports

### 11.1 The `use` Statement

Oscan supports source code modularity via the `use` statement, which imports declarations from other Oscan files.

**Syntax:**

```
use "path/to/file.osc"
```

**Rules:**

- **Paths are relative** to the importing file's directory. For example, if `src/main.osc` contains `use "libs/math.osc"`, the compiler looks for `src/libs/math.osc`.
- **Textual inclusion before lexing.** Import resolution happens before lexing and parsing. The compiler recursively resolves all `use` statements, inlining the imported file's source text into the importing file. The result is a single combined source text that is then lexed and parsed as one unit.
- **Circular imports are silently skipped.** If file A imports file B, and file B imports file A (directly or indirectly), the circular import is detected via a set of already-visited canonical paths, and the redundant import is ignored without error.
- **All declarations are available.** After a `use` statement, all structs, enums, functions, and constants from the imported file are in scope.
- **Declaration order does not matter across files.** Oscan uses two-phase semantic analysis, so you can call a function from an imported file even if it is defined after the `use` statement (or in a file that was imported later in the compilation order).
- **`use` at top level only.** Import statements must appear at the top level of a file, not inside function bodies or nested blocks.

### 11.2 Example

Consider two files:

**libs/math.osc:**
```
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn multiply(a: i32, b: i32) -> i32 {
    a * b
}
```

**main.osc:**
```
use "libs/math.osc"

fn! main() {
    let result = add(3, 4);
    println(str_from_i32(result));
}
```

When `main.osc` is compiled, the `add` and `multiply` functions from `libs/math.osc` are available for use. The output is `7`.

### 11.3 Standard Library

The standard library includes a pure-Oscan UI widget library at `libs/ui.osc`, which provides reusable UI components:

- **Widgets:** `button`, `checkbox`, `slider`, `panel`, `label`, `separator`
- **Implementation:** Built entirely in Oscan using the graphics builtins (§10.8), with no C dependencies.
- **Usage:** Import with `use "libs/ui.osc"` to access pre-built UI components for graphical applications.

---

## 12. Example Programs

### 12.1 Hello World

```
fn! main() {
    println("Hello, World!");
}
```

### 12.2 Fibonacci (Recursion, If/Else)

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

### 12.3 Structs, Enums, and Match

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

### 12.4 Error Handling with Result

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

## 13. Compiler Architecture Overview

### 13.1 Pipeline

```
Source Code (.osc)
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

### 13.2 Lexer

- Input: UTF-8 source file.
- Output: Stream of tokens.
- Handles: keywords (including `fn!` as a single token), identifiers, literals, operators, punctuation, comments (discarded), whitespace (discarded).
- The lexer is context-free. No lexer modes or state.
- `fn!` is tokenized as a single `FN_BANG` token by checking if `fn` is immediately followed by `!`.

### 13.3 Parser

- Input: Token stream.
- Output: Untyped AST.
- Strategy: Recursive descent, LL(2).
- No backtracking needed.
- Struct literal ambiguity: When the parser sees `IDENT {`, it checks if the identifier is a known type name (from the name collection pass, or via a pre-scan of top-level declarations). If yes, parse as struct literal; otherwise, parse as block expression. **Implementation note:** Since we do two passes anyway, the parser can run after name collection. Alternatively, the parser can do a quick pre-scan of top-level `struct` declarations before full parsing.

### 13.4 Name Collection (Pass 1)

Walk the top-level declarations and build a symbol table of:
- All struct names and their fields
- All enum names and their variants
- All function names and their signatures
- All top-level constants

This enables order-independent references.

### 13.5 Name Resolution & Type Checking (Pass 2)

Walk the full AST:
1. **Resolve names:** Every identifier reference maps to a declaration. Unresolved names are compile errors.
2. **Check anti-shadowing:** Every local declaration is checked against all enclosing scopes. Duplicate names are compile errors.
3. **Type check:** Every expression is assigned a type. Type mismatches are compile errors.
4. **Purity check:** `fn` functions are verified to not call `fn!` functions or extern functions.
5. **Exhaustiveness check:** `match` expressions over enums must cover all variants.
6. **Result propagation check:** `try` expressions are verified to be inside functions returning compatible `Result` types.

Output: A typed AST where every expression node carries its resolved type.

### 13.6 C Code Generator

Transforms the typed AST into C99 source code:

1. **Header section:** `#include` directives, type definitions (structs, enums as tagged unions), function prototypes (for order independence).

2. **Runtime integration:** The generated C code links against `osc_runtime.h` / `osc_runtime.c` which provides:
   - Arena allocator
   - Overflow-checked arithmetic functions
   - Bounds-checked array access
   - Panic handler
   - String operations

3. **Translation patterns:**

| Oscan Construct     | C Output                                    |
|----------------------|---------------------------------------------|
| `fn foo(x: i32) -> i32` | `int32_t foo(osc_arena* _arena, int32_t x)` |
| `fn! bar()`          | `void bar(osc_arena* _arena)`                |
| `let x: i32 = 5;`   | `const int32_t x = 5;`                      |
| `let mut x: i32 = 5;` | `int32_t x = 5;`                           |
| `a + b` (i32)        | `osc_add_i32(a, b)` (checked)                |
| `a[i]`               | `osc_array_get(a, i)` (bounds-checked)       |
| `if c { a } else { b }` | Ternary or temp variable + if/else        |
| `match expr { ... }` | `switch` on tag + cast payloads              |
| `try f(x)`           | `result = f(_arena, x); if (result.tag == osc_ERR) return result;` |
| `struct Foo { ... }` | `typedef struct { ... } Foo;`               |
| `enum Bar { A(i32), B }` | Tagged union with tag enum + payload union |
| `Result::Ok(v)`      | `(osc_result){ .tag = osc_OK, .ok = v }`      |
| `Result::Err(e)`     | `(osc_result){ .tag = osc_ERR, .err = e }`    |

4. **The `main` function:** The compiler generates a C `main()` that creates the arena, calls the Oscan `main`, destroys the arena, and returns.

```c
int main(void) {
    osc_arena* _arena = osc_arena_create(1048576); // 1 MB
    oscan_main(_arena);
    osc_arena_destroy(_arena);
    return 0;
}
```

### 13.7 File Extension

- Oscan source files use the `.osc` extension.
- Generated C files use the `.c` extension with the same base name.

### 13.8 Compilation Command

```bash
oscan input.osc -o output.c    # Transpile to C
cc output.c osc_runtime.c -o program  # Compile with C compiler
```

---

## Appendix A: Available Runtime Primitives (Future Builtins)

The `deps/laststanding` library (`l_os.h`) provides a freestanding C runtime with no libc dependency. Many functions already back existing Oscan builtins (e.g., `l_strlen` → `str_len`). The following **new** functions are available as potential future builtins but are not yet exposed in Oscan v0.1.

> **Note:** This is a reference for future work, not a commitment to implement all of these.

### Sorting & Searching

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_qsort(base, n, size, cmp)` | In-place Shell sort, no malloc/recursion | `sort(arr, cmp)` |
| `l_bsearch(key, base, n, size, cmp)` | Binary search in sorted array | `binary_search(arr, key)` |

### Random Number Generation

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_srand(seed)` | Seed xorshift32 PRNG | `seed_random(n)` |
| `l_rand()` | Pseudo-random unsigned int | `random()` |

### Math

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_abs(x)` / `l_labs(x)` | Absolute value (int / long) | `abs(n)` (already partially exposed) |
| `L_MIN(a, b)` / `L_MAX(a, b)` | Numeric min/max macros | `min(a, b)` / `max(a, b)` |
| `L_CLAMP(v, lo, hi)` | Clamp value to range | `clamp(v, lo, hi)` |

### Time

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_time(t)` | Unix timestamp (seconds since epoch) | `time()` |
| `l_sleep_ms(ms)` | Sleep for milliseconds | `sleep(ms)` |

### String & Character Classification

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_strtok_r(str, delim, saveptr)` | Reentrant string tokenizer | `split(str, delim)` |
| `l_strcasecmp(s1, s2)` | Case-insensitive comparison | `str_eq_ignore_case(a, b)` |
| `l_isprint(c)` | Printable ASCII test | `is_printable(c)` |
| `l_isxdigit(c)` | Hex digit test | `is_hex(c)` |
| `l_basename(path)` / `l_dirname(path, buf, sz)` | Path component extraction | `basename(path)` / `dirname(path)` |

### Error Reporting

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_errno()` | Last syscall error code | Better `Result<T, E>` error types |
| `l_strerror(errnum)` | Human-readable error string | Richer error messages in `Result` |
| `L_ENOENT`, `L_EACCES`, … | Cross-platform error code constants | Typed error enums |

### I/O

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_read_line(fd, buf, bufsz)` | Read line from any fd (not just stdin) | Improve `read_line()` to accept fd |
| `l_dprintf(fd, fmt, ...)` | Formatted output to fd | `fprintf(fd, fmt, ...)` |
| `l_open_append(file)` / `l_open_trunc(file)` | Additional file open modes | `file_open_append` / `file_open_trunc` |
| `l_read_nonblock(fd, buf, count)` | Non-blocking read | `try_read(fd)` |

### CLI & Environment

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_getopt(argc, argv, optstring)` | POSIX-style option parsing | Enhanced arg parsing |
| `l_getenv(name)` | Read environment variable | `env(name)` |
| `l_find_executable(cmd, out, outsz)` | Search PATH for executable | `which(cmd)` |

### File System

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_symlink(target, linkpath)` | Create symbolic link | `symlink(target, path)` |
| `l_readlink(path, buf, bufsiz)` | Read symlink target | `readlink(path)` |
| `l_realpath(path, resolved)` | Canonical absolute path | `realpath(path)` |
| `l_stat(path, st)` / `l_fstat(fd, st)` | File metadata | `file_stat(path)` |
| `l_opendir` / `l_readdir` / `l_closedir` | Directory listing | `list_dir(path)` |
| `l_rename(old, new)` | Rename/move file | `file_rename(old, new)` |
| `l_access(path, mode)` | Check file permissions | `file_exists(path)` |
| `l_getcwd(buf, size)` / `l_chdir(path)` | Working directory | `cwd()` / `chdir(path)` |

### Process Management

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_spawn(path, argv, envp)` | Spawn child process | `exec(cmd, args)` |
| `l_wait(pid, exitcode)` | Wait for child | `wait(pid)` |
| `l_pipe(fds)` / `l_dup2(old, new)` | Pipe and fd redirection | Pipeline support |

### Terminal

| `l_os.h` function | Description | Potential Oscan builtin |
|--------------------|-------------|------------------------|
| `l_term_raw()` / `l_term_restore(mode)` | Raw terminal mode | TUI / interactive input |
| `l_term_size(rows, cols)` | Terminal dimensions | `term_size()` |

---

## Appendix B: Roadmap (Feature Phases)

### Phase 1 (Delivered in v0.2)

**String Interpolation** — `"text {expr} text"` syntax now lowers to nested `str_concat` calls plus the existing string conversion helpers.

### Intentionally Reserved for Later Consideration (Phase 2+)

The following features are explicitly **NOT** in v0.1 and are deferred pending further rationale:

- **Evented / multiplexed I/O** (`poll`, nonblocking patterns) — enables truly concurrent I/O servers
- **User-defined generics** — complexity tradeoff; current `Result<T,E>` is sufficient for v0.1
- **Closures / first-class functions** — eliminates capture semantics, keeps functions simple for now
- **Traits / interfaces** — deferred; nominal typing sufficient for v0.1
- **Pattern matching on structs** — nice-to-have for convenience; match on enums covers most cases
- **Iterators** — nice-to-have; `for`-in loops and arrays sufficient for v0.1
- **Richer error ergonomics** (typed error enums) — future direction for error composability

These are documented here to confirm they are intentional deferrals, not oversights.

## Appendix C: Design Rationale Summary

| Decision                         | Rationale                                                |
|----------------------------------|----------------------------------------------------------|
| No type inference                | LLMs benefit from seeing every type explicitly            |
| No methods / no OOP             | One way to call functions — maximizes uniformity          |
| `and`/`or`/`not` as keywords    | Clearer token boundaries for LLM tokenizers              |
| Arena allocation                 | Uniform, simple, no lifetime tracking needed             |
| No closures                      | Eliminates capture semantics, keeps functions simple      |
| Mandatory type annotations       | Every binding is self-documenting                        |
| `fn` vs `fn!`                    | Purity is visible at the declaration site                |
| No generics (except Result)     | Keeps type system trivially simple for v0.1              |
| Panics for programmer bugs       | Overflow, bounds — these are bugs, not handleable errors |
| Trailing commas everywhere       | Reduces LLM diff noise when adding/removing items        |
