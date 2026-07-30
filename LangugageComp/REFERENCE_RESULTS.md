# Corrected reference implementation results

Measured 2026-07-30 on:

- Windows 11 Enterprise 10.0.26200, 64-bit
- Intel Core i9-13950HX
- Oscan compiler at repository commit
  `c620ecb8ffaa28ce6e445db2d54474fcbbb24a90`, C backend
- Rust 1.94.1
- Node.js 22.14.0 and TypeScript 5.9.3
- .NET SDK 9.0.316 and Microsoft.DotNet.ILCompiler 9.0.18
- C# published for `win-x64` as self-contained Native AOT

The previous C# row used a framework-dependent DLL launched by `dotnet`. It has
been invalidated and replaced throughout this file.

## Correctness

Every implementation passed all 47 shared executable cases:

| Language | Passed | Total |
|---|---:|---:|
| Oscan | 47 | 47 |
| Rust | 47 | 47 |
| TypeScript | 47 | 47 |
| C# Native AOT | 47 | 47 |
| **Combined** | **188** | **188** |

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

The TypeScript value is only the emitted JavaScript and excludes Node.js, so it
is not a self-contained deployment-size figure. The C# value is the
self-contained Native AOT executable and excludes an optional 8,015,872-byte
PDB. The Oscan executable is 7.13 times smaller than Rust's and 56.98 times
smaller than C# Native AOT's. Oscan source is 42-53% larger in bytes than each
baseline source.

## Process-level timing

The input is a 1,000-task dependency chain. Each median uses 50 fresh process
invocations after five warmups.

| Language | Analyze median | Analyze min-max | `--help` median | Approximate difference |
|---|---:|---:|---:|---:|
| Oscan | 21.820 ms | 16.390-35.205 ms | 14.444 ms | 7.376 ms |
| Rust | 20.683 ms | 15.575-41.198 ms | 17.834 ms | 2.849 ms |
| TypeScript | 77.742 ms | 73.899-111.086 ms | 61.843 ms | 15.899 ms |
| C# Native AOT | 27.526 ms | 23.427-55.603 ms | 23.161 ms | 4.365 ms |

These are process-level measurements on one Windows host, not in-process
algorithm timings. The difference subtracts independently measured medians and
is diagnostic only. Oscan and Rust are close at this scale; Oscan's process
median is 1.26 times faster than C# Native AOT's and 3.56 times faster than
TypeScript/Node's.

## C# correction impact

| C# mode | Invocation | Primary artifact | Analyze median | Startup median |
|---|---|---:|---:|---:|
| Framework-dependent, **invalidated** | `dotnet BuildGraph.dll` | 17,920 B | 92.715 ms | 79.220 ms |
| Native AOT, corrected | `BuildGraph.exe` | 1,546,240 B | 27.526 ms | 23.161 ms |

The corrected process median is 70.31% below the old value and the startup
median is 70.76% below it, while the honest self-contained artifact is 86.29
times larger than the old application-only DLL. The timing comparison is
descriptive, not a causal estimate of AOT alone: the invalid run used 20
iterations, two warmups, and a .NET 10 RC SDK, while the corrected run used 50
iterations, five warmups, and the pinned .NET 9 SDK.

## Interpretation

These results establish semantic parity and characterize four reviewed
reference implementations. They show that Oscan produced the smallest
self-contained native artifact and near-Rust process time for this application,
but also the largest source. They do not establish that an LLM is more likely
to generate correct Oscan. See `EXPERIMENT_REPORT.md` for the complete analysis
and the required repeated one-shot protocol.
