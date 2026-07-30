# BuildGraph contract

BuildGraph is a deterministic command-line dependency graph analyzer.

## Commands

```text
buildgraph analyze <file>
buildgraph affected <file> <task>
buildgraph --help
```

Exit codes:

| Code | Meaning |
|---:|---|
| 0 | Success |
| 2 | Invalid command-line usage |
| 3 | Input file could not be read |
| 4 | Input graph or queried task is invalid |

Diagnostics are written to standard error with one of the fixed prefixes
`usage error:`, `io error:`, or `input error:`. Successful output is written to
standard output. Output uses UTF-8 and LF line endings.

## Input

Each nonblank, noncomment line defines one task:

```text
name | duration | dependency-1, dependency-2
```

- Leading and trailing whitespace around lines, fields, and dependencies is ignored.
- A full line whose first nonwhitespace character is `#` is a comment.
- Each task line has exactly three `|`-separated fields.
- An identifier is 1-32 ASCII characters, begins with `A-Z` or `a-z`, and then
  contains only ASCII letters, digits, `_`, or `-`.
- Duration contains only decimal digits and is in `1..2147483647`.
- An empty third field means no dependencies.
- Empty dependency items, duplicate tasks, duplicate dependencies, unknown
  dependencies, self-dependencies, invalid records, and cycles are errors.
- LF and CRLF inputs are accepted.
- A graph must contain at least one task.

Validation is deterministic. Records are checked in source order, followed by
dependency lists in source order.

## Ordering and analysis

Task declaration index is the universal tie-break.

- Topological order uses stable Kahn sorting: choose the earliest-declared
  currently-ready task.
- Critical duration is the sum of task durations on the longest dependency path
  and uses checked signed 64-bit arithmetic.
- Equal-duration critical paths are compared lexicographically by their sequence
  of declaration indices; the smaller path wins.
- `affected` includes the queried task and every direct or indirect dependent,
  printed in stable topological order.

`analyze` prints:

```text
tasks: <count>
order: <comma-separated task names>
critical-duration: <duration>
critical-path: <task names separated by " -> ">
```

`affected` prints:

```text
affected: <comma-separated task names>
```

## Fixed diagnostics

The implementations use these messages:

```text
usage error: expected 'analyze <file>' or 'affected <file> <task>'
io error: unable to read '<path>'
input error: line <n>: expected exactly three '|' separated fields
input error: line <n>: invalid task identifier '<value>'
input error: line <n>: invalid duration '<value>'
input error: line <n>: duplicate task '<name>'
input error: no tasks
input error: line <n>: empty dependency for task '<name>'
input error: line <n>: invalid dependency identifier '<value>'
input error: line <n>: task '<name>' depends on itself
input error: line <n>: duplicate dependency '<dependency>' for task '<name>'
input error: line <n>: unknown dependency '<dependency>' for task '<name>'
input error: cycle detected
input error: critical duration overflow
input error: unknown task '<name>'
```

