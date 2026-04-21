#!/usr/bin/env python3
"""Generate the Oscan language reference for Copilot instruction files.

Reads @builtin structured comments from src/semantic.rs, selected example
files from examples/, and combines with an embedded language-reference
template to produce .github/instructions/oscan.instructions.md.

Usage:
    python scripts/gen-copilot-instructions.py           # print to stdout
    python scripts/gen-copilot-instructions.py --inject   # update file in place
    python scripts/gen-copilot-instructions.py --check    # verify file is up to date
"""

import argparse
import re
import sys
from pathlib import Path
from collections import OrderedDict

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

CATEGORY_ORDER = [
    "I/O",
    "String",
    "Conversion",
    "Character",
    "Math",
    "Bitwise",
    "File I/O",
    "Filesystem",
    "Path",
    "Socket",
    "HashMap",
    "Array",
    "Date/Time",
    "System",
    "Environment",
    "Terminal",
    "Process",
    "Graphics",
    "TrueType",
]

# Examples to include (order matters for readability)
EXAMPLE_FILES = [
    "hello.osc",
    "fibonacci.osc",
    "error_handling.osc",
    "file_io.osc",
    "string_interpolation.osc",
    "countlines.osc",
    "word_freq.osc",
    "hexdump.osc",
]

BEGIN_MARKER = "<!-- BEGIN OSCAN INSTRUCTIONS -->"
END_MARKER = "<!-- END OSCAN INSTRUCTIONS -->"

BUILTIN_RE = re.compile(
    r'// @builtin'
    r'\s+category="(?P<cat>[^"]+)"'
    r'\s+name="(?P<name>[^"]+)"'
    r'\s+sig="(?P<sig>[^"]+)"'
    r'\s+desc="(?P<desc>[^"]+)"'
)

# ---------------------------------------------------------------------------
# Static content: Critical Differences & Anti-Patterns
# ---------------------------------------------------------------------------

CRITICAL_DIFFERENCES = """\
## Critical Differences from C/Rust

These are the features that most often trip up code-generation models.

### Purity system
- `fn` = **pure** (no I/O, no extern calls, no `fn!` calls).
- `fn!` = **impure** (can do anything: I/O, extern, mutable globals).
- `main` must be declared `fn! main()`.

### Logical operators
- Use `and`, `or`, `not` — **never** `&&`, `||`, `!`.

### Error handling
- `try expr` propagates errors (like Rust `?` but **prefix**, not postfix).
- The enclosing function must return a compatible `Result<T, E>`.
- Results are always qualified: `Result::Ok(val)`, `Result::Err(msg)`.

### Type annotations are mandatory
- Every `let` binding needs an explicit type: `let x: i32 = 5;`
- No type inference on bindings.

### Semicolons after blocks
- Blocks used as statements need a trailing semicolon:
  `if cond { ... } else { ... };`
  `while cond { ... };`
  `for x in xs { ... };`
  `match expr { ... };`

### Match
- `match` is exhaustive — use `_` for wildcard.
- `match` is an expression (can return a value).

### Arrays
- `[i32]` = dynamic array, `[i32; 5]` = fixed-size.
- Free functions, not methods: `push(arr, val)`, `len(arr)`, `pop(arr)`.

### Ranges
- `for i in 0..n { };` — exclusive upper bound.

### String interpolation
- `"hello {name}"` — expressions inside `{}`.
- Literal braces: `{{` and `}}`.

### Enums
- Variants are always qualified: `Shape::Circle(r)`, never `Circle(r)`.

### Defer
- `defer expr;` — cleanup runs at end of scope, LIFO order.
- Only available in `fn!` functions.

### Arena
- `arena { ... };` — scoped memory reclamation for long-running programs.

### Type casts
- `as` keyword: `x as i64`. Only 8 pairs: i32↔i64, i32↔f64, i64↔f64, handle↔i64.
- No implicit coercions ever. No null. No exceptions.

### Parameters
- Always immutable and passed by value.

### Structs
- Field order in struct literals doesn't need to match declaration order.

### Top-level `let`
- Top-level `let` bindings are constants.

### Imports
- `use "path/file.osc";` — no module system, just file inclusion.

### C FFI
- `extern { fn c_function(a: i32) -> i32; }` for calling C functions.
- `handle` type for opaque C pointers (compiles to `uintptr_t`).
- `--extra-c <file>` to link additional C source files (repeatable).
- `--extra-cflags <flag>` to pass extra flags to the C compiler (repeatable).

### Function pointers
- `let f: fn(i32) -> i32 = add;` — only user-defined fns, not builtins.

### Comments
- `//` line comments, `/* */` block comments (no nesting).
"""

ANTI_PATTERNS = """\
## Common Anti-Patterns

What **not** to write → what to write instead:

| ❌ Wrong | ✅ Correct | Why |
|----------|-----------|-----|
| `let x = 5;` | `let x: i32 = 5;` | Type annotation required |
| `if x > 0 && y > 0` | `if x > 0 and y > 0` | Use `and` not `&&` |
| `!flag` | `not flag` | Use `not` not `!` |
| `x?` | `try x` | Prefix `try`, not postfix `?` |
| `arr.push(val)` | `push(arr, val)` | Free function, not method |
| `arr.len()` | `len(arr)` | Free function, not method |
| `Ok(val)` | `Result::Ok(val)` | Must qualify with `Result::` |
| `println!("text")` | `println("text")` | No macro syntax |
| missing `;` after `};` | `if ... { } else { };` | Semicolons after block statements |
| `fn main()` | `fn! main()` | main must be impure |
"""

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def find_repo_root() -> Path:
    """Walk up from this script to find the repo root (contains Cargo.toml)."""
    d = Path(__file__).resolve().parent
    while d != d.parent:
        if (d / "Cargo.toml").exists():
            return d
        d = d.parent
    return Path(__file__).resolve().parent.parent


