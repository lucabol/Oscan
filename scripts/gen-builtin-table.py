#!/usr/bin/env python3
"""Generate a categorized builtin-function table for README.md.

Reads @builtin structured comments from src/semantic.rs, groups them by
category, and either prints the markdown table to stdout or injects it
into README.md between marker comments.

Usage:
    python scripts/gen-builtin-table.py           # print to stdout
    python scripts/gen-builtin-table.py --inject   # update README.md in place
    python scripts/gen-builtin-table.py --check    # verify README.md is up to date
"""

import argparse
import re
import sys
from pathlib import Path
from collections import OrderedDict

# Category display order
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

BEGIN_MARKER = "<!-- BEGIN BUILTIN TABLE -->"
END_MARKER = "<!-- END BUILTIN TABLE -->"

BUILTIN_RE = re.compile(
    r'// @builtin'
    r'\s+category="(?P<cat>[^"]+)"'
    r'\s+name="(?P<name>[^"]+)"'
    r'\s+sig="(?P<sig>[^"]+)"'
    r'\s+desc="(?P<desc>[^"]+)"'
)


def find_repo_root() -> Path:
    """Walk up from this script to find the repo root (contains Cargo.toml)."""
    d = Path(__file__).resolve().parent
    while d != d.parent:
        if (d / "Cargo.toml").exists():
            return d
        d = d.parent
    # Fallback: assume script is in scripts/ under repo root
    return Path(__file__).resolve().parent.parent


def extract_builtins(semantic_path: Path) -> OrderedDict:
    """Parse @builtin comments and return {category: [(sig, desc), ...]}."""
    text = semantic_path.read_text(encoding="utf-8")
    groups: dict[str, list[tuple[str, str]]] = {}

    for m in BUILTIN_RE.finditer(text):
        cat = m.group("cat")
        sig = m.group("sig")
        desc = m.group("desc")
        groups.setdefault(cat, []).append((sig, desc))

    # Order by CATEGORY_ORDER, then any unknown categories alphabetically
    ordered = OrderedDict()
    for cat in CATEGORY_ORDER:
        if cat in groups:
            ordered[cat] = groups.pop(cat)
    for cat in sorted(groups.keys()):
        ordered[cat] = groups[cat]

    return ordered


def generate_table(groups: OrderedDict) -> str:
    """Build markdown table sections from grouped builtins."""
    total = sum(len(fns) for fns in groups.values())
    lines = [f"**{total} built-in functions** across {len(groups)} categories.\n"]

    for cat, fns in groups.items():
        lines.append(f"### {cat} ({len(fns)} functions)\n")
        lines.append("| Function | Description |")
        lines.append("|----------|-------------|")
        for sig, desc in fns:
            lines.append(f"| `{sig}` | {desc} |")
        lines.append("")  # blank line between categories

    return "\n".join(lines).rstrip() + "\n"


def inject_into_readme(readme_path: Path, table: str) -> str:
    """Replace content between markers in README.md. Returns new content."""
    text = readme_path.read_text(encoding="utf-8")
    begin_idx = text.find(BEGIN_MARKER)
    end_idx = text.find(END_MARKER)

    if begin_idx == -1 or end_idx == -1:
        print(f"ERROR: Markers not found in {readme_path}", file=sys.stderr)
        print(f"  Expected: {BEGIN_MARKER}", file=sys.stderr)
        print(f"       and: {END_MARKER}", file=sys.stderr)
        sys.exit(1)

    before = text[: begin_idx + len(BEGIN_MARKER)]
    after = text[end_idx:]
    return before + "\n\n" + table + "\n" + after


def main():
    parser = argparse.ArgumentParser(description="Generate builtin function table")
    parser.add_argument(
        "--inject", action="store_true", help="Update README.md in place"
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="Verify README.md is up to date (exit 1 if stale)",
    )
    args = parser.parse_args()

    root = find_repo_root()
    semantic_path = root / "src" / "semantic.rs"
    readme_path = root / "README.md"

    if not semantic_path.exists():
        print(f"ERROR: {semantic_path} not found", file=sys.stderr)
        sys.exit(1)

    groups = extract_builtins(semantic_path)
    if not groups:
        print("ERROR: No @builtin annotations found in semantic.rs", file=sys.stderr)
        sys.exit(1)

    table = generate_table(groups)

    if args.check:
        if not readme_path.exists():
            print(f"ERROR: {readme_path} not found", file=sys.stderr)
            sys.exit(1)
        expected = inject_into_readme(readme_path, table)
        actual = readme_path.read_text(encoding="utf-8")
        if expected == actual:
            total = sum(len(fns) for fns in groups.values())
            print(f"OK: README.md builtin table is up to date ({total} functions)")
            sys.exit(0)
        else:
            print(
                "ERROR: README.md builtin table is out of date.",
                file=sys.stderr,
            )
            print(
                "Run: python scripts/gen-builtin-table.py --inject",
                file=sys.stderr,
            )
            sys.exit(1)

    if args.inject:
        if not readme_path.exists():
            print(f"ERROR: {readme_path} not found", file=sys.stderr)
            sys.exit(1)
        new_content = inject_into_readme(readme_path, table)
        readme_path.write_text(new_content, encoding="utf-8")
        total = sum(len(fns) for fns in groups.values())
        print(f"Injected {total} builtins into {readme_path}")
    else:
        print(table)


if __name__ == "__main__":
    main()
