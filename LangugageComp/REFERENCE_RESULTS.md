# Five-language reference implementation results

Measured 2026-07-30 on:

- Windows 11 Enterprise 10.0.26200, 64-bit
- Intel Core i9-13950HX
- Oscan compiler at repository base
  `7291ebea65083da2c419e6466f802e0351bcac43` plus the measured collection-API
  working-tree changes, C backend
- Rust 1.94.1
- Node.js 22.14.0 and TypeScript 5.9.3
- .NET SDK 9.0.316 and Microsoft.DotNet.ILCompiler 9.0.18
- C# published for `win-x64` as self-contained Native AOT
- SBCL 2.6.7 with an uncompressed executable image

The original C# row used a framework-dependent DLL launched by `dotnet`. It
remains invalidated and has been replaced throughout this file.

## Correctness

Every implementation passed all 47 shared executable cases:

| Language | Passed | Total |
|---|---:|---:|
| Oscan | 47 | 47 |
| Rust | 47 | 47 |
| TypeScript | 47 | 47 |
| C# Native AOT | 47 | 47 |
| Common Lisp (SBCL) | 47 | 47 |
| **Combined** | **235** | **235** |

The suite covers CLI and exit behavior, parsing, validation order, duplicate
and missing dependencies, cycles, stable topological ties, critical-path ties,
64-bit totals, affected-task closure, CRLF, a 150-task chain, and a 101-task
wide DAG.

## Source and primary artifact size

| Language | Physical source lines | Source bytes | Primary artifact bytes |
|---|---:|---:|---:|
| Oscan | 486 | 15,236 | 28,672 |
| Rust | 335 | 10,935 | 193,536 |
| TypeScript | 357 | 10,687 | 9,958 |
| C# Native AOT | 381 | 11,448 | 1,546,240 |
| Common Lisp (SBCL) | 358 | 14,977 | 39,456,848 |

The TypeScript value is only the emitted JavaScript and excludes Node.js, so it
is not a self-contained deployment-size figure. The C# value is the
self-contained Native AOT executable and excludes an optional 8,015,872-byte
PDB. The Common Lisp value is the directly executable SBCL image, including the
Lisp runtime; the official Windows SBCL build used here cannot create compressed
cores because it lacks zstd support.

Among self-contained executables, Oscan is 6.75 times smaller than Rust, 53.93
times smaller than C# Native AOT, and 1,376.15 times smaller than Common Lisp.
Oscan source is 2-43% larger in bytes than each baseline source.

The collection refactor reduced the Oscan implementation from 542 lines and
16,229 bytes to 486 lines and 15,236 bytes: 56 fewer lines (10.33%) and 993
fewer bytes (6.12%). It replaced manual loops with collection primitives
without changing the BuildGraph algorithm.

## Process-level timing

The input is a 1,000-task dependency chain. Each median uses 50 fresh process
invocations after five warmups.

| Language | Analyze median | Analyze min-max | `--help` median | Approximate difference |
|---|---:|---:|---:|---:|
| Oscan | 25.515 ms | 20.897-67.269 ms | 17.509 ms | 8.006 ms |
| Rust | 32.158 ms | 25.307-55.243 ms | 28.382 ms | 3.776 ms |
| TypeScript | 101.637 ms | 92.452-124.876 ms | 81.953 ms | 19.684 ms |
| C# Native AOT | 39.492 ms | 32.360-56.614 ms | 36.734 ms | 2.758 ms |
| Common Lisp (SBCL) | 86.305 ms | 75.805-774.949 ms | 63.028 ms | 23.277 ms |

These are process-level measurements on one Windows host, not in-process
algorithm timings. The difference subtracts independently measured medians and
is diagnostic only. Oscan had the lowest process median in this rerun: Rust was
1.26 times, C# Native AOT 1.55 times, Common Lisp 3.38 times, and TypeScript/Node
3.98 times Oscan. The ranking changed from the prior run, so it
should not be treated as an isolated algorithm-speed result.

## C# correction impact

| C# mode | Invocation | Primary artifact | Analyze median | Startup median |
|---|---|---:|---:|---:|
| Framework-dependent, **invalidated** | `dotnet BuildGraph.dll` | 17,920 B | 92.715 ms | 79.220 ms |
| Native AOT, corrected, current rerun | `BuildGraph.exe` | 1,546,240 B | 39.492 ms | 36.734 ms |

The current process median is 57.41% below the old value and the startup
median is 53.63% below it, while the honest self-contained artifact is 86.29
times larger than the old application-only DLL. The timing comparison is
descriptive, not a causal estimate of AOT alone: the invalid run used 20
iterations, two warmups, and a .NET 10 RC SDK, while this run used 50
iterations, five warmups, and the pinned .NET 9 SDK.

## Interpretation

These results establish semantic parity and characterize five reviewed
reference implementations. They show that Oscan produced by far the smallest
self-contained artifact and the lowest process median in this rerun, but also
the largest source. The collection API narrowed that source gap. Common Lisp
adds a distinct tradeoff: source length close to TypeScript, cold startup below
Node in this run, and a much larger runtime image. None of these results
establishes that an LLM is more likely to generate correct Oscan. See
`EXPERIMENT_REPORT.md` for the complete analysis and the required repeated
one-shot protocol.
