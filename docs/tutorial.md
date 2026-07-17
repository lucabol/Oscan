# Build a Tiny `wc` in Oscan

In this tutorial, you will grow a "Hello, world" program into `oscwc`, a
command-line tool that counts lines, words, and bytes in a text file.

It takes about 60-90 minutes and assumes you already know basic programming
ideas such as variables, functions, conditions, and loops. No C or systems
programming experience is required.

By the end, you will have used:

- the Oscan compiler and its `--run` workflow;
- explicit types and immutable-by-default bindings;
- loops, conditions, arrays, and string interpolation;
- pure `fn` and side-effecting `fn!` functions;
- structs;
- exhaustive `match` and `Result` error handling; and
- standalone executable and generated C output.

Every complete program below is runnable. Replace the contents of `oscwc.osc`
at each checkpoint rather than trying to combine the checkpoints.

## 1. Install Oscan

Current prebuilt releases target x86-64. Download the archive for your
operating system from
[GitHub Releases](https://github.com/lucabol/Oscan/releases):

- **Windows x86-64 and Linux x86-64:** choose the `full` bundle, which includes
  a C toolchain.
- **macOS x86-64:** use the macOS archive and install Xcode Command Line Tools
  first with
  `xcode-select --install`.

Extract the bundle, add its directory to `PATH`, and verify the installation:

```text
oscan --help
```

For another host architecture, or if you prefer to build the compiler
yourself, follow the [source installation guide](guide.md#build-from-source).

Create a new directory for the tutorial and open a terminal there:

```text
mkdir oscan-tutorial
cd oscan-tutorial
```

## 2. Run your first program

Create `oscwc.osc`:

```osc
fn! main() {
    println("Hello, Oscan!");
}
```

Compile and run it:

```text
oscan oscwc.osc --run
```

You should see:

```text
Hello, Oscan!
```

`main` is the program's entry point. The `!` in `fn!` says that the function
may have side effects. Printing is a side effect, so `main` cannot be a pure
`fn`.

Statements end in semicolons. Oscan deliberately makes effects and statement
boundaries visible.

### Try it

Add a typed binding and interpolate it into the greeting:

```osc
fn! main() {
    let name: str = "Ada";
    println("Hello, {name}!");
}
```

Every `let` binding needs a type annotation. Here, `name` has type `str`.

## 3. Count lines and bytes

Replace `oscwc.osc` with:

```osc
fn! main() {
    let text: str = "one two\nthree\n";
    let bytes: [i32] = str_to_chars(text);
    let mut lines: i32 = 0;
    let mut byte_count: i32 = 0;

    for byte in bytes {
        byte_count += 1;
        if byte == 10 {
            lines += 1;
        };
    };

    println("Lines: {lines}");
    println("Bytes: {byte_count}");
}
```

Run it:

```text
oscan oscwc.osc --run
```

Expected output:

```text
Lines: 2
Bytes: 14
```

There are several new ideas here:

- `[i32]` is a dynamic array of 32-bit integers.
- `str_to_chars` exposes each UTF-8 byte as an `i32`.
- Bindings are immutable unless they use `let mut`.
- `for byte in bytes` visits every element.
- `if` and `for` blocks used as statements end with `;`.
- `+=` updates a mutable numeric binding.

This is a byte count, not a count of Unicode code points or user-perceived
characters. The distinction matters for non-ASCII text.

### Try it

Change the text, predict both counts, and run the program again.

### Break it on purpose

Remove `mut` from `lines`, then compile again. Read the diagnostic before
restoring `mut`. Compiler errors are part of the Oscan workflow: the compiler
is explaining which language rule the program violated.

## 4. Count words with a pure function

A word begins when a non-whitespace character follows whitespace or the start
of the input. Add a helper that recognizes common ASCII whitespace.

Replace `oscwc.osc` with:

```osc
fn is_whitespace(byte: i32) -> bool {
    byte == 32 or byte == 9 or byte == 10 or byte == 13
}

fn! main() {
    let text: str = "one two\nthree\n";
    let bytes: [i32] = str_to_chars(text);
    let mut lines: i32 = 0;
    let mut words: i32 = 0;
    let mut byte_count: i32 = 0;
    let mut in_word: bool = false;

    for byte in bytes {
        byte_count += 1;

        if byte == 10 {
            lines += 1;
        };

        if is_whitespace(byte) {
            in_word = false;
        } else if not in_word {
            in_word = true;
            words += 1;
        };
    };

    println("Lines: {lines}");
    println("Words: {words}");
    println("Bytes: {byte_count}");
}
```

Expected output:

```text
Lines: 2
Words: 3
Bytes: 14
```

`is_whitespace` is declared with `fn`, not `fn!`. It is pure: it depends only
on its argument and has no side effects. A pure function may call other pure
functions, but it cannot print, perform I/O, call `fn!`, or call C.

The last expression in a function body is its return value. That is why
`is_whitespace` does not need an explicit `return`.

Oscan uses the words `and`, `or`, and `not` for Boolean logic. It does not use
`&&`, `||`, or `!`.

### Try it

Change the input to include several adjacent spaces and tabs. The word count
should not increase for empty gaps.

### Break it on purpose

Add `println("checking");` inside `is_whitespace`. The compiler rejects the
program because a pure function cannot perform output. Remove the line to
continue.

## 5. Read a command-line argument and a file

The program becomes useful when it accepts a path. Reading a file can fail, so
this checkpoint also introduces Oscan's `Result` type.

Replace `oscwc.osc` with:

```osc
fn is_whitespace(byte: i32) -> bool {
    byte == 32 or byte == 9 or byte == 10 or byte == 13
}

fn! main() {
    if arg_count() < 2 {
        println("Usage: oscwc <filename>");
        exit(1);
    };

    let path: str = arg_get(1);

    match read_file(path) {
        Result::Ok(text) => {
            let bytes: [i32] = str_to_chars(text);
            let mut lines: i32 = 0;
            let mut words: i32 = 0;
            let mut in_word: bool = false;

            for byte in bytes {
                if byte == 10 {
                    lines += 1;
                };

                if is_whitespace(byte) {
                    in_word = false;
                } else if not in_word {
                    in_word = true;
                    words += 1;
                };
            };

            let byte_count: i32 = len(bytes);
            println("{lines} {words} {byte_count} {path}");
        },
        Result::Err(message) => {
            println("Error: {message}");
            exit(1);
        },
    };
}
```

`arg_count()` includes the program name. `arg_get(1)` therefore returns the
first argument supplied by the user.

`read_file(path)` returns a `Result<str, str>`:

- `Result::Ok(text)` contains the file contents.
- `Result::Err(message)` contains an error message.

`match` must handle both variants. The names `text` and `message` bind each
variant's payload inside its arm. Expected failures use `Result`; panics are
reserved for program bugs such as out-of-bounds indexing or division by zero.

### Create a repeatable sample

Create `sample.txt` with exact LF line endings.

**PowerShell:**

```powershell
[IO.File]::WriteAllText("sample.txt", "Hello Oscan`nSmall language, clear code.`n")
```

**Linux or macOS shell:**

```bash
printf 'Hello Oscan\nSmall language, clear code.\n' > sample.txt
```

Pass the path after the source file and `--run`:

```text
oscan oscwc.osc --run sample.txt
```

Expected output:

```text
2 6 40 sample.txt
```

Now try a missing path:

```text
oscan oscwc.osc --run missing.txt
```

The exact operating-system message varies, but the program should print an
error and exit unsuccessfully.

### Break it on purpose

Remove the `Result::Err` arm. The compiler rejects the non-exhaustive `match`.
Restore the arm before continuing.

## 6. Model the result with a struct

The three counters describe one result. A struct lets us name that concept and
move the counting algorithm into a reusable pure function.

Replace `oscwc.osc` with the finished program:

```osc
struct Stats {
    lines: i32,
    words: i32,
    bytes: i32,
}

fn is_whitespace(byte: i32) -> bool {
    byte == 32 or byte == 9 or byte == 10 or byte == 13
}

fn analyze(bytes: [i32]) -> Stats {
    let byte_count: i32 = len(bytes);
    let mut lines: i32 = 0;
    let mut words: i32 = 0;
    let mut in_word: bool = false;

    for byte in bytes {
        if byte == 10 {
            lines += 1;
        };

        if is_whitespace(byte) {
            in_word = false;
        } else if not in_word {
            in_word = true;
            words += 1;
        };
    };

    Stats {
        lines: lines,
        words: words,
        bytes: byte_count,
    }
}

fn! main() {
    if arg_count() < 2 {
        println("Usage: oscwc <filename>");
        exit(1);
    };

    let path: str = arg_get(1);

    match read_file(path) {
        Result::Ok(text) => {
            let bytes: [i32] = str_to_chars(text);
            let stats: Stats = analyze(bytes);
            println("{stats.lines} {stats.words} {stats.bytes} {path}");
        },
        Result::Err(message) => {
            println("Error: {message}");
            exit(1);
        },
    };
}
```

`Stats` groups related fields under one type. `analyze` can remain pure because
it only examines its input and constructs a value. File access, conversion
that allocates, printing, arguments, and exiting remain in the impure `main`.

Run the final program:

```text
oscan oscwc.osc --run sample.txt
```

You should still see:

```text
2 6 40 sample.txt
```

### Try it

Add a `whitespace: i32` field to `Stats`, count whitespace characters in
`analyze`, and include the value in the output.

## 7. Build a standalone executable

So far, `--run` has compiled and immediately executed a temporary program.
Without `--run`, Oscan writes a standalone executable:

```text
oscan oscwc.osc
```

Run it on Windows:

```powershell
.\oscwc.exe sample.txt
```

Run it on Linux or macOS:

```bash
./oscwc sample.txt
```

The output should be the same:

```text
2 6 40 sample.txt
```

Oscan normally transpiles your program to C and invokes a C toolchain. To keep
the generated C instead of building an executable:

```text
oscan oscwc.osc -o oscwc.c
```

You do not need to understand the generated file to use Oscan, but it can be
useful when embedding code or inspecting what the compiler produced.

## 8. Check edge cases

Oscan does not currently provide a user-facing unit-test command, so use a few
small files to verify this tutorial program:

| Input | Expected behavior |
|---|---|
| Empty file | `0 0 0 <path>` |
| `hello` with no final newline | `0 1 5 <path>` |
| `hello` followed by LF | `1 1 6 <path>` |
| Spaces and tabs between two words | Two words |
| Missing path | Error message and unsuccessful exit |

This program counts newline characters, matching the traditional core of
`wc -l`; a final unterminated line does not add to the line count.

## What you learned

- `fn!` marks code that can perform side effects; `fn` is pure.
- Every `let` binding has an explicit type.
- Bindings are immutable unless declared with `let mut`.
- Control-flow blocks used as statements may end with `;`.
- Boolean operators are `and`, `or`, and `not`.
- Structs group related values.
- Fallible operations return `Result`, and `match` handles every outcome.
- `--run` is the fast edit-run loop; omitting it creates an executable.
- A `.c` output path keeps Oscan's generated C.

## Where to go next

1. Learn error propagation with `try` in
   [`examples/error_handling.osc`](../examples/error_handling.osc).
2. Compare high-level and low-level file I/O in
   [`examples/file_io.osc`](../examples/file_io.osc).
3. Extend this project with maps and sorting using
   [`examples/word_freq.osc`](../examples/word_freq.osc).
4. Read the concise [Language Guide](guide.md).
5. Look up runtime APIs in the [Built-in Function Reference](builtins.md).
6. Use the [Language Specification](spec/oscan-spec.md) for definitive
   semantics.

Networking, graphics, C FFI, arena blocks, and cross-compilation are valuable
Oscan features, but they are deliberately outside this first tutorial. The
[example catalog](../README.md#examples) provides runnable starting points
when you are ready for them.
