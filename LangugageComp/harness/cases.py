"""Language-neutral executable test cases for BuildGraph."""

from __future__ import annotations

from dataclasses import dataclass


HELP = (
    "BuildGraph - deterministic dependency graph analyzer\n\n"
    "usage:\n"
    "  buildgraph analyze <file>\n"
    "  buildgraph affected <file> <task>\n"
    "  buildgraph --help\n"
)
USAGE_ERROR = (
    "usage error: expected 'analyze <file>' or 'affected <file> <task>'\n"
)
SAMPLE = (
    "fetch | 3 |\n"
    "parse | 5 | fetch\n"
    "lint | 4 | fetch\n"
    "compile | 8 | parse\n"
    "test | 6 | compile, lint\n"
    "package | 2 | test\n"
)


@dataclass(frozen=True)
class TestCase:
    name: str
    args: tuple[str, ...]
    code: int
    stdout: str = ""
    stderr: str = ""
    content: str | bytes | None = None


def analyzed(
    names: list[str], durations: dict[str, int], path: list[str] | None = None
) -> str:
    chosen_path = path if path is not None else names
    return (
        f"tasks: {len(names)}\n"
        f"order: {', '.join(names)}\n"
        f"critical-duration: {sum(durations[name] for name in chosen_path)}\n"
        f"critical-path: {' -> '.join(chosen_path)}\n"
    )


def error_case(name: str, content: str | bytes, message: str) -> TestCase:
    return TestCase(
        name,
        ("analyze", "{file}"),
        4,
        stderr=f"input error: {message}\n",
        content=content,
    )


