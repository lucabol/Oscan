# Five-language reference implementation results

Measured 2026-07-30 on:

- Windows 11 Enterprise 10.0.26200, 64-bit
- Intel Core i9-13950HX
- Oscan compiler at repository commit
  `c620ecb8ffaa28ce6e445db2d54474fcbbb24a90`, C backend
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
| Oscan | 542 | 16,229 | 27,136 |
| Rust | 335 | 10,600 | 193,536 |
| TypeScript | 357 | 10,687 | 9,958 |
| C# Native AOT | 381 | 11,448 | 1,546,240 |
| Common Lisp (SBCL) | 358 | 14,977 | 39,522,400 |

The TypeScript value is only the emitted JavaScript and excludes Node.js, so it
is not a self-contained deployment-size figure. The C# value is the
self-contained Native AOT executable and excludes an optional 8,015,872-byte
PDB. The Common Lisp value is the directly executable SBCL image, including the
Lisp runtime; the official Windows SBCL build used here cannot create compressed
cores because it lacks zstd support.

Among self-contained executables, Oscan is 7.13 times smaller than Rust, 56.98
times smaller than C# Native AOT, and 1,456.46 times smaller than Common Lisp.
Oscan source is 8-53% larger in bytes than each baseline source.

## Process-level timing

The input is a 1,000-task dependency chain. Each median uses 50 fresh process
invocations after five warmups.

| Language | Analyze median | Analyze min-max | `--help` median | Approximate difference |
|---|---:|---:|---:|---:|
| Oscan | 19.585 ms | 15.730-26.193 ms | 11.936 ms | 7.649 ms |
| Rust | 25.954 ms | 20.871-44.569 ms | 16.831 ms | 9.123 ms |
| TypeScript | 83.085 ms | 77.567-111.106 ms | 67.254 ms | 15.831 ms |
| C# Native AOT | 30.120 ms | 27.146-55.310 ms | 26.742 ms | 3.378 ms |
| Common Lisp (SBCL) | 73.429 ms | 67.826-819.723 ms | 53.271 ms | 20.158 ms |

These are process-level measurements on one Windows host, not in-process
algorithm timings. The difference subtracts independently measured medians and
is diagnostic only. Oscan had the lowest median in this rerun. A prior run on
the same host had Rust slightly ahead, so the report does not infer a robust
Oscan-versus-Rust ranking from these noisy whole-process measurements. Common
Lisp's median was 11.62% below TypeScript/Node's, but its sample set contained
one 819.723 ms outlier.

## C# correction impact

| C# mode | Invocation | Primary artifact | Analyze median | Startup median |
|---|---|---:|---:|---:|
| Framework-dependent, **invalidated** | `dotnet BuildGraph.dll` | 17,920 B | 92.715 ms | 79.220 ms |
| Native AOT, current rerun | `BuildGraph.exe` | 1,546,240 B | 30.120 ms | 26.742 ms |

The current Native AOT process median is 67.51% below the old value and the
startup median is 66.24% below it, while the honest self-contained artifact is
86.29 times larger than the old application-only DLL. The timing comparison is
descriptive, not a causal estimate of AOT alone: the invalid run used 20
iterations, two warmups, and a .NET 10 RC SDK, while this run used 50
iterations, five warmups, and the pinned .NET 9 SDK.

## Interpretation

These results establish semantic parity and characterize five reviewed
reference implementations. They show that Oscan produced by far the smallest
self-contained artifact and a low cold-process median for this application, but
also the largest source. Common Lisp adds a distinct tradeoff: source length
close to TypeScript, cold startup below Node in this run, and a much larger
runtime image. None of these results establishes that an LLM is more likely to
generate correct Oscan. See `EXPERIMENT_REPORT.md` for the complete analysis and
the required repeated one-shot protocol.