def extract_builtins(semantic_path: Path) -> OrderedDict:
    """Parse @builtin comments and return {category: [(name, sig), ...]}."""
    text = semantic_path.read_text(encoding="utf-8")
    groups: dict[str, list[tuple[str, str]]] = {}

    for m in BUILTIN_RE.finditer(text):
        cat = m.group("cat")
        name = m.group("name")
        sig = m.group("sig")
        groups.setdefault(cat, []).append((name, sig))

    ordered = OrderedDict()
    for cat in CATEGORY_ORDER:
        if cat in groups:
            ordered[cat] = groups.pop(cat)
    for cat in sorted(groups.keys()):
        ordered[cat] = groups[cat]

    return ordered


def generate_compact_builtin_table(groups: OrderedDict) -> str:
    """Build a compact builtin reference: category headers + signatures."""
    total = sum(len(fns) for fns in groups.values())
    lines = [
        f"## Built-in Functions ({total} functions, {len(groups)} categories)\n",
    ]

    for cat, fns in groups.items():
        lines.append(f"### {cat}\n")
        lines.append("```")
        for _name, sig in fns:
            lines.append(sig)
        lines.append("```\n")

    return "\n".join(lines).rstrip() + "\n"


def read_examples(examples_dir: Path) -> str:
    """Read selected example files and format them as markdown sections."""
    lines = ["## Annotated Examples\n"]
    lines.append(
        "These are real Oscan programs from the `examples/` directory. "
        "Study them to learn idiomatic patterns.\n"
    )

    for filename in EXAMPLE_FILES:
        path = examples_dir / filename
        if not path.exists():
            print(f"WARNING: {path} not found, skipping", file=sys.stderr)
            continue
        content = path.read_text(encoding="utf-8").rstrip()
        lines.append(f"### `{filename}`\n")
        lines.append(f"```osc\n{content}\n```\n")

    return "\n".join(lines).rstrip() + "\n"


def assemble_instructions(groups: OrderedDict, examples_dir: Path) -> str:
    """Combine all sections into the full instruction content."""
    parts = [
        CRITICAL_DIFFERENCES,
        ANTI_PATTERNS,
        read_examples(examples_dir),
        generate_compact_builtin_table(groups),
    ]
    return "\n".join(parts).rstrip() + "\n"


def inject_into_file(file_path: Path, content: str) -> str:
    """Replace content between markers. Returns new file content."""
    text = file_path.read_text(encoding="utf-8")
    begin_idx = text.find(BEGIN_MARKER)
    end_idx = text.find(END_MARKER)

    if begin_idx == -1 or end_idx == -1:
        print(f"ERROR: Markers not found in {file_path}", file=sys.stderr)
        print(f"  Expected: {BEGIN_MARKER}", file=sys.stderr)
        print(f"       and: {END_MARKER}", file=sys.stderr)
        sys.exit(1)

    before = text[: begin_idx + len(BEGIN_MARKER)]
    after = text[end_idx:]
    return before + "\n\n" + content + "\n" + after


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main():
    parser = argparse.ArgumentParser(
        description="Generate Oscan Copilot language instructions"
    )
    parser.add_argument(
        "--inject", action="store_true", help="Update instructions file in place"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Verify instructions file is up to date (exit 1 if stale)",
    )
    args = parser.parse_args()

    root = find_repo_root()
    semantic_path = root / "src" / "semantic.rs"
    examples_dir = root / "examples"
    instructions_dir = root / ".github" / "instructions"
    instructions_path = instructions_dir / "oscan.instructions.md"

    if not semantic_path.exists():
        print(f"ERROR: {semantic_path} not found", file=sys.stderr)
        sys.exit(1)

    groups = extract_builtins(semantic_path)
    if not groups:
        print("ERROR: No @builtin annotations found in semantic.rs", file=sys.stderr)
        sys.exit(1)

    content = assemble_instructions(groups, examples_dir)

    if args.check:
        if not instructions_path.exists():
            print(f"ERROR: {instructions_path} not found", file=sys.stderr)
            sys.exit(1)
        expected = inject_into_file(instructions_path, content)
        actual = instructions_path.read_text(encoding="utf-8")
        if expected == actual:
            total = sum(len(fns) for fns in groups.values())
            print(
                f"OK: instructions file is up to date "
                f"({total} builtins, {len(EXAMPLE_FILES)} examples)"
            )
            sys.exit(0)
        else:
            print(
                "ERROR: instructions file is out of date.",
                file=sys.stderr,
            )
            print(
                "Run: python scripts/gen-copilot-instructions.py --inject",
                file=sys.stderr,
            )
            sys.exit(1)

    if args.inject:
        instructions_dir.mkdir(parents=True, exist_ok=True)
        if not instructions_path.exists():
            print(f"ERROR: {instructions_path} not found", file=sys.stderr)
            print(
                "Create the template file with BEGIN/END markers first.",
                file=sys.stderr,
            )
            sys.exit(1)
        new_content = inject_into_file(instructions_path, content)
        instructions_path.write_text(new_content, encoding="utf-8")
        total = sum(len(fns) for fns in groups.values())
        size_kb = len(new_content.encode("utf-8")) / 1024
        print(
            f"Injected {total} builtins + {len(EXAMPLE_FILES)} examples "
            f"into {instructions_path} ({size_kb:.1f} KB)"
        )
    else:
        print(content)


if __name__ == "__main__":
    main()