def all_cases() -> list[TestCase]:
    cases = [
        TestCase("help-long", ("--help",), 0, HELP),
        TestCase("help-short", ("-h",), 0, HELP),
        TestCase("usage-empty", (), 2, stderr=USAGE_ERROR),
        TestCase(
            "usage-command",
            ("inspect", "{file}"),
            2,
            stderr=USAGE_ERROR,
            content="a | 1 |\n",
        ),
        TestCase(
            "usage-extra",
            ("analyze", "{file}", "extra"),
            2,
            stderr=USAGE_ERROR,
            content="a | 1 |\n",
        ),
        TestCase(
            "usage-affected-missing-task",
            ("affected", "{file}"),
            2,
            stderr=USAGE_ERROR,
            content="a | 1 |\n",
        ),
        TestCase(
            "io-missing",
            ("analyze", "missing.bg"),
            3,
            stderr="io error: unable to read 'missing.bg'\n",
        ),
        error_case("no-tasks-empty", "", "no tasks"),
        error_case("no-tasks-comments", " # comment\n \t\n", "no tasks"),
        error_case(
            "fields-too-few",
            "a | 1\n",
            "line 1: expected exactly three '|' separated fields",
        ),
        error_case(
            "fields-too-many",
            "a | 1 | | extra\n",
            "line 1: expected exactly three '|' separated fields",
        ),
        error_case(
            "identifier-empty", " | 1 |\n", "line 1: invalid task identifier ''"
        ),
        error_case(
            "identifier-leading-digit",
            "1a | 1 |\n",
            "line 1: invalid task identifier '1a'",
        ),
        error_case(
            "identifier-symbol",
            "a.b | 1 |\n",
            "line 1: invalid task identifier 'a.b'",
        ),
        error_case(
            "identifier-nonascii",
            "é | 1 |\n",
            "line 1: invalid task identifier 'é'",
        ),
        error_case(
            "identifier-too-long",
            f"{'a' * 33} | 1 |\n",
            f"line 1: invalid task identifier '{'a' * 33}'",
        ),
        error_case("duration-empty", "a | |\n", "line 1: invalid duration ''"),
        error_case(
            "duration-negative", "a | -1 |\n", "line 1: invalid duration '-1'"
        ),
        error_case("duration-plus", "a | +1 |\n", "line 1: invalid duration '+1'"),
        error_case("duration-zero", "a | 0 |\n", "line 1: invalid duration '0'"),
        error_case(
            "duration-too-large",
            "a | 2147483648 |\n",
            "line 1: invalid duration '2147483648'",
        ),
        error_case(
            "duration-overflow",
            "a | 999999999999999999999999 |\n",
            "line 1: invalid duration '999999999999999999999999'",
        ),
        error_case(
            "duplicate-task", "a | 1 |\na | 2 |\n", "line 2: duplicate task 'a'"
        ),
        error_case(
            "empty-dependency",
            "a | 1 |\nb | 1 | a,\n",
            "line 2: empty dependency for task 'b'",
        ),
        error_case(
            "invalid-dependency",
            "a | 1 |\nb | 1 | .a\n",
            "line 2: invalid dependency identifier '.a'",
        ),
        error_case(
            "self-dependency",
            "a | 1 | a\n",
            "line 1: task 'a' depends on itself",
        ),
        error_case(
            "duplicate-dependency",
            "a | 1 |\nb | 1 | a, a\n",
            "line 2: duplicate dependency 'a' for task 'b'",
        ),
        error_case(
            "unknown-dependency",
            "a | 1 | missing\n",
            "line 1: unknown dependency 'missing' for task 'a'",
        ),
        error_case("cycle-two", "a | 1 | b\nb | 1 | a\n", "cycle detected"),
        error_case(
            "cycle-after-root",
            "root | 1 |\na | 1 | b\nb | 1 | a\n",
            "cycle detected",
        ),
        TestCase(
            "single",
            ("analyze", "{file}"),
            0,
            analyzed(["one"], {"one": 5}),
            content="one | 0005 |\n",
        ),
        TestCase(
            "sample",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 6\n"
                "order: fetch, parse, lint, compile, test, package\n"
                "critical-duration: 24\n"
                "critical-path: fetch -> parse -> compile -> test -> package\n"
            ),
            content=SAMPLE,
        ),
        TestCase(
            "forward-reference",
            ("analyze", "{file}"),
            0,
            analyzed(["base", "late"], {"base": 3, "late": 2}),
            content="late | 2 | base\nbase | 3 |\n",
        ),
        TestCase(
            "stable-root-and-critical-tie",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 3\n"
                "order: first, second, join\n"
                "critical-duration: 2\n"
                "critical-path: first -> join\n"
            ),
            content="first | 1 |\nsecond | 1 |\njoin | 1 | second, first\n",
        ),
        TestCase(
            "newly-ready-order",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 3\n"
                "order: b, c, a\n"
                "critical-duration: 2\n"
                "critical-path: c -> a\n"
            ),
            content="a | 1 | c\nb | 1 |\nc | 1 |\n",
        ),
        TestCase(
            "deep-critical-tie",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 5\n"
                "order: a, b, c, d, end\n"
                "critical-duration: 3\n"
                "critical-path: a -> d -> end\n"
            ),
            content=(
                "a | 1 |\nb | 1 |\nc | 1 | b\n"
                "d | 1 | a\nend | 1 | c, d\n"
            ),
        ),
        TestCase(
            "i64-critical-duration",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 2\n"
                "order: a, b\n"
                "critical-duration: 4294967294\n"
                "critical-path: a -> b\n"
            ),
            content="a | 2147483647 |\nb | 2147483647 | a\n",
        ),
        TestCase(
            "disconnected-critical",
            ("analyze", "{file}"),
            0,
            (
                "tasks: 3\n"
                "order: a, b, c\n"
                "critical-duration: 4\n"
                "critical-path: a\n"
            ),
            content="a | 4 |\nb | 1 |\nc | 2 | b\n",
        ),
        TestCase(
            "crlf-comments",
            ("analyze", "{file}"),
            0,
            analyzed(["a", "b"], {"a": 1, "b": 2}),
            content=b" # comment\r\n\r\na | 1 |\r\nb | 2 | a\r\n",
        ),
        TestCase(
            "affected-sample-middle",
            ("affected", "{file}", "parse"),
            0,
            "affected: parse, compile, test, package\n",
            content=SAMPLE,
        ),
        TestCase(
            "affected-root",
            ("affected", "{file}", "fetch"),
            0,
            "affected: fetch, parse, lint, compile, test, package\n",
            content=SAMPLE,
        ),
        TestCase(
            "affected-leaf",
            ("affected", "{file}", "package"),
            0,
            "affected: package\n",
            content=SAMPLE,
        ),
        TestCase(
            "affected-disconnected",
            ("affected", "{file}", "b"),
            0,
            "affected: b, c\n",
            content="a | 4 |\nb | 1 |\nc | 2 | b\n",
        ),
        TestCase(
            "affected-unknown",
            ("affected", "{file}", "missing"),
            4,
            stderr="input error: unknown task 'missing'\n",
            content="a | 1 |\n",
        ),
    ]

    chain_count = 150
    chain_names = [f"task{index}" for index in range(chain_count)]
    chain_lines = ["task0 | 1 |"] + [
        f"task{index} | 1 | task{index - 1}" for index in range(1, chain_count)
    ]
    chain = "\n".join(chain_lines) + "\n"
    cases.extend(
        [
            TestCase(
                "stress-deep-chain",
                ("analyze", "{file}"),
                0,
                analyzed(chain_names, {name: 1 for name in chain_names}),
                content=chain,
            ),
            TestCase(
                "stress-deep-affected",
                ("affected", "{file}", "task75"),
                0,
                f"affected: {', '.join(chain_names[75:])}\n",
                content=chain,
            ),
        ]
    )

    width = 100
    root_names = [f"root{index}" for index in range(width)]
    wide = "\n".join(
        [f"{name} | 1 |" for name in root_names]
        + [f"sink | 2 | {', '.join(reversed(root_names))}"]
    )
    cases.append(
        TestCase(
            "stress-wide-dag",
            ("analyze", "{file}"),
            0,
            (
                f"tasks: {width + 1}\n"
                f"order: {', '.join(root_names)}, sink\n"
                "critical-duration: 3\n"
                "critical-path: root0 -> sink\n"
            ),
            content=wide + "\n",
        )
    )
    return cases
